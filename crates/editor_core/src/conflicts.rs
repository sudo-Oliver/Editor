//! The blocks that git leaves in a file where a merge needs a decision.

use std::ops::Range;

/// One conflict, as byte ranges of the text. The sides do not include the
/// marker lines, and `whole` runs from the first marker line to the end of
/// the last.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conflict {
    pub whole: Range<usize>,
    pub ours: Range<usize>,
    /// What both started from, with `merge.conflictStyle=diff3`.
    pub base: Option<Range<usize>>,
    pub theirs: Range<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    Ours,
    Theirs,
    Both,
}

impl Conflict {
    /// The text that replaces `whole` for a choice.
    pub fn resolve(&self, text: &str, choice: Choice) -> String {
        match choice {
            Choice::Ours => text[self.ours.clone()].to_string(),
            Choice::Theirs => text[self.theirs.clone()].to_string(),
            Choice::Both => {
                let ours = &text[self.ours.clone()];
                let separator = if ours.is_empty() || ours.ends_with('\n') { "" } else { "\n" };
                format!("{ours}{separator}{}", &text[self.theirs.clone()])
            }
        }
    }
}

/// Every complete conflict of `text`. Marker lines that do not form one are
/// left alone, since a document may well contain such a line.
pub fn conflicts(text: &str) -> Vec<Conflict> {
    let mut found = Vec::new();
    let mut lines = Vec::new();
    let mut position = 0;
    for line in text.split_inclusive('\n') {
        lines.push(position..position + line.len());
        position += line.len();
    }
    let marker = |range: &Range<usize>, prefix: &str| {
        let line = text[range.clone()].trim_end_matches(['\n', '\r']);
        line.starts_with(prefix) && (line.len() == 7 || line.as_bytes()[7] == b' ' || prefix == "=======" && line.len() == 7)
    };
    let mut index = 0;
    while index < lines.len() {
        if !marker(&lines[index], "<<<<<<<") {
            index += 1;
            continue;
        }
        let start = index;
        let mut base_marker = None;
        let mut middle = None;
        let mut end = None;
        for (offset, line) in lines[start + 1..].iter().enumerate() {
            let at = start + 1 + offset;
            if marker(line, "<<<<<<<") {
                break;
            } else if middle.is_none() && base_marker.is_none() && marker(line, "|||||||") {
                base_marker = Some(at);
            } else if middle.is_none() && text[line.clone()].trim_end_matches(['\n', '\r']) == "=======" {
                middle = Some(at);
            } else if middle.is_some() && marker(line, ">>>>>>>") {
                end = Some(at);
                break;
            }
        }
        match (middle, end) {
            (Some(middle), Some(end)) => {
                let ours_end = base_marker.unwrap_or(middle);
                found.push(Conflict {
                    whole: lines[start].start..lines[end].end,
                    ours: lines[start].end..lines[ours_end].start,
                    base: base_marker.map(|base| lines[base].end..lines[middle].start),
                    theirs: lines[middle].end..lines[end].start,
                });
                index = end + 1;
            }
            _ => index = start + 1,
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_with_and_without_a_base_resolve_every_way() {
        let text = "a\n<<<<<<< HEAD\nmine\n||||||| base\nold\n=======\ntheirs\n>>>>>>> origin/main\nb\n<<<<<<< HEAD\nx\n=======\ny\n>>>>>>> other\n";
        let found = conflicts(text);
        assert_eq!(found.len(), 2);
        assert_eq!(&text[found[0].ours.clone()], "mine\n");
        assert_eq!(found[0].base.as_ref().map(|base| &text[base.clone()]), Some("old\n"));
        assert_eq!(&text[found[0].theirs.clone()], "theirs\n");
        assert_eq!(found[1].base, None);
        let mut resolved = text.to_string();
        for (conflict, choice) in found.iter().zip([Choice::Both, Choice::Theirs]).rev() {
            resolved.replace_range(conflict.whole.clone(), &conflict.resolve(text, choice));
        }
        assert_eq!(resolved, "a\nmine\ntheirs\nb\ny\n");
    }

    #[test]
    fn stray_markers_are_no_conflict() {
        assert!(conflicts("<<<<<<< only\ntext\n").is_empty());
        assert!(conflicts("=======\n>>>>>>> x\n").is_empty());
        assert!(conflicts("<<<<<<<< eight\n=======\n>>>>>>>\n").is_empty());
        // An unfinished block before a complete one does not swallow it.
        assert_eq!(conflicts("<<<<<<< a\n<<<<<<< b\nx\n=======\ny\n>>>>>>> c\n").len(), 1);
    }
}
