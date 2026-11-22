//! Which lines changed between two texts, for marks next to the line numbers.
//! Myers' algorithm on the lines between a common start and end, which keeps
//! the usual edit, a few lines somewhere, cheap.

use std::{
    hash::{DefaultHasher, Hash, Hasher},
    ops::Range,
};

/// Past this many changed lines, marks would say little and cost much.
const MAX_EDITS: usize = 1000;
const MAX_LINES: usize = 50_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HunkKind {
    Added,
    Modified,
    /// Lines went away right before `lines.start`, which is empty.
    Deleted,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LineHunk {
    pub kind: HunkKind,
    /// Lines of the new text, counted from zero.
    pub lines: Range<usize>,
    /// The lines of the old text they replace, empty for an addition.
    pub old_lines: Range<usize>,
}

/// The changes from `old` to `new`, or `None` where they are too many to tell.
pub fn line_diff(old: &str, new: &str) -> Option<Vec<LineHunk>> {
    let old = hash_lines(old);
    let new = hash_lines(new);
    if old.len() > MAX_LINES || new.len() > MAX_LINES {
        return None;
    }
    let prefix = old.iter().zip(&new).take_while(|(a, b)| a == b).count();
    let suffix = old[prefix..].iter().rev().zip(new[prefix..].iter().rev()).take_while(|(a, b)| a == b).count();
    let old_middle = &old[prefix..old.len() - suffix];
    let new_middle = &new[prefix..new.len() - suffix];
    let edits = myers(old_middle, new_middle)?;

    // Runs of deletions and insertions become one hunk each.
    let mut hunks = Vec::new();
    let (mut new_line, mut old_line) = (prefix, prefix);
    let (mut deleted, mut inserted) = (0, 0);
    let flush = |new_line: usize, old_line: usize, deleted: &mut usize, inserted: &mut usize, hunks: &mut Vec<LineHunk>| {
        let start = new_line - *inserted;
        let old_lines = old_line - *deleted..old_line;
        let kind = match (*deleted > 0, *inserted > 0) {
            (true, true) => Some(HunkKind::Modified),
            (false, true) => Some(HunkKind::Added),
            (true, false) => Some(HunkKind::Deleted),
            (false, false) => None,
        };
        if let Some(kind) = kind {
            hunks.push(LineHunk { kind, lines: start..new_line, old_lines });
        }
        *deleted = 0;
        *inserted = 0;
    };
    for edit in edits {
        match edit {
            Edit::Equal => {
                flush(new_line, old_line, &mut deleted, &mut inserted, &mut hunks);
                new_line += 1;
                old_line += 1;
            }
            Edit::Delete => {
                deleted += 1;
                old_line += 1;
            }
            Edit::Insert => {
                inserted += 1;
                new_line += 1;
            }
        }
    }
    flush(new_line, old_line, &mut deleted, &mut inserted, &mut hunks);
    Some(hunks)
}

/// A line ending in `\r\n` counts as the same line as with `\n`.
fn hash_lines(text: &str) -> Vec<u64> {
    if text.is_empty() {
        return Vec::new();
    }
    text.split('\n')
        .map(|line| {
            let mut hasher = DefaultHasher::new();
            line.strip_suffix('\r').unwrap_or(line).hash(&mut hasher);
            hasher.finish()
        })
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Edit {
    Equal,
    Delete,
    Insert,
}

/// The shortest edit script from `a` to `b`. Keeps the frontier of every
/// step to walk back, which stays small since steps are few.
fn myers(a: &[u64], b: &[u64]) -> Option<Vec<Edit>> {
    let (n, m) = (a.len() as isize, b.len() as isize);
    let max = (n + m) as usize;
    let offset = max as isize + 1;
    let mut frontier = vec![0isize; 2 * max + 3];
    let mut trace: Vec<Vec<isize>> = Vec::new();
    let mut found = None;
    'search: for d in 0..=max.min(MAX_EDITS) as isize {
        trace.push(frontier[(offset - d) as usize..=(offset + d) as usize].to_vec());
        for k in (-d..=d).step_by(2) {
            let index = (offset + k) as usize;
            let mut x = if k == -d || (k != d && frontier[index - 1] < frontier[index + 1]) {
                frontier[index + 1]
            } else {
                frontier[index - 1] + 1
            };
            let mut y = x - k;
            while x < n && y < m && a[x as usize] == b[y as usize] {
                x += 1;
                y += 1;
            }
            frontier[index] = x;
            if x >= n && y >= m {
                found = Some(d);
                break 'search;
            }
        }
    }
    let steps = found?;

    let mut edits = Vec::new();
    let (mut x, mut y) = (n, m);
    for d in (1..=steps).rev() {
        // The frontier as it was before step `d`.
        let before = &trace[d as usize];
        let at = |k: isize| before[(k + d) as usize];
        let k = x - y;
        let previous_k = if k == -d || (k != d && at(k - 1) < at(k + 1)) { k + 1 } else { k - 1 };
        let previous_x = at(previous_k);
        let previous_y = previous_x - previous_k;
        while x > previous_x && y > previous_y {
            x -= 1;
            y -= 1;
            edits.push(Edit::Equal);
        }
        edits.push(if x == previous_x { Edit::Insert } else { Edit::Delete });
        x = previous_x;
        y = previous_y;
    }
    while x > 0 && y > 0 {
        x -= 1;
        y -= 1;
        edits.push(Edit::Equal);
    }
    edits.reverse();
    Some(edits)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(old: &str, new: &str) -> Vec<(HunkKind, Range<usize>)> {
        line_diff(old, new).unwrap().into_iter().map(|hunk| (hunk.kind, hunk.lines)).collect()
    }

    #[test]
    fn hunks_say_what_happened_where() {
        assert_eq!(kinds("a\nb\nc", "a\nb\nc"), []);
        assert_eq!(kinds("a\nb\nc", "a\nX\nb\nc"), [(HunkKind::Added, 1..2)]);
        assert_eq!(kinds("a\nb\nc", "a\nB\nc"), [(HunkKind::Modified, 1..2)]);
        assert_eq!(kinds("a\nb\nc", "a\nc"), [(HunkKind::Deleted, 1..1)]);
        assert_eq!(kinds("", "a\nb"), [(HunkKind::Added, 0..2)]);
        assert_eq!(kinds("a\nb", ""), [(HunkKind::Deleted, 0..0)]);
        assert_eq!(kinds("a\r\nb\r\n", "a\nb\n"), []);
        assert_eq!(kinds("x\na\ny\nb\nz", "a\nb"), [(HunkKind::Deleted, 0..0), (HunkKind::Deleted, 1..1), (HunkKind::Deleted, 2..2)]);
    }

    /// The edit script must turn the old lines into the new ones, and be as
    /// short as the length of the longest common subsequence allows.
    #[test]
    fn random_texts_get_a_shortest_correct_script() {
        let mut seed = 0x9e37_79b9_7f4a_7c15u64;
        let mut random = move |below: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed % below
        };
        for _ in 0..500 {
            let a: Vec<u64> = (0..random(12)).map(|_| random(4)).collect();
            let b: Vec<u64> = (0..random(12)).map(|_| random(4)).collect();
            let edits = myers(&a, &b).unwrap();
            let (mut i, mut j, mut rebuilt) = (0, 0, Vec::new());
            for edit in &edits {
                match edit {
                    Edit::Equal => {
                        assert_eq!(a[i], b[j]);
                        rebuilt.push(a[i]);
                        i += 1;
                        j += 1;
                    }
                    Edit::Delete => i += 1,
                    Edit::Insert => {
                        rebuilt.push(b[j]);
                        j += 1;
                    }
                }
            }
            assert_eq!((i, j), (a.len(), b.len()));
            assert_eq!(rebuilt, b);
            let mut lcs = vec![vec![0usize; b.len() + 1]; a.len() + 1];
            for x in (0..a.len()).rev() {
                for y in (0..b.len()).rev() {
                    lcs[x][y] = if a[x] == b[y] { lcs[x + 1][y + 1] + 1 } else { lcs[x + 1][y].max(lcs[x][y + 1]) };
                }
            }
            let changes = edits.iter().filter(|edit| **edit != Edit::Equal).count();
            assert_eq!(changes, a.len() + b.len() - 2 * lcs[0][0]);
        }
    }
}
