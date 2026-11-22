//! A small Markdown reader for what add-ons write: headings, paragraphs, bold,
//! italic, inline code, code blocks, lists, quotes, rules and links. Tables
//! stay as written, in a code block. Text that streams in is parsed again only
//! from its last block on, since everything before it cannot change.

use std::ops::Range;

use gpui::SharedString;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Marks {
    pub bold: bool,
    pub italic: bool,
    pub code: bool,
    pub link: Option<SharedString>,
}

/// Text with its marks, in ranges that do not overlap and cover only marked text.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Inline {
    pub text: SharedString,
    pub marks: Vec<(Range<usize>, Marks)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Block {
    Heading { level: u8, text: Inline },
    Paragraph(Inline),
    Code { language: SharedString, text: SharedString },
    List { ordered: bool, start: u32, items: Vec<Inline> },
    Quote(Inline),
    Rule,
}

/// The parsed form of a text that grows at its end.
#[derive(Debug, Default)]
pub struct Document {
    pub blocks: Vec<Block>,
    /// Where each block starts in `source`.
    starts: Vec<usize>,
    source: String,
}

impl Document {
    #[cfg(test)]
    pub fn new(text: &str) -> Self {
        let mut document = Self::default();
        document.update(text);
        document
    }

    /// Takes the new text. When it only grew, the blocks before the last blank
    /// line stay: a blank line ends every block but a code block, which then
    /// holds it, so nothing after it can change what came before.
    pub fn update(&mut self, text: &str) {
        let from = if text.starts_with(self.source.as_str()) {
            (1..self.starts.len()).rev().find(|&index| after_blank_line(&self.source, self.starts[index])).unwrap_or(0)
        } else {
            0
        };
        let offset = self.starts.get(from).copied().unwrap_or(0);
        self.blocks.truncate(from);
        self.starts.truncate(from);
        for (start, block) in parse(&text[offset..]) {
            self.starts.push(offset + start);
            self.blocks.push(block);
        }
        self.source.clear();
        self.source.push_str(text);
    }
}

/// Whether the line before `start` is empty.
fn after_blank_line(text: &str, start: usize) -> bool {
    let before = text[..start].trim_end_matches(['\n', '\r']);
    let removed = &text[before.len()..start];
    removed.matches('\n').count() >= 2 || before.rsplit('\n').next().is_some_and(|line| line.trim().is_empty() && removed.contains('\n'))
}

pub fn parse(text: &str) -> Vec<(usize, Block)> {
    let mut blocks = Vec::new();
    let mut lines = Lines::new(text);
    while let Some((start, line)) = lines.peek() {
        let trimmed = line.trim_start();
        if trimmed.is_empty() {
            lines.next();
            continue;
        }
        let block = if let Some(fence) = fence(trimmed) {
            lines.next();
            let language = trimmed[fence.len()..].trim().to_string();
            let mut code = Vec::new();
            while let Some((_, line)) = lines.next() {
                if line.trim_start().starts_with(fence) {
                    break;
                }
                code.push(line);
            }
            Block::Code { language: language.into(), text: code.join("\n").into() }
        } else if let Some((level, title)) = heading(trimmed) {
            lines.next();
            Block::Heading { level, text: inline(title) }
        } else if is_rule(trimmed) {
            lines.next();
            Block::Rule
        } else if trimmed.starts_with('|') {
            let mut rows = Vec::new();
            while let Some((_, line)) = lines.peek().filter(|(_, line)| line.trim_start().starts_with('|')) {
                rows.push(line.trim());
                lines.next();
            }
            Block::Code { language: "table".into(), text: rows.join("\n").into() }
        } else if let Some(rest) = trimmed.strip_prefix('>') {
            let mut quote = vec![rest.trim_start()];
            lines.next();
            while let Some((_, line)) = lines.peek() {
                let Some(rest) = line.trim_start().strip_prefix('>') else { break };
                quote.push(rest.trim_start());
                lines.next();
            }
            Block::Quote(inline(&join_lines(&quote)))
        } else if let Some(item) = list_item(trimmed) {
            let (ordered, start) = (item.number.is_some(), item.number.unwrap_or(1));
            let mut items: Vec<Vec<&str>> = Vec::new();
            while let Some((_, line)) = lines.peek() {
                let trimmed = line.trim_start();
                if let Some(item) = list_item(trimmed).filter(|item| item.number.is_some() == ordered) {
                    items.push(vec![item.text]);
                } else if !trimmed.is_empty() && line.starts_with([' ', '\t']) && !items.is_empty() {
                    items.last_mut().unwrap().push(trimmed);
                } else {
                    break;
                }
                lines.next();
            }
            Block::List { ordered, start, items: items.iter().map(|lines| inline(&join_lines(lines))).collect() }
        } else {
            let mut paragraph = Vec::new();
            while let Some((_, line)) = lines.peek() {
                let trimmed = line.trim_start();
                let starts_other = trimmed.is_empty() || fence(trimmed).is_some() || heading(trimmed).is_some() || trimmed.starts_with('>') || (!paragraph.is_empty() && list_item(trimmed).is_some());
                if starts_other {
                    break;
                }
                paragraph.push(line);
                lines.next();
            }
            Block::Paragraph(inline(&join_lines(&paragraph)))
        };
        blocks.push((start, block));
    }
    blocks
}

/// Lines with the offset where each starts.
struct Lines<'a> {
    text: &'a str,
    at: usize,
}

impl<'a> Lines<'a> {
    fn new(text: &'a str) -> Self {
        Self { text, at: 0 }
    }

    fn peek(&self) -> Option<(usize, &'a str)> {
        (self.at < self.text.len()).then(|| {
            let rest = &self.text[self.at..];
            (self.at, rest.split('\n').next().unwrap_or(rest).trim_end_matches('\r'))
        })
    }

    fn next(&mut self) -> Option<(usize, &'a str)> {
        let line = self.peek()?;
        let rest = &self.text[self.at..];
        self.at += rest.find('\n').map_or(rest.len(), |end| end + 1);
        Some(line)
    }
}

/// Soft line breaks become spaces, and two spaces at the end of a line a break.
fn join_lines(lines: &[&str]) -> String {
    let mut text = String::new();
    for (index, line) in lines.iter().enumerate() {
        if index > 0 {
            text.push(if lines[index - 1].ends_with("  ") { '\n' } else { ' ' });
        }
        text.push_str(line.trim());
    }
    text
}

fn fence(line: &str) -> Option<&'static str> {
    ["```", "~~~"].into_iter().find(|fence| line.starts_with(fence))
}

fn heading(line: &str) -> Option<(u8, &str)> {
    let level = line.bytes().take_while(|&byte| byte == b'#').count();
    let rest = &line[level..];
    ((1..=6).contains(&level) && (rest.is_empty() || rest.starts_with(' '))).then(|| (level as u8, rest.trim().trim_end_matches('#').trim_end()))
}

fn is_rule(line: &str) -> bool {
    let line = line.trim_end();
    line.len() >= 3 && ["-", "*", "_"].iter().any(|mark| line.chars().all(|ch| ch.to_string() == *mark || ch == ' ') && line.matches(mark).count() >= 3)
}

struct Item<'a> {
    number: Option<u32>,
    text: &'a str,
}

fn list_item(line: &str) -> Option<Item<'_>> {
    if let Some(rest) = line.strip_prefix(['-', '*', '+']) {
        return rest.strip_prefix(' ').map(|text| Item { number: None, text: text.trim_start() });
    }
    let digits = line.bytes().take_while(u8::is_ascii_digit).count();
    let rest = &line[digits..];
    let text = rest.strip_prefix(". ").or_else(|| rest.strip_prefix(") "))?;
    (digits > 0 && digits < 10).then(|| Item { number: line[..digits].parse().ok(), text: text.trim_start() })
}

/// Reads the marks of one paragraph.
pub fn inline(source: &str) -> Inline {
    let mut text = String::with_capacity(source.len());
    let mut marks: Vec<(Range<usize>, Marks)> = Vec::new();
    let mut current = Marks::default();
    let mut run_start = 0;
    let chars: Vec<char> = source.chars().collect();
    let flush = |text: &str, marks: &mut Vec<(Range<usize>, Marks)>, current: &Marks, run_start: &mut usize| {
        if *current != Marks::default() && *run_start < text.len() {
            marks.push((*run_start..text.len(), current.clone()));
        }
        *run_start = text.len();
    };
    let mut index = 0;
    while index < chars.len() {
        let ch = chars[index];
        let next = chars.get(index + 1).copied();
        let previous = index.checked_sub(1).map(|at| chars[at]);
        match ch {
            '\\' if next.is_some_and(|next| next.is_ascii_punctuation()) => {
                text.push(next.unwrap());
                index += 2;
                continue;
            }
            '`' => {
                if let Some(end) = chars[index + 1..].iter().position(|&ch| ch == '`') {
                    flush(&text, &mut marks, &current, &mut run_start);
                    let code: String = chars[index + 1..index + 1 + end].iter().collect();
                    text.push_str(&code);
                    marks.push((run_start..text.len(), Marks { code: true, ..current.clone() }));
                    run_start = text.len();
                    index += end + 2;
                    continue;
                }
            }
            '[' => {
                if let Some((label, url, used)) = link(&chars[index..]) {
                    flush(&text, &mut marks, &current, &mut run_start);
                    let inner = inline(&label);
                    let base = text.len();
                    text.push_str(&inner.text);
                    let link = Some(SharedString::from(url));
                    // The label's own marks stay, with the link added to all of it.
                    let mut covered = base;
                    for (range, inner_marks) in inner.marks {
                        if range.start + base > covered {
                            marks.push((covered..range.start + base, Marks { link: link.clone(), ..current.clone() }));
                        }
                        marks.push((range.start + base..range.end + base, Marks { link: link.clone(), ..inner_marks }));
                        covered = range.end + base;
                    }
                    if covered < text.len() {
                        marks.push((covered..text.len(), Marks { link: link.clone(), ..current.clone() }));
                    }
                    run_start = text.len();
                    index += used;
                    continue;
                }
            }
            '*' | '_' => {
                // An underscore inside a word, as in snake_case, is text.
                let inside_word = ch == '_' && previous.is_some_and(char::is_alphanumeric) && next.is_some_and(char::is_alphanumeric);
                if !inside_word {
                    let double = next == Some(ch);
                    flush(&text, &mut marks, &current, &mut run_start);
                    if double {
                        current.bold = !current.bold;
                        index += 2;
                    } else {
                        current.italic = !current.italic;
                        index += 1;
                    }
                    continue;
                }
            }
            _ => {}
        }
        text.push(ch);
        index += 1;
    }
    flush(&text, &mut marks, &current, &mut run_start);
    Inline { text: text.into(), marks }
}

/// `[label](url)` at the start of `chars`: the label, the url and the chars used.
fn link(chars: &[char]) -> Option<(String, String, usize)> {
    let close = chars.iter().position(|&ch| ch == ']')?;
    if chars.get(close + 1) != Some(&'(') {
        return None;
    }
    let end = chars[close + 2..].iter().position(|&ch| ch == ')')? + close + 2;
    let url: String = chars[close + 2..end].iter().collect();
    let url = url.split_whitespace().next()?.to_string();
    Some((chars[1..close].iter().collect(), url, end + 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blocks(text: &str) -> Vec<Block> {
        parse(text).into_iter().map(|(_, block)| block).collect()
    }

    #[test]
    fn blocks_are_recognized() {
        let text = "# Title\n\nSome *text*\non two lines.\n\n```latex\n\\begin{x}\n\n\\end{x}\n```\n- one\n- two\n  more\n\n1. first\n2. second\n\n> quoted\n> on\n\n---\n| a | b |\n|---|---|\n";
        let parsed = blocks(text);
        assert!(matches!(&parsed[0], Block::Heading { level: 1, text } if text.text == "Title"));
        assert!(matches!(&parsed[1], Block::Paragraph(inline) if inline.text == "Some text on two lines."));
        assert_eq!(parsed[2], Block::Code { language: "latex".into(), text: "\\begin{x}\n\n\\end{x}".into() });
        assert!(matches!(&parsed[3], Block::List { ordered: false, items, .. } if items.len() == 2 && items[1].text == "two more"));
        assert!(matches!(&parsed[4], Block::List { ordered: true, start: 1, items } if items.len() == 2));
        assert!(matches!(&parsed[5], Block::Quote(inline) if inline.text == "quoted on"));
        assert_eq!(parsed[6], Block::Rule);
        assert!(matches!(&parsed[7], Block::Code { language, .. } if language == "table"));
    }

    #[test]
    fn inline_marks_and_links() {
        let parsed = inline("a **bold** and *it* `x_y` [see **here**](https://a.org) snake_case_name \\*no\\*");
        assert_eq!(parsed.text, "a bold and it x_y see here snake_case_name *no*");
        let text = parsed.text.to_string();
        let marked = |part: &str| {
            let start = text.find(part).unwrap();
            parsed.marks.iter().filter(|(range, _)| range.start <= start && range.end >= start + part.len()).map(|(_, marks)| marks.clone()).next()
        };
        assert!(marked("bold").unwrap().bold);
        assert!(marked("it").unwrap().italic);
        assert!(marked("x_y").unwrap().code);
        let here = marked("here").unwrap();
        assert!(here.bold && here.link.as_deref() == Some("https://a.org"));
        assert_eq!(marked("see").unwrap().link.as_deref(), Some("https://a.org"));
        assert_eq!(marked("snake_case_name"), None);
    }

    #[test]
    fn marks_do_not_overlap() {
        let parsed = inline("**a *b* c** [x *y*](u) `z`");
        for pair in parsed.marks.windows(2) {
            assert!(pair[0].0.end <= pair[1].0.start, "{:?}", parsed.marks);
        }
    }

    /// Streaming must end where reading the whole text at once does, wherever
    /// the text was split.
    #[test]
    fn streaming_matches_reading_at_once() {
        let text = "# Plan\n\nFirst **step** here.\nStill the paragraph.\n\n```tex\na\n\nb\n```\n\n- x\n- y\n\nEnd with [link](https://x.y) and more text.\n";
        let whole = Document::new(text);
        for split in 0..=text.len() {
            if !text.is_char_boundary(split) {
                continue;
            }
            let mut document = Document::new(&text[..split]);
            document.update(text);
            assert_eq!(document.blocks, whole.blocks, "split at {split}");
            // And in many small steps.
            let mut document = Document::default();
            for end in (split..=text.len()).step_by(7).chain([text.len()]) {
                if text.is_char_boundary(end) {
                    document.update(&text[..end]);
                }
            }
            assert_eq!(document.blocks, whole.blocks, "steps from {split}");
        }
    }

    #[test]
    fn a_changed_start_reads_everything_again() {
        let mut document = Document::new("# A\n\nb\n");
        document.update("# C\n\nb\n");
        assert!(matches!(&document.blocks[0], Block::Heading { text, .. } if text.text == "C"));
    }
}

#[cfg(test)]
mod speed {
    use super::*;

    /// A long answer arriving in small pieces, as a model streams it:
    /// cargo test -p app markdown::speed -- --ignored --nocapture
    #[test]
    #[ignore]
    fn streaming_a_long_answer() {
        let words = "some **bold** text with `code` and [a link](https://x.org) more words here ";
        let mut text = String::new();
        let mut document = Document::default();
        let started = std::time::Instant::now();
        for step in 0..4000 {
            text.push_str(&words[(step * 7) % 40..]);
            if step % 40 == 39 {
                text.push_str("\n\n");
            }
            if step % 400 == 399 {
                text.push_str("```latex\n\\begin{x}\nline\n\\end{x}\n```\n\n");
            }
            document.update(&text);
        }
        let elapsed = started.elapsed();
        println!("{} bytes, {} blocks: {:?} in all, {:?} per piece", text.len(), document.blocks.len(), elapsed, elapsed / 4000);
        assert_eq!(document.blocks, parse(&text).into_iter().map(|(_, block)| block).collect::<Vec<_>>());
    }
}
