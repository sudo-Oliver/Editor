//! Reads a Markdown file as Pandoc does for citations: `[@key]`, `[see @a; @b, p. 3]`,
//! `[-@key]` and, when the front matter names a bibliography, `@key` in running
//! text too. Code spans, code blocks and math hold no citations. A plain `@` in
//! a README, as in a mention, counts only in brackets, where Pandoc would cite.

use crate::scan::{BibKind, BibSource, Key, Scan};

pub fn scan(text: &str) -> Scan {
    let mut scan = Scan::default();
    let body = front_matter(text, &mut scan);
    let bare = !scan.bibliographies.is_empty();
    let bytes = text.as_bytes();
    let mut pos = body;
    // The `[` of the bracket the position is in, if it holds an `@`.
    let mut bracket: Option<usize> = None;
    let mut line_start = true;
    while pos < bytes.len() {
        if line_start {
            line_start = false;
            let indent = bytes[pos..].iter().take_while(|byte| **byte == b' ').count();
            let rest = &text[pos + indent..];
            if let Some(fence) = ["```", "~~~"].into_iter().find(|fence| rest.starts_with(fence)) {
                pos = skip_fence(text, pos + indent, fence);
                line_start = true;
                continue;
            }
            if rest.starts_with("$$") {
                pos = text[pos + indent + 2..].find("$$").map_or(bytes.len(), |at| pos + indent + 2 + at + 2);
                continue;
            }
        }
        match bytes[pos] {
            b'\n' => {
                line_start = true;
                // A blank line ends any open bracket.
                if bytes.get(pos + 1) == Some(&b'\n') {
                    bracket = None;
                }
                pos += 1;
            }
            b'\\' => pos += 2,
            b'`' => {
                let ticks = bytes[pos..].iter().take_while(|byte| **byte == b'`').count();
                let fence = &text[pos..pos + ticks];
                pos = text[pos + ticks..].find(fence).map_or(pos + ticks, |at| pos + ticks + at + ticks);
            }
            b'$' if bytes.get(pos + 1).is_some_and(|next| !next.is_ascii_whitespace()) => {
                // Inline math closes on the same paragraph, before a blank line.
                let end = text[pos + 1..].find('$').filter(|at| !text[pos + 1..pos + 1 + at].contains("\n\n"));
                pos = end.map_or(pos + 1, |at| pos + 1 + at + 1);
            }
            b'[' => {
                bracket = Some(pos);
                pos += 1;
            }
            b']' => {
                bracket = None;
                pos += 1;
            }
            b'@' => {
                let before = text[..pos].chars().next_back();
                // Not an e-mail address and not part of a word.
                let starts = before.is_none_or(|ch| !ch.is_alphanumeric() && !matches!(ch, '.' | '_' | '+'));
                let range = key_run(text, pos + 1);
                if starts && !range.is_empty() && (bracket.is_some() || bare) {
                    scan.cites.push(Key { name: text[range.clone()].to_string(), range: range.clone(), command: "@".into() });
                }
                pos = range.end.max(pos + 1);
            }
            _ => pos += text[pos..].chars().next().map_or(1, char::len_utf8),
        }
    }
    scan
}

/// A Pandoc citation key from `from`: letters, digits and `_`, with `:.#$%&-+?<>~/`
/// inside but not at its end.
fn key_run(text: &str, from: usize) -> std::ops::Range<usize> {
    let mut end = from;
    let mut last_word = from;
    for (offset, ch) in text[from..].char_indices() {
        if ch.is_alphanumeric() || ch == '_' {
            end = from + offset + ch.len_utf8();
            last_word = end;
        } else if offset > 0 && ":.#$%&-+?<>~/".contains(ch) {
            end = from + offset + ch.len_utf8();
        } else {
            break;
        }
    }
    from..end.min(last_word)
}

/// Past the code block that opens at `start` with `fence`, to the start of the
/// line after its closing fence.
fn skip_fence(text: &str, start: usize, fence: &str) -> usize {
    let mut pos = text[start..].find('\n').map_or(text.len(), |at| start + at + 1);
    while pos < text.len() {
        let end = text[pos..].find('\n').map_or(text.len(), |at| pos + at + 1);
        if text[pos..end].trim_start().starts_with(fence) {
            return end;
        }
        pos = end;
    }
    text.len()
}

/// Reads `bibliography:` of a YAML front matter, as one name or a list, and
/// returns where the body starts.
fn front_matter(text: &str, scan: &mut Scan) -> usize {
    let Some(rest) = text.strip_prefix("---\n").or_else(|| text.strip_prefix("---\r\n")) else {
        return 0;
    };
    let mut pos = text.len() - rest.len();
    let mut in_list = false;
    while pos < text.len() {
        let end = text[pos..].find('\n').map_or(text.len(), |at| pos + at + 1);
        let line = text[pos..end].trim_end();
        if line == "---" || line == "..." {
            return end;
        }
        if let Some(value) = line.strip_prefix("bibliography:") {
            let value = value.trim();
            in_list = value.is_empty();
            if value.starts_with('[') {
                let inner = &value[1..value.rfind(']').unwrap_or(value.len())];
                let base = pos + line.len() - value.len() + 1;
                let mut offset = 0;
                for item in inner.split(',') {
                    push_bib(scan, item, base + offset);
                    offset += item.len() + 1;
                }
            } else if !in_list {
                push_bib(scan, value, pos + line.len() - value.len());
            }
        } else if in_list && let Some(item) = line.trim_start().strip_prefix("- ") {
            push_bib(scan, item, pos + line.len() - item.len());
        } else if !line.starts_with([' ', '\t']) {
            in_list = false;
        }
        pos = end;
    }
    0
}

/// `raw` is a YAML scalar at `offset`, perhaps quoted and padded.
fn push_bib(scan: &mut Scan, raw: &str, offset: usize) {
    let lead = raw.len() - raw.trim_start().len();
    let trimmed = raw.trim();
    let name = trimmed.trim_matches(['"', '\'']);
    if name.is_empty() {
        return;
    }
    let start = offset + lead + (trimmed.len() - trimmed.trim_start_matches(['"', '\'']).len());
    let lower = name.to_ascii_lowercase();
    scan.bibliographies.push(BibSource {
        name: name.to_string(),
        range: start..start + name.len(),
        kind: BibKind::Pandoc,
        remote: lower.starts_with("http://") || lower.starts_with("https://"),
        // CSL JSON and CSL YAML are not BibTeX.
        foreign: !(lower.ends_with(".bib") || lower.ends_with(".bibtex")),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(keys: &[Key]) -> Vec<&str> {
        keys.iter().map(|key| key.name.as_str()).collect()
    }

    #[test]
    fn brackets_cite_and_mentions_do_not() {
        let text = "Thanks @olli. See [@smith2020; -@jones, p. 3] and [@doe.]\nMail a@b.org.\n";
        let scan = scan(text);
        assert_eq!(names(&scan.cites), ["smith2020", "jones", "doe"]);
        assert_eq!(&text[scan.cites[0].range.clone()], "smith2020");
    }

    #[test]
    fn code_and_math_hold_no_citations() {
        let text = "`[@a]` and ``x [@b] y``\n\n```\n[@c]\n```\n\n$x [@d]$\n\n$$\n[@e]\n$$\n\n[@f]\n";
        assert_eq!(names(&scan(text).cites), ["f"]);
    }

    #[test]
    fn front_matter_names_bibliographies_and_allows_bare_keys() {
        let text = "---\ntitle: X\nbibliography: [\"refs.bib\", more.json]\n---\n@smith says\n";
        let scan = scan(text);
        assert_eq!(scan.bibliographies.iter().map(|bib| (bib.name.as_str(), bib.foreign)).collect::<Vec<_>>(), [("refs.bib", false), ("more.json", true)]);
        assert_eq!(&text[scan.bibliographies[0].range.clone()], "refs.bib");
        assert_eq!(&text[scan.bibliographies[1].range.clone()], "more.json");
        assert_eq!(names(&scan.cites), ["smith"]);

        let list = "---\nbibliography:\n  - a.bib\n  - 'b.bib'\nlink-citations: true\n---\n";
        let scan = super::scan(list);
        assert_eq!(scan.bibliographies.iter().map(|bib| &list[bib.range.clone()]).collect::<Vec<_>>(), ["a.bib", "b.bib"]);
    }
}
