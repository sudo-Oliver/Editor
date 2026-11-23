//! What would go wrong in the PDF, found before LaTeX runs: keys and labels
//! that do not exist, labels defined twice or before their caption, entries
//! that lack what the style prints, the same work under two keys, and text
//! that BibTeX or biber would mangle.

use std::{collections::HashMap, ops::Range, path::Path};

use crate::{
    Diagnostic, Severity,
    bib::{self, Part},
    index::{CitationIndex, Entries, IndexedEntry},
    project::Backend,
    text::fold,
};

fn diagnostic(range: Range<usize>, severity: Severity, message: String) -> Diagnostic {
    Diagnostic { range, severity, message }
}

/// `chapter2.tex:14`, for messages that point to another place.
fn place(path: &Path, line: usize) -> String {
    format!("{}:{}", path.file_name().map_or_else(String::new, |name| name.to_string_lossy().into_owned()), line + 1)
}

/// Everything wrong in one source file of the document, its own mistakes and
/// those of the index that point to it, such as a missing bibliography.
pub fn source(index: &CitationIndex, path: &Path) -> Vec<Diagnostic> {
    let mut out = index.diagnostics(path).to_vec();
    let Some(scan) = index.scan_of(path) else {
        return out;
    };
    // A bibliography that is missing or on the web could have any key.
    if index.bibs_complete {
        for key in scan.cites.iter().filter(|key| index.entry(&key.name).is_none()) {
            let message = match index.entry_ignoring_case(&key.name) {
                Some(entry) => format!("No entry has the key {}. There is {}, which differs in case.", key.name, entry.key),
                None => format!("No bibliography has the key {}.", key.name),
            };
            out.push(diagnostic(key.range.clone(), Severity::Error, message));
        }
    }
    for key in scan.either.iter().filter(|key| index.entry(&key.name).is_none() && !index.knows_label(&key.name)) {
        if index.bibs_complete {
            out.push(diagnostic(key.range.clone(), Severity::Error, format!("There is no label or bibliography entry {}.", key.name)));
        }
    }
    if !index.external_unknown {
        for key in scan.refs.iter().filter(|key| !index.knows_label(&key.name)) {
            out.push(diagnostic(key.range.clone(), Severity::Warning, format!("No label {} in the document, so the PDF shows ??.", key.name)));
        }
    }
    for label in &scan.labels {
        let definitions = index.label(&label.name);
        if definitions.len() > 1
            && let Some(other) = definitions.iter().find(|other| !(other.path.as_path() == path && other.label.range == label.range))
        {
            let message = format!(
                "The label {} is defined {} times, so references to it may point to the wrong place. Also at {}.",
                label.name,
                definitions.len(),
                place(&other.path, other.label.line)
            );
            out.push(diagnostic(label.range.clone(), Severity::Error, message));
        }
        if label.before_caption {
            let message = format!("This \\label comes before \\caption, so \\ref{{{}}} gives the number of the section, not of the {}. Move it after the caption.", label.name, label.env.trim_end_matches('*'));
            out.push(diagnostic(label.range.clone(), Severity::Warning, message));
        }
    }
    out
}

/// What a style prints for each kind of entry: each group needs one of its fields.
fn required(kind: &str, biblatex: bool) -> &'static [&'static [&'static str]] {
    const AUTHOR: &[&str] = &["author"];
    const PEOPLE: &[&str] = &["author", "editor"];
    const TITLE: &[&str] = &["title"];
    const YEAR: &[&str] = &["year", "date"];
    const JOURNAL: &[&str] = &["journal", "journaltitle"];
    const BOOKTITLE: &[&str] = &["booktitle"];
    const PUBLISHER: &[&str] = &["publisher"];
    const SCHOOL: &[&str] = &["school", "institution"];
    const INSTITUTION: &[&str] = &["institution"];
    const LINK: &[&str] = &["url", "doi", "eprint"];
    match kind {
        "article" => &[AUTHOR, TITLE, JOURNAL, YEAR],
        "book" => &[PEOPLE, TITLE, PUBLISHER, YEAR],
        "incollection" => &[AUTHOR, TITLE, BOOKTITLE, PUBLISHER, YEAR],
        "inproceedings" | "conference" => &[AUTHOR, TITLE, BOOKTITLE, YEAR],
        "phdthesis" | "mastersthesis" => &[AUTHOR, TITLE, SCHOOL, YEAR],
        "techreport" | "report" => &[AUTHOR, TITLE, INSTITUTION, YEAR],
        "online" if biblatex => &[PEOPLE, TITLE, LINK],
        "booklet" | "manual" => &[TITLE],
        "unpublished" => &[AUTHOR, TITLE],
        _ => &[],
    }
}

/// Fields a style prints that an entry lacks, the same work under two keys.
/// For BibTeX and biber; Typst prints what there is.
pub(crate) fn entries(index: &Entries, backend: Backend, biblatex: bool, line_of: &dyn Fn(&IndexedEntry) -> String) -> Vec<(std::path::PathBuf, Diagnostic)> {
    let mut out = Vec::new();
    if backend == Backend::Typst {
        return out;
    }
    let program = if backend == Backend::Biber { "biber" } else { "BibTeX" };
    for entry in index.entries.iter().filter(|entry| entry.kind != "bibitem") {
        let missing: Vec<&str> = required(&entry.kind, biblatex)
            .iter()
            .filter(|group| !group.iter().any(|name| entry.field(name).is_some_and(|value| !value.trim().is_empty())))
            .map(|group| group[0])
            .collect();
        if !missing.is_empty() {
            let message = format!("This @{} has no {}, which the bibliography prints. {program} warns and leaves it out.", entry.kind, join(&missing));
            out.push((entry.path.to_path_buf(), diagnostic(entry.key_range.clone(), Severity::Warning, message)));
        }
    }
    // The same work, copied in twice from different sources over the years.
    let mut by_doi: HashMap<String, usize> = HashMap::new();
    let mut by_title: HashMap<(String, String), usize> = HashMap::new();
    for (position, entry) in index.entries.iter().enumerate() {
        let doi = entry.doi.trim().to_lowercase();
        let doi = doi.trim_start_matches("https://doi.org/").trim_start_matches("http://dx.doi.org/").trim_start_matches("doi:").to_string();
        let first = if doi.is_empty() {
            None
        } else {
            by_doi.get(&doi).copied().map(|first| (first, "DOI")).or_else(|| {
                by_doi.insert(doi, position);
                None
            })
        };
        let title: String = fold(&entry.title).chars().filter(|ch| ch.is_alphanumeric()).collect();
        let first = first.or_else(|| {
            if title.len() < 12 || entry.year.is_empty() {
                return None;
            }
            let key = (title, entry.year.clone());
            by_title.get(&key).copied().map(|first| (first, "title and year")).or_else(|| {
                by_title.insert(key, position);
                None
            })
        });
        if let Some((first, what)) = first {
            let other = &index.entries[first];
            if other.key == entry.key {
                continue;
            }
            let message = format!("Same {what} as {} at {}: the same work under two keys?", other.key, line_of(other));
            out.push((entry.path.to_path_buf(), diagnostic(entry.key_range.clone(), Severity::Hint, message)));
            let message = format!("Same {what} as {} at {}: the same work under two keys?", entry.key, line_of(entry));
            out.push((other.path.to_path_buf(), diagnostic(other.key_range.clone(), Severity::Hint, message)));
        }
    }
    out
}

fn join(names: &[&str]) -> String {
    match names {
        [] => String::new(),
        [one] => one.to_string(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// Where the text of a field starts in the file, when it is one braced or
/// quoted part, so that a mark can point into it.
fn text_start(field: &bib::Field) -> Option<(usize, &str)> {
    match field.value.parts.as_slice() {
        [Part::Text(text)] => Some((field.value_range.start + 1, text.as_str())),
        _ => None,
    }
}

/// The text of one entry as BibTeX or biber will read it: capitals that a
/// BibTeX style lowercases, and accents the program reads badly or that UTF-8
/// says more plainly.
pub(crate) fn entry_text(entry: &bib::Entry, backend: Backend, biblatex: bool, out: &mut Vec<Diagnostic>) {
    if backend == Backend::Typst {
        return;
    }
    // Classic BibTeX styles lowercase titles; biblatex leaves them as written.
    if backend == Backend::Bibtex
        && !biblatex
        && let Some(title) = entry.field("title")
        && let Some((start, text)) = text_start(title)
    {
        for (offset, word) in unprotected_capitals(text).into_iter().take(3) {
            let message = format!("BibTeX styles lowercase titles, so {word} becomes {}. Write {{{word}}} to keep it.", word.to_lowercase());
            out.push(diagnostic(start + offset..start + offset + word.len(), Severity::Hint, message));
        }
    }
    let typeset = || entry.fields.iter().filter(|field| !matches!(field.name.as_str(), "url" | "doi" | "file" | "eprint" | "pdf" | "urldate"));
    match backend {
        Backend::Bibtex => {
            let first = typeset().find_map(|field| {
                let (start, text) = text_start(field)?;
                let at = text.char_indices().find(|(_, ch)| !ch.is_ascii())?;
                Some((start + at.0, at.1))
            });
            if let Some((at, ch)) = first {
                let message = format!("BibTeX reads bytes, not UTF-8, so {ch} can sort wrong or break the style. Write it with TeX, as in {{\\\"u}}, or use biblatex with biber.");
                out.push(diagnostic(at..at + ch.len_utf8(), Severity::Hint, message));
            }
        }
        Backend::Biber => {
            if let Some(field) = typeset().find(|field| text_start(field).is_some_and(|(_, text)| crate::text::has_tex_accent(text))) {
                let message = "biber reads UTF-8, so the letter itself, such as ü, says more plainly what the TeX accent does.".to_string();
                out.push(diagnostic(field.value_range.clone(), Severity::Hint, message));
            }
        }
        Backend::Typst => {}
    }
}

/// Words in a title, outside braces, math and commands, with two capitals or
/// a capital after a small letter, such as GPU or LaTeX.
fn unprotected_capitals(text: &str) -> Vec<(usize, &str)> {
    let mut words = Vec::new();
    let bytes = text.as_bytes();
    let (mut depth, mut math, mut index) = (0usize, false, 0);
    while index < bytes.len() {
        match bytes[index] {
            b'{' => depth += 1,
            b'}' => depth = depth.saturating_sub(1),
            b'$' => math = !math,
            b'\\' => {
                index += 1;
                while index < bytes.len() && bytes[index].is_ascii_alphabetic() {
                    index += 1;
                }
                continue;
            }
            byte if byte.is_ascii_alphanumeric() && depth == 0 && !math => {
                let start = index;
                while index < bytes.len() && (bytes[index].is_ascii_alphanumeric() || bytes[index] == b'-') {
                    index += 1;
                }
                let word = &text[start..index];
                let capitals = word.bytes().filter(u8::is_ascii_uppercase).count();
                let inner = word.as_bytes().windows(2).any(|pair| pair[0].is_ascii_lowercase() && pair[1].is_ascii_uppercase());
                if capitals >= 2 || inner {
                    words.push((start, word));
                }
                continue;
            }
            _ => {}
        }
        index += 1;
    }
    words
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capitals_that_a_style_would_lowercase() {
        let words: Vec<&str> = unprotected_capitals("Using GPUs in {Rust} with {LaTeX} and LaTeX, $O(N)$ and \\TeX and iPhone and Deep-Learning").into_iter().map(|(_, word)| word).collect();
        assert_eq!(words, ["GPUs", "LaTeX", "iPhone", "Deep-Learning"]);
    }

    #[test]
    fn entry_text_by_backend() {
        let bib = bib::parse("@article{a, title = {Using GPU on Rust}, author = {M{\\\"u}ller and Müller}}");
        let entry = &bib.entries[0];
        let mut out = Vec::new();
        entry_text(entry, Backend::Bibtex, false, &mut out);
        let messages: Vec<&str> = out.iter().map(|diagnostic| diagnostic.message.as_str()).collect();
        assert_eq!(messages.len(), 2, "{messages:?}");
        assert!(messages[0].contains("GPU becomes gpu"));
        assert!(messages[1].starts_with("BibTeX reads bytes"));
        let text = "@article{a, title = {Using GPU on Rust}, author = {M{\\\"u}ller and Müller}}";
        assert_eq!(&text[out[0].range.clone()], "GPU");
        assert_eq!(&text[out[1].range.clone()], "ü");
        let mut out = Vec::new();
        entry_text(entry, Backend::Biber, true, &mut out);
        assert_eq!(out.len(), 1);
        assert!(out[0].message.starts_with("biber reads UTF-8"));
        let mut out = Vec::new();
        entry_text(entry, Backend::Bibtex, true, &mut out);
        assert!(!out.iter().any(|diagnostic| diagnostic.message.contains("lowercase")), "biblatex keeps capitals");
    }
}
