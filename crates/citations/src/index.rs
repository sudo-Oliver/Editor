//! Everything about one document's citations and references, built off the
//! main thread and then only read: every entry with its text folded for
//! search once, every label, and what is wrong across files. A [`Cache`]
//! keeps each parsed file until it changes, so a new index after an edit
//! reads only the edited file again.

use std::{
    collections::{HashMap, HashSet},
    hash::{Hash, Hasher},
    ops::Range,
    path::{Path, PathBuf},
    sync::Arc,
    time::SystemTime,
};

use crate::{
    Diagnostic, Severity, aux, lint,
    bib::{self, Bib, Part},
    fuzzy, hayagriva,
    project::{self, Backend, Env, Language, Project},
    scan::{Key, Label, Scan},
    tex,
    text::{authors, clean, fold, fold_into, full_authors},
    typst,
};

/// What a build needs to know about the editor.
pub struct Inputs<'a> {
    pub root: &'a Path,
    /// The main file the build would compile, if there is one.
    pub main: Option<PathBuf>,
    /// The file being edited, which counts even before anything includes it.
    pub active: Option<PathBuf>,
    /// The text of open documents, which wins over the disk.
    pub open: &'a HashMap<PathBuf, Arc<str>>,
    /// Every file of the project, for `.bib` files when no source names one.
    /// Walked only when needed.
    pub tree: &'a dyn Fn() -> Vec<PathBuf>,
    /// Finds a bibliography of the TeX installation, such as `IEEEabrv.bib`.
    pub system_bib: &'a dyn Fn(&str) -> Option<PathBuf>,
}

/// Parsed files, kept until their text changes.
#[derive(Default)]
pub struct Cache {
    files: HashMap<PathBuf, Cached>,
    generation: u64,
    /// The entries of the last build, which a build reuses while what they
    /// come from is unchanged, as when only a chapter was edited.
    entries: Option<(EntriesKey, Arc<Entries>)>,
}

/// What the entries come from: the bibliographies as they are now, the
/// `\bibitem`s, the program that reads them, and the last build's letters.
#[derive(Clone, PartialEq, Eq)]
struct EntriesKey {
    files: Vec<(PathBuf, Option<Stamp>)>,
    bibitems: Vec<(PathBuf, String, Range<usize>)>,
    backend: Backend,
    biblatex: bool,
    bbl: Option<Stamp>,
}

struct Cached {
    stamp: Stamp,
    parsed: Parsed,
    used: u64,
}

#[derive(Clone, PartialEq, Eq)]
enum Stamp {
    Open(u64),
    Disk(Option<SystemTime>, u64),
}

#[derive(Clone)]
enum Parsed {
    Scan(Arc<Scan>),
    Bib(Arc<BibFile>),
    Text(Arc<str>),
}

pub struct BibFile {
    pub bib: Bib,
    /// Not UTF-8, so read as Windows-1252, as an old editor saved it.
    pub latin1: bool,
}

#[derive(Clone, Copy)]
enum Kind {
    Scan(Language),
    Bib,
    Text,
}

/// The text of a file on the disk, as UTF-8 or else as Windows-1252.
pub fn decode(bytes: Vec<u8>) -> (String, bool) {
    match String::from_utf8(bytes) {
        Ok(text) => (text, false),
        Err(error) => (encoding_rs::WINDOWS_1252.decode(error.as_bytes()).0.into_owned(), true),
    }
}

/// What tells one version of a file from the next: the hash of an open
/// document's text, or the time and size on the disk.
fn stamp(path: &Path, open: &HashMap<PathBuf, Arc<str>>) -> Option<(Stamp, Option<Arc<str>>)> {
    match open.get(path) {
        Some(text) => {
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            text.hash(&mut hasher);
            Some((Stamp::Open(hasher.finish()), Some(text.clone())))
        }
        None => {
            let metadata = std::fs::metadata(path).ok().filter(std::fs::Metadata::is_file)?;
            Some((Stamp::Disk(metadata.modified().ok(), metadata.len()), None))
        }
    }
}

impl Cache {
    /// Keeps a parsed file that a reused part of the index still stands on.
    fn touch(&mut self, path: &Path) {
        if let Some(cached) = self.files.get_mut(path) {
            cached.used = self.generation;
        }
    }

    fn get(&mut self, path: &Path, open: &HashMap<PathBuf, Arc<str>>, kind: Kind) -> Option<Parsed> {
        let (stamp, text) = stamp(path, open)?;
        let generation = self.generation;
        if let Some(cached) = self.files.get_mut(path)
            && cached.stamp == stamp
        {
            cached.used = generation;
            return Some(cached.parsed.clone());
        }
        let (text, latin1) = match text {
            Some(text) => (text.to_string(), false),
            None => decode(std::fs::read(path).ok()?),
        };
        let parsed = match kind {
            Kind::Scan(Language::Latex) => Parsed::Scan(Arc::new(tex::scan(&text))),
            Kind::Scan(Language::Typst) => Parsed::Scan(Arc::new(typst::scan(&text))),
            Kind::Bib => {
                let yaml = path.extension().is_some_and(|extension| extension == "yml" || extension == "yaml");
                Parsed::Bib(Arc::new(BibFile { bib: if yaml { hayagriva::parse(&text) } else { bib::parse(&text) }, latin1 }))
            }
            Kind::Text => Parsed::Text(text.into()),
        };
        self.files.insert(path.to_path_buf(), Cached { stamp, parsed: parsed.clone(), used: generation });
        Some(parsed)
    }

    fn scan(&mut self, path: &Path, open: &HashMap<PathBuf, Arc<str>>, language: Language) -> Option<Arc<Scan>> {
        match self.get(path, open, Kind::Scan(language))? {
            Parsed::Scan(scan) => Some(scan),
            _ => None,
        }
    }

    fn bib(&mut self, path: &Path, open: &HashMap<PathBuf, Arc<str>>) -> Option<Arc<BibFile>> {
        match self.get(path, open, Kind::Bib)? {
            Parsed::Bib(bib) => Some(bib),
            _ => None,
        }
    }

    fn text(&mut self, path: &Path, open: &HashMap<PathBuf, Arc<str>>) -> Option<Arc<str>> {
        match self.get(path, open, Kind::Text)? {
            Parsed::Text(text) => Some(text),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct IndexedEntry {
    pub key: String,
    /// Such as `article`; `bibitem` for one that `\bibitem` defines.
    pub kind: String,
    pub path: Arc<PathBuf>,
    pub key_range: Range<usize>,
    pub range: Range<usize>,
    /// The line of the entry in its file, from 0.
    pub line: usize,
    /// `Müller et al.`, as natbib writes them.
    pub authors: String,
    pub full_authors: String,
    pub year: String,
    /// The letter of `2024a`, from the last build or worked out.
    pub letter: Option<char>,
    pub title: String,
    pub venue: String,
    pub doi: String,
    pub url: String,
    pub abstract_text: String,
    /// Every field as written, with `@string`s resolved and crossref's inherited.
    pub fields: Vec<(String, String)>,
}

/// Where an entry's key, authors, year, title and venue are in the search text. Apart from the entries, so that a search
/// walks one small table.
#[derive(Clone, Default)]
struct Spans {
    fields: [Range<u32>; 5],
    /// Which letters and digits each field has, to pass over most fields
    /// without reading them.
    masks: [u64; 5],
}

/// A bit for each ASCII letter and digit in `text`.
fn mask(text: &[u8]) -> u64 {
    text.iter().fold(0, |mask, byte| match byte {
        b'a'..=b'z' => mask | 1 << (byte - b'a'),
        b'0'..=b'9' => mask | 1 << (26 + byte - b'0'),
        _ => mask,
    })
}

impl IndexedEntry {
    pub fn field(&self, name: &str) -> Option<&str> {
        self.fields.iter().find(|(field, _)| field == name).map(|(_, value)| value.as_str())
    }

    /// `Müller et al. 2024a`.
    pub fn summary(&self) -> String {
        let mut summary = self.authors.clone();
        if !self.year.is_empty() {
            if !summary.is_empty() {
                summary.push(' ');
            }
            summary.push_str(&self.year);
            summary.extend(self.letter);
        }
        summary
    }

    /// `Title. Venue`.
    pub fn detail(&self) -> String {
        match (self.title.is_empty(), self.venue.is_empty()) {
            (false, false) => format!("{}. {}", self.title.trim_end_matches('.'), self.venue),
            (false, true) => self.title.clone(),
            _ => self.venue.clone(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct LabelDef {
    pub path: Arc<PathBuf>,
    pub label: Label,
    /// The number and page of the last build.
    pub number: Option<String>,
    pub page: Option<String>,
}

/// The entries of the bibliographies with what is wrong in them, ready to
/// search. Shared between builds while the bibliographies stay the same.
#[derive(Default)]
pub struct Entries {
    pub entries: Vec<IndexedEntry>,
    /// Entries by author, year and letter, as the picker lists them unfiltered.
    order: Vec<u32>,
    arena: String,
    spans: Vec<Spans>,
    by_key: HashMap<String, Vec<u32>>,
    by_folded_key: HashMap<String, Vec<u32>>,
    /// The bibliographies read, in order, with whether each is Windows-1252.
    pub bib_files: Vec<(Arc<PathBuf>, bool)>,
    /// What is wrong in each bibliography.
    pub diagnostics: HashMap<PathBuf, Vec<Diagnostic>>,
}

#[derive(Default)]
pub struct CitationIndex {
    pub project: Project,
    /// The entries, which reading `index.entries` and searching reach directly.
    bib: Arc<Entries>,
    pub labels: HashMap<String, Vec<LabelDef>>,
    /// Labels of other documents through `\externaldocument`.
    pub external_labels: HashSet<String>,
    /// An external document has no `.aux` yet, so its labels are unknown.
    pub external_unknown: bool,
    /// What is wrong in the sources, such as a bibliography that is not there.
    source_diagnostics: HashMap<PathBuf, Vec<Diagnostic>>,
    /// Every entry counts as cited, through `\nocite{*}`.
    pub cite_all: bool,
    /// Every declared bibliography was read, so a key that is not here is wrong.
    pub bibs_complete: bool,
}

impl std::ops::Deref for CitationIndex {
    type Target = Entries;

    fn deref(&self) -> &Entries {
        &self.bib
    }
}

/// Field weights: a hit in the key counts most, in the venue least.
const WEIGHTS: [i64; 5] = [6, 4, 3, 2, 1];

/// Fields that only say where to find more of the entry, and are not inherited.
const NOT_INHERITED: &[&str] = &["crossref", "xdata", "ids", "entryset", "related", "execute"];

pub fn build(cache: &mut Cache, inputs: &Inputs) -> CitationIndex {
    cache.generation += 1;
    let open = inputs.open;
    let walked = std::cell::OnceCell::new();
    let tree = || walked.get_or_init(|| (inputs.tree)()).as_slice();
    // `% !TEX root` or a subfile's main file decides, then the build's choice.
    let main = inputs
        .active
        .as_ref()
        .and_then(|active| {
            let scan = cache.scan(active, open, Language::of(active)?)?;
            let root = scan.root.as_ref()?;
            let path = project::normalize(&active.parent()?.join(root));
            (open.contains_key(&path) || path.is_file()).then_some(path)
        })
        .or_else(|| inputs.main.clone());
    let extra: Vec<PathBuf> = match &main {
        Some(_) => inputs.active.iter().cloned().collect(),
        // Without a main file, every source counts, so that no label is missed.
        None => tree().iter().filter(|path| Language::of(path).is_some()).cloned().chain(inputs.active.iter().cloned()).collect(),
    };
    let exists = |path: &Path| open.contains_key(path) || path.is_file();
    let project = {
        let mut scan = |path: &Path, language: Language| cache.scan(path, open, language);
        let mut env = Env { root: inputs.root, scan: &mut scan, exists: &exists, system_bib: inputs.system_bib };
        project::build(main.as_deref(), &extra, &mut env)
    };
    let mut index = CitationIndex {
        cite_all: project.sources.iter().any(|source| source.scan.cite_all),
        bibs_complete: project.bibliographies.iter().all(|bibliography| bibliography.path.is_some()),
        ..CitationIndex::default()
    };
    let mut bib = Entries::default();

    // The bibliographies, in the order BibTeX reads them, since an
    // `@string` of one is known in the ones after it.
    let mut bib_paths: Vec<PathBuf> = Vec::new();
    if project.every_bib {
        let mut found: Vec<PathBuf> = tree().iter().filter(|path| path.extension().is_some_and(|extension| extension == "bib")).cloned().collect();
        found.sort();
        bib_paths = found;
    } else {
        for bibliography in &project.bibliographies {
            match &bibliography.path {
                Some(path) if !bib_paths.contains(path) => bib_paths.push(path.clone()),
                Some(_) => {}
                None if bibliography.elsewhere => {}
                None => index.source_diagnostics.entry(bibliography.declared_in.clone()).or_default().push(Diagnostic {
                    range: bibliography.range.clone(),
                    severity: Severity::Error,
                    message: format!("There is no bibliography {}.", bibliography.name),
                }),
            }
        }
    }
    let key = EntriesKey {
        files: bib_paths.iter().map(|path| (path.clone(), stamp(path, open).map(|(stamp, _)| stamp))).collect(),
        bibitems: project.sources.iter().flat_map(|source| source.scan.bibitems.iter().map(|item| (source.path.clone(), item.name.clone(), item.range.clone()))).collect(),
        backend: project.backend,
        biblatex: project.biblatex,
        bbl: project.main.as_deref().and_then(bbl_path).and_then(|path| stamp(&path, open)).map(|(stamp, _)| stamp),
    };
    if let Some((known, entries)) = &cache.entries
        && *known == key
    {
        index.bib = entries.clone();
        for path in bib_paths.iter().cloned().chain(project.main.as_deref().and_then(bbl_path)) {
            cache.touch(&path);
        }
        labels(&mut index, &project, cache, open);
        index.project = project;
        cache.files.retain(|_, cached| cached.used == cache.generation);
        return index;
    }
    let mut macros: HashMap<String, String> = HashMap::new();
    for path in &bib_paths {
        let Some(file) = cache.bib(path, open) else {
            continue;
        };
        let shared = Arc::new(path.clone());
        bib.bib_files.push((shared.clone(), file.latin1));
        let diagnostics = bib.diagnostics.entry(path.clone()).or_default();
        diagnostics.extend(file.bib.diagnostics.iter().cloned());
        if file.latin1 && project.backend == Backend::Biber {
            diagnostics.push(Diagnostic {
                range: 0..0,
                severity: Severity::Warning,
                message: "This file is not UTF-8, which biber expects, so its accents come out wrong. Save it as UTF-8.".into(),
            });
        }
        let mut strings = file.bib.strings.iter().peekable();
        for entry in &file.bib.entries {
            while let Some(string) = strings.next_if(|string| string.range.start < entry.range.start) {
                define(string, &mut macros, diagnostics);
            }
            lint::entry_text(entry, project.backend, project.biblatex, diagnostics);
            let mut fields = Vec::with_capacity(entry.fields.len());
            for field in &entry.fields {
                let mut unknown = Vec::new();
                let value = field.value.resolve(&macros, &mut unknown);
                unknown_macros(&unknown, diagnostics);
                fields.push((field.name.clone(), value));
            }
            bib.entries.push(IndexedEntry {
                key: entry.key.clone(),
                kind: entry.kind.clone(),
                path: shared.clone(),
                key_range: entry.key_range.clone(),
                range: entry.range.clone(),
                line: entry.line,
                fields,
                ..IndexedEntry::default()
            });
        }
        for string in strings {
            define(string, &mut macros, diagnostics);
        }
    }
    for source in &project.sources {
        let shared = Arc::new(source.path.clone());
        for item in &source.scan.bibitems {
            bib.entries.push(IndexedEntry { key: item.name.clone(), kind: "bibitem".into(), path: shared.clone(), key_range: item.range.clone(), range: item.range.clone(), ..IndexedEntry::default() });
        }
    }
    bib.entries.retain(|entry| !entry.key.is_empty());
    for (position, entry) in bib.entries.iter().enumerate() {
        bib.by_key.entry(entry.key.clone()).or_default().push(position as u32);
        bib.by_folded_key.entry(entry.key.to_lowercase()).or_default().push(position as u32);
    }
    duplicate_keys(&mut bib);
    inherit(&mut bib);
    describe(&mut bib);
    for (path, diagnostic) in lint::entries(&bib, project.backend, project.biblatex, &place) {
        bib.diagnostics.entry(path).or_default().push(diagnostic);
    }
    letters(&mut bib, cache, open, project.main.as_deref());
    search_text(&mut bib);
    let bib = Arc::new(bib);
    cache.entries = Some((key, bib.clone()));
    index.bib = bib;
    labels(&mut index, &project, cache, open);
    index.project = project;
    cache.files.retain(|_, cached| cached.used == cache.generation);
    index
}

fn define(string: &bib::StringDef, macros: &mut HashMap<String, String>, diagnostics: &mut Vec<Diagnostic>) {
    let mut unknown = Vec::new();
    let value = string.value.resolve(macros, &mut unknown);
    unknown_macros(&unknown, diagnostics);
    macros.insert(string.name.clone(), value);
}

fn unknown_macros(unknown: &[&Part], diagnostics: &mut Vec<Diagnostic>) {
    for part in unknown {
        if let Part::Macro { name, range } = part {
            diagnostics.push(Diagnostic { range: range.clone(), severity: Severity::Warning, message: format!("No @string defines {name}, so its text is missing.") });
        }
    }
}

/// `refs.bib:12`, for messages that point to another entry.
fn place(entry: &IndexedEntry) -> String {
    format!("{}:{}", entry.path.file_name().map_or_else(String::new, |name| name.to_string_lossy().into_owned()), entry.line + 1)
}

fn duplicate_keys(index: &mut Entries) {
    let mut found: Vec<(PathBuf, Diagnostic)> = Vec::new();
    for positions in index.by_folded_key.values().filter(|positions| positions.len() > 1) {
        let first = &index.entries[positions[0] as usize];
        for position in &positions[1..] {
            let entry = &index.entries[*position as usize];
            let (severity, message) = if entry.key == first.key {
                (Severity::Error, format!("The key {} is already defined at {}.", entry.key, place(first)))
            } else {
                (Severity::Warning, format!("The key {} differs from {} at {} only in case, which BibTeX takes for the same key.", entry.key, first.key, place(first)))
            };
            found.push((entry.path.to_path_buf(), Diagnostic { range: entry.key_range.clone(), severity, message }));
        }
    }
    for (path, diagnostic) in found {
        index.diagnostics.entry(path).or_default().push(diagnostic);
    }
}

/// Fields from `crossref` and `xdata` parents that an entry does not set itself.
fn inherit(index: &mut Entries) {
    let mut additions: Vec<(usize, Vec<(String, String)>)> = Vec::new();
    let mut found: Vec<(PathBuf, Diagnostic)> = Vec::new();
    for (position, entry) in index.entries.iter().enumerate() {
        let parents: Vec<String> = ["crossref", "xdata"]
            .iter()
            .filter_map(|name| entry.field(name))
            .flat_map(|value| value.split(','))
            .map(|key| key.trim().to_string())
            .filter(|key| !key.is_empty())
            .collect();
        if parents.is_empty() {
            continue;
        }
        let mut inherited: Vec<(String, String)> = Vec::new();
        let mut visited = vec![position];
        let mut queue = parents;
        while let Some(key) = queue.pop() {
            let Some(&parent) = index.by_folded_key.get(&key.to_lowercase()).and_then(|positions| positions.first()) else {
                found.push((entry.path.to_path_buf(), Diagnostic { range: entry.key_range.clone(), severity: Severity::Warning, message: format!("{} inherits from {key}, which no bibliography has.", entry.key) }));
                continue;
            };
            let parent = parent as usize;
            if visited.contains(&parent) {
                found.push((entry.path.to_path_buf(), Diagnostic { range: entry.key_range.clone(), severity: Severity::Error, message: format!("{} inherits from {key}, which leads back to it.", entry.key) }));
                continue;
            }
            visited.push(parent);
            let source = &index.entries[parent];
            for (name, value) in &source.fields {
                if NOT_INHERITED.contains(&name.as_str()) {
                    if name == "crossref" || name == "xdata" {
                        queue.extend(value.split(',').map(|key| key.trim().to_string()).filter(|key| !key.is_empty()));
                    }
                    continue;
                }
                // A book's title is the booktitle of what is in it.
                let name = if name == "title" && matches!(source.kind.as_str(), "proceedings" | "book" | "collection" | "mvbook" | "mvproceedings" | "mvcollection" | "periodical") {
                    if source.kind == "periodical" { "journaltitle".to_string() } else { "booktitle".to_string() }
                } else {
                    name.clone()
                };
                if entry.field(&name).is_none() && !inherited.iter().any(|(other, _)| *other == name) {
                    inherited.push((name, value.clone()));
                }
            }
        }
        if !inherited.is_empty() {
            additions.push((position, inherited));
        }
    }
    for (position, inherited) in additions {
        index.entries[position].fields.extend(inherited);
    }
    for (path, diagnostic) in found {
        index.diagnostics.entry(path).or_default().push(diagnostic);
    }
}

fn describe(index: &mut Entries) {
    for entry in &mut index.entries {
        let field = |names: &[&str]| names.iter().find_map(|name| entry.field(name)).unwrap_or_default().to_string();
        let people = field(&["author", "editor"]);
        let year = match field(&["year"]) {
            year if year.is_empty() => field(&["date"]).chars().take(4).collect(),
            year => clean(&year),
        };
        let title = clean(&field(&["title"]));
        let venue = clean(&field(&["journal", "journaltitle", "booktitle", "publisher", "school", "institution", "organization"]));
        let doi = field(&["doi"]).trim().to_string();
        let url = field(&["url"]).trim().to_string();
        let abstract_text = clean(&field(&["abstract"])).chars().take(600).collect();
        (entry.authors, entry.full_authors) = (authors(&people), full_authors(&people));
        (entry.year, entry.title, entry.venue, entry.doi, entry.url, entry.abstract_text) = (year, title, venue, doi, url, abstract_text);
    }
}

/// Letters for entries that share authors and year: those of the last build,
/// or else in the order of their keys, as plainnat gives them.
fn letters(index: &mut Entries, cache: &mut Cache, open: &HashMap<PathBuf, Arc<str>>, main: Option<&Path>) {
    let mut groups: HashMap<(String, String), Vec<usize>> = HashMap::new();
    for (position, entry) in index.entries.iter().enumerate().filter(|(_, entry)| !entry.year.is_empty()) {
        groups.entry((fold(&entry.authors), entry.year.clone())).or_default().push(position);
    }
    for group in groups.values_mut().filter(|group| group.len() > 1) {
        group.sort_by(|left, right| index.entries[*left].key.cmp(&index.entries[*right].key));
        for (letter, position) in ('a'..='z').zip(group.iter()) {
            index.entries[*position].letter = Some(letter);
        }
    }
    let Some(bbl) = main.and_then(bbl_path).and_then(|path| cache.text(&path, open)) else {
        return;
    };
    for (key, letter) in bbl_letters(&bbl) {
        for position in index.by_key.get(&key).into_iter().flatten() {
            index.entries[*position as usize].letter = letter;
        }
    }
}

/// Where a LaTeX build leaves the bibliography of `main`.
pub fn bbl_path(main: &Path) -> Option<PathBuf> {
    Some(main.parent()?.join(".build").join(format!("{}.bbl", main.file_stem()?.to_string_lossy())))
}

/// Where a LaTeX build leaves the `.aux` of `main`.
pub fn aux_path(main: &Path) -> Option<PathBuf> {
    Some(main.parent()?.join(".build").join(format!("{}.aux", main.file_stem()?.to_string_lossy())))
}

/// The letters of the last build. natbib writes `\bibitem[Müller(2024{\natexlab{a}})]{key}`
/// or `\bibitem[{Müller(2024a)}...]{key}`,
/// biblatex `\entry{key}` with `\field{extradate}{1}`. A cited entry without
/// a letter has none in the PDF either.
pub fn bbl_letters(bbl: &str) -> HashMap<String, Option<char>> {
    let mut letters = HashMap::new();
    for chunk in bbl.split("\\bibitem[").skip(1) {
        let Some(close) = chunk.find("]{") else {
            continue;
        };
        let label = &chunk[..close];
        let Some(key) = chunk[close + 2..].split('}').next() else {
            continue;
        };
        // Only author-year labels carry a year in parentheses.
        let year = label.match_indices('(').find_map(|(index, _)| {
            let rest = &label[index + 1..];
            let digits = rest.get(..4).filter(|year| year.bytes().all(|byte| byte.is_ascii_digit()))?;
            let after = &rest[digits.len()..];
            after.strip_prefix("{\\natexlab{").unwrap_or(after).bytes().next()
        });
        if let Some(after) = year {
            letters.insert(key.trim().to_string(), after.is_ascii_lowercase().then_some(after as char));
        }
    }
    for chunk in bbl.split("\\entry{").skip(1) {
        let Some(key) = chunk.split('}').next() else {
            continue;
        };
        let body = chunk.split("\\endentry").next().unwrap_or_default();
        let extra = body
            .split("\\field{extradate}{")
            .nth(1)
            .and_then(|rest| rest.split('}').next())
            .and_then(|number| number.trim().parse::<u8>().ok())
            .filter(|number| (1..=26).contains(number))
            .map(|number| (b'a' + number - 1) as char);
        letters.insert(key.trim().to_string(), extra);
    }
    letters
}

/// Folds key, authors, year, title and venue of every entry into one text,
/// once, and sorts the entries as the picker lists them.
fn search_text(index: &mut Entries) {
    let mut arena = String::new();
    let mut spans = Vec::with_capacity(index.entries.len());
    for entry in &index.entries {
        let mut span = Spans::default();
        let year = format!("{}{}", entry.year, entry.letter.map_or(String::new(), String::from));
        let parts = [entry.key.as_str(), entry.full_authors.as_str(), year.as_str(), entry.title.as_str(), entry.venue.as_str()];
        for (slot, part) in parts.iter().enumerate() {
            let start = arena.len() as u32;
            fold_into(part, &mut arena);
            span.fields[slot] = start..arena.len() as u32;
            span.masks[slot] = mask(&arena.as_bytes()[start as usize..]);
            arena.push(' ');
        }
        spans.push(span);
    }
    index.arena = arena;
    index.spans = spans;
    let sort_keys: Vec<String> = index.entries.iter().map(|entry| fold(&format!("{} {} {}", entry.authors, entry.year, entry.letter.unwrap_or(' ')))).collect();
    let mut order: Vec<u32> = (0..index.entries.len() as u32).collect();
    order.sort_by(|left, right| sort_keys[*left as usize].cmp(&sort_keys[*right as usize]).then_with(|| index.entries[*left as usize].key.cmp(&index.entries[*right as usize].key)));
    index.order = order;
}

fn labels(index: &mut CitationIndex, project: &Project, cache: &mut Cache, open: &HashMap<PathBuf, Arc<str>>) {
    // Numbers from the last build, following `\@input` into chapters.
    let mut numbers: HashMap<String, (String, String)> = HashMap::new();
    if let Some(main) = &project.main
        && project.backend != Backend::Typst
    {
        let mut pending: Vec<PathBuf> = aux_path(main).into_iter().collect();
        let mut seen = HashSet::new();
        while let Some(path) = pending.pop() {
            if !seen.insert(path.clone()) {
                continue;
            }
            let Some(text) = cache.text(&path, open) else {
                continue;
            };
            let parsed = aux::parse(&text);
            let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
            pending.extend(parsed.inputs.iter().map(|input| dir.join(input)));
            for (name, number, page) in parsed.labels {
                numbers.entry(name).or_insert((number, page));
            }
        }
    }
    for (aux, prefix) in &project.external {
        let build_aux = aux.parent().map(|dir| dir.join(".build").join(aux.file_name().unwrap_or_default()));
        match cache.text(aux, open).or_else(|| build_aux.and_then(|path| cache.text(&path, open))) {
            Some(text) => index.external_labels.extend(aux::parse(&text).labels.into_iter().map(|(name, ..)| format!("{prefix}{name}"))),
            None => index.external_unknown = true,
        }
    }
    for source in &project.sources {
        let shared = Arc::new(source.path.clone());
        for label in &source.scan.labels {
            let number = numbers.get(&label.name);
            index.labels.entry(label.name.clone()).or_default().push(LabelDef {
                path: shared.clone(),
                label: label.clone(),
                number: number.map(|(number, _)| number.clone()),
                page: number.map(|(_, page)| page.clone()),
            });
        }
    }
}

impl Entries {
    /// The entries with exactly this key.
    pub fn entries_with(&self, key: &str) -> impl Iterator<Item = &IndexedEntry> {
        self.by_key.get(key).into_iter().flatten().map(|position| &self.entries[*position as usize])
    }

    pub fn entry(&self, key: &str) -> Option<&IndexedEntry> {
        self.entries_with(key).next()
    }

    /// An entry whose key differs only in case, for a hint.
    pub fn entry_ignoring_case(&self, key: &str) -> Option<&IndexedEntry> {
        self.by_folded_key.get(&key.to_lowercase())?.first().map(|position| &self.entries[*position as usize])
    }

}

impl CitationIndex {
    pub fn label(&self, name: &str) -> &[LabelDef] {
        self.labels.get(name).map_or(&[], Vec::as_slice)
    }

    /// Whether `\ref{name}` finds a label, here or in an external document.
    pub fn knows_label(&self, name: &str) -> bool {
        self.labels.contains_key(name) || self.external_labels.contains(name)
    }

    /// What is wrong in one file: in a bibliography, or in a source.
    pub fn diagnostics(&self, path: &Path) -> Vec<Diagnostic> {
        let bib = self.bib.diagnostics.get(path).into_iter().flatten();
        bib.chain(self.source_diagnostics.get(path).into_iter().flatten()).cloned().collect()
    }

    /// Every source file of the document.
    pub fn sources(&self) -> impl Iterator<Item = (&Path, &Scan)> {
        self.project.sources.iter().map(|source| (source.path.as_path(), source.scan.as_ref()))
    }

    /// The scan of one source, if it belongs to the document.
    pub fn scan_of(&self, path: &Path) -> Option<&Scan> {
        self.project.sources.iter().find(|source| source.path == path).map(|source| source.scan.as_ref())
    }

}

impl Entries {
    /// Entries that match every word of `query` in key, authors, year, title
    /// or venue, best first. An empty query lists them by author and year.
    /// One pass per entry over text folded beforehand; nothing is allocated
    /// per entry.
    pub fn search(&self, query: &str, limit: usize) -> Vec<&IndexedEntry> {
        let tokens: Vec<(String, u64)> = query.split_whitespace().map(fold).map(|token| {
            let bits = mask(token.as_bytes());
            (token, bits)
        }).collect();
        if tokens.is_empty() {
            return self.order.iter().take(limit).map(|position| &self.entries[*position as usize]).collect();
        }
        let mut scored: Vec<(i64, u32)> = Vec::new();
        'entries: for (position, span) in self.spans.iter().enumerate() {
            let mut total = 0;
            for (token, bits) in &tokens {
                let best = (0..5)
                    .filter(|slot| span.masks[*slot] & bits == *bits)
                    .filter_map(|slot| {
                        let range = &span.fields[slot];
                        field_score(token, &self.arena[range.start as usize..range.end as usize]).map(|score| score * WEIGHTS[slot])
                    })
                    .max();
                // Letters scattered over several fields are no match.
                let Some(best) = best else {
                    continue 'entries;
                };
                total += best;
            }
            scored.push((total, position as u32));
        }
        let by_score = |left: &(i64, u32), right: &(i64, u32)| right.0.cmp(&left.0).then_with(|| self.entries[left.1 as usize].key.cmp(&self.entries[right.1 as usize].key));
        if scored.len() > limit {
            scored.select_nth_unstable_by(limit, by_score);
            scored.truncate(limit);
        }
        scored.sort_unstable_by(by_score);
        scored.into_iter().map(|(_, position)| &self.entries[position as usize]).collect()
    }
}

/// Whether the bytes of `wanted` appear in `text` in order: the quick test
/// before scoring, which most entries fail.
fn in_order(wanted: &[u8], text: &[u8]) -> bool {
    let mut rest = text;
    for byte in wanted {
        match rest.iter().position(|candidate| candidate == byte) {
            Some(at) => rest = &rest[at + 1..],
            None => return false,
        }
    }
    true
}

/// How well a folded field holds a folded token: whole and at a word start
/// best, then anywhere, then as letters in order.
fn field_score(token: &str, field: &str) -> Option<i64> {
    if let Some(at) = field.find(token) {
        let word_start = at == 0 || !field.as_bytes()[at - 1].is_ascii_alphanumeric();
        let whole = field.len() == token.len();
        return Some(1000 + if word_start { 500 } else { 0 } + if whole { 300 } else { 0 } - at.min(200) as i64);
    }
    if !in_order(token.as_bytes(), field.as_bytes()) {
        return None;
    }
    fuzzy::score(token, field).map(|score| score.max(1))
}

/// Keys with the command they appear in: what `cites` and `either` of every
/// source hold, to check against the index.
pub fn uses(scan: &Scan) -> impl Iterator<Item = &Key> {
    scan.cites.iter().chain(scan.either.iter())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Project {
        dir: tempfile::TempDir,
    }

    impl Project {
        fn new(files: &[(&str, &str)]) -> Self {
            let dir = tempfile::tempdir().unwrap();
            for (name, text) in files {
                let path = dir.path().join(name);
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(path, text).unwrap();
            }
            Self { dir }
        }

        fn path(&self, name: &str) -> PathBuf {
            self.dir.path().join(name)
        }

        fn tree(&self) -> Vec<PathBuf> {
            fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
                for entry in std::fs::read_dir(dir).unwrap().flatten() {
                    let path = entry.path();
                    if path.is_dir() { walk(&path, out) } else { out.push(path) }
                }
            }
            let mut out = Vec::new();
            walk(self.dir.path(), &mut out);
            out
        }

        fn index(&self, cache: &mut Cache, main: Option<&str>, active: Option<&str>, open: &HashMap<PathBuf, Arc<str>>) -> CitationIndex {
            let tree = || self.tree();
            let system = |name: &str| (name == "IEEEabrv.bib").then(|| PathBuf::from("/nowhere/IEEEabrv.bib"));
            let inputs = Inputs { root: self.dir.path(), main: main.map(|name| self.path(name)), active: active.map(|name| self.path(name)), open, tree: &tree, system_bib: &system };
            build(cache, &inputs)
        }
    }

    fn keys<'a>(entries: impl IntoIterator<Item = &'a IndexedEntry>) -> Vec<&'a str> {
        entries.into_iter().map(|entry| entry.key.as_str()).collect()
    }

    fn messages(index: &CitationIndex, path: &Path) -> Vec<String> {
        index.diagnostics(path).iter().map(|diagnostic| diagnostic.message.clone()).collect()
    }

    const THESIS: &[(&str, &str)] = &[
        ("thesis.tex", "\\documentclass{book}\n\\usepackage[backend=biber]{biblatex}\n\\usepackage{cleveref}\n\\input{preamble}\n\\begin{document}\n\\include{chapters/01_intro}\n\\subfile{chapters/02_eval}\n\\end{document}\n"),
        ("preamble.tex", "\\addbibresource{refs/main.bib}\n\\addbibresource{refs/extra.bib}\n"),
        ("chapters/01_intro.tex", "\\chapter{Intro}\\label{chap:intro}\nAs \\textcite{vaswani2017} show \\cref{fig:a}.\n"),
        ("chapters/02_eval.tex", "\\documentclass[../thesis.tex]{subfiles}\n\\begin{document}\n\\begin{figure}\\caption{A}\\label{fig:a}\\end{figure}\n\\parencite{smith2020}\n\\end{document}\n"),
        (
            "refs/main.bib",
            "@string{neurips = {Advances in Neural Information Processing Systems}}\n@inproceedings{vaswani2017,\n  author = {Vaswani, Ashish and Shazeer, Noam and others},\n  title = {Attention Is All You Need},\n  booktitle = neurips,\n  year = 2017,\n}\n",
        ),
        ("refs/extra.bib", "@article{smith2020, author = {Smith, John}, title = {Deep Things}, journal = neurips, year = {2020}}\n@proceedings{conf, title = {Proceedings of Things}, year = 2019}\n@inproceedings{part, author = {Doe, Jane}, title = {A Part}, crossref = {conf}}\n"),
        ("unused.bib", "@misc{unused, title = {Not declared}}\n"),
    ];

    #[test]
    fn a_thesis_in_chapters_finds_its_bibliographies_and_labels() {
        let project = Project::new(THESIS);
        let mut cache = Cache::default();
        let index = project.index(&mut cache, Some("thesis.tex"), Some("chapters/02_eval.tex"), &HashMap::new());
        assert_eq!(index.project.sources.len(), 4);
        assert_eq!(index.project.backend, Backend::Biber);
        assert!(index.project.biblatex && index.project.cleveref);
        // Only the declared files, and an @string of the first is known in the second.
        assert_eq!(keys(&index.entries), ["vaswani2017", "smith2020", "conf", "part"]);
        assert_eq!(index.entry("smith2020").unwrap().venue, "Advances in Neural Information Processing Systems");
        // crossref brings the booktitle and the year.
        let part = index.entry("part").unwrap();
        assert_eq!((part.venue.as_str(), part.year.as_str()), ("Proceedings of Things", "2019"));
        assert!(index.knows_label("chap:intro") && index.knows_label("fig:a"));
        assert_eq!(index.label("chap:intro")[0].label.env, "chapter");
        assert!(index.diagnostics.values().all(Vec::is_empty), "{:?}", index.diagnostics);
    }

    #[test]
    fn search_finds_by_any_field_and_ranks_the_key_first() {
        let project = Project::new(THESIS);
        let mut cache = Cache::default();
        let index = project.index(&mut cache, Some("thesis.tex"), None, &HashMap::new());
        assert_eq!(keys(index.search("attention 2017", 10)), ["vaswani2017"]);
        assert_eq!(keys(index.search("vaswani", 10)), ["vaswani2017"]);
        assert_eq!(keys(index.search("shazeer", 10)), ["vaswani2017"]);
        assert_eq!(keys(index.search("neural 2020", 10)), ["smith2020"]);
        assert_eq!(keys(index.search("smi", 10))[0], "smith2020");
        assert!(index.search("zzz", 10).is_empty());
        // Unfiltered: by author and year.
        assert_eq!(keys(index.search("", 10)), ["conf", "part", "smith2020", "vaswani2017"]);
    }

    #[test]
    fn missing_and_system_bibliographies() {
        let project = Project::new(&[("main.tex", "\\documentclass{article}\n\\bibliography{IEEEabrv, refs ,gone}\n\\cite{a}\n"), ("refs.bib", "@article{a, journal = IEEE_J_X}\n")]);
        let mut cache = Cache::default();
        let index = project.index(&mut cache, Some("main.tex"), None, &HashMap::new());
        assert_eq!(index.project.backend, Backend::Bibtex);
        assert_eq!(messages(&index, &project.path("main.tex")), ["There is no bibliography gone."]);
        let main = std::fs::read_to_string(project.path("main.tex")).unwrap();
        assert_eq!(&main[index.diagnostics(&project.path("main.tex"))[0].range.clone()], "gone");
        // The system file cannot be read here, so its macro is unknown.
        assert_eq!(
            messages(&index, &project.path("refs.bib")),
            ["No @string defines ieee_j_x, so its text is missing.", "This @article has no author, title, journal and year, which the bibliography prints. BibTeX warns and leaves it out."]
        );
        // A missing bibliography could hold any key, so none is called unknown.
        assert!(!index.bibs_complete);
        assert_eq!(crate::lint::source(&index, &project.path("main.tex")).len(), 1);
    }

    #[test]
    fn duplicate_keys_across_files_and_in_case() {
        let project = Project::new(&[
            ("main.tex", "\\documentclass{article}\\bibliography{a,b}"),
            ("a.bib", "@misc{same, title = {One}}\n@misc{Case, title = {C}}\n"),
            ("b.bib", "@misc{x}\n@misc{same, title = {Two}}\n@misc{case}\n@misc{loop1, crossref = {loop2}}\n@misc{loop2, crossref = {loop1}}\n@misc{orphan, crossref = {nowhere}}\n"),
        ]);
        let mut cache = Cache::default();
        let index = project.index(&mut cache, Some("main.tex"), None, &HashMap::new());
        let mut found = messages(&index, &project.path("b.bib"));
        found.sort();
        assert_eq!(
            found,
            [
                "The key case differs from Case at a.bib:2 only in case, which BibTeX takes for the same key.",
                "The key same is already defined at a.bib:1.",
                "loop1 inherits from loop1, which leads back to it.",
                "loop2 inherits from loop2, which leads back to it.",
                "orphan inherits from nowhere, which no bibliography has.",
            ]
        );
    }

    #[test]
    fn without_a_declared_bibliography_every_bib_counts() {
        let project = Project::new(&[("notes.tex", "\\cite{a}"), ("x/one.bib", "@misc{a}"), ("two.bib", "@misc{b}")]);
        let mut cache = Cache::default();
        let index = project.index(&mut cache, None, Some("notes.tex"), &HashMap::new());
        let mut found = keys(&index.entries);
        found.sort();
        assert_eq!(found, ["a", "b"]);
    }

    #[test]
    fn bibitems_latin1_and_open_documents() {
        let project = Project::new(&[("main.tex", "\\documentclass{article}\\bibliography{old}\n\\begin{thebibliography}{1}\\bibitem{manual}\\end{thebibliography}")]);
        std::fs::write(project.path("old.bib"), b"@misc{m\xfcller, title = {Gr\xfc\xdfe}}\n").unwrap();
        let mut cache = Cache::default();
        let index = project.index(&mut cache, Some("main.tex"), None, &HashMap::new());
        assert_eq!(keys(&index.entries), ["müller", "manual"]);
        assert_eq!(index.entry("müller").unwrap().title, "Grüße");
        assert!(index.bib_files[0].1, "read as Windows-1252");
        // An open document wins over the disk.
        let open: HashMap<PathBuf, Arc<str>> = [(project.path("old.bib"), Arc::from("@misc{edited}"))].into();
        let index = project.index(&mut cache, Some("main.tex"), None, &open);
        assert_eq!(keys(&index.entries), ["edited", "manual"]);
    }

    #[test]
    fn the_root_comment_of_the_active_file_decides_the_main_file() {
        let project = Project::new(&[
            ("book/main.tex", "\\documentclass{book}\\addbibresource{refs.bib}\\input{ch1}"),
            ("book/ch1.tex", "% !TEX root = main.tex\n\\label{one}"),
            ("book/refs.bib", "@misc{k}"),
            ("other.tex", "\\documentclass{article}"),
        ]);
        let mut cache = Cache::default();
        let index = project.index(&mut cache, Some("other.tex"), Some("book/ch1.tex"), &HashMap::new());
        assert_eq!(index.project.main, Some(project.path("book/main.tex")));
        assert!(index.knows_label("one"));
        assert_eq!(keys(&index.entries), ["k"]);
    }

    #[test]
    fn numbers_come_from_the_aux_of_the_last_build() {
        let project = Project::new(&[
            ("main.tex", "\\documentclass{book}\\include{ch}\\externaldocument[S-]{supp}"),
            ("ch.tex", "\\begin{figure}\\caption{X}\\label{fig:x}\\end{figure}"),
            (".build/main.aux", "\\relax\n\\@input{ch.aux}\n"),
            (".build/ch.aux", "\\newlabel{fig:x}{{2.1}{14}}\n"),
            ("supp.aux", "\\newlabel{tab:s}{{1}{2}}\n"),
        ]);
        let mut cache = Cache::default();
        let index = project.index(&mut cache, Some("main.tex"), None, &HashMap::new());
        let label = &index.label("fig:x")[0];
        assert_eq!((label.number.as_deref(), label.page.as_deref()), (Some("2.1"), Some("14")));
        assert!(index.knows_label("S-tab:s") && !index.external_unknown);
    }

    #[test]
    fn typst_documents_read_bib_and_yaml() {
        let project = Project::new(&[
            ("main.typ", "#include \"ch.typ\"\n#bibliography((\"refs.bib\", \"more.yml\"))"),
            ("ch.typ", "= Intro <intro>\nSee @harry and @intro."),
            ("refs.bib", "@book{knuth, title = {TAOCP}}"),
            ("more.yml", "harry:\n    type: book\n    title: Harry\n"),
        ]);
        let mut cache = Cache::default();
        let index = project.index(&mut cache, Some("main.typ"), None, &HashMap::new());
        assert_eq!(index.project.backend, Backend::Typst);
        assert_eq!(keys(&index.entries), ["knuth", "harry"]);
        assert!(index.knows_label("intro"));
    }

    #[test]
    fn one_author_and_year_get_letters_by_key_unless_the_build_says_otherwise() {
        let bib = "@misc{mueller2024b, author = {M{\\\"u}ller, Anna}, year = 2024}\n@misc{mueller2024a, author = {Anna M{\\\"u}ller}, date = {2024-03-01}}\n@misc{rossum, author = {Guido {van Rossum}}, year = 1995}\n";
        let project = Project::new(&[("main.tex", "\\documentclass{article}\\bibliography{refs}"), ("refs.bib", bib)]);
        let mut cache = Cache::default();
        let index = project.index(&mut cache, Some("main.tex"), None, &HashMap::new());
        let letter = |index: &CitationIndex, key: &str| index.entry(key).unwrap().letter;
        assert_eq!((letter(&index, "mueller2024a"), letter(&index, "mueller2024b"), letter(&index, "rossum")), (Some('a'), Some('b'), None));
        assert_eq!(index.entry("mueller2024a").unwrap().summary(), "Müller 2024a");
        std::fs::create_dir_all(project.path(".build")).unwrap();
        std::fs::write(project.path(".build/main.bbl"), "\\bibitem[{M{\\\"u}ller(2024a)}]{mueller2024b}\n\\bibitem[{M{\\\"u}ller(2024b)}]{mueller2024a}\n").unwrap();
        let index = project.index(&mut cache, Some("main.tex"), None, &HashMap::new());
        assert_eq!((letter(&index, "mueller2024a"), letter(&index, "mueller2024b")), (Some('b'), Some('a')));
    }

    #[test]
    fn the_build_decides_the_letters() {
        let natbib = r#"\bibitem[{M{\"u}ller et~al.(2024a)M{\"u}ller, Schmidt, and Ott}]{mueller2024b}
\bibitem[{M{\"u}ller et~al.(2024)}]{mueller2024a}
\bibitem[M{\"u}ller(2024{\natexlab{b}})]{plainnat}
\bibitem[Gre93]{alpha}"#;
        let letters = bbl_letters(natbib);
        assert_eq!(letters["mueller2024b"], Some('a'));
        assert_eq!(letters["mueller2024a"], None);
        assert_eq!(letters["plainnat"], Some('b'));
        assert!(!letters.contains_key("alpha"));
        let biblatex = "\\entry{one}{article}{}\n  \\field{extradate}{2}\n\\endentry\n\\entry{two}{article}{}\n\\endentry";
        let letters = bbl_letters(biblatex);
        assert_eq!((letters["one"], letters["two"]), (Some('b'), None));
    }

    #[test]
    fn sources_learn_what_is_missing_twice_or_misplaced() {
        let project = Project::new(&[
            ("main.tex", "\\documentclass{article}\n\\usepackage{xr}\n\\bibliography{refs}\n\\input{two}\n\\cite{known, typo2024, Known}\n\\ref{sec:a} \\ref{nowhere}\n\\section{A}\\label{sec:a}\n\\begin{figure}\\label{fig:early}\\caption{C}\\end{figure}\n"),
            ("two.tex", "\\section{B}\\label{sec:a}\n"),
            ("refs.bib", "@misc{known, title = {K}}\n@article{dup1, author = {A}, title = {Attention Is All You Need}, journal = {J}, year = 2017, doi = {10.1/X}}\n@article{dup2, author = {A}, title = {Attention is all you need}, journal = {J}, year = 2017}\n"),
        ]);
        let mut cache = Cache::default();
        let index = project.index(&mut cache, Some("main.tex"), None, &HashMap::new());
        let main = std::fs::read_to_string(project.path("main.tex")).unwrap();
        let found: Vec<(String, Severity)> = crate::lint::source(&index, &project.path("main.tex")).into_iter().map(|diagnostic| (main[diagnostic.range].to_string(), diagnostic.severity)).collect();
        assert_eq!(
            found,
            [
                ("typo2024".to_string(), Severity::Error),
                ("Known".to_string(), Severity::Error),
                ("nowhere".to_string(), Severity::Warning),
                ("sec:a".to_string(), Severity::Error),
                ("fig:early".to_string(), Severity::Warning),
            ]
        );
        let two = crate::lint::source(&index, &project.path("two.tex"));
        assert_eq!(two.len(), 1);
        assert!(two[0].message.ends_with("Also at main.tex:7."), "{}", two[0].message);
        let hints: Vec<String> = messages(&index, &project.path("refs.bib"));
        assert!(hints.iter().any(|message| message == "Same title and year as dup1 at refs.bib:2: the same work under two keys?"), "{hints:?}");
    }

    #[test]
    fn an_edit_of_a_chapter_reuses_the_entries_and_an_edit_of_the_bib_does_not() {
        let project = Project::new(THESIS);
        let mut cache = Cache::default();
        let first = project.index(&mut cache, Some("thesis.tex"), None, &HashMap::new());
        let chapter: HashMap<PathBuf, Arc<str>> = [(project.path("chapters/01_intro.tex"), Arc::from("\\chapter{Intro}\\label{chap:new}\n"))].into();
        let edited = project.index(&mut cache, Some("thesis.tex"), None, &chapter);
        assert!(Arc::ptr_eq(&first.bib, &edited.bib));
        assert!(edited.knows_label("chap:new") && !edited.knows_label("chap:intro"));
        let bib: HashMap<PathBuf, Arc<str>> = [(project.path("refs/extra.bib"), Arc::from("@misc{fresh}"))].into();
        let changed = project.index(&mut cache, Some("thesis.tex"), None, &bib);
        assert!(!Arc::ptr_eq(&first.bib, &changed.bib));
        assert!(changed.entry("fresh").is_some() && changed.entry("smith2020").is_none());
    }

    /// A large bibliography, as it grows over a career, searched as one types.
    #[test]
    #[ignore = "timing, run with --release --ignored"]
    fn search_timing() {
        let mut text = String::from("@string{jfill = \"Journal of Filler\"}\n");
        let surnames = [r#"M{\"u}ller"#, "Schmidt", "{van der Berg}", "O'Neil", "Nakamura", r"Dvo{\v r}{\'a}k"];
        for index in 0..5000 {
            let surname = surnames[index % surnames.len()];
            text.push_str(&format!("@article{{key{index},\n  author = {{{surname}, Alex and Other, Bo}},\n  title = {{Filler Title about Attention number {index}}},\n  journal = jfill,\n  year = {},\n}}\n", 1950 + index % 70));
        }
        let project = Project::new(&[("main.tex", "\\documentclass{article}\\bibliography{big}"), ("big.bib", &text)]);
        let mut cache = Cache::default();
        let started = std::time::Instant::now();
        let index = project.index(&mut cache, Some("main.tex"), None, &HashMap::new());
        let built = started.elapsed();
        let started = std::time::Instant::now();
        let again = project.index(&mut cache, Some("main.tex"), None, &HashMap::new());
        let rebuilt = started.elapsed();
        assert_eq!(again.entries.len(), 5000);
        // Typing in the document: its text changes, the bibliography does not.
        let typed: HashMap<PathBuf, Arc<str>> = [(project.path("main.tex"), Arc::from("\\documentclass{article}\\bibliography{big}\\cite{key1}"))].into();
        let started = std::time::Instant::now();
        project.index(&mut cache, Some("main.tex"), None, &typed);
        let typed = started.elapsed();
        println!("after typing in the document, rebuilt in {typed:?}");
        for query in ["a", "mu", "mueller 1999", "attention 4711", "jfill", "xyz"] {
            let mut fastest = std::time::Duration::MAX;
            let mut found = 0;
            for _ in 0..20 {
                let started = std::time::Instant::now();
                found = index.search(query, 200).len();
                fastest = fastest.min(started.elapsed());
            }
            println!("{query:>14}: {found:>4} found in {fastest:?}");
            assert!(fastest < std::time::Duration::from_millis(1), "{query}: {fastest:?}");
        }
        println!("built in {built:?}, rebuilt from the cache in {rebuilt:?}");
    }
}
