//! The search overlay for `Space f` and `Space g`. Text search reads the files
//! on disk, so unsaved edits do not count.

use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use editor_core::vim::find_literal;
use gpui::Task;

const MAX_RESULTS: usize = 200;
const MAX_SEARCHED_FILE_BYTES: u64 = 4 << 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PickerKind {
    Files,
    Text,
    /// Images for a figure at the cursor.
    Figures,
    /// `.bib` entries for a citation at the cursor.
    Citations,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PickerItem {
    pub path: PathBuf,
    /// Path relative to the project.
    pub label: String,
    /// Zero-based line and byte column of a text hit.
    pub position: Option<(usize, usize)>,
    pub detail: Option<String>,
}

pub struct Picker {
    pub kind: PickerKind,
    pub query: String,
    pub items: Vec<PickerItem>,
    pub selected: usize,
    pub root: PathBuf,
    pub files: Arc<Vec<PathBuf>>,
    /// The citations to choose from.
    pub entries: Arc<Vec<PickerItem>>,
    /// Set to stop the running text search when the query changes.
    pub cancel: Arc<AtomicBool>,
    pub task: Option<Task<()>>,
}

pub fn relative_label(path: &Path, root: &Path) -> String {
    path.strip_prefix(root).unwrap_or(path).to_string_lossy().into_owned()
}

/// Files that match `query`, best first. The letters must appear in order.
pub fn match_files(files: &[PathBuf], root: &Path, query: &str) -> Vec<PickerItem> {
    let mut scored: Vec<(i64, PickerItem)> = files
        .iter()
        .filter_map(|path| {
            let label = relative_label(path, root);
            let score = fuzzy_score(query, &label)?;
            Some((score, PickerItem { path: path.clone(), label, position: None, detail: None }))
        })
        .collect();
    scored.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.label.cmp(&right.1.label)));
    scored.into_iter().take(MAX_RESULTS).map(|(_, item)| item).collect()
}

/// The entries of the `.bib` files. `label` is the key, `detail` the title.
pub fn read_citations(files: &[PathBuf]) -> Vec<PickerItem> {
    let mut items = Vec::new();
    for path in files {
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        items.extend(parse_bib(&text).into_iter().map(|(label, title)| PickerItem {
            path: path.clone(),
            label,
            position: None,
            detail: title,
        }));
    }
    items
}

/// Reads keys and titles line by line, which fits how `.bib` files are written.
fn parse_bib(text: &str) -> Vec<(String, Option<String>)> {
    let mut entries: Vec<(String, Option<String>)> = Vec::new();
    for line in text.lines().map(str::trim) {
        if let Some((kind, rest)) = line.strip_prefix('@').and_then(|rest| rest.split_once(['{', '('])) {
            let kind = kind.trim().to_ascii_lowercase();
            let key = rest.split(',').next().unwrap_or_default().trim();
            if !key.is_empty() && !matches!(kind.as_str(), "comment" | "string" | "preamble") {
                entries.push((key.to_string(), None));
            }
        } else if let Some((name, value)) = line.split_once('=')
            && name.trim().eq_ignore_ascii_case("title")
            && let Some((_, title)) = entries.last_mut()
        {
            let value = value.trim().trim_end_matches(',').replace(['{', '}', '"'], "");
            *title = Some(value.trim().to_string());
        }
    }
    entries
}

/// Entries that match `query` in the label or detail, best first.
pub fn match_entries(entries: &[PickerItem], query: &str) -> Vec<PickerItem> {
    let mut scored: Vec<(i64, &PickerItem)> = entries
        .iter()
        .filter_map(|item| {
            let text = format!("{} {}", item.label, item.detail.as_deref().unwrap_or_default());
            Some((fuzzy_score(query, &text)?, item))
        })
        .collect();
    scored.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.label.cmp(&right.1.label)));
    scored.into_iter().take(MAX_RESULTS).map(|(_, item)| item.clone()).collect()
}

/// Rates a subsequence match. Runs of letters and starts of names count more.
pub fn fuzzy_score(query: &str, candidate: &str) -> Option<i64> {
    let mut score = 0i64;
    let mut previous: Option<usize> = None;
    let chars: Vec<char> = candidate.chars().collect();
    let mut position = 0;
    for wanted in query.chars().filter(|ch| !ch.is_whitespace()) {
        let wanted = wanted.to_lowercase().next().unwrap_or(wanted);
        let found = (position..chars.len()).find(|index| chars[*index].to_lowercase().next() == Some(wanted))?;
        score += 1;
        if previous.is_some_and(|previous| previous + 1 == found) {
            score += 8;
        }
        if found == 0 || matches!(chars[found - 1], '/' | '\\' | '_' | '-' | ' ' | '.') {
            score += 6;
        }
        previous = Some(found);
        position = found + 1;
    }
    // Among equal matches, shorter paths win.
    Some(score * 16 - chars.len() as i64)
}

/// Finds literal text in the given files. Stops early when `cancel` is set.
pub fn search_text(files: &[PathBuf], root: &Path, query: &str, cancel: &AtomicBool) -> Vec<PickerItem> {
    let mut items = Vec::new();
    if query.is_empty() {
        return items;
    }
    for path in files {
        if cancel.load(Ordering::Relaxed) || items.len() >= MAX_RESULTS {
            break;
        }
        if std::fs::metadata(path).map_or(true, |metadata| metadata.len() > MAX_SEARCHED_FILE_BYTES) {
            continue;
        }
        // Binary files fail here, because they are not UTF-8.
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        let label = relative_label(path, root);
        for (line, content) in text.lines().enumerate() {
            let Some(hit) = find_literal(content, query, false).into_iter().next() else {
                continue;
            };
            items.push(PickerItem {
                path: path.clone(),
                label: label.clone(),
                position: Some((line, hit.start)),
                detail: Some(content.trim().to_string()),
            });
            if items.len() >= MAX_RESULTS {
                break;
            }
        }
    }
    items
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuzzy_needs_the_letters_in_order() {
        assert!(fuzzy_score("mlb", "ml_basics.typ").is_some());
        assert!(fuzzy_score("bml", "ml_basics.typ").is_none());
    }

    #[test]
    fn fuzzy_prefers_runs_and_name_starts() {
        let files: Vec<PathBuf> = ["/p/extra/lang_notes.typ", "/p/logistic.typ", "/p/a_logistic_b.typ"]
            .iter()
            .map(PathBuf::from)
            .collect();
        let items = match_files(&files, Path::new("/p"), "log");
        assert_eq!(items[0].label, "logistic.typ");
    }

    #[test]
    fn empty_query_lists_all_files() {
        let files = vec![PathBuf::from("/p/a"), PathBuf::from("/p/b")];
        assert_eq!(match_files(&files, Path::new("/p"), "").len(), 2);
    }

    #[test]
    fn text_search_reports_line_column_and_smartcase() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("my notes.typ");
        std::fs::write(&path, "first\näh Needle here\n").unwrap();
        let cancel = AtomicBool::new(false);
        let files = vec![path];
        let items = search_text(&files, directory.path(), "needle", &cancel);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].label, "my notes.typ");
        assert_eq!(items[0].position, Some((1, 4)));
        assert_eq!(items[0].detail.as_deref(), Some("äh Needle here"));
        assert!(search_text(&files, directory.path(), "NEEDLE", &cancel).is_empty());
    }

    #[test]
    fn text_search_skips_binary_files_and_stops_when_cancelled() {
        let directory = tempfile::tempdir().unwrap();
        let binary = directory.path().join("a.pdf");
        std::fs::write(&binary, [0xff, 0xfe, b'x']).unwrap();
        let text = directory.path().join("b.txt");
        std::fs::write(&text, "x\n").unwrap();
        let files = vec![binary, text];
        assert_eq!(search_text(&files, directory.path(), "x", &AtomicBool::new(false)).len(), 1);
        assert!(search_text(&files, directory.path(), "x", &AtomicBool::new(true)).is_empty());
    }

    #[test]
    fn bib_entries_give_key_and_title() {
        let text = "@comment{skip,}\n@string{a = \"b\"}\n@article{one,\n  booktitle = {No},\n  TITLE = \"The {C}omprehensive\",\n}\n@book{two, year = 1}\n";
        assert_eq!(
            parse_bib(text),
            vec![("one".to_string(), Some("The Comprehensive".to_string())), ("two".to_string(), None)]
        );
    }

    #[test]
    fn citations_match_key_and_title() {
        let item = |label: &str, title: &str| PickerItem {
            path: PathBuf::new(),
            label: label.to_string(),
            position: None,
            detail: Some(title.to_string()),
        };
        let citations = [item("knuth", "Art"), item("greenwade", "Network")];
        assert_eq!(match_entries(&citations, "network")[0].label, "greenwade");
        assert_eq!(match_entries(&citations, "kn")[0].label, "knuth");
    }
}
