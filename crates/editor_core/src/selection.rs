use std::ops::Range;

#[derive(Clone, Copy, Debug)]
pub struct Selection {
    pub anchor: usize,
    pub head: usize,
    /// Remembered x position for vertical motion, in pixels of the view.
    pub goal_x: Option<f32>,
}

impl PartialEq for Selection {
    fn eq(&self, other: &Self) -> bool {
        self.anchor == other.anchor && self.head == other.head
    }
}

impl Selection {
    pub fn cursor(offset: usize) -> Self {
        Self::new(offset, offset)
    }

    pub fn new(anchor: usize, head: usize) -> Self {
        Self {
            anchor,
            head,
            goal_x: None,
        }
    }

    pub fn start(&self) -> usize {
        self.anchor.min(self.head)
    }

    pub fn end(&self) -> usize {
        self.anchor.max(self.head)
    }

    pub fn range(&self) -> Range<usize> {
        self.start()..self.end()
    }

    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }

    pub fn is_reversed(&self) -> bool {
        self.head < self.anchor
    }
}

/// Selections stay sorted and never overlap. `primary` is the newest selection.
#[derive(Clone, Debug, PartialEq)]
pub struct SelectionSet {
    selections: Vec<Selection>,
    primary: usize,
}

impl Default for SelectionSet {
    fn default() -> Self {
        Self::single(Selection::cursor(0))
    }
}

impl SelectionSet {
    pub fn single(selection: Selection) -> Self {
        Self {
            selections: vec![selection],
            primary: 0,
        }
    }

    /// Builds a normalized set. The last entry of `selections` becomes primary.
    pub fn from_vec(selections: Vec<Selection>) -> Self {
        assert!(!selections.is_empty());
        let primary = selections[selections.len() - 1];
        let mut sorted = selections;
        sorted.sort_by_key(|selection| (selection.start(), selection.end()));

        let mut merged: Vec<Selection> = Vec::with_capacity(sorted.len());
        for selection in sorted {
            match merged.last_mut() {
                Some(last) if overlaps(last, &selection) => {
                    let start = last.start();
                    let end = last.end().max(selection.end());
                    *last = if last.is_reversed() {
                        Selection::new(end, start)
                    } else {
                        Selection::new(start, end)
                    };
                }
                _ => merged.push(selection),
            }
        }

        let primary = merged
            .iter()
            .position(|selection| {
                selection.start() <= primary.start() && primary.end() <= selection.end()
            })
            .unwrap_or(merged.len() - 1);
        Self {
            selections: merged,
            primary,
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = &Selection> {
        self.selections.iter()
    }

    pub fn as_slice(&self) -> &[Selection] {
        &self.selections
    }

    pub fn len(&self) -> usize {
        self.selections.len()
    }

    pub fn is_empty(&self) -> bool {
        self.selections.is_empty()
    }

    pub fn primary(&self) -> &Selection {
        &self.selections[self.primary]
    }

    /// Returns the selections with the primary one last, ready for `from_vec`.
    pub fn to_vec_primary_last(&self) -> Vec<Selection> {
        let mut selections = self.selections.clone();
        let primary = selections.remove(self.primary);
        selections.push(primary);
        selections
    }
}

fn overlaps(left: &Selection, right: &Selection) -> bool {
    if left.is_empty() || right.is_empty() {
        right.start() <= left.end() && (left.start() == right.start() || right.start() < left.end())
    } else {
        right.start() < left.end()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sorts_and_merges_overlaps() {
        let set = SelectionSet::from_vec(vec![
            Selection::new(10, 15),
            Selection::cursor(2),
            Selection::new(12, 20),
        ]);
        let ranges: Vec<_> = set.iter().map(Selection::range).collect();
        assert_eq!(ranges, vec![2..2, 10..20]);
        assert_eq!(set.primary().range(), 10..20);
    }

    #[test]
    fn adjacent_ranges_stay_separate() {
        let set = SelectionSet::from_vec(vec![Selection::new(0, 2), Selection::new(2, 4)]);
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn equal_cursors_collapse() {
        let set = SelectionSet::from_vec(vec![Selection::cursor(3), Selection::cursor(3)]);
        assert_eq!(set.len(), 1);
    }

    #[test]
    fn primary_is_kept_last_for_rebuild() {
        let set = SelectionSet::from_vec(vec![Selection::cursor(9), Selection::cursor(1)]);
        assert_eq!(set.primary().head, 1);
        assert_eq!(set.to_vec_primary_last().last().unwrap().head, 1);
    }
}
