//! A small Markdown reader for what add-ons write and for the preview of
//! Markdown files: headings, paragraphs, bold, italic, inline code, math, code
//! blocks, lists, quotes, rules, pipe tables, images and links. After an edit,
//! the text is parsed again only from the block the change is in, since a
//! block that follows a blank line cannot change what came before it.

use std::ops::Range;

use gpui::SharedString;

use crate::math_text::math_text;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Marks {
    pub bold: bool,
    pub italic: bool,
    pub code: bool,
    /// `$x$`, as Unicode text.
    pub math: bool,
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
    Table { align: Vec<Align>, header: Vec<Inline>, rows: Vec<Vec<Inline>> },
    /// `$$…$$`: the LaTeX as written, and as Unicode text until it is typeset.
    Math { source: SharedString, text: SharedString },
    /// An image alone in its paragraph, with the path as written.
    Image { alt: SharedString, url: SharedString },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Align {
    #[default]
    Left,
    Center,
    Right,
}

/// The parsed form of a text that grows at its end.
#[derive(Debug, Default)]
pub struct Document {
    pub blocks: Vec<Block>,
    /// Where each block starts in `source`.
    starts: Vec<usize>,
    source: String,
    /// A Markdown file, whose YAML front matter is read for its title. In what
    /// add-ons write, a `---` at the start is a rule.
    file: bool,
}

impl Document {
    pub fn file() -> Self {
        Self { file: true, ..Self::default() }
    }

    #[cfg(test)]
    pub fn new(text: &str) -> Self {
        let mut document = Self::file();
        document.update(text);
        document
    }

    /// Where each block starts in the text.
    pub fn starts(&self) -> &[usize] {
        &self.starts
    }

    /// Takes the new text. The blocks before the last blank line ahead of the
    /// first change stay: a blank line ends every block but a code block,
    /// which then holds it, so nothing after it can change what came before.
    /// Reading stops where a block starts in the unchanged end of the text at
    /// the place an old block started, since blocks do not depend on the ones
    /// before them. Returns the old blocks that were replaced and how many new
    /// ones took their place.
    pub fn update(&mut self, text: &str) -> (Range<usize>, usize) {
        if text == self.source {
            return (self.blocks.len()..self.blocks.len(), 0);
        }
        let same = common_prefix(&self.source, text);
        let suffix = self.source[same..].bytes().rev().zip(text[same..].bytes().rev()).take_while(|(a, b)| a == b).count();
        let (old_len, new_end) = (self.source.len(), text.len() - suffix);
        let mut from = self.starts.partition_point(|start| *start <= same).saturating_sub(1);
        while from > 0 && !after_blank_line(&self.source, self.starts[from]) {
            from -= 1;
        }
        let offset = if from == 0 { 0 } else { self.starts[from] };
        // The old blocks that start in the unchanged end, where reading may stop.
        // Not the first, which may be the title of a front matter.
        let tail = self.starts.partition_point(|start| *start < old_len - suffix).max(1).min(self.starts.len());
        let mut resume = None;
        let mut fresh = Vec::new();
        let starts = &self.starts;
        parse_from(text, offset, self.file, &mut fresh, |start| {
            if start < new_end {
                return false;
            }
            let old = start + old_len - text.len();
            resume = starts[tail..].binary_search(&old).ok().map(|index| tail + index);
            resume.is_some()
        });
        let old_end = resume.unwrap_or(self.blocks.len());
        let kept_blocks = self.blocks.split_off(old_end);
        let kept_starts = self.starts.split_off(old_end);
        self.blocks.truncate(from);
        self.starts.truncate(from);
        let count = fresh.len();
        for (start, block) in fresh {
            self.starts.push(start);
            self.blocks.push(block);
        }
        self.blocks.extend(kept_blocks);
        self.starts.extend(kept_starts.into_iter().map(|start| start + text.len() - old_len));
        self.source.clear();
        self.source.push_str(text);
        (from..old_end, count)
    }
}

/// How many bytes both texts start with, at a char boundary of both.
fn common_prefix(left: &str, right: &str) -> usize {
    let mut same = left.bytes().zip(right.bytes()).take_while(|(a, b)| a == b).count();
    while !left.is_char_boundary(same) || !right.is_char_boundary(same) {
        same -= 1;
    }
    same
}

/// Whether the line before `start` is empty.
fn after_blank_line(text: &str, start: usize) -> bool {
    let before = text[..start].trim_end_matches(['\n', '\r']);
    let removed = &text[before.len()..start];
    removed.matches('\n').count() >= 2 || before.rsplit('\n').next().is_some_and(|line| line.trim().is_empty() && removed.contains('\n'))
}

pub fn parse(text: &str) -> Vec<(usize, Block)> {
    let mut blocks = Vec::new();
    parse_from(text, 0, false, &mut blocks, |_| false);
    blocks
}

/// Like `parse`, for a Markdown file with its front matter.
#[cfg(test)]
fn parse_file(text: &str) -> Vec<(usize, Block)> {
    let mut blocks = Vec::new();
    parse_from(text, 0, true, &mut blocks, |_| false);
    blocks
}

/// Reads blocks from the line start `from` on, until `stop` says yes to where
/// the next one starts. Offsets are in all of `text`.
fn parse_from(text: &str, from: usize, file: bool, blocks: &mut Vec<(usize, Block)>, mut stop: impl FnMut(usize) -> bool) {
    let mut lines = Lines { text, at: from };
    if file
        && from == 0
        && let Some(title) = front_matter(&mut lines)
    {
        blocks.push((0, Block::Heading { level: 1, text: inline(&title) }));
    }
    while let Some((start, line)) = lines.peek() {
        let trimmed = line.trim_start();
        if trimmed.is_empty() {
            lines.next();
            continue;
        }
        if stop(start) {
            return;
        }
        let block = if let Some((source, end)) = math_block(&lines) {
            lines.at = end;
            Block::Math { text: math_text(&source).into(), source: source.into() }
        } else if let Some(fence) = fence(trimmed) {
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
            match rows.get(1).and_then(|rule| alignments(rule)) {
                Some(align) => {
                    let cells = |row: &str| cells(row).into_iter().map(inline).collect::<Vec<_>>();
                    Block::Table { align, header: cells(rows[0]), rows: rows[2..].iter().map(|row| cells(row)).collect() }
                }
                None => Block::Code { language: "table".into(), text: rows.join("\n").into() },
            }
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
                let starts_other = trimmed.is_empty()
                    || fence(trimmed).is_some()
                    || heading(trimmed).is_some()
                    || trimmed.starts_with('>')
                    || math_block(&lines).is_some()
                    || (!paragraph.is_empty() && list_item(trimmed).is_some());
                if starts_other {
                    break;
                }
                paragraph.push(line);
                lines.next();
            }
            let joined = join_lines(&paragraph);
            match image(&joined) {
                Some((alt, url)) => Block::Image { alt: alt.into(), url: url.into() },
                None => Block::Paragraph(inline(&joined)),
            }
        };
        blocks.push((start, block));
    }
}

/// `$$…$$` from the line `lines` is at, as Pandoc reads display math: closed
/// on that line or a later one, before a blank line, with nothing after the
/// closing dollars. Returns the formula and where the lines go on, or nothing,
/// so that dollars typed and not yet closed stay text.
fn math_block(lines: &Lines) -> Option<(String, usize)> {
    let mut ahead = Lines { text: lines.text, at: lines.at };
    let (_, first) = ahead.next()?;
    let rest = first.trim_start().strip_prefix("$$")?;
    if let Some(inner) = rest.trim_end().strip_suffix("$$") {
        return (!inner.contains("$$")).then(|| (inner.trim().to_string(), ahead.at));
    }
    if rest.contains("$$") {
        return None;
    }
    let mut math = vec![rest];
    while let Some((_, line)) = ahead.next() {
        if line.trim().is_empty() {
            return None;
        }
        if let Some(last) = line.trim_end().strip_suffix("$$") {
            if last.contains("$$") {
                return None;
            }
            math.push(last);
            return Some((math.join("\n").trim().to_string(), ahead.at));
        }
        if line.contains("$$") {
            return None;
        }
        math.push(line);
    }
    None
}

/// Skips a YAML front matter at the very start and returns its title.
fn front_matter(lines: &mut Lines) -> Option<String> {
    if lines.peek().map(|(_, line)| line) != Some("---") {
        return None;
    }
    let mut ahead = Lines { text: lines.text, at: lines.at };
    ahead.next();
    let mut title = None;
    while let Some((_, line)) = ahead.next() {
        if line == "---" || line == "..." {
            *lines = ahead;
            return title;
        }
        if let Some(value) = line.strip_prefix("title:") {
            title = Some(value.trim().trim_matches(['"', '\'']).to_string()).filter(|title| !title.is_empty());
        }
    }
    None
}

/// The cells of a table row, without its outer pipes. `\|` stays in its cell.
fn cells(row: &str) -> Vec<&str> {
    let row = row.trim();
    let row = row.strip_prefix('|').unwrap_or(row);
    let row = if row.ends_with('|') && !row.ends_with("\\|") { &row[..row.len() - 1] } else { row };
    let mut out = Vec::new();
    let mut start = 0;
    let bytes = row.as_bytes();
    for (index, byte) in bytes.iter().enumerate() {
        if *byte == b'|' && (index == 0 || bytes[index - 1] != b'\\') {
            out.push(row[start..index].trim());
            start = index + 1;
        }
    }
    out.push(row[start..].trim());
    out
}

/// The alignments of a table's rule row, as `|:--|:-:|--:|`, or nothing if it is not one.
fn alignments(rule: &str) -> Option<Vec<Align>> {
    cells(rule)
        .into_iter()
        .map(|cell| {
            let dashes = cell.trim_start_matches(':').trim_end_matches(':');
            if dashes.is_empty() || !dashes.bytes().all(|byte| byte == b'-') {
                return None;
            }
            Some(match (cell.starts_with(':'), cell.ends_with(':')) {
                (true, true) => Align::Center,
                (false, true) => Align::Right,
                _ => Align::Left,
            })
        })
        .collect()
}

/// `![alt](url)` as the whole paragraph.
fn image(paragraph: &str) -> Option<(String, String)> {
    let rest = paragraph.strip_prefix('!')?;
    let chars: Vec<char> = rest.chars().collect();
    let (alt, url, used) = link(&chars)?;
    (used == chars.len()).then_some((alt, url))
}

/// Lines with the offset where each starts.
struct Lines<'a> {
    text: &'a str,
    at: usize,
}

impl<'a> Lines<'a> {
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
            '$' if next.is_some_and(|next| !next.is_whitespace()) => {
                // Pandoc's rule: no space inside the dollars, no digit right after.
                let close = chars[index + 1..].iter().enumerate().position(|(at, &ch)| {
                    ch == '$' && !chars[index + at].is_whitespace() && !chars.get(index + at + 2).is_some_and(char::is_ascii_digit)
                });
                if let Some(end) = close {
                    flush(&text, &mut marks, &current, &mut run_start);
                    text.push_str(&math_text(&chars[index + 1..index + end + 1].iter().collect::<String>()));
                    marks.push((run_start..text.len(), Marks { math: true, ..current.clone() }));
                    run_start = text.len();
                    index += end + 2;
                    continue;
                }
            }
            '!' if next == Some('[') => {
                // An image inside text shows its description.
                if let Some((alt, _, used)) = link(&chars[index + 1..]) {
                    text.push_str(&alt);
                    index += used + 1;
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
    let url = url.trim();
    // `<path with spaces>` keeps them; otherwise a title may follow the url.
    let url = match url.strip_prefix('<').and_then(|url| url.split_once('>')) {
        Some((inside, _)) => inside.to_string(),
        None => url.split_whitespace().next()?.to_string(),
    };
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
        assert!(matches!(&parsed[7], Block::Table { header, rows, .. } if header.len() == 2 && rows.is_empty()));
    }

    #[test]
    fn tables_math_images_and_front_matter() {
        let text = "---\ntitle: \"Notes\"\nbibliography: refs.bib\n---\n| a | b \\| c |\n|:--|--:|\n| 1 | 2 |\n\n$$\nx^2\n$$\n\n![A frog](<img/a frog.png>)\n\nCosts 5 $ and $x_1$ is math.\n";
        let parsed: Vec<Block> = parse_file(text).into_iter().map(|(_, block)| block).collect();
        // What an add-on writes has no front matter: the dashes are a rule.
        assert_eq!(blocks(text)[0], Block::Rule);
        assert!(matches!(&parsed[0], Block::Heading { level: 1, text } if text.text == "Notes"));
        let Block::Table { align, header, rows } = &parsed[1] else { panic!("{:?}", parsed[1]) };
        assert_eq!(align, &[Align::Left, Align::Right]);
        assert_eq!(header[1].text, "b | c");
        assert_eq!(rows[0][1].text, "2");
        assert_eq!(parsed[2], Block::Math { source: "x^2".into(), text: "x²".into() });
        assert_eq!(parsed[3], Block::Image { alt: "A frog".into(), url: "img/a frog.png".into() });
        let Block::Paragraph(paragraph) = &parsed[4] else { panic!() };
        let math: Vec<_> = paragraph.marks.iter().filter(|(_, marks)| marks.math).map(|(range, _)| &paragraph.text[range.clone()]).collect();
        assert_eq!(math, ["x₁"]);
        assert_eq!(blocks("$$x$$\n"), [Block::Math { source: "x".into(), text: "x".into() }]);
        // Open dollars, or text after the closing ones, stay text.
        assert!(matches!(&blocks("$$a$$ is the area\n")[..], [Block::Paragraph(_)]));
        assert!(matches!(&blocks("$$\nx\n\n# Next\n")[..], [Block::Paragraph(_), Block::Heading { .. }]));
        assert!(matches!(&blocks("$$\nx^2\n$$\n")[..], [Block::Math { .. }]));
    }

    /// Editing anywhere must end where reading the whole text at once does.
    #[test]
    fn edits_match_reading_at_once() {
        let text = "---\ntitle: T\n---\n# Plan\n\n---\n\nFirst **step** here.\n\n```tex\na\n\nb\n```\n\n- x\n- y\n\n| a |\n|---|\n| 1 |\n\n$$\nx\n\ny\n$$\n\nEnd.\n";
        for at in 0..=text.len() {
            if !text.is_char_boundary(at) {
                continue;
            }
            for insert in ["x", "\n", "\n\n", "```", "$$", "| ", "---\n"] {
                let edited = format!("{}{insert}{}", &text[..at], &text[at..]);
                let mut document = Document::new(text);
                let before = document.blocks.clone();
                let (replaced, count) = document.update(&edited);
                assert_eq!(document.blocks, Document::new(&edited).blocks, "{insert:?} at {at}");
                // What it says changed is all that changed.
                assert_eq!(document.blocks[..replaced.start], before[..replaced.start]);
                assert_eq!(document.blocks[replaced.start + count..], before[replaced.end..]);
                assert_eq!(document.starts, parse_file(&edited).into_iter().map(|(start, _)| start).collect::<Vec<_>>());
                document.update(text);
                assert_eq!(document.blocks, Document::new(text).blocks, "{insert:?} taken back at {at}");
            }
        }
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

    /// Typing into a 1 MB file, near its end and near its start, as the preview reads it:
    /// cargo test -p app --release markdown::speed -- --ignored --nocapture
    #[test]
    #[ignore]
    fn editing_a_large_file() {
        let section = "## Section\n\nSome **bold** text with `code`, $x^2$ and [a link](https://x.org) in a longer paragraph of words.\n\n- one\n- two\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n```rust\nfn main() {}\n```\n\n";
        let text = section.repeat(1_000_000 / section.len());
        let started = std::time::Instant::now();
        let mut document = Document::new(&text);
        println!("{} bytes, {} blocks: first read {:?}", text.len(), document.blocks.len(), started.elapsed());
        for (name, at) in [("end", text.len() - 40), ("start", 40)] {
            let mut edited = text.clone();
            let started = std::time::Instant::now();
            for _ in 0..100 {
                edited.insert(at, 'x');
                document.update(&edited);
            }
            println!("typing near the {name}: {:?} per key", started.elapsed() / 100);
        }
    }
}
