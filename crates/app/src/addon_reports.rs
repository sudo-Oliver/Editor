//! What add-ons can ask about a project beyond its files, worked out without
//! a window: labels and citations with their uses, the changes of a file as a
//! diff, word counts, fixes by a name that stays, and hits in the PDF.

use std::collections::{BTreeMap, HashSet};

use addon_api::{CitationInfo, FixInfo, LabelInfo, MissingName, PdfHit, PdfRect, ReferenceReport, SectionWords, SourcePlace, WordCountInfo};
use build::Fix;
use editor_core::{
    Language,
    line_diff::{HunkKind, line_diff},
    word_count::WordCount,
};
use pdf::PageContent;

use crate::pdf_text;

/// Commands whose braces hold labels to point to.
const REF_COMMANDS: &[&str] =
    &["ref", "eqref", "pageref", "autoref", "Autoref", "cref", "Cref", "cpageref", "Cpageref", "vref", "Vref", "nameref", "Nameref", "labelcref", "subref"];
/// Characters of the PDF shown on each side of a hit.
const HIT_CONTEXT: usize = 40;

/// A name in a source file and what it does there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Use {
    Label,
    Ref,
    Cite,
    /// `@name` in Typst, which points to a label or cites a key.
    At,
}

/// Labels and citations of the source files, which are pairs of a path
/// relative to the project and the text. `bib_keys` are the keys of the
/// project's bibliography files.
pub fn references(files: &[(String, String)], bib_keys: &[String]) -> ReferenceReport {
    let mut labels: BTreeMap<String, LabelInfo> = BTreeMap::new();
    let mut refs: Vec<(String, SourcePlace)> = Vec::new();
    let mut cites: Vec<(String, SourcePlace)> = Vec::new();
    let mut ats: Vec<(String, SourcePlace)> = Vec::new();
    let mut cites_everything = false;
    for (path, text) in files {
        let found = match Language::from_path(std::path::Path::new(path)) {
            Language::Latex => latex_names(text),
            Language::Typst => typst_names(text),
            _ => continue,
        };
        for (kind, name, line) in found {
            let place = SourcePlace { path: path.clone(), line };
            match kind {
                Use::Label => {
                    labels.entry(name.clone()).or_insert(LabelInfo { name, path: place.path, line, uses: Vec::new() });
                }
                Use::Ref => refs.push((name, place)),
                Use::Cite if name == "*" => cites_everything = true,
                Use::Cite => cites.push((name, place)),
                Use::At => ats.push((name, place)),
            }
        }
    }
    // In Typst one sign does both, and a label wins over a key of the same name.
    for (name, place) in ats {
        if labels.contains_key(&name) || !bib_keys.contains(&name) {
            refs.push((name, place));
        } else {
            cites.push((name, place));
        }
    }
    let mut report = ReferenceReport::default();
    for (name, place) in refs {
        match labels.get_mut(&name) {
            Some(label) => label.uses.push(place),
            None => report.undefined_refs.push(MissingName { name, path: place.path, line: place.line }),
        }
    }
    let bib: HashSet<&str> = bib_keys.iter().map(String::as_str).collect();
    let mut citations: BTreeMap<String, CitationInfo> = BTreeMap::new();
    for (key, place) in cites {
        let defined = bib.contains(key.as_str());
        if !defined {
            report.undefined_citations.push(MissingName { name: key.clone(), path: place.path.clone(), line: place.line });
        }
        citations.entry(key.clone()).or_insert(CitationInfo { key, defined, uses: Vec::new() }).uses.push(place);
    }
    report.unused_labels = labels.values().filter(|label| label.uses.is_empty()).map(|label| label.name.clone()).collect();
    if !cites_everything {
        let mut unused: Vec<String> = bib_keys.iter().filter(|key| !citations.contains_key(*key)).cloned().collect();
        unused.sort();
        unused.dedup();
        report.unused_entries = unused;
    }
    report.labels = labels.into_values().collect();
    report.citations = citations.into_values().collect();
    report
}

/// `\label`, the `\ref` kin and every `\…cite…` command, with their lines from 1.
fn latex_names(text: &str) -> Vec<(Use, String, usize)> {
    let mut found = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let line = strip_latex_comment(line);
        let mut rest = line;
        while let Some(slash) = rest.find('\\') {
            rest = &rest[slash + 1..];
            let name_end = rest.find(|ch: char| !ch.is_ascii_alphabetic()).unwrap_or(rest.len());
            let name = &rest[..name_end];
            let kind = if name == "label" {
                Use::Label
            } else if REF_COMMANDS.contains(&name) {
                Use::Ref
            } else if name.to_ascii_lowercase().contains("cite") {
                Use::Cite
            } else {
                // `\\` and `\%` name no command, so the second sign is skipped too.
                let skip = if name_end > 0 { name_end } else { rest.chars().next().map_or(0, char::len_utf8) };
                rest = &rest[skip..];
                continue;
            };
            rest = &rest[name_end..];
            let Some((argument, after)) = braced_argument(rest) else {
                continue;
            };
            rest = after;
            for name in argument.split(',').map(str::trim).filter(|name| !name.is_empty()) {
                found.push((kind, name.to_string(), index + 1));
            }
        }
    }
    found
}

/// The text before a `%` that no backslash escapes.
fn strip_latex_comment(line: &str) -> &str {
    let mut backslashes = 0;
    for (index, ch) in line.char_indices() {
        match ch {
            '%' if backslashes % 2 == 0 => return &line[..index],
            '\\' => backslashes += 1,
            _ => backslashes = 0,
        }
    }
    line
}

/// After a command name: a star, up to two optional arguments in brackets,
/// then the first argument in braces and the text after it.
fn braced_argument(text: &str) -> Option<(&str, &str)> {
    let mut rest = text.strip_prefix('*').unwrap_or(text).trim_start();
    for _ in 0..2 {
        if let Some(inner) = rest.strip_prefix('[') {
            rest = inner[inner.find(']')? + 1..].trim_start();
        }
    }
    let inner = rest.strip_prefix('{')?;
    let close = inner.find('}')?;
    Some((&inner[..close], &inner[close + 1..]))
}

/// `<label>` and `@name` in Typst, outside of comments.
fn typst_names(text: &str) -> Vec<(Use, String, usize)> {
    let is_name = |ch: char| ch.is_alphanumeric() || matches!(ch, '_' | '-' | ':' | '.');
    let mut found = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let line = line.find("//").map_or(line, |comment| &line[..comment]);
        let chars: Vec<(usize, char)> = line.char_indices().collect();
        for (position, &(at, ch)) in chars.iter().enumerate() {
            let before = position.checked_sub(1).map(|previous| chars[previous].1);
            match ch {
                '<' => {
                    let rest = &line[at + 1..];
                    let end = rest.find(|ch: char| !is_name(ch)).unwrap_or(rest.len());
                    if end > 0 && rest[end..].starts_with('>') && rest.starts_with(char::is_alphabetic) {
                        found.push((Use::Label, rest[..end].to_string(), index + 1));
                    }
                }
                // `a@b` is a mail address.
                '@' if !before.is_some_and(char::is_alphanumeric) => {
                    let rest = &line[at + 1..];
                    let name = rest[..rest.find(|ch: char| !is_name(ch)).unwrap_or(rest.len())].trim_end_matches(['.', ':']);
                    if !name.is_empty() {
                        found.push((Use::At, name.to_string(), index + 1));
                    }
                }
                _ => {}
            }
        }
    }
    found
}

/// The changes from `old` to `new` as a unified diff without context lines,
/// empty when there are none.
pub fn unified_diff(path: &str, old: &str, new: &str) -> String {
    if old == new {
        return String::new();
    }
    let mut diff = format!("--- a/{path}\n+++ b/{path}\n");
    let Some(hunks) = line_diff(old, new) else {
        diff.push_str("@@ The change is too large to show line by line. @@\n");
        return diff;
    };
    let (old_lines, new_lines): (Vec<&str>, Vec<&str>) = (old.lines().collect(), new.lines().collect());
    // A count of zero names the line before, as `diff -U0` does.
    let range = |lines: &std::ops::Range<usize>| if lines.is_empty() { format!("{},0", lines.start) } else { format!("{},{}", lines.start + 1, lines.len()) };
    for hunk in hunks {
        let removed = if hunk.kind == HunkKind::Added { hunk.old_lines.start..hunk.old_lines.start } else { hunk.old_lines.clone() };
        let added = if hunk.kind == HunkKind::Deleted { hunk.lines.start..hunk.lines.start } else { hunk.lines.clone() };
        diff.push_str(&format!("@@ -{} +{} @@\n", range(&removed), range(&added)));
        for line in removed.filter_map(|line| old_lines.get(line)) {
            diff.push_str(&format!("-{line}\n"));
        }
        for line in added.filter_map(|line| new_lines.get(line)) {
            diff.push_str(&format!("+{line}\n"));
        }
    }
    diff
}

pub fn word_count_info(path: String, count: &WordCount) -> WordCountInfo {
    WordCountInfo {
        path,
        total: count.total(),
        body: count.body,
        headings: count.headings,
        captions: count.captions,
        inline_math: count.inline_math,
        display_math: count.display_math,
        sections: count.sections.iter().map(|section| SectionWords { title: section.title.clone(), words: section.words }).collect(),
        missing: count.missing.clone(),
    }
}

/// The name of a fix, which stays the same from one build to the next.
pub fn fix_id(fix: &Fix) -> String {
    match fix {
        Fix::AddPackage(package) => format!("add_package:{package}"),
        Fix::RemovePackage(package) => format!("remove_package:{package}"),
        Fix::InstallTool(tool) => format!("install_tool:{tool}"),
    }
}

pub fn fix_info(fix: &Fix) -> FixInfo {
    FixInfo { id: fix_id(fix), label: fix.label() }
}

/// Where `query` occurs on one page, ignoring case. `page` starts at 1.
pub fn page_hits(page: usize, content: &PageContent, query: &str, limit: usize) -> Vec<PdfHit> {
    let fold = |ch: char| ch.to_lowercase().next().unwrap_or(ch);
    let query: Vec<char> = query.chars().map(fold).collect();
    let text: Vec<char> = content.chars.iter().map(|char| fold(char.ch)).collect();
    if query.is_empty() || query.len() > text.len() {
        return Vec::new();
    }
    let mut hits = Vec::new();
    let mut start = 0;
    while start + query.len() <= text.len() && hits.len() < limit {
        if text[start..start + query.len()] != query[..] {
            start += 1;
            continue;
        }
        let range = start..start + query.len();
        let rects = pdf_text::selection_rects(&content.chars, range.clone())
            .iter()
            .map(|rect| PdfRect { left: rect.left, top: rect.top, right: rect.right, bottom: rect.bottom })
            .collect();
        let around = range.start.saturating_sub(HIT_CONTEXT)..(range.end + HIT_CONTEXT).min(text.len());
        let context: String = content.chars[around].iter().map(|char| if char.ch.is_whitespace() { ' ' } else { char.ch }).collect();
        hits.push(PdfHit { page, rects, context: context.split_whitespace().collect::<Vec<_>>().join(" ") });
        start = range.end;
    }
    hits
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_meet_their_references_and_keys_their_citations() {
        let files = vec![
            (
                "main.tex".to_string(),
                "\\section{Intro}\\label{sec:intro}\nSee \\cref{sec:intro, fig:a} and \\ref{sec:gone}.\n% \\ref{sec:commented}\n\\citep[p.~3]{knuth84,lamport94} \\textcite{nobody}\n".to_string(),
            ),
            ("ch/b.tex".to_string(), "\\begin{figure}\\label{fig:a}\\end{figure}\\label{tab:unused}\n100\\% \\autoref*{fig:a}\n".to_string()),
        ];
        let bib = vec!["knuth84".to_string(), "lamport94".to_string(), "unread".to_string()];
        let report = references(&files, &bib);
        let labels: Vec<(&str, usize)> = report.labels.iter().map(|label| (label.name.as_str(), label.uses.len())).collect();
        assert_eq!(labels, [("fig:a", 2), ("sec:intro", 1), ("tab:unused", 0)]);
        assert_eq!(report.labels[0].path, "ch/b.tex");
        assert_eq!(report.labels[0].uses[1], SourcePlace { path: "ch/b.tex".into(), line: 2 });
        assert_eq!(report.undefined_refs, [MissingName { name: "sec:gone".into(), path: "main.tex".into(), line: 2 }]);
        assert_eq!(report.unused_labels, ["tab:unused"]);
        let keys: Vec<(&str, bool)> = report.citations.iter().map(|citation| (citation.key.as_str(), citation.defined)).collect();
        assert_eq!(keys, [("knuth84", true), ("lamport94", true), ("nobody", false)]);
        assert_eq!(report.undefined_citations[0].name, "nobody");
        assert_eq!(report.unused_entries, ["unread"]);
    }

    #[test]
    fn nocite_everything_leaves_no_entry_unused() {
        let files = vec![("main.tex".to_string(), "\\nocite{*}".to_string())];
        assert!(references(&files, &["a".to_string()]).unused_entries.is_empty());
    }

    #[test]
    fn typst_at_points_to_labels_or_cites_keys() {
        let files = vec![("main.typ".to_string(), "= Intro <intro>\nAs @intro shows, and @knuth84. Mail a@b.org. // @hidden\n$a < b > c$\n".to_string())];
        let report = references(&files, &["knuth84".to_string()]);
        assert_eq!(report.labels.len(), 1);
        assert_eq!(report.labels[0].uses, [SourcePlace { path: "main.typ".into(), line: 2 }]);
        assert_eq!(report.citations[0].key, "knuth84");
        assert!(report.undefined_refs.is_empty(), "{:?}", report.undefined_refs);
    }

    #[test]
    fn diffs_name_lines_like_diff_does() {
        let diff = unified_diff("a.tex", "one\ntwo\nthree\nfour\n", "one\n2\nthree\nfour\nfive\n");
        assert_eq!(diff, "--- a/a.tex\n+++ b/a.tex\n@@ -2,1 +2,1 @@\n-two\n+2\n@@ -4,0 +5,1 @@\n+five\n");
        let deleted = unified_diff("a.tex", "one\ntwo\nthree\n", "one\nthree\n");
        assert_eq!(deleted, "--- a/a.tex\n+++ b/a.tex\n@@ -2,1 +1,0 @@\n-two\n");
        assert_eq!(unified_diff("a.tex", "same", "same"), "");
    }

    #[test]
    fn word_counts_come_per_section() {
        let text = "\\documentclass{article}\n\\begin{document}\n\\section{One}\nTwo words.\n\\section{Two}\nThree more words.\n\\end{document}\n";
        let count = editor_core::word_count::count_words(std::path::Path::new("/p/main.tex"), text, std::path::Path::new("/p"), &mut |_| None);
        let info = word_count_info("main.tex".into(), &count);
        let sections: Vec<(&str, usize)> = info.sections.iter().map(|section| (section.title.as_str(), section.words)).collect();
        assert_eq!(sections, [("One", 3), ("Two", 4)]);
        assert_eq!(info.total, 7);
    }

    #[test]
    fn fixes_keep_their_names() {
        assert_eq!(fix_id(&Fix::AddPackage("amsmath".into())), "add_package:amsmath");
        assert_eq!(fix_info(&Fix::RemovePackage("x".into())).label, "Remove x");
    }

    #[test]
    fn pdf_hits_ignore_case_and_carry_rects() {
        let chars = "Hello World, hello"
            .chars()
            .enumerate()
            .map(|(index, ch)| pdf::PageChar { ch, rect: pdf::Rect { left: index as f32, top: 0.0, right: index as f32 + 1.0, bottom: 10.0 } })
            .collect();
        let content = PageContent { chars, links: Box::new([]) };
        let hits = page_hits(3, &content, "HELLO", 10);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[1].page, 3);
        assert_eq!(hits[1].rects, [PdfRect { left: 13.0, top: 0.0, right: 18.0, bottom: 10.0 }]);
        assert_eq!(page_hits(1, &content, "hello", 1).len(), 1);
    }
}
