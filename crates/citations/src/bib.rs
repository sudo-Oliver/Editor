//! A forgiving reader of `.bib` files. One pass over the bytes, no regex: every
//! entry keeps where it is, and every mistake that would stop BibTeX or biber
//! gets a range, so that one broken entry costs only itself.

use std::{collections::HashMap, ops::Range};

use crate::{Diagnostic, Severity};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Bib {
    pub entries: Vec<Entry>,
    /// `@string` definitions in the order of the file.
    pub strings: Vec<StringDef>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Entry {
    /// Such as `article`, in lowercase.
    pub kind: String,
    pub key: String,
    pub key_range: Range<usize>,
    /// From `@` to the closing brace, or to where an unclosed entry stops.
    pub range: Range<usize>,
    /// The line of the `@`, from 0.
    pub line: usize,
    pub fields: Vec<Field>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Field {
    /// In lowercase.
    pub name: String,
    pub name_range: Range<usize>,
    pub value: Value,
    pub value_range: Range<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StringDef {
    /// In lowercase, as BibTeX compares them.
    pub name: String,
    pub value: Value,
    pub range: Range<usize>,
}

/// The parts of a value that `#` joins.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Value {
    pub parts: Vec<Part>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Part {
    /// What braces or quotes hold, or a number.
    Text(String),
    /// A name an `@string` defines, in lowercase.
    Macro { name: String, range: Range<usize> },
}

/// The months BibTeX defines without an `@string`.
const MONTHS: [(&str, &str); 12] = [
    ("jan", "January"),
    ("feb", "February"),
    ("mar", "March"),
    ("apr", "April"),
    ("may", "May"),
    ("jun", "June"),
    ("jul", "July"),
    ("aug", "August"),
    ("sep", "September"),
    ("oct", "October"),
    ("nov", "November"),
    ("dec", "December"),
];

impl Value {
    /// The text, with macros looked up in `macros` and then among the months.
    /// Names neither knows go into `unknown`.
    pub fn resolve<'a>(&'a self, macros: &HashMap<String, String>, unknown: &mut Vec<&'a Part>) -> String {
        let mut out = String::new();
        for part in &self.parts {
            match part {
                Part::Text(text) => out.push_str(text),
                Part::Macro { name, .. } => match macros.get(name).map(String::as_str).or_else(|| MONTHS.iter().find(|(month, _)| month == name).map(|(_, full)| *full)) {
                    Some(text) => out.push_str(text),
                    None => unknown.push(part),
                },
            }
        }
        out
    }
}

impl Entry {
    pub fn field(&self, name: &str) -> Option<&Field> {
        self.fields.iter().find(|field| field.name == name)
    }
}

/// Fields whose text is a link or a path, where `&`, `%` and `#` are fine.
const VERBATIM_FIELDS: &[&str] = &["url", "doi", "eprint", "file", "pdf", "urldate", "verba", "verbb", "verbc", "uri", "urlraw"];

pub fn parse(text: &str) -> Bib {
    let mut parser = Parser { text, bytes: text.as_bytes(), pos: 0, bib: Bib::default(), counted: (0, 0) };
    parser.run();
    parser.bib
}

struct Parser<'a> {
    text: &'a str,
    bytes: &'a [u8],
    pos: usize,
    bib: Bib,
    /// A position and its line, from which the next line is counted on.
    counted: (usize, usize),
}

/// How a value ended.
enum Ended {
    Fine,
    /// A brace or quote never closed, so the entry stops at the next one.
    Unclosed,
}

impl Parser<'_> {
    fn error(&mut self, range: Range<usize>, message: impl Into<String>) {
        self.bib.diagnostics.push(Diagnostic { range, severity: Severity::Error, message: message.into() });
    }

    fn warn(&mut self, range: Range<usize>, message: impl Into<String>) {
        self.bib.diagnostics.push(Diagnostic { range, severity: Severity::Warning, message: message.into() });
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    /// The line of `at`, counted on from the last one asked, since entries come in order.
    fn line_of(&mut self, at: usize) -> usize {
        let (from, line) = self.counted;
        let line = if at >= from { line + self.bytes[from..at].iter().filter(|byte| **byte == b'\n').count() } else { self.bytes[..at].iter().filter(|byte| **byte == b'\n').count() };
        self.counted = (at, line);
        line
    }

    fn skip_whitespace(&mut self) {
        while self.peek().is_some_and(|byte| byte.is_ascii_whitespace()) {
            self.pos += 1;
        }
    }

    /// Whether only spaces stand between the start of the line and `at`.
    fn starts_line(&self, at: usize) -> bool {
        self.bytes[..at].iter().rev().take_while(|byte| **byte != b'\n').all(|byte| *byte == b' ' || *byte == b'\t')
    }

    /// Whether an entry such as `@article{` starts a line at `at`, where an
    /// unclosed entry before it gives up.
    fn entry_starts(&self, at: usize) -> bool {
        if self.bytes.get(at) != Some(&b'@') || !self.starts_line(at) {
            return false;
        }
        let mut index = at + 1;
        let name = index;
        while self.bytes.get(index).is_some_and(u8::is_ascii_alphabetic) {
            index += 1;
        }
        if index == name {
            return false;
        }
        while self.bytes.get(index).is_some_and(|byte| *byte == b' ' || *byte == b'\t') {
            index += 1;
        }
        matches!(self.bytes.get(index), Some(b'{' | b'('))
    }

    fn run(&mut self) {
        while self.pos < self.bytes.len() {
            match self.bytes[self.pos] {
                // Outside entries, biber reads `%` as a comment, and BibTeX ignores the line anyway.
                b'%' => self.skip_line(),
                b'@' => self.item(),
                _ => self.pos += 1,
            }
        }
    }

    fn skip_line(&mut self) {
        self.pos = self.bytes[self.pos..].iter().position(|byte| *byte == b'\n').map_or(self.bytes.len(), |newline| self.pos + newline + 1);
    }

    fn item(&mut self) {
        let at = self.pos;
        self.pos += 1;
        self.skip_whitespace();
        let name_start = self.pos;
        while self.peek().is_some_and(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-') {
            self.pos += 1;
        }
        // A lone `@`, as in an address between entries, is text BibTeX skips.
        if self.pos == name_start {
            return;
        }
        let kind = self.text[name_start..self.pos].to_ascii_lowercase();
        self.skip_whitespace();
        let closer = match self.peek() {
            Some(b'{') => b'}',
            Some(b'(') => b')',
            _ => {
                self.error(at..self.pos, format!("@{kind} needs a {{ after it."));
                return;
            }
        };
        self.pos += 1;
        match kind.as_str() {
            "comment" => self.skip_comment(at, closer),
            "preamble" => {
                self.value("preamble");
                self.close(at, closer);
            }
            "string" => self.string(at, closer),
            _ => self.entry(at, kind, closer),
        }
    }

    /// `@comment{…}` takes everything up to its closing brace.
    fn skip_comment(&mut self, at: usize, closer: u8) {
        let mut depth = 0usize;
        while let Some(byte) = self.peek() {
            match byte {
                b'{' => depth += 1,
                b'}' if depth > 0 => depth -= 1,
                byte if byte == closer && depth == 0 => {
                    self.pos += 1;
                    return;
                }
                b'@' if self.entry_starts(self.pos) => break,
                _ => {}
            }
            self.pos += 1;
        }
        self.error(at..at + 1, "This @comment is never closed.");
    }

    /// The closing brace of an entry, after optional commas.
    fn close(&mut self, at: usize, closer: u8) {
        loop {
            self.skip_whitespace();
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(byte) if byte == closer => {
                    self.pos += 1;
                    return;
                }
                _ => break,
            }
        }
        self.error(at..at + 1, format!("A '{}' is missing here.", closer as char));
        self.recover();
    }

    /// Skips to the next line that starts an entry.
    fn recover(&mut self) {
        while self.pos < self.bytes.len() && !self.entry_starts(self.pos) {
            self.pos += 1;
        }
    }

    fn string(&mut self, at: usize, closer: u8) {
        self.skip_whitespace();
        let start = self.pos;
        let name = self.identifier();
        if name.is_empty() {
            self.error(at..self.pos.max(at + 1), "@string needs a name.");
            self.recover();
            return;
        }
        self.skip_whitespace();
        if self.peek() != Some(b'=') {
            self.error(start..self.pos, format!("@string {name} needs an = before its text."));
            self.recover();
            return;
        }
        self.pos += 1;
        let (value, _, ended) = self.value(&name);
        if let Ended::Unclosed = ended {
            return;
        }
        self.bib.strings.push(StringDef { name: name.to_ascii_lowercase(), value, range: at..self.pos });
        self.close(at, closer);
    }

    /// A field name, a key of `@string`, or a macro: what BibTeX allows in names.
    fn identifier(&mut self) -> String {
        let start = self.pos;
        while self.peek().is_some_and(|byte| !byte.is_ascii_whitespace() && !matches!(byte, b'"' | b'#' | b'%' | b'\'' | b'(' | b')' | b',' | b'=' | b'{' | b'}')) {
            self.pos += 1;
        }
        self.text[start..self.pos].to_string()
    }

    fn entry(&mut self, at: usize, kind: String, closer: u8) {
        self.skip_whitespace();
        let key_start = self.pos;
        while self.peek().is_some_and(|byte| !byte.is_ascii_whitespace() && byte != b',' && byte != closer && byte != b'=' && byte != b'}') {
            self.pos += 1;
        }
        let mut key_range = key_start..self.pos;
        self.skip_whitespace();
        if self.peek() == Some(b'=') {
            // `@article{title = …`: the key is missing and this is the first field.
            self.pos = key_start;
            key_range = key_start..key_start;
        }
        if key_range.is_empty() {
            self.error(at..key_start.max(at + 1), format!("This @{kind} has no key."));
        }
        let line = self.line_of(at);
        let mut entry = Entry { kind, key: self.text[key_range.clone()].to_string(), key_range, range: at..at, line, fields: Vec::new() };
        loop {
            self.skip_whitespace();
            let Some(byte) = self.peek() else {
                self.error(at..entry.key_range.end.max(at + 1), format!("The entry {} is never closed.", entry.key));
                break;
            };
            match byte {
                byte if byte == closer => {
                    self.pos += 1;
                    break;
                }
                b',' => self.pos += 1,
                b'@' if self.entry_starts(self.pos) => {
                    self.error(at..entry.key_range.end.max(at + 1), format!("The entry {} is never closed: a '{}' is missing.", entry.key, closer as char));
                    break;
                }
                b'%' => {
                    let line = self.pos;
                    self.skip_line();
                    self.warn(line..self.pos.saturating_sub(1).max(line), "Only biber reads % as a comment inside an entry. BibTeX takes this line for a field and stops.");
                }
                _ => {
                    if !self.field(&mut entry, closer) {
                        break;
                    }
                }
            }
        }
        entry.range = at..self.pos;
        self.bib.entries.push(entry);
    }

    /// Reads `name = value`. False when the entry has to stop.
    fn field(&mut self, entry: &mut Entry, closer: u8) -> bool {
        let start = self.pos;
        let name = self.identifier();
        if name.is_empty() {
            let end = self.pos + 1;
            self.error(start..end.min(self.bytes.len()), format!("Unexpected '{}' in {}.", self.bytes[start] as char, entry.key));
            self.pos = end;
            self.skip_to_next_field(closer);
            return true;
        }
        let name_range = start..self.pos;
        self.skip_whitespace();
        if self.peek() != Some(b'=') {
            self.error(name_range.clone(), format!("The field {name} in {} needs an = before its value.", entry.key));
            self.skip_to_next_field(closer);
            return true;
        }
        self.pos += 1;
        let name = name.to_ascii_lowercase();
        let (value, value_range, ended) = self.value(&name);
        if let Ended::Unclosed = ended {
            entry.fields.push(Field { name, name_range, value, value_range });
            return false;
        }
        if !VERBATIM_FIELDS.contains(&name.as_str()) {
            self.check_text(&name, &value, value_range.clone());
        }
        entry.fields.push(Field { name, name_range, value, value_range: value_range.clone() });
        self.skip_whitespace();
        match self.peek() {
            Some(b',') | None => {}
            Some(byte) if byte == closer => {}
            Some(b'@') if self.entry_starts(self.pos) => {}
            Some(_) => self.error(value_range.end..self.pos.max(value_range.end + 1).min(self.bytes.len()), format!("A comma is missing after the field {} in {}.", entry.fields.last().map_or("", |field| field.name.as_str()), entry.key)),
        }
        true
    }

    /// After a broken field: on at the next comma or the end of the entry.
    fn skip_to_next_field(&mut self, closer: u8) {
        let mut depth = 0usize;
        while let Some(byte) = self.peek() {
            match byte {
                b'{' => depth += 1,
                b'}' if depth > 0 => depth -= 1,
                b',' if depth == 0 => return,
                byte if byte == closer && depth == 0 => return,
                b'@' if self.entry_starts(self.pos) => return,
                _ => {}
            }
            self.pos += 1;
        }
    }

    /// `&`, `%` and `#` mean something to TeX, so in a field they need a backslash.
    fn check_text(&mut self, field: &str, value: &Value, range: Range<usize>) {
        let raw = &self.text[range.clone()];
        if raw.contains("\\url") || raw.contains("\\href") {
            return;
        }
        let mut found = Vec::new();
        for part in &value.parts {
            if let Part::Text(text) = part {
                let bytes = text.as_bytes();
                for (index, byte) in bytes.iter().enumerate() {
                    if matches!(byte, b'&' | b'%' | b'#') && (index == 0 || bytes[index - 1] != b'\\') && !found.contains(byte) {
                        found.push(*byte);
                    }
                }
            }
        }
        for byte in found {
            let what = byte as char;
            // The first raw one, for the mark.
            let at = raw.bytes().enumerate().position(|(index, candidate)| candidate == byte && (index == 0 || raw.as_bytes()[index - 1] != b'\\')).map_or(range.start, |index| range.start + index);
            self.warn(at..at + 1, format!("The {what} in {field} needs a backslash, as in \\{what}, or the build stops."));
        }
    }

    /// Parts joined by `#`, after the `=`.
    fn value(&mut self, field: &str) -> (Value, Range<usize>, Ended) {
        let mut value = Value::default();
        self.skip_whitespace();
        let start = self.pos;
        // Where the last part ends, before the spaces that look for a `#`.
        let mut end = start;
        loop {
            self.skip_whitespace();
            let part_start = self.pos;
            match self.peek() {
                Some(b'{') => match self.delimited(b'}') {
                    Some(text) => value.parts.push(Part::Text(text)),
                    None => {
                        self.error(part_start..part_start + 1, format!("The {{ of {field} is never closed."));
                        return (value, start..self.pos, Ended::Unclosed);
                    }
                },
                Some(b'"') => match self.delimited(b'"') {
                    Some(text) => value.parts.push(Part::Text(text)),
                    None => {
                        self.error(part_start..part_start + 1, format!("The quote of {field} is never closed."));
                        return (value, start..self.pos, Ended::Unclosed);
                    }
                },
                Some(byte) if byte.is_ascii_digit() => {
                    while self.peek().is_some_and(|byte| byte.is_ascii_digit()) {
                        self.pos += 1;
                    }
                    value.parts.push(Part::Text(self.text[part_start..self.pos].to_string()));
                }
                Some(_) => {
                    let name = self.identifier();
                    if name.is_empty() {
                        self.error(part_start..(part_start + 1).min(self.bytes.len()).max(part_start), format!("{field} has no value."));
                        break;
                    }
                    value.parts.push(Part::Macro { name: name.to_ascii_lowercase(), range: part_start..self.pos });
                }
                None => {
                    self.error(start.saturating_sub(1)..start, format!("{field} has no value."));
                    break;
                }
            }
            end = self.pos;
            self.skip_whitespace();
            if self.peek() == Some(b'#') {
                self.pos += 1;
            } else {
                break;
            }
        }
        (value, start..end, Ended::Fine)
    }

    /// The text between `{…}` or `"…"`, with nested braces. `None`, with the
    /// position at the next entry, when it never closes.
    fn delimited(&mut self, closer: u8) -> Option<String> {
        let open = self.pos;
        self.pos += 1;
        let mut depth = 0usize;
        while let Some(byte) = self.peek() {
            match byte {
                b'{' => depth += 1,
                b'}' if closer == b'}' && depth == 0 => {
                    self.pos += 1;
                    return Some(self.text[open + 1..self.pos - 1].to_string());
                }
                b'}' => depth = depth.saturating_sub(1),
                b'"' if closer == b'"' && depth == 0 => {
                    self.pos += 1;
                    return Some(self.text[open + 1..self.pos - 1].to_string());
                }
                // A line that starts the next entry: this one swallowed it.
                b'@' if self.entry_starts(self.pos) => return None,
                _ => {}
            }
            self.pos += 1;
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(bib: &Bib) -> Vec<&str> {
        bib.entries.iter().map(|entry| entry.key.as_str()).collect()
    }

    fn text_of(bib: &Bib, key: &str, field: &str) -> String {
        let macros: HashMap<String, String> = bib.strings.iter().map(|string| (string.name.clone(), string.value.resolve(&HashMap::new(), &mut Vec::new()))).collect();
        bib.entries.iter().find(|entry| entry.key == key).and_then(|entry| entry.field(field)).map(|field| field.value.resolve(&macros, &mut Vec::new())).unwrap_or_default()
    }

    fn messages(bib: &Bib) -> Vec<&str> {
        bib.diagnostics.iter().map(|diagnostic| diagnostic.message.as_str()).collect()
    }

    #[test]
    fn a_clean_file_has_no_diagnostics() {
        let text = "@string{tug = \"TUGBoat\"}\n@preamble{\"\\newcommand{\\x}{y}\"}\n@comment{anything {nested} here}\n@article{a,\n  author = {M{\\\"u}ller, Anna},\n  title = \"A {Rust} \\& {GPU} story\",\n  journal = tug # \" Extra\",\n  year = 2024,\n  month = mar,\n}\n@book(b, title = {With (parens)})\n@misc{c}\n";
        let bib = parse(text);
        assert_eq!(keys(&bib), ["a", "b", "c"]);
        assert!(bib.diagnostics.is_empty(), "{:?}", bib.diagnostics);
        assert_eq!(text_of(&bib, "a", "journal"), "TUGBoat Extra");
        assert_eq!(text_of(&bib, "a", "year"), "2024");
        assert_eq!(text_of(&bib, "a", "month"), "March");
        assert_eq!(text_of(&bib, "b", "title"), "With (parens)");
        let a = &bib.entries[0];
        assert_eq!((a.line, bib.entries[1].line, bib.entries[2].line), (3, 10, 11));
        assert_eq!(&text[a.key_range.clone()], "a");
        assert!(text[a.range.clone()].starts_with("@article{a,") && text[a.range.clone()].ends_with("}"));
    }

    #[test]
    fn a_missing_comma_is_found_and_the_rest_still_reads() {
        let text = "@article{a,\n  title = {One}\n  year = 2001,\n}\n@book{b, title = {Two}}\n";
        let bib = parse(text);
        assert_eq!(keys(&bib), ["a", "b"]);
        assert_eq!(text_of(&bib, "a", "year"), "2001");
        assert_eq!(messages(&bib), ["A comma is missing after the field title in a."]);
        let (line, _) = crate::text::line_col(text, bib.diagnostics[0].range.start);
        assert_eq!(line, 1);
    }

    #[test]
    fn an_unclosed_entry_loses_only_itself() {
        let text = "@article{first, title = {One}, year = 2001}\n@article{broken, title = {Never closed, year = 2002\n\n@book{after, title = {Two}, year = 2003}\n@misc{last, title = {Three}}\n";
        let bib = parse(text);
        assert_eq!(keys(&bib), ["first", "broken", "after", "last"]);
        assert_eq!(text_of(&bib, "after", "year"), "2003");
        assert_eq!(messages(&bib), ["The { of title is never closed."]);
        let (line, col) = crate::text::line_col(text, bib.diagnostics[0].range.start);
        assert_eq!((line, col), (1, 25));
    }

    #[test]
    fn an_entry_without_its_closing_brace_ends_at_the_next() {
        let bib = parse("@article{a, title = {One}, year = 2001\n@book{b, title = {Two}}\n");
        assert_eq!(keys(&bib), ["a", "b"]);
        assert_eq!(messages(&bib), ["The entry a is never closed: a '}' is missing."]);
    }

    #[test]
    fn an_unclosed_quote_is_found() {
        let bib = parse("@article{a, title = \"One, year = 2001}\n@book{b, title = {Two}}\n");
        assert_eq!(keys(&bib), ["a", "b"]);
        assert_eq!(messages(&bib), ["The quote of title is never closed."]);
    }

    #[test]
    fn raw_tex_specials_warn_but_links_do_not() {
        let bib = parse("@article{a, title = {Salt & Pepper}, abstract = {50% more \\& #1}, url = {https://x.org/?a=1&b=2%20}, note = {\\url{a%b}}}");
        assert_eq!(
            messages(&bib),
            ["The & in title needs a backslash, as in \\&, or the build stops.", "The % in abstract needs a backslash, as in \\%, or the build stops.", "The # in abstract needs a backslash, as in \\#, or the build stops."]
        );
    }

    #[test]
    fn keys_and_equals_signs_that_are_missing() {
        let bib = parse("@article{title = {No key}, year = 1}\n@book{b, title {No equals}, year = 2}\n");
        assert_eq!(keys(&bib), ["", "b"]);
        assert_eq!(text_of(&bib, "", "year"), "1");
        assert_eq!(text_of(&bib, "b", "year"), "2");
        assert_eq!(messages(&bib), ["This @article has no key.", "The field title in b needs an = before its value."]);
    }

    #[test]
    fn percent_lines_are_comments_outside_and_warned_inside() {
        let bib = parse("% @article{gone, title = {X}}\n@article{a,\n  % note = {old},\n  title = {T},\n}\n");
        assert_eq!(keys(&bib), ["a"]);
        assert_eq!(text_of(&bib, "a", "title"), "T");
        assert_eq!(bib.diagnostics.len(), 1);
        assert_eq!(bib.diagnostics[0].severity, Severity::Warning);
    }

    #[test]
    fn an_at_sign_inside_a_value_is_text() {
        let bib = parse("@misc{a, note = {mail me@example.org}, howpublished = \"@home\"}\n@misc{b}");
        assert_eq!(keys(&bib), ["a", "b"]);
        assert_eq!(text_of(&bib, "a", "note"), "mail me@example.org");
        assert!(bib.diagnostics.is_empty());
    }

    #[test]
    fn crlf_and_unicode_keep_their_ranges() {
        let text = "\u{feff}@article{müller2024,\r\n  title = {Straße},\r\n}\r\n";
        let bib = parse(text);
        assert_eq!(keys(&bib), ["müller2024"]);
        assert_eq!(&text[bib.entries[0].key_range.clone()], "müller2024");
        assert!(bib.diagnostics.is_empty());
    }

    #[test]
    fn unknown_macros_are_reported_by_resolve() {
        let bib = parse("@article{a, journal = icml # \" 2020\"}");
        let mut unknown = Vec::new();
        let text = bib.entries[0].fields[0].value.resolve(&HashMap::new(), &mut unknown);
        assert_eq!(text, " 2020");
        assert!(matches!(unknown.as_slice(), [Part::Macro { name, .. }] if name == "icml"));
    }

    /// Byte by byte damage must never panic, and every range must stay in the text.
    #[test]
    fn damaged_files_never_panic() {
        let base = "@string{s = \"S\"}\n@article{a,\n  author = {M{\\\"u}ller and Others},\n  title = \"T {x} y\" # s,\n  year = 2020,\n}\n@book(b, title = {(x)})\n% c\n@comment{z}\n";
        let specials = *b"{}\"@,=#%()\nx";
        let mut seed = 7u64;
        for _ in 0..4000 {
            let mut bytes = base.as_bytes().to_vec();
            for _ in 0..3 {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                let at = (seed >> 33) as usize % bytes.len();
                let byte = specials[(seed >> 20) as usize % specials.len()];
                match (seed >> 10) % 3 {
                    0 => bytes[at] = byte,
                    1 => bytes.insert(at, byte),
                    _ => {
                        bytes.remove(at);
                    }
                }
            }
            let text = String::from_utf8_lossy(&bytes);
            let bib = parse(&text);
            for entry in &bib.entries {
                assert!(entry.range.end <= text.len() && text.get(entry.key_range.clone()).is_some());
                for field in &entry.fields {
                    assert!(text.get(field.value_range.clone()).is_some() && text.get(field.name_range.clone()).is_some());
                }
            }
            for diagnostic in &bib.diagnostics {
                assert!(diagnostic.range.start <= diagnostic.range.end && diagnostic.range.end <= text.len(), "{diagnostic:?} in {text:?}");
            }
        }
    }
}
