//! Selecting text on PDF pages. Works on data from the PDF worker, without PDFium.

use std::ops::Range;

use pdf::{PageChar, PageContent, Rect};

/// PDFium marks a hyphen at the end of a line with one of these.
const LINE_END_HYPHENS: [char; 2] = ['\u{2}', '\u{fffe}'];
/// A vertical gap counts this much more than a horizontal one, so that the
/// closest character comes from the row under the pointer.
const ROW_WEIGHT: f32 = 1000.0;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct TextPoint {
    pub page: usize,
    pub index: usize,
}

/// Both ends are selected. The head can be before the anchor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Selection {
    pub anchor: TextPoint,
    pub head: TextPoint,
}

impl Selection {
    pub fn ordered(&self) -> (TextPoint, TextPoint) {
        (self.anchor.min(self.head), self.anchor.max(self.head))
    }

    /// The selected characters of a page with `length` characters.
    pub fn range_on(&self, page: usize, length: usize) -> Option<Range<usize>> {
        let (first, last) = self.ordered();
        if page < first.page || page > last.page || length == 0 {
            return None;
        }
        let start = if page == first.page { first.index } else { 0 };
        let end = if page == last.page { last.index + 1 } else { length };
        Some(start.min(length)..end.min(length))
    }
}

fn has_area(rect: &Rect) -> bool {
    rect.right > rect.left && rect.bottom > rect.top
}

/// The visible character closest to a position, preferring the same row.
pub fn char_at(chars: &[PageChar], x: f32, y: f32) -> Option<usize> {
    let gap = |low: f32, high: f32, value: f32| (low - value).max(value - high).max(0.0);
    chars
        .iter()
        .enumerate()
        .filter(|(_, character)| has_area(&character.rect))
        .map(|(index, character)| {
            let rect = &character.rect;
            (index, gap(rect.top, rect.bottom, y) * ROW_WEIGHT + gap(rect.left, rect.right, x))
        })
        .min_by(|left, right| left.1.total_cmp(&right.1))
        .map(|(index, _)| index)
}

/// One rectangle per row of the selected characters.
pub fn selection_rects(chars: &[PageChar], range: Range<usize>) -> Box<[Rect]> {
    let mut rects: Vec<Rect> = Vec::new();
    for character in chars[range].iter().filter(|character| has_area(&character.rect)) {
        let rect = character.rect;
        let middle = (rect.top + rect.bottom) / 2.0;
        match rects.last_mut() {
            Some(row) if middle >= row.top && middle <= row.bottom && rect.left >= row.left => {
                row.right = row.right.max(rect.right);
                row.top = row.top.min(rect.top);
                row.bottom = row.bottom.max(rect.bottom);
            }
            _ => rects.push(rect),
        }
    }
    rects.into_boxed_slice()
}

/// The text of a selection. Pages without loaded content are left out.
pub fn selected_text<'a>(selection: &Selection, content: impl Fn(usize) -> Option<&'a PageContent>) -> String {
    let (first, last) = selection.ordered();
    let mut text = String::new();
    for page in first.page..=last.page {
        let Some(content) = content(page) else {
            continue;
        };
        let Some(range) = selection.range_on(page, content.chars.len()) else {
            continue;
        };
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        // A hyphen at the end of a row only splits the word for the layout.
        let mut joining = false;
        for character in &content.chars[range] {
            match character.ch {
                ch if LINE_END_HYPHENS.contains(&ch) => joining = true,
                '\r' | '\0' => {}
                '\n' if joining => joining = false,
                ch => {
                    joining = false;
                    text.push(ch);
                }
            }
        }
    }
    text
}

/// A PDF can name any program in a link, so only harmless schemes get opened.
pub fn is_openable_uri(uri: &str) -> bool {
    let Some((scheme, _)) = uri.split_once(':') else {
        return false;
    };
    ["http", "https", "mailto"].iter().any(|allowed| scheme.eq_ignore_ascii_case(allowed))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn character(ch: char, column: usize, row: usize) -> PageChar {
        let (left, top) = (column as f32 * 10.0, row as f32 * 20.0);
        PageChar { ch, rect: Rect { left, top, right: left + 10.0, bottom: top + 12.0 } }
    }

    fn generated(ch: char) -> PageChar {
        PageChar { ch, rect: Rect::default() }
    }

    fn page(chars: Vec<PageChar>) -> PageContent {
        PageContent { chars: chars.into(), links: Box::default() }
    }

    fn selection(anchor: (usize, usize), head: (usize, usize)) -> Selection {
        Selection {
            anchor: TextPoint { page: anchor.0, index: anchor.1 },
            head: TextPoint { page: head.0, index: head.1 },
        }
    }

    #[test]
    fn char_at_prefers_the_row_under_the_pointer() {
        let chars = [character('a', 0, 0), character('b', 9, 0), character('c', 0, 1)];
        assert_eq!(char_at(&chars, 85.0, 5.0), Some(1));
        assert_eq!(char_at(&chars, 85.0, 25.0), Some(2));
    }

    #[test]
    fn char_at_skips_generated_characters() {
        assert_eq!(char_at(&[generated(' ')], 0.0, 0.0), None);
        assert_eq!(char_at(&[generated(' '), character('a', 5, 5)], 0.0, 0.0), Some(1));
    }

    #[test]
    fn a_backward_selection_covers_the_same_characters() {
        let backward = selection((0, 3), (0, 1));
        assert_eq!(backward.range_on(0, 10), Some(1..4));
        assert_eq!(backward.range_on(0, 10), selection((0, 1), (0, 3)).range_on(0, 10));
    }

    #[test]
    fn a_selection_over_pages_covers_the_whole_middle_page() {
        let across = selection((2, 8), (0, 5));
        assert_eq!(across.range_on(0, 10), Some(5..10));
        assert_eq!(across.range_on(1, 7), Some(0..7));
        assert_eq!(across.range_on(2, 10), Some(0..9));
        assert_eq!(across.range_on(3, 10), None);
    }

    #[test]
    fn a_range_stays_inside_a_page_that_shrank() {
        assert_eq!(selection((0, 0), (0, 50)).range_on(0, 3), Some(0..3));
    }

    #[test]
    fn rects_are_joined_per_row() {
        let chars = [character('a', 0, 0), character('b', 1, 0), generated('\n'), character('c', 0, 1)];
        let rects = selection_rects(&chars, 0..4);
        assert_eq!(rects.len(), 2);
        assert_eq!((rects[0].left, rects[0].right), (0.0, 20.0));
        assert_eq!(rects[1].top, 20.0);
    }

    #[test]
    fn text_keeps_line_breaks_and_joins_pages() {
        let first = page(vec![character('a', 0, 0), generated('\r'), generated('\n'), character('b', 0, 1)]);
        let second = page(vec![character('c', 0, 0)]);
        let text = selected_text(&selection((0, 0), (1, 0)), |index| [&first, &second].get(index).copied());
        assert_eq!(text, "a\nb\nc");
    }

    #[test]
    fn text_joins_words_split_by_a_line_end_hyphen() {
        let content = page(vec![
            character('w', 0, 0),
            generated('\u{2}'),
            generated('\r'),
            generated('\n'),
            character('o', 0, 1),
        ]);
        assert_eq!(selected_text(&selection((0, 0), (0, 4)), |_| Some(&content)), "wo");
    }

    #[test]
    fn text_leaves_out_pages_without_content() {
        let content = page(vec![character('a', 0, 0)]);
        let text = selected_text(&selection((0, 0), (2, 0)), |index| (index != 1).then_some(&content));
        assert_eq!(text, "a\na");
    }

    #[test]
    fn only_harmless_uris_can_be_opened() {
        assert!(is_openable_uri("https://example.org"));
        assert!(is_openable_uri("HTTP://example.org"));
        assert!(is_openable_uri("mailto:a@example.org"));
        assert!(!is_openable_uri("file:///etc/passwd"));
        assert!(!is_openable_uri("javascript:alert(1)"));
        assert!(!is_openable_uri("no-scheme"));
    }
}
