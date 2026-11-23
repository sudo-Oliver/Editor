//! Reads a Typst file once: markup, embedded code after `#`, content blocks in
//! code, math, raw text and comments, each as Typst tells them apart, so that
//! an `@` or a `<label>` counts only where Typst reads one.

use std::ops::Range;

use crate::scan::{BibKind, BibSource, Include, IncludeKind, Key, Label, Scan};

pub fn scan(text: &str) -> Scan {
    let mut scanner = Scanner { text, bytes: text.as_bytes(), pos: 0, scan: Scan::default(), calls: Vec::new(), last: None, heading: None };
    scanner.heading_at_line_start();
    scanner.markup(None);
    let mut scan = scanner.scan;
    crate::scan::number_lines(text, &mut scan.labels);
    scan
}

/// What a label right after it belongs to.
#[derive(Clone)]
struct Element {
    end: usize,
    env: &'static str,
    caption: Option<String>,
}

struct Scanner<'a> {
    text: &'a str,
    bytes: &'a [u8],
    pos: usize,
    scan: Scan,
    /// The functions whose arguments are being read, innermost last.
    calls: Vec<String>,
    last: Option<Element>,
    /// The title of the heading on the current line.
    heading: Option<(usize, String)>,
}

fn label_char(ch: char) -> bool {
    ch.is_alphanumeric() || matches!(ch, '_' | '-' | ':' | '.')
}

impl Scanner<'_> {
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn at(&self, text: &str) -> bool {
        self.text[self.pos..].starts_with(text)
    }

    fn char_at(&self, at: usize) -> Option<char> {
        self.text.get(at..)?.chars().next()
    }

    /// One char on, whole even when it takes several bytes.
    fn step(&mut self) {
        self.pos += self.char_at(self.pos).map_or(1, char::len_utf8);
    }

    fn line_end(&self, from: usize) -> usize {
        self.bytes[from..].iter().position(|byte| *byte == b'\n').map_or(self.bytes.len(), |newline| from + newline)
    }

    /// Skips `//` and `/* */` comments. True when it skipped one.
    fn comment(&mut self) -> bool {
        if self.at("//") {
            self.pos = self.line_end(self.pos);
            return true;
        }
        if self.at("/*") {
            let mut depth = 0usize;
            while self.pos < self.bytes.len() {
                if self.at("/*") {
                    depth += 1;
                    self.pos += 2;
                } else if self.at("*/") {
                    depth -= 1;
                    self.pos += 2;
                    if depth == 0 {
                        break;
                    }
                } else {
                    self.step();
                }
            }
            return true;
        }
        false
    }

    /// Skips raw text in backticks, inline or as a block.
    fn raw(&mut self) {
        let start = self.pos;
        while self.peek() == Some(b'`') {
            self.pos += 1;
        }
        let ticks = self.pos - start;
        if ticks == 2 {
            return;
        }
        let fence = &self.text[start..self.pos];
        self.pos = self.text[self.pos..].find(fence).map_or(self.bytes.len(), |at| self.pos + at + ticks);
    }

    /// The label chars from `from`, as a range.
    fn label_run(&self, from: usize) -> Range<usize> {
        let mut end = from;
        while let Some(ch) = self.char_at(end).filter(|ch| label_char(*ch)) {
            end += ch.len_utf8();
        }
        from..end
    }

    /// `<name>` at the position, read whole, or nothing.
    fn label_literal(&mut self) -> Option<Range<usize>> {
        let run = self.label_run(self.pos + 1);
        if run.is_empty() || self.bytes.get(run.end) != Some(&b'>') {
            return None;
        }
        self.pos = run.end + 1;
        Some(run)
    }

    fn markup(&mut self, close: Option<u8>) {
        let mut depth = 0usize;
        while let Some(byte) = self.peek() {
            match byte {
                b'\\' => {
                    self.pos += 1;
                    self.step();
                }
                b'/' if self.comment() => {}
                b'`' => self.raw(),
                b'$' => self.math(),
                b'[' => {
                    depth += 1;
                    self.pos += 1;
                }
                b']' if depth > 0 => {
                    depth -= 1;
                    self.pos += 1;
                }
                b']' if close == Some(b']') => {
                    self.pos += 1;
                    return;
                }
                b'\n' => {
                    self.heading = None;
                    self.pos += 1;
                    self.heading_at_line_start();
                }
                b'<' => match self.label_literal() {
                    Some(run) => self.define_label(run),
                    None => self.pos += 1,
                },
                b'@' => self.reference(),
                b'#' => {
                    self.pos += 1;
                    self.embedded();
                }
                _ => self.step(),
            }
        }
    }

    /// `= Title` at the start of a line names the labels on it.
    fn heading_at_line_start(&mut self) {
        let rest = &self.text[self.pos..];
        let trimmed = rest.trim_start_matches([' ', '\t']);
        let level = trimmed.bytes().take_while(|byte| *byte == b'=').count();
        if level > 0 && trimmed[level..].starts_with(' ') {
            let line = &trimmed[level..trimmed.find('\n').unwrap_or(trimmed.len())];
            let title = line.split('<').next().unwrap_or_default().trim().to_string();
            self.heading = Some((self.pos, title));
        }
    }

    fn define_label(&mut self, run: Range<usize>) {
        let before = self.text[..run.start - 1].trim_end();
        let (env, caption) = match (&self.last, &self.heading) {
            (Some(element), _) if element.end == before.len() => (element.env, element.caption.clone()),
            (_, Some((_, title))) => ("heading", Some(title.clone())),
            _ => ("", None),
        };
        self.scan.labels.push(Label { name: self.text[run.clone()].to_string(), range: run, line: 0, env: env.to_string(), caption, before_caption: false });
    }

    fn reference(&mut self) {
        let start = self.pos;
        let after_word = self.text[..start].chars().next_back().is_some_and(|ch| ch.is_alphanumeric());
        self.pos += 1;
        let mut run = self.label_run(self.pos);
        // A full stop or colon after it ends the sentence, not the name.
        while run.end > run.start && matches!(self.bytes[run.end - 1], b'.' | b':') {
            run.end -= 1;
        }
        if after_word || run.is_empty() {
            return;
        }
        self.pos = run.end;
        self.scan.either.push(Key { name: self.text[run.clone()].to_string(), range: run, command: "@".into() });
        // `@key[p. 5]`: the supplement is markup.
        if self.peek() == Some(b'[') {
            self.pos += 1;
            self.markup(Some(b']'));
        }
    }

    fn math(&mut self) {
        self.pos += 1;
        while let Some(byte) = self.peek() {
            match byte {
                b'\\' => {
                    self.pos += 1;
                    self.step();
                }
                b'$' => {
                    self.pos += 1;
                    self.last = Some(Element { end: self.pos, env: "equation", caption: None });
                    return;
                }
                b'#' => {
                    self.pos += 1;
                    self.embedded();
                }
                b'/' if self.comment() => {}
                _ => self.step(),
            }
        }
    }

    /// The expression after `#` in markup or math.
    fn embedded(&mut self) {
        match self.peek() {
            Some(b'(') => {
                self.pos += 1;
                self.code(b')');
            }
            Some(b'{') => {
                self.pos += 1;
                self.code(b'}');
            }
            Some(b'[') => {
                self.pos += 1;
                self.markup(Some(b']'));
            }
            Some(byte) if byte.is_ascii_alphabetic() || byte == b'_' => {
                let name = self.identifier();
                match name.as_str() {
                    "include" => {
                        self.spaces();
                        if self.peek() == Some(b'"') {
                            self.include_string();
                        }
                        self.statement();
                    }
                    "let" | "set" | "show" | "import" | "if" | "for" | "while" | "return" | "context" => self.statement(),
                    _ => self.postfix(name),
                }
            }
            _ => {}
        }
    }

    fn identifier(&mut self) -> String {
        let start = self.pos;
        while let Some(ch) = self.char_at(self.pos).filter(|ch| ch.is_alphanumeric() || *ch == '_' || *ch == '-') {
            self.pos += ch.len_utf8();
        }
        self.text[start..self.pos].to_string()
    }

    fn spaces(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t')) {
            self.pos += 1;
        }
    }

    /// Calls, fields and trailing content blocks after a name, as in `#figure(…)[…]`.
    fn postfix(&mut self, mut name: String) {
        loop {
            match self.peek() {
                Some(b'(') => {
                    self.pos += 1;
                    self.call(&name);
                }
                Some(b'[') => {
                    self.pos += 1;
                    self.markup(Some(b']'));
                }
                Some(b'.') if self.char_at(self.pos + 1).is_some_and(|ch| ch.is_alphabetic()) => {
                    self.pos += 1;
                    name = self.identifier();
                }
                _ => return,
            }
        }
    }

    fn call(&mut self, name: &str) {
        let start = self.pos;
        self.calls.push(name.to_string());
        self.code(b')');
        self.calls.pop();
        if name == "figure" {
            let caption = caption_of(&self.text[start..self.pos]);
            self.last = Some(Element { end: self.pos, env: "figure", caption });
        }
    }

    /// A statement after `#let` and the like runs to the end of the line,
    /// or further while brackets are open.
    fn statement(&mut self) {
        while let Some(byte) = self.peek() {
            match byte {
                b'\n' | b';' => return,
                _ => {
                    if !self.code_token() {
                        self.step();
                    }
                }
            }
        }
    }

    /// Code up to `close`.
    fn code(&mut self, close: u8) {
        while let Some(byte) = self.peek() {
            if byte == close {
                self.pos += 1;
                return;
            }
            if !self.code_token() {
                self.step();
            }
        }
    }

    /// One thing in code that matters: strings, comments, blocks, calls and
    /// labels. False when the byte is plain and the caller steps over it.
    fn code_token(&mut self) -> bool {
        match self.peek() {
            Some(b'"') => {
                let positional = !self.text[..self.pos].trim_end().ends_with(':');
                let range = self.string();
                if positional {
                    self.string_argument(range);
                }
            }
            Some(b'/') if self.comment() => {}
            Some(b'`') => self.raw(),
            Some(b'[') => {
                self.pos += 1;
                self.markup(Some(b']'));
            }
            Some(b'(') => {
                self.pos += 1;
                self.code(b')');
            }
            Some(b'{') => {
                self.pos += 1;
                self.code(b'}');
            }
            Some(b'<') => match self.label_literal() {
                Some(run) => self.label_value(run),
                None => self.pos += 1,
            },
            Some(byte) if byte.is_ascii_alphabetic() || byte == b'_' => {
                let name = self.identifier();
                if name == "include" {
                    self.spaces();
                    if self.peek() == Some(b'"') {
                        self.include_string();
                    }
                } else if self.peek() == Some(b'(') {
                    self.pos += 1;
                    self.call(&name);
                }
            }
            _ => return false,
        }
        true
    }

    /// A string literal, and the range of what is between its quotes.
    fn string(&mut self) -> Range<usize> {
        self.pos += 1;
        let start = self.pos;
        while let Some(byte) = self.peek() {
            match byte {
                b'\\' => {
                    self.pos += 1;
                    self.step();
                }
                b'"' => {
                    self.pos += 1;
                    return start..self.pos - 1;
                }
                b'\n' => break,
                _ => self.step(),
            }
        }
        start..self.pos
    }

    fn include_string(&mut self) {
        let range = self.string();
        self.scan.includes.push(Include { path: self.text[range.clone()].to_string(), range, kind: IncludeKind::Typst });
    }

    /// A string given to `bibliography(…)` or to `label(…)`.
    fn string_argument(&mut self, range: Range<usize>) {
        let inner = self.calls.last().map(String::as_str);
        let value = &self.text[range.clone()];
        match inner {
            Some("bibliography") => self.scan.bibliographies.push(BibSource { name: value.to_string(), range, kind: BibKind::Typst, remote: false, foreign: false }),
            Some("label") if !value.is_empty() => self.label_value(range),
            _ => {}
        }
    }

    /// A label value in code: a cite in `cite(…)`, a reference in `ref(…)`.
    fn label_value(&mut self, run: Range<usize>) {
        let call = self.calls.iter().rev().find(|call| *call != "label").map(String::as_str);
        let key = Key { name: self.text[run.clone()].to_string(), range: run, command: call.unwrap_or_default().to_string() };
        match call {
            Some("cite") => self.scan.cites.push(key),
            Some("ref") => self.scan.either.push(key),
            _ => {}
        }
    }
}

/// The text of `caption: […]` among the arguments of a figure.
fn caption_of(arguments: &str) -> Option<String> {
    let at = arguments.find("caption:")?;
    let rest = arguments[at + 8..].trim_start().strip_prefix('[')?;
    let mut depth = 0usize;
    let end = rest.char_indices().find(|(_, ch)| match ch {
        '[' => {
            depth += 1;
            false
        }
        ']' if depth == 0 => true,
        ']' => {
            depth -= 1;
            false
        }
        _ => false,
    })?;
    Some(rest[..end.0].trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(keys: &[Key]) -> Vec<&str> {
        keys.iter().map(|key| key.name.as_str()).collect()
    }

    #[test]
    fn references_cites_and_what_is_not_one() {
        let text = r#"As @smith2020 shows, and @jones[p. 5] too. See @fig:plot.
Mail me at olli@example.org or \@escaped.
`@raw` and ```typ @block``` // @comment
/* @block-comment */
#cite(<vaswani2017>, form: "prose") #cite(label("lamport94"))
#ref(<eq:one>) $ x = @notref $ "@quoted" text
#link("mailto:a@b.c")[@insidelink]"#;
        let scan = scan(text);
        // In markup, quotes are text, so `"@quoted"` refers; math has no references.
        assert_eq!(names(&scan.either), ["smith2020", "jones", "fig:plot", "eq:one", "quoted", "insidelink"]);
        assert_eq!(names(&scan.cites), ["vaswani2017", "lamport94"]);
        let smith = &scan.either[0];
        assert_eq!(&text[smith.range.clone()], "smith2020");
    }

    #[test]
    fn labels_belong_to_headings_figures_and_equations() {
        let text = "= Introduction <sec:intro>\n#figure(image(\"a.png\"), caption: [A *plot*]) <fig:plot>\n$ x = 1 $ <eq:one>\nloose <free>\n";
        let scan = scan(text);
        let labels: Vec<(&str, &str, Option<&str>)> = scan.labels.iter().map(|label| (label.name.as_str(), label.env.as_str(), label.caption.as_deref())).collect();
        assert_eq!(labels, [("sec:intro", "heading", Some("Introduction")), ("fig:plot", "figure", Some("A *plot*")), ("eq:one", "equation", None), ("free", "", None)]);
    }

    #[test]
    fn bibliographies_and_includes() {
        let text = "#include \"chapters/one.typ\"\n#bibliography((\"refs.bib\", \"more.yml\"), title: \"Sources\", style: \"ieee\")\n#let x = include \"two.typ\"\n";
        let scan = scan(text);
        let bibs: Vec<&str> = scan.bibliographies.iter().map(|bib| bib.name.as_str()).collect();
        assert_eq!(bibs, ["refs.bib", "more.yml"]);
        let includes: Vec<&str> = scan.includes.iter().map(|include| include.path.as_str()).collect();
        assert_eq!(includes, ["chapters/one.typ", "two.typ"]);
    }
}
