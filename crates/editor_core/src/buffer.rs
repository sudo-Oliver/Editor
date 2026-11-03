use std::ops::Range;

use ropey::Rope;
use unicode_segmentation::UnicodeSegmentation;

/// Text storage addressed by UTF-8 byte offsets, the unit tree-sitter uses.
///
/// Lines end at `\n` only. A trailing `\r` stays in the rope so that files keep
/// their line endings, but line accessors hide it.
#[derive(Clone, Default)]
pub struct Buffer {
    rope: Rope,
}

impl Buffer {
    pub fn new(text: &str) -> Self {
        Self {
            rope: Rope::from_str(text),
        }
    }

    pub fn rope(&self) -> &Rope {
        &self.rope
    }

    pub fn len(&self) -> usize {
        self.rope.len_bytes()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn line_count(&self) -> usize {
        self.rope.len_lines()
    }

    pub fn offset_to_line(&self, offset: usize) -> usize {
        self.rope.byte_to_line(offset.min(self.len()))
    }

    /// Returns the zero-based line and the byte column inside this line.
    pub fn offset_to_point(&self, offset: usize) -> (usize, usize) {
        let offset = offset.min(self.len());
        let line = self.rope.byte_to_line(offset);
        (line, offset - self.rope.line_to_byte(line))
    }

    pub fn line_start(&self, line: usize) -> usize {
        self.rope.line_to_byte(line.min(self.line_count()))
    }

    /// Returns the offset after the last visible character of `line`.
    pub fn line_end(&self, line: usize) -> usize {
        if line >= self.line_count() {
            return self.len();
        }
        let start = self.rope.line_to_byte(line);
        let mut end = self.rope.line_to_byte(line + 1);
        if end > start && self.byte_at(end - 1) == b'\n' {
            end -= 1;
        }
        if end > start && self.byte_at(end - 1) == b'\r' {
            end -= 1;
        }
        end
    }

    pub fn line_text(&self, line: usize) -> String {
        self.slice(self.line_start(line)..self.line_end(line))
    }

    pub fn point_to_offset(&self, line: usize, column: usize) -> usize {
        if line >= self.line_count() {
            return self.len();
        }
        self.clip((self.line_start(line) + column).min(self.line_end(line)))
    }

    pub fn slice(&self, range: Range<usize>) -> String {
        self.rope.byte_slice(range).to_string()
    }

    pub fn byte_at(&self, offset: usize) -> u8 {
        self.rope.byte(offset)
    }

    pub fn char_at(&self, offset: usize) -> Option<char> {
        if offset >= self.len() {
            return None;
        }
        self.rope.get_char(self.rope.byte_to_char(offset))
    }

    pub fn char_before(&self, offset: usize) -> Option<char> {
        let index = self.rope.byte_to_char(offset.min(self.len()));
        index.checked_sub(1).and_then(|i| self.rope.get_char(i))
    }

    /// Moves `offset` back to the nearest character boundary.
    pub fn clip(&self, offset: usize) -> usize {
        let offset = offset.min(self.len());
        self.rope.char_to_byte(self.rope.byte_to_char(offset))
    }

    pub fn replace(&mut self, range: Range<usize>, text: &str) {
        let start = self.rope.byte_to_char(range.start);
        let end = self.rope.byte_to_char(range.end);
        self.rope.remove(start..end);
        self.rope.insert(start, text);
    }

    pub fn offset_to_utf16(&self, offset: usize) -> usize {
        self.rope
            .char_to_utf16_cu(self.rope.byte_to_char(offset.min(self.len())))
    }

    pub fn utf16_to_offset(&self, utf16: usize) -> usize {
        let utf16 = utf16.min(self.rope.len_utf16_cu());
        self.rope.char_to_byte(self.rope.utf16_cu_to_char(utf16))
    }

    pub fn next_boundary(&self, offset: usize) -> usize {
        let (line, column) = self.offset_to_point(offset);
        let text = self.line_text(line);
        if column >= text.len() {
            return if line + 1 < self.line_count() {
                self.line_start(line + 1)
            } else {
                self.len()
            };
        }
        let next = text
            .grapheme_indices(true)
            .find_map(|(index, _)| (index > column).then_some(index))
            .unwrap_or(text.len());
        self.line_start(line) + next
    }

    pub fn previous_boundary(&self, offset: usize) -> usize {
        let (line, column) = self.offset_to_point(offset);
        if column == 0 {
            return if line == 0 { 0 } else { self.line_end(line - 1) };
        }
        let text = self.line_text(line);
        let previous = text
            .grapheme_indices(true)
            .rev()
            .find_map(|(index, _)| (index < column).then_some(index))
            .unwrap_or(0);
        self.line_start(line) + previous
    }

    pub fn next_word_end(&self, offset: usize) -> usize {
        let (line, column) = self.offset_to_point(offset);
        let text = self.line_text(line);
        if column >= text.len() {
            return self.next_boundary(offset);
        }
        let mut chars = text[column..].char_indices().peekable();
        while let Some((_, ch)) = chars.peek() {
            if !ch.is_whitespace() {
                break;
            }
            chars.next();
        }
        let Some(&(_, first)) = chars.peek() else {
            return self.line_start(line) + text.len();
        };
        let kind = char_kind(first);
        for (index, ch) in chars {
            if char_kind(ch) != kind {
                return self.line_start(line) + column + index;
            }
        }
        self.line_start(line) + text.len()
    }

    pub fn previous_word_start(&self, offset: usize) -> usize {
        let (line, column) = self.offset_to_point(offset);
        if column == 0 {
            return self.previous_boundary(offset);
        }
        let text = self.line_text(line);
        let mut chars = text[..column].char_indices().rev().peekable();
        while let Some((_, ch)) = chars.peek() {
            if !ch.is_whitespace() {
                break;
            }
            chars.next();
        }
        let Some(&(mut start, first)) = chars.peek() else {
            return self.line_start(line);
        };
        let kind = char_kind(first);
        for (index, ch) in chars {
            if char_kind(ch) != kind {
                break;
            }
            start = index;
        }
        self.line_start(line) + start
    }

    /// Returns the word that touches `offset`, or an empty range at `offset`.
    pub fn word_range(&self, offset: usize) -> Range<usize> {
        let (line, column) = self.offset_to_point(offset);
        let text = self.line_text(line);
        let base = self.line_start(line);
        let mut start = column;
        for (index, ch) in text[..column].char_indices().rev() {
            if char_kind(ch) != CharKind::Word {
                break;
            }
            start = index;
        }
        let mut end = column;
        for (index, ch) in text[column..].char_indices() {
            if char_kind(ch) != CharKind::Word {
                break;
            }
            end = column + index + ch.len_utf8();
        }
        base + start..base + end
    }

    pub fn first_non_whitespace(&self, line: usize) -> usize {
        let text = self.line_text(line);
        let indent = text.len() - text.trim_start().len();
        self.line_start(line) + indent
    }
}

impl std::fmt::Display for Buffer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for chunk in self.rope.chunks() {
            f.write_str(chunk)?;
        }
        Ok(())
    }
}

#[derive(PartialEq, Eq, Clone, Copy)]
enum CharKind {
    Word,
    Whitespace,
    Punctuation,
}

fn char_kind(ch: char) -> CharKind {
    if ch.is_alphanumeric() || ch == '_' {
        CharKind::Word
    } else if ch.is_whitespace() {
        CharKind::Whitespace
    } else {
        CharKind::Punctuation
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_hide_line_endings() {
        let buffer = Buffer::new("one\r\ntwo\nthree");
        assert_eq!(buffer.line_count(), 3);
        assert_eq!(buffer.line_text(0), "one");
        assert_eq!(buffer.line_text(1), "two");
        assert_eq!(buffer.line_text(2), "three");
        assert_eq!(buffer.next_boundary(buffer.line_end(0)), 5);
        assert_eq!(buffer.previous_boundary(5), 3);
    }

    #[test]
    fn boundaries_respect_graphemes() {
        let buffer = Buffer::new("ae\u{301}x");
        assert_eq!(buffer.next_boundary(1), 4);
        assert_eq!(buffer.previous_boundary(4), 1);
    }

    #[test]
    fn utf16_conversion_round_trips() {
        let buffer = Buffer::new("a😀b");
        assert_eq!(buffer.offset_to_utf16(5), 3);
        assert_eq!(buffer.utf16_to_offset(3), 5);
    }

    #[test]
    fn word_motion_groups_character_kinds() {
        let buffer = Buffer::new("\\section{Über uns}");
        assert_eq!(buffer.next_word_end(0), 1);
        assert_eq!(buffer.next_word_end(1), 8);
        assert_eq!(buffer.previous_word_start(8), 1);
        assert_eq!(buffer.word_range(11), 9..14);
    }
}
