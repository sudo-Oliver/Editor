//! The citation picker's view of the bibliographies, on top of the
//! `citations` crate, which reads them.

use std::{collections::HashMap, sync::Arc};

use citations::index::{CitationIndex, IndexedEntry};
pub use citations::text::{clean, fold};

use crate::picker::PickerItem;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Entry {
    pub key: String,
    /// `Müller`, `Müller and Schmidt` or `Müller et al.`, as natbib writes them.
    pub authors: String,
    pub year: String,
    pub title: String,
    pub venue: String,
    /// Such as `article`, in lowercase.
    pub kind: String,
    pub doi: String,
    pub url: String,
    /// The entry as written, from `@` to its closing brace.
    pub raw: String,
}

/// The entries of one `.bib` file, with its own `@string`s resolved.
pub fn parse(text: &str) -> Vec<Entry> {
    let bib = citations::bib::parse(text);
    let mut macros: HashMap<String, String> = HashMap::new();
    let mut strings = bib.strings.iter().peekable();
    let mut entries = Vec::new();
    for entry in bib.entries.iter().filter(|entry| !entry.key.is_empty()) {
        while let Some(string) = strings.next_if(|string| string.range.start < entry.range.start) {
            let value = string.value.resolve(&macros, &mut Vec::new());
            macros.insert(string.name.clone(), value);
        }
        let fields: HashMap<&str, String> = entry.fields.iter().map(|field| (field.name.as_str(), field.value.resolve(&macros, &mut Vec::new()))).collect();
        let field = |names: &[&str]| names.iter().find_map(|name| fields.get(name).cloned()).unwrap_or_default();
        let year = match field(&["year"]) {
            year if year.is_empty() => field(&["date"]).chars().take(4).collect(),
            year => clean(&year),
        };
        entries.push(Entry {
            key: entry.key.clone(),
            authors: citations::text::authors(&field(&["author", "editor"])),
            year,
            title: clean(&field(&["title"])),
            venue: clean(&field(&["journal", "journaltitle", "booktitle", "publisher", "school", "institution"])),
            doi: field(&["doi"]).trim().to_string(),
            url: field(&["url"]).trim().to_string(),
            kind: entry.kind.clone(),
            raw: text[entry.range.clone()].to_string(),
        });
    }
    entries
}

/// One entry of the index as a picker row.
pub fn item(entry: &IndexedEntry) -> PickerItem {
    let summary = entry.summary();
    let detail = entry.detail();
    PickerItem {
        path: entry.path.to_path_buf(),
        label: entry.key.clone(),
        position: None,
        summary: (!summary.is_empty()).then_some(summary),
        detail: (!detail.is_empty()).then_some(detail),
    }
}

/// What the citation picker lists: every entry of the index, in its order,
/// with what add-ons offer and the labels Typst cites with `@`.
pub struct CitationRows {
    index: Arc<CitationIndex>,
    pub rows: Vec<PickerItem>,
    /// Where each key of the index is among the rows.
    positions: HashMap<String, usize>,
    /// Rows the index does not know, searched on their own.
    others: Vec<PickerItem>,
}

impl CitationRows {
    pub fn new(index: Arc<CitationIndex>, offered: Vec<(String, String)>, labels: Vec<PickerItem>) -> Self {
        let indexed: Vec<PickerItem> = index.search("", usize::MAX).into_iter().map(item).collect();
        let known = indexed.len();
        let mut rows = crate::picker::merge_citations(indexed, offered);
        rows.extend(labels);
        let positions = rows[..known].iter().enumerate().map(|(position, row)| (row.label.clone(), position)).collect();
        let others = rows[known..].to_vec();
        Self { index, rows, positions, others }
    }

    /// The rows that match `query`: the index's search first, then the rest.
    pub fn matches(&self, query: &str) -> Vec<PickerItem> {
        if query.trim().is_empty() {
            return self.rows.clone();
        }
        let mut found: Vec<PickerItem> =
            self.index.search(query, crate::picker::MAX_RESULTS).into_iter().filter_map(|entry| self.positions.get(&entry.key)).map(|position| self.rows[*position].clone()).collect();
        found.extend(crate::picker::match_entries(&self.others, query));
        found.truncate(crate::picker::MAX_RESULTS);
        found
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    const BIB: &str = r#"
@comment{ignored, author = {Nobody}}
@string{tug = "TUGBoat"}
@article{mueller2024b,
  author = {M{\"u}ller, Anna and Schmidt, Ben and Ott, Carl},
  title  = {GPUI Rendering
            Pipelines},
  year   = 2024,
  journal = tug # " Extra",
}
@inproceedings(mueller2024a,
  author = "Anna M{\"u}ller and Ben Schmidt and others",
  title = "Efficient {Rust} Architectures",
  date = {2024-03-01},
  booktitle = {Proc. of {RustConf}},
)
@book{rossum, author = {Guido {van Rossum}}, title = {Python}, year = {1995}}
@misc{beethoven, author = {Ludwig van Beethoven and {Barnes and Noble}}, title = {Op.~1 --- Trios}}
"#;

    #[test]
    fn entries_have_authors_years_titles_and_venues() {
        let entries = parse(BIB);
        let keys: Vec<&str> = entries.iter().map(|entry| entry.key.as_str()).collect();
        assert_eq!(keys, ["mueller2024b", "mueller2024a", "rossum", "beethoven"]);
        assert_eq!(entries[0].authors, "Müller et al.");
        assert_eq!(entries[0].title, "GPUI Rendering Pipelines");
        assert_eq!(entries[0].year, "2024");
        assert_eq!(entries[0].venue, "TUGBoat Extra");
        assert_eq!((entries[1].authors.as_str(), entries[1].year.as_str()), ("Müller et al.", "2024"));
        assert_eq!(entries[1].venue, "Proc. of RustConf");
        assert_eq!(entries[2].authors, "van Rossum");
        assert_eq!(entries[3].authors, "van Beethoven and Barnes and Noble");
        assert_eq!(entries[3].title, "Op. 1 — Trios");
        assert_eq!(entries[2].kind, "book");
        assert_eq!(entries[2].raw, "@book{rossum, author = {Guido {van Rossum}}, title = {Python}, year = {1995}}");
    }

    #[test]
    fn entries_keep_their_doi_and_url() {
        let entries = parse("@article{a, doi = {10.1000/x1}, url = \"https://example.org/a\"}\n@misc(b, title = {B})");
        assert_eq!((entries[0].doi.as_str(), entries[0].url.as_str()), ("10.1000/x1", "https://example.org/a"));
        assert_eq!(entries[1].raw, "@misc(b, title = {B})");
    }

    /// One entry without its closing brace must not take the rest of the file with it.
    #[test]
    fn an_unclosed_entry_loses_only_itself() {
        let text = "@article{first, title = {One}, year = 2001}\n@article{broken, title = {Never closed, year = 2002\n\n@book{after, title = {Two}, year = 2003}\n@misc{last, title = {Three}}\n";
        let keys: Vec<String> = parse(text).into_iter().map(|entry| entry.key).collect();
        assert_eq!(keys, ["first", "broken", "after", "last"]);
    }
}
