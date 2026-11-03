//! Text templates with slots that `Tab` walks through.
//!
//! `${}` marks an empty slot and `${text}` a slot that starts with `text`
//! selected. Slots count in document order. `$$` stands for a literal `$`.

use std::ops::Range;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snippet {
    pub text: String,
    pub slots: Vec<Range<usize>>,
}

impl Snippet {
    /// Reads `template` and puts `indent` in front of every line but the first.
    pub fn parse(template: &str, indent: &str) -> Self {
        let mut text = String::with_capacity(template.len());
        let mut slots = Vec::new();
        let mut rest = template;
        while let Some(ch) = rest.chars().next() {
            if let Some(after) = rest.strip_prefix("$$") {
                text.push('$');
                rest = after;
            } else if let Some((default, after)) =
                rest.strip_prefix("${").and_then(|after| after.split_once('}'))
            {
                let start = text.len();
                text.push_str(default);
                slots.push(start..text.len());
                rest = after;
            } else {
                text.push(ch);
                if ch == '\n' {
                    text.push_str(indent);
                }
                rest = &rest[ch.len_utf8()..];
            }
        }
        Self { text, slots }
    }

    /// Replaces the first slot with `filling`, which stays one slot.
    pub fn fill_first(&mut self, filling: &str) {
        let Some(first) = self.slots.first().cloned() else {
            return;
        };
        self.text.replace_range(first.clone(), filling);
        let delta = filling.len() as isize - first.len() as isize;
        self.slots[0] = first.start..first.start + filling.len();
        for slot in &mut self.slots[1..] {
            *slot = (slot.start as isize + delta) as usize..(slot.end as isize + delta) as usize;
        }
    }

    /// Adds text before and after the template.
    pub fn surround(&mut self, prefix: &str, suffix: &str) {
        self.text.insert_str(0, prefix);
        self.text.push_str(suffix);
        for slot in &mut self.slots {
            *slot = slot.start + prefix.len()..slot.end + prefix.len();
        }
    }
}

/// Makes outside text, such as a file name, safe to use in a template.
pub fn literal(text: &str) -> String {
    text.replace('$', "$$")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_slots_and_defaults() {
        let snippet = Snippet::parse("\\frac{${}}{${b}}", "");
        assert_eq!(snippet.text, "\\frac{}{b}");
        assert_eq!(snippet.slots, vec![6..6, 8..9]);
    }

    #[test]
    fn offsets_count_bytes() {
        let snippet = Snippet::parse("ä${ö}ü${}", "");
        assert_eq!(snippet.text, "äöü");
        assert_eq!(snippet.slots, vec![2..4, 6..6]);
    }

    #[test]
    fn indents_following_lines() {
        let snippet = Snippet::parse("a\n${}\nb", "  ");
        assert_eq!(snippet.text, "a\n  \n  b");
        assert_eq!(snippet.slots, vec![4..4]);
    }

    #[test]
    fn literal_text_stays_out_of_slots() {
        let snippet = Snippet::parse(&literal("a${b}$"), "");
        assert_eq!(snippet.text, "a${b}$");
        assert!(snippet.slots.is_empty());
    }

    #[test]
    fn filling_moves_later_slots() {
        let mut snippet = Snippet::parse("${x}-${}", "");
        snippet.fill_first("long");
        assert_eq!(snippet.text, "long-");
        assert_eq!(snippet.slots, vec![0..4, 5..5]);
    }

    #[test]
    fn surrounding_moves_slots() {
        let mut snippet = Snippet::parse("a${}", "");
        snippet.surround("$", "$");
        assert_eq!(snippet.text, "$a$");
        assert_eq!(snippet.slots, vec![2..2]);
    }
}
