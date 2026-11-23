//! Typst's own bibliography format, Hayagriva YAML. Only the part of YAML it
//! uses: entries as keys at the start of a line, their fields indented below,
//! lists of names, and a `parent` for the journal or the proceedings. Keys are
//! read reliably, the rest as well as the picker needs.

use std::{collections::HashMap, ops::Range};

use crate::{
    Diagnostic, Severity,
    bib::{Bib, Entry, Field, Part, Value},
};

pub fn parse(text: &str) -> Bib {
    let mut bib = Bib::default();
    let mut seen: HashMap<String, Range<usize>> = HashMap::new();
    // The entry being read, the field whose items follow, and the nested map.
    let mut entry: Option<Entry> = None;
    let mut list: Option<(String, Vec<String>, Range<usize>)> = None;
    let mut nested: Option<(String, usize)> = None;
    let mut parent_kind = String::new();
    let mut offset = 0;
    for (number, raw_line) in text.split_inclusive('\n').enumerate() {
        let start = offset;
        offset += raw_line.len();
        let line = raw_line.trim_end_matches(['\n', '\r']);
        let content = line.trim_start_matches(' ');
        let indent = line.len() - content.len();
        if content.is_empty() || content.starts_with('#') || (indent == 0 && (line == "---" || line == "...")) {
            continue;
        }
        if content.starts_with('\t') {
            bib.diagnostics.push(Diagnostic { range: start + indent..start + indent + 1, severity: Severity::Error, message: "YAML does not allow tabs for indentation.".into() });
            continue;
        }
        if indent == 0 {
            finish(&mut bib, &mut entry, &mut list, &mut parent_kind);
            nested = None;
            let Some((key, rest)) = split_key(content) else {
                bib.diagnostics.push(Diagnostic { range: start..start + line.len(), severity: Severity::Error, message: "An entry starts with its key and a colon, as in `key:`.".into() });
                continue;
            };
            let key_start = start + line.find(key).unwrap_or(0);
            let key_range = key_start..key_start + key.len();
            if !rest.is_empty() {
                bib.diagnostics.push(Diagnostic { range: key_range.clone(), severity: Severity::Error, message: format!("The fields of {key} go on the lines below it, indented.") });
            }
            if let Some(first) = seen.get(key) {
                let line = crate::text::line_col(text, first.start).0 + 1;
                bib.diagnostics.push(Diagnostic { range: key_range.clone(), severity: Severity::Error, message: format!("The key {key} is already used on line {line}.") });
            } else {
                seen.insert(key.to_string(), key_range.clone());
            }
            entry = Some(Entry { kind: "misc".into(), key: key.to_string(), key_range, range: start..start + line.len(), line: number, fields: Vec::new() });
            continue;
        }
        let Some(current) = entry.as_mut() else {
            continue;
        };
        current.range.end = start + line.len();
        let value_start = start + indent;
        let range = value_start..start + line.len();
        if let Some(item) = content.strip_prefix("- ").or_else(|| (content == "-").then_some("")) {
            if let Some((_, items, list_range)) = list.as_mut() {
                items.push(scalar(item));
                list_range.end = range.end;
            }
            continue;
        }
        let Some((name, rest)) = split_key(content) else {
            continue;
        };
        let name = name.to_ascii_lowercase();
        // A map one level in: the fields of `parent`, or of `serial-number`.
        if let Some((outer, outer_indent)) = nested.clone() {
            if indent > outer_indent {
                if rest.is_empty() {
                    continue;
                }
                let value = scalar(rest);
                match (outer.as_str(), name.as_str()) {
                    ("parent", "title") => push(current, "parent-title", value, range),
                    ("parent", "type") => parent_kind = value.to_ascii_lowercase(),
                    ("serial-number", "doi") => push(current, "doi", value, range),
                    ("title", "value") => push(current, "title", value, range),
                    ("url", "value") => push(current, "url", value, range),
                    _ => {}
                }
                continue;
            }
            nested = None;
        }
        if let Some((field, items, list_range)) = list.take() {
            push(current, &field, items.join(" and "), list_range);
        }
        if rest.is_empty() {
            if matches!(name.as_str(), "author" | "editor") {
                list = Some((name, Vec::new(), range));
            } else {
                nested = Some((name, indent));
            }
            continue;
        }
        let value = if rest.starts_with('[') { flow_list(rest).join(" and ") } else { scalar(rest) };
        match name.as_str() {
            "type" => current.kind = value.to_ascii_lowercase(),
            _ => push(current, &name, value, range),
        }
    }
    finish(&mut bib, &mut entry, &mut list, &mut parent_kind);
    bib
}

fn push(entry: &mut Entry, name: &str, value: String, range: Range<usize>) {
    entry.fields.push(Field { name: name.to_string(), name_range: range.clone(), value: Value { parts: vec![Part::Text(value)] }, value_range: range });
}

/// Ends the entry: lists of names become one field, and the parent's title
/// becomes the journal or the book it appeared in.
fn finish(bib: &mut Bib, entry: &mut Option<Entry>, list: &mut Option<(String, Vec<String>, Range<usize>)>, parent_kind: &mut String) {
    let Some(mut done) = entry.take() else {
        return;
    };
    if let Some((field, items, range)) = list.take() {
        push(&mut done, &field, items.join(" and "), range);
    }
    if let Some(parent) = done.fields.iter_mut().find(|field| field.name == "parent-title") {
        parent.name = if matches!(parent_kind.as_str(), "periodical" | "newspaper") { "journal" } else { "booktitle" }.into();
    }
    parent_kind.clear();
    bib.entries.push(done);
}

/// `name: rest`, with the colon outside quotes, followed by a space or the end.
fn split_key(content: &str) -> Option<(&str, &str)> {
    let (key, rest) = if let Some(quoted) = content.strip_prefix('"') {
        let end = quoted.find('"')?;
        (&quoted[..end], quoted[end + 1..].strip_prefix(':')?)
    } else if let Some(quoted) = content.strip_prefix('\'') {
        let end = quoted.find('\'')?;
        (&quoted[..end], quoted[end + 1..].strip_prefix(':')?)
    } else {
        let colon = content.match_indices(':').map(|(index, _)| index).find(|index| content[index + 1..].is_empty() || content[index + 1..].starts_with(' '))?;
        (&content[..colon], &content[colon + 1..])
    };
    let rest = rest.trim();
    let rest = if rest.starts_with('#') { "" } else { rest };
    (!key.is_empty()).then_some((key, rest))
}

/// A plain, single quoted or double quoted value, without a trailing comment.
fn scalar(text: &str) -> String {
    let text = text.trim();
    if let Some(inner) = text.strip_prefix('"') {
        let mut out = String::new();
        let mut chars = inner.chars();
        while let Some(ch) = chars.next() {
            match ch {
                '"' => break,
                '\\' => out.extend(chars.next()),
                ch => out.push(ch),
            }
        }
        return out;
    }
    if let Some(inner) = text.strip_prefix('\'') {
        return inner.rsplit_once('\'').map_or(inner, |(inner, _)| inner).replace("''", "'");
    }
    text.split(" #").next().unwrap_or_default().trim().to_string()
}

/// `[a, "b, c"]` as its items.
fn flow_list(text: &str) -> Vec<String> {
    let inner = text.trim().trim_start_matches('[');
    let inner = inner.rsplit_once(']').map_or(inner, |(inner, _)| inner);
    let mut items = Vec::new();
    let mut quote = None;
    let mut start = 0;
    for (index, ch) in inner.char_indices() {
        match ch {
            '"' | '\'' if quote.is_none() => quote = Some(ch),
            ch if quote == Some(ch) => quote = None,
            ',' if quote.is_none() => {
                items.push(scalar(&inner[start..index]));
                start = index + 1;
            }
            _ => {}
        }
    }
    items.push(scalar(&inner[start..]));
    items.retain(|item| !item.is_empty());
    items
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field<'a>(entry: &'a Entry, name: &str) -> &'a str {
        match entry.field(name).map(|field| &field.value.parts[..]) {
            Some([Part::Text(text)]) => text,
            _ => "",
        }
    }

    const YAML: &str = r#"# Sources
harry:
    type: Book
    title: Harry Potter and the Order of the Phoenix
    author: Rowling, J. K.
    date: 2003-06-21

vaswani2017:
    type: article
    title: "Attention Is All You Need"
    author:
        - Vaswani, Ashish
        - Shazeer, Noam
    date: 2017
    parent:
        type: periodical
        title: Advances in Neural Information Processing Systems
    serial-number:
        doi: 10.5555/3295222

"key:with-colon":
    type: web
    author: ["Doe, Jane", 'Roe, Rick']
    title:
        value: A Title
        short: Title
    url: https://example.org
"#;

    #[test]
    fn entries_with_lists_and_parents() {
        let bib = parse(YAML);
        assert!(bib.diagnostics.is_empty(), "{:?}", bib.diagnostics);
        let keys: Vec<&str> = bib.entries.iter().map(|entry| entry.key.as_str()).collect();
        assert_eq!(keys, ["harry", "vaswani2017", "key:with-colon"]);
        let harry = &bib.entries[0];
        assert_eq!((harry.kind.as_str(), field(harry, "author"), field(harry, "date")), ("book", "Rowling, J. K.", "2003-06-21"));
        let vaswani = &bib.entries[1];
        assert_eq!(field(vaswani, "title"), "Attention Is All You Need");
        assert_eq!(field(vaswani, "author"), "Vaswani, Ashish and Shazeer, Noam");
        assert_eq!(field(vaswani, "journal"), "Advances in Neural Information Processing Systems");
        assert_eq!(field(vaswani, "doi"), "10.5555/3295222");
        let web = &bib.entries[2];
        assert_eq!(field(web, "author"), "Doe, Jane and Roe, Rick");
        assert_eq!(field(web, "title"), "A Title");
        assert_eq!(field(web, "url"), "https://example.org");
        assert_eq!(&YAML[vaswani.key_range.clone()], "vaswani2017");
    }

    #[test]
    fn mistakes_have_lines() {
        let text = "a:\n    title: One\na:\n    title: Two\n\tbad: x\nloose text\nb: inline\n";
        let bib = parse(text);
        let messages: Vec<(usize, &str)> = bib.diagnostics.iter().map(|diagnostic| (crate::text::line_col(text, diagnostic.range.start).0, diagnostic.message.as_str())).collect();
        assert_eq!(
            messages,
            [
                (2, "The key a is already used on line 1."),
                (4, "YAML does not allow tabs for indentation."),
                (5, "An entry starts with its key and a colon, as in `key:`."),
                (6, "The fields of b go on the lines below it, indented.")
            ]
        );
    }
}
