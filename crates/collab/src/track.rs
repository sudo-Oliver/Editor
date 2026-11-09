//! Turns the edits of someone whose changes are tracked into suggestions.
//! The editor deletes text at once, but the shared text keeps it, struck
//! through, so the editor gets that text back.

use std::ops::Range;

use editor_core::TextChange;

use crate::model::TrackedOp;

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Tracked {
    /// For `Project::edit_tracked`.
    pub ops: Vec<TrackedOp>,
    /// Text the editor has to put back, as insertions in ascending order.
    pub restores: Vec<TextChange>,
}

/// `changes` are local edits in the order they happened, with the text they
/// removed. `own` holds one's own suggested insertions, where deleting
/// removes for good, since nobody has to review a typo of one's own.
pub fn track(changes: &[TextChange], own: &[Range<usize>]) -> Tracked {
    let mut own = own.to_vec();
    // Struck text of these changes, by position in the shared text. The
    // editor lacks it until the restores.
    let mut kept: Vec<(usize, String)> = Vec::new();
    let mut ops = Vec::new();
    for change in changes {
        if change.removed > 0 {
            let start = shared_position(&kept, change.start, true);
            let end = shared_position(&kept, change.start + change.removed, false);
            let inside_own = own.iter().any(|range| range.start <= start && end <= range.end);
            if inside_own || change.removed_text.len() != change.removed {
                ops.push(TrackedOp::Remove { range: start..end });
                let removed = end - start;
                for (at, _) in &mut kept {
                    if *at >= end {
                        *at -= removed;
                    }
                }
                for range in &mut own {
                    range.start = shrink(range.start, start..end);
                    range.end = shrink(range.end, start..end);
                }
                own.retain(|range| !range.is_empty());
            } else {
                // Struck text of these changes inside the range joins this one.
                let mut text = String::new();
                let mut taken = 0;
                let mut position = start;
                let inner: Vec<(usize, String)> = kept.iter().filter(|(at, _)| start <= *at && *at <= end).cloned().collect();
                kept.retain(|(at, _)| !(start <= *at && *at <= end));
                for (at, struck) in inner {
                    let piece = at - position;
                    text.push_str(&change.removed_text[taken..taken + piece]);
                    taken += piece;
                    text.push_str(&struck);
                    position = at + struck.len();
                }
                text.push_str(&change.removed_text[taken..]);
                ops.push(TrackedOp::Strike { range: start..start + text.len() });
                let index = kept.partition_point(|(at, _)| *at < start);
                kept.insert(index, (start, text));
            }
        }
        if !change.inserted.is_empty() {
            let at = shared_position(&kept, change.start, true);
            let length = change.inserted.len();
            ops.push(TrackedOp::Insert {
                at,
                text: change.inserted.clone(),
            });
            for (position, _) in &mut kept {
                if *position >= at {
                    *position += length;
                }
            }
            for range in &mut own {
                if range.start >= at {
                    range.start += length;
                    range.end += length;
                } else if range.end >= at {
                    range.end += length;
                }
            }
            own.push(at..at + length);
        }
    }
    Tracked {
        ops,
        restores: kept.into_iter().map(|(at, text)| TextChange::new(at, 0, text)).collect(),
    }
}

/// Where a position of the editor lies in the shared text. Struck text at
/// the very position comes first for a start, and stays outside for an end.
fn shared_position(kept: &[(usize, String)], position: usize, start: bool) -> usize {
    let mut shared = position;
    for (at, text) in kept {
        if *at < shared || (start && *at == shared) {
            shared += text.len();
        } else {
            break;
        }
    }
    shared
}

fn shrink(offset: usize, removed: Range<usize>) -> usize {
    if offset <= removed.start {
        offset
    } else if offset >= removed.end {
        offset - removed.len()
    } else {
        removed.start
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn deletion(start: usize, removed_text: &str) -> TextChange {
        TextChange {
            removed_text: removed_text.to_string(),
            ..TextChange::new(start, removed_text.len(), "")
        }
    }

    /// Plays the ops onto the old text, and the changes then the restores
    /// onto the editor's text. Both have to end up the same.
    fn check(old: &str, changes: &[TextChange], own: &[Range<usize>]) -> (String, Tracked) {
        let tracked = track(changes, own);
        let mut shared = old.to_string();
        for op in &tracked.ops {
            match op {
                TrackedOp::Insert { at, text } => shared.insert_str(*at, text),
                TrackedOp::Remove { range } => shared.replace_range(range.clone(), ""),
                TrackedOp::Strike { .. } => {}
            }
        }
        let mut editor = old.to_string();
        for change in changes.iter().chain(&tracked.restores) {
            editor.replace_range(change.start..change.start + change.removed, &change.inserted);
        }
        assert_eq!(editor, shared);
        (shared, tracked)
    }

    #[test]
    fn a_deletion_strikes_and_comes_back() {
        let (text, tracked) = check("Hallo Welt", &[deletion(5, " Welt")], &[]);
        assert_eq!(text, "Hallo Welt");
        assert_eq!(tracked.ops, vec![TrackedOp::Strike { range: 5..10 }]);
    }

    #[test]
    fn replacing_puts_the_new_text_after_the_struck_text() {
        let change = TextChange {
            removed_text: "Welt".into(),
            ..TextChange::new(6, 4, "Erde")
        };
        let (text, tracked) = check("Hallo Welt!", &[change], &[]);
        assert_eq!(text, "Hallo WeltErde!");
        assert_eq!(tracked.ops, vec![TrackedOp::Strike { range: 6..10 }, TrackedOp::Insert { at: 10, text: "Erde".into() }]);
    }

    #[test]
    fn one_s_own_insertion_goes_away_for_good() {
        let (text, tracked) = check("abXYc", &[deletion(3, "Y")], &[Range { start: 2, end: 4 }]);
        assert_eq!(text, "abXc");
        assert!(tracked.restores.is_empty());
    }

    #[test]
    fn several_cursors_strike_in_one_go() {
        // Two cursors deleting a character each, the later one first.
        let (text, tracked) = check("a1b2c", &[deletion(3, "2"), deletion(1, "1"), TextChange::new(1, 0, "x")], &[]);
        assert_eq!(text, "a1xb2c");
        assert_eq!(tracked.restores.len(), 2);
    }

    #[test]
    fn deleting_across_struck_text_of_the_same_batch_keeps_its_order() {
        let (text, _) = check("abcdef", &[deletion(2, "c"), deletion(1, "bd")], &[]);
        assert_eq!(text, "abcdef");
    }
}
