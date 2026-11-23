//! The search overlay for `Space f`, `Space g` and `Space tc`. Text search reads the files
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

use crate::{bib, nvim_themes::Themes, theme};

pub const MAX_RESULTS: usize = 200;
const MAX_SEARCHED_FILE_BYTES: u64 = 4 << 20;
/// Marks the picker item that creates a branch.
pub const CREATE_BRANCH: &str = "Create branch";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PickerKind {
    Files,
    Text,
    /// Images for a figure at the cursor.
    Figures,
    /// `.bib` entries for a citation at the cursor.
    Citations,
    /// Branches to switch to, or a new one by the name typed.
    Branches,
    /// Neovim colorschemes, shown while moving through them.
    Themes,
    /// Installed fonts, shown while moving through them.
    Fonts(FontSlot),
    /// Earlier versions of one file, from the local history.
    History,
    /// Every command, the editor's and those of add-ons that are on.
    Commands,
}

/// Which text a font picked in the settings is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FontSlot {
    Editor,
    Interface,
}

/// The label of the picker item that goes back to the font of the mode.
pub const DEFAULT_FONT: &str = "Default";

/// The installed fonts by family, after one that keeps the default.
pub fn font_items(names: Vec<String>) -> Vec<PickerItem> {
    let item = |label: String, detail: Option<&str>| PickerItem {
        path: PathBuf::new(),
        label,
        position: None,
        summary: None,
        detail: detail.map(str::to_string),
    };
    let mut names: Vec<String> = names.into_iter().filter(|name| !name.starts_with('.')).collect();
    names.sort_by_key(|name| name.to_lowercase());
    names.dedup();
    std::iter::once(item(DEFAULT_FONT.to_string(), Some("Of the mode")))
        .chain(names.into_iter().map(|name| item(name, None)))
        .collect()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PickerItem {
    pub path: PathBuf,
    /// Path relative to the project.
    pub label: String,
    /// Zero-based line and byte column of a text hit.
    pub position: Option<(usize, usize)>,
    /// A line above the detail, such as the authors and year of a citation.
    pub summary: Option<String>,
    pub detail: Option<String>,
}

pub struct Picker {
    pub kind: PickerKind,
    pub query: String,
    pub items: Vec<PickerItem>,
    pub selected: usize,
    pub root: PathBuf,
    pub files: Arc<Vec<PathBuf>>,
    /// The citations or branches to choose from.
    pub entries: Arc<Vec<PickerItem>>,
    /// Set to stop the running text search when the query changes.
    pub cancel: Arc<AtomicBool>,
    pub task: Option<Task<()>>,
    pub themes: Arc<Themes>,
    /// The palette to restore when the theme picker closes without a choice.
    pub original: Option<theme::Palette>,
    /// The citation picker's rows, searched through the citation index.
    pub citations: Option<Arc<crate::bib::CitationRows>>,
    /// Keys the citation at the cursor already has, which the list leaves out.
    pub cited: Vec<String>,
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
            Some((score, PickerItem { path: path.clone(), label, position: None, summary: None, detail: None }))
        })
        .collect();
    scored.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.label.cmp(&right.1.label)));
    scored.into_iter().take(MAX_RESULTS).map(|(_, item)| item).collect()
}

/// Adds what add-ons offer to the `.bib` entries. A key in both keeps its place
/// from the `.bib`, and the richer text of the add-on for searching.
pub fn merge_citations(mut items: Vec<PickerItem>, offered: Vec<(String, String)>) -> Vec<PickerItem> {
    for (key, detail) in offered {
        match items.iter_mut().find(|item| item.label == key) {
            Some(item) => item.detail = Some(detail),
            None => items.push(PickerItem { path: PathBuf::new(), label: key, position: None, summary: None, detail: Some(detail) }),
        }
    }
    items
}

/// The branches as picker items.
pub fn branch_items(branches: Vec<String>) -> Vec<PickerItem> {
    branches.into_iter().map(|label| PickerItem { path: PathBuf::new(), label, position: None, summary: None, detail: None }).collect()
}

/// Branches that match `query`, then the offer to create one by that name.
pub fn match_branches(branches: &[PickerItem], query: &str) -> Vec<PickerItem> {
    let query = query.trim();
    let mut items = match_entries(branches, query);
    if !query.is_empty() && !branches.iter().any(|item| item.label == query) {
        items.push(PickerItem {
            path: PathBuf::new(),
            label: query.to_string(),
            position: None,
            summary: None,
            detail: Some(CREATE_BRANCH.to_string()),
        });
    }
    items
}

/// The built-in themes first, then the Neovim ones by name.
pub fn theme_items(themes: &Themes) -> Vec<PickerItem> {
    let item = |label: &str, detail: Option<&str>| PickerItem {
        path: PathBuf::new(),
        label: label.to_string(),
        position: None,
        summary: None,
        detail: detail.map(str::to_string),
    };
    theme::BUILT_IN
        .iter()
        .map(|(name, _)| item(name, Some("Built-in")))
        .chain(themes.keys().map(|name| item(name, None)))
        .collect()
}

/// The labels of the documents, which `@` in Typst cites like `.bib` entries.
pub fn label_items(labels: Vec<String>) -> Vec<PickerItem> {
    labels
        .into_iter()
        .map(|label| PickerItem { path: PathBuf::new(), label, position: None, summary: None, detail: Some("Label".to_string()) })
        .collect()
}

/// Entries keep their order until the query asks for something.
pub fn match_in_order(entries: &[PickerItem], query: &str) -> Vec<PickerItem> {
    if query.trim().is_empty() { entries.to_vec() } else { match_entries(entries, query) }
}

/// Entries that match `query` in the label or detail, best first.
pub fn match_entries(entries: &[PickerItem], query: &str) -> Vec<PickerItem> {
    let mut scored: Vec<(i64, &PickerItem)> = entries
        .iter()
        .filter_map(|item| {
            let text = bib::fold(&format!(
                "{} {} {}",
                item.label,
                item.summary.as_deref().unwrap_or_default(),
                item.detail.as_deref().unwrap_or_default()
            ));
            Some((fuzzy_score(query, &text)?, item))
        })
        .collect();
    scored.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.label.cmp(&right.1.label)));
    scored.into_iter().take(MAX_RESULTS).map(|(_, item)| item.clone()).collect()
}

pub use citations::fuzzy::score as fuzzy_score;

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
                summary: None,
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
    fn offered_citations_join_the_bib_entries_without_doubles() {
        let bib = PickerItem { path: PathBuf::from("a.bib"), label: "a".to_string(), position: None, summary: None, detail: Some("Title".to_string()) };
        let merged = merge_citations(vec![bib], vec![("a".to_string(), "Ada 2024".to_string()), ("b".to_string(), "Bob".to_string())]);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].path, PathBuf::from("a.bib"));
        assert_eq!(merged[0].detail.as_deref(), Some("Ada 2024"));
        assert_eq!(merged[1].label, "b");
    }

    /// The first version, which collected the chars, as the reference.
    fn reference_score(query: &str, candidate: &str) -> Option<i64> {
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
        Some(score * 16 - chars.len() as i64)
    }

    #[test]
    fn fuzzy_scores_match_the_reference() {
        let candidates = ["", "a", "\\citep", "\\citep*", "src/lang_notes.typ", "Müller et al. 2024a", "ÄÖÜ äöü", "a_b-c.d/e f", "aaaa", "\\section*{x}"];
        let queries = ["", "a", "ab", "\\c", "\\cit", "cite", "MÜ", "ü", "aa", "aaaaa", "l n", "sec", "x}", "zz"];
        for candidate in candidates {
            for query in queries {
                assert_eq!(fuzzy_score(query, candidate), reference_score(query, candidate), "{query:?} in {candidate:?}");
            }
        }
    }

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
    fn citations_match_key_and_title() {
        let item = |label: &str, title: &str| PickerItem {
            path: PathBuf::new(),
            label: label.to_string(),
            position: None,
            summary: None,
            detail: Some(title.to_string()),
        };
        let citations = [item("knuth", "Art"), item("greenwade", "Network")];
        assert_eq!(match_entries(&citations, "network")[0].label, "greenwade");
        assert_eq!(match_entries(&citations, "kn")[0].label, "knuth");
    }

    #[test]
    fn themes_list_the_built_in_one_first() {
        let palette = theme::Palette::OVERLEAF_DARK;
        let themes = Themes::from([("vesper".to_string(), palette), ("koda".to_string(), palette)]);
        let items = theme_items(&themes);
        let labels: Vec<String> = match_in_order(&items, "").into_iter().map(|item| item.label).collect();
        assert_eq!(labels, ["overleaf-dark", "overleaf-light", "paper", "koda", "vesper"]);
        assert_eq!(match_in_order(&items, "ves")[0].label, "vesper");
    }

    #[test]
    fn branches_offer_creation_only_for_new_names() {
        let branches = branch_items(vec!["main".to_string(), "chapter".to_string()]);
        assert_eq!(match_branches(&branches, "").len(), 2);
        let exact = match_branches(&branches, "main");
        assert_eq!((exact.len(), exact[0].detail.as_deref()), (1, None));
        let new = match_branches(&branches, "ma");
        assert_eq!(new[0].label, "main");
        assert_eq!((new[1].label.as_str(), new[1].detail.as_deref()), ("ma", Some(CREATE_BRANCH)));
    }
}
