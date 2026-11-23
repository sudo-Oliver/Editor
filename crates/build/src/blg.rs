//! The logs of BibTeX and biber, the `.blg` file: each problem as a
//! diagnostic with the file and line of the entry where the log names one,
//! instead of one line that only says how many there were.

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

use crate::{Diagnostic, Severity};

/// At most this many problems are listed, since a bibliography copied
/// together over years can have hundreds.
const MAX_PROBLEMS: usize = 50;

/// Finds entries in the bibliographies, each file read once.
struct Bibs<'a> {
    cwd: &'a Path,
    files: Vec<PathBuf>,
    read: HashMap<PathBuf, Vec<(String, usize)>>,
}

impl Bibs<'_> {
    fn resolve(&self, name: &str) -> Option<PathBuf> {
        let name = name.trim().trim_matches(['\'', '"']);
        let path = self.cwd.join(name);
        if path.is_file() {
            return Some(path);
        }
        let with_bib = self.cwd.join(format!("{name}.bib"));
        if with_bib.is_file() {
            return Some(with_bib);
        }
        // biber names the UTF-8 copy it made, as in `refs.bib_1234.utf8`.
        let file = Path::new(name).file_name()?.to_string_lossy().into_owned();
        let file = file.strip_suffix(".utf8").and_then(|rest| rest.rsplit_once('_')).map_or(file.as_str(), |(file, _)| file).to_string();
        self.files.iter().find(|path| path.file_name().is_some_and(|known| known.to_string_lossy() == file)).cloned()
    }

    /// The file and line of the entry with `key`, from 1.
    fn entry(&mut self, key: &str) -> Option<(PathBuf, usize)> {
        for path in self.files.clone() {
            let entries = self.read.entry(path.clone()).or_insert_with(|| {
                let text = std::fs::read(&path).map(|bytes| String::from_utf8_lossy(&bytes).into_owned()).unwrap_or_default();
                citations::bib::parse(&text).entries.into_iter().map(|entry| (entry.key, entry.line)).collect()
            });
            if let Some((_, line)) = entries.iter().find(|(known, _)| known == key) {
                return Some((path, line + 1));
            }
        }
        None
    }
}

fn problem(tool: &str, file: Option<PathBuf>, line: Option<usize>, message: &str) -> Diagnostic {
    Diagnostic { file, line, column: None, severity: Severity::Warning, message: format!("{tool}: {}", message.trim()), ..Default::default() }
}

/// The problems a `.blg` names, from BibTeX or from biber. `cwd` is where
/// the bibliography names are relative to.
pub fn parse(blg: &str, cwd: &Path) -> Vec<Diagnostic> {
    let biber = blg.lines().take(5).any(|line| line.contains("biber") || line.contains("Biber"));
    let mut bibs = Bibs { cwd, files: Vec::new(), read: HashMap::new() };
    let mut out = if biber { parse_biber(blg, &mut bibs) } else { parse_bibtex(blg, &mut bibs) };
    out.truncate(MAX_PROBLEMS);
    out
}

fn parse_bibtex(blg: &str, bibs: &mut Bibs) -> Vec<Diagnostic> {
    let lines: Vec<&str> = blg.lines().collect();
    for line in &lines {
        if let Some(name) = line.strip_prefix("Database file #").and_then(|rest| rest.split_once(": ")).map(|(_, name)| name)
            && let Some(path) = bibs.resolve(name)
        {
            bibs.files.push(path);
        }
    }
    let mut out = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        let line = line.trim_end();
        if let Some((message, place)) = line.split_once("---line ") {
            let (number, file) = place.split_once(" of file ").unwrap_or((place, ""));
            let number = number.trim().parse::<usize>().ok();
            if file.ends_with(".bst") {
                // A style complained while working on an entry, named on the line before.
                let Some(before) = index.checked_sub(1).map(|before| lines[before].trim()) else {
                    continue;
                };
                let key = before.rsplit_once(" for entry ").map(|(_, key)| key.trim());
                let (file, line) = key.and_then(|key| bibs.entry(key)).map_or((None, None), |(file, line)| (Some(file), Some(line)));
                out.push(problem("BibTeX", file, line, before));
            } else {
                out.push(problem("BibTeX", bibs.resolve(file), number, message));
            }
        } else if let Some(message) = line.strip_prefix("Warning--") {
            // `--line 3 of file refs.bib` may follow, for an undefined string.
            let next = lines.get(index + 1).and_then(|next| next.strip_prefix("--line ")).and_then(|place| place.split_once(" of file "));
            let (file, number) = match next {
                Some((number, file)) => (bibs.resolve(file), number.trim().parse().ok()),
                None => match message.rsplit_once(" in ") {
                    Some((_, key)) => bibs.entry(key.trim()).map_or((None, None), |(file, line)| (Some(file), Some(line))),
                    None => (None, None),
                },
            };
            out.push(problem("BibTeX", file, number, message));
        } else if line.starts_with("I couldn't open") || line.starts_with("I found no") {
            out.push(problem("BibTeX", None, None, line));
        }
    }
    out
}

fn parse_biber(blg: &str, bibs: &mut Bibs) -> Vec<Diagnostic> {
    for line in blg.lines() {
        if let Some((_, rest)) = line.split_once("Found BibTeX data source '")
            && let Some((name, _)) = rest.split_once('\'')
            && let Some(path) = bibs.resolve(name)
        {
            bibs.files.push(path);
        }
    }
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for line in blg.lines() {
        let Some(message) = line.split_once("WARN - ").or_else(|| line.split_once("ERROR - ")).map(|(_, message)| message.trim()) else {
            continue;
        };
        if !seen.insert(message.to_string()) {
            continue;
        }
        // `BibTeX subsystem: /tmp/refs.bib_1234.utf8, line 12, syntax error: …`
        if let Some(rest) = message.strip_prefix("BibTeX subsystem: ")
            && let Some((file, rest)) = rest.split_once(", line ")
            && let Some((number, what)) = rest.split_once(", ")
        {
            out.push(problem("biber", bibs.resolve(file), number.trim().parse().ok(), what));
            continue;
        }
        // `Datamodel: Entry 'x' (refs.bib): Missing mandatory field 'author'`
        let key = message.split_once("Entry '").and_then(|(_, rest)| rest.split_once('\'')).map(|(key, _)| key.to_string());
        let (file, number) = key.and_then(|key| bibs.entry(&key)).map_or((None, None), |(file, line)| (Some(file), Some(line)));
        let message = message.strip_prefix("Datamodel: ").unwrap_or(message);
        out.push(problem("biber", file, number, message));
    }
    out
}

/// The bibliographies the last run read: `\bibdata` in the `.aux` for
/// BibTeX, the data sources in the `.bcf` for biber.
pub fn bibliographies(cwd: &Path, output_directory: &Path, stem: &str) -> Vec<PathBuf> {
    let mut names = Vec::new();
    if let Ok(aux) = std::fs::read_to_string(output_directory.join(format!("{stem}.aux"))) {
        for line in aux.lines() {
            if let Some(list) = line.strip_prefix("\\bibdata{").and_then(|rest| rest.strip_suffix('}')) {
                names.extend(list.split(',').map(|name| if name.trim().ends_with(".bib") { name.trim().to_string() } else { format!("{}.bib", name.trim()) }));
            }
        }
    }
    if let Ok(bcf) = std::fs::read_to_string(output_directory.join(format!("{stem}.bcf"))) {
        for chunk in bcf.split("<bcf:datasource").skip(1) {
            if chunk.contains("type=\"file\"")
                && let Some((_, rest)) = chunk.split_once('>')
                && let Some((name, _)) = rest.split_once('<')
            {
                names.push(name.trim().to_string());
            }
        }
    }
    names.into_iter().map(|name| cwd.join(name)).filter(|path| path.is_file()).collect()
}

/// Keys LaTeX reported as undefined citations in its log.
pub fn undefined_citations(log: &str) -> Vec<String> {
    let mut keys = Vec::new();
    for line in log.lines() {
        // LaTeX quotes with ` and ', biblatex with ' and '.
        let Some(rest) = line.split_once("Citation `").or_else(|| line.split_once("Citation '")).map(|(_, rest)| rest) else {
            continue;
        };
        if let Some((key, after)) = rest.split_once('\'')
            && after.contains("undefined")
        {
            keys.push(key.to_string());
        }
    }
    keys.sort();
    keys.dedup();
    keys
}

/// Keys the last BibTeX or biber run could not find in any bibliography.
pub fn not_found(blg: &str) -> HashSet<String> {
    blg.lines()
        .filter_map(|line| line.split_once("I didn't find a database entry for ").map(|(_, rest)| rest))
        .filter_map(|rest| {
            let rest = rest.trim_start_matches(['"', '\'']);
            rest.split(['"', '\'']).next().map(str::to_string)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bibtex_problems_point_to_their_entries() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::create_dir(directory.path().join("refs")).unwrap();
        std::fs::write(directory.path().join("refs/main.bib"), "@string{x = {X}}\n\n@article{a2020,\n  title = {T},\n}\n@book{b, title = {B}}\n").unwrap();
        let blg = "This is BibTeX, Version 0.99d\nDatabase file #1: refs/main.bib\nI was expecting a `,' or a `}'---line 6 of file refs/main.bib\n :  title = {B}\nI'm skipping whatever remains of this entry\nWarning--string name \"icml\" is undefined\n--line 4 of file refs/main.bib\nWarning--empty journal in a2020\nWarning--I didn't find a database entry for \"gone\"\n`journal' is a missing field, not a string, for entry a2020\nwhile executing---line 1733 of file plainnat.bst\n(There were 2 error messages)\n";
        let found: Vec<(Option<String>, Option<usize>, String)> = parse(blg, directory.path())
            .into_iter()
            .map(|diagnostic| (diagnostic.file.map(|file| file.strip_prefix(directory.path()).unwrap().display().to_string()), diagnostic.line, diagnostic.message))
            .collect();
        let main = Some("refs/main.bib".to_string());
        assert_eq!(
            found,
            [
                (main.clone(), Some(6), "BibTeX: I was expecting a `,' or a `}'".to_string()),
                (main.clone(), Some(4), "BibTeX: string name \"icml\" is undefined".to_string()),
                (main.clone(), Some(3), "BibTeX: empty journal in a2020".to_string()),
                (None, None, "BibTeX: I didn't find a database entry for \"gone\"".to_string()),
                (main, Some(3), "BibTeX: `journal' is a missing field, not a string, for entry a2020".to_string()),
            ]
        );
        assert_eq!(not_found(blg), HashSet::from(["gone".to_string()]));
    }

    #[test]
    fn biber_problems_point_to_their_entries() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("refs.bib"), "@article{x, title = {T}}\n\n@book{y, title = {Y}}\n").unwrap();
        let blg = "[0] Config.pm:307> INFO - This is Biber 2.19\n[71] Biber.pm:4419> INFO - Found BibTeX data source 'refs.bib'\n[90] Utils.pm:418> ERROR - BibTeX subsystem: /var/folders/xy/refs.bib_4711.utf8, line 3, syntax error: found \"title\", expected \",\"\n[95] Utils.pm:418> WARN - Datamodel: Entry 'y' (refs.bib): Missing mandatory field 'author'\n[96] Utils.pm:418> WARN - Datamodel: Entry 'y' (refs.bib): Missing mandatory field 'author'\n[97] Biber.pm:131> WARN - I didn't find a database entry for 'nope' (section 0)\n";
        let found: Vec<(bool, Option<usize>, String)> = parse(blg, directory.path()).into_iter().map(|diagnostic| (diagnostic.file.is_some(), diagnostic.line, diagnostic.message)).collect();
        assert_eq!(
            found,
            [
                (true, Some(3), "biber: syntax error: found \"title\", expected \",\"".to_string()),
                (true, Some(3), "biber: Entry 'y' (refs.bib): Missing mandatory field 'author'".to_string()),
                (false, None, "biber: I didn't find a database entry for 'nope' (section 0)".to_string()),
            ]
        );
        assert_eq!(not_found(blg), HashSet::from(["nope".to_string()]));
    }

    /// The editor marks a broken .bib where BibTeX itself complains: each
    /// line BibTeX names lies within a mark of the editor's parser.
    #[test]
    #[ignore = "runs BibTeX"]
    fn the_editor_finds_what_bibtex_finds() {
        let Some(bibtex) = crate::find_tool("bibtex") else {
            return;
        };
        let directory = tempfile::tempdir().unwrap();
        let bib = "@article{a,\n  title = {One}\n  year = 2001,\n}\n\n@book{b, title {No equals}, year = 2}\n\n@misc{a, title = {Again}}\n\n@misc{c, note = {fine}}\n";
        std::fs::write(directory.path().join("refs.bib"), bib).unwrap();
        std::fs::write(directory.path().join("main.aux"), "\\citation{*}\n\\bibstyle{plain}\n\\bibdata{refs}\n").unwrap();
        std::process::Command::new(bibtex).arg("main").current_dir(directory.path()).output().unwrap();
        let blg = std::fs::read_to_string(directory.path().join("main.blg")).unwrap();
        // The syntax errors BibTeX names with their line in the .bib.
        let reported: Vec<usize> = blg
            .lines()
            .filter(|line| !line.starts_with("Repeated entry"))
            .filter_map(|line| line.split_once("---line ")?.1.strip_suffix(" of file refs.bib")?.parse().ok())
            .collect();
        assert!(!reported.is_empty(), "{blg}");
        let marks: Vec<(usize, usize)> = citations::bib::parse(bib)
            .diagnostics
            .iter()
            .map(|diagnostic| (citations::text::line_col(bib, diagnostic.range.start).0 + 1, citations::text::line_col(bib, diagnostic.range.end).0 + 1))
            .collect();
        for line in reported {
            assert!(marks.iter().any(|(start, end)| *start <= line && line <= *end), "BibTeX names line {line}, the editor marks {marks:?}\n{blg}");
        }
    }

    #[test]
    fn the_bibliographies_of_the_last_run_and_its_undefined_keys() {
        let directory = tempfile::tempdir().unwrap();
        let build = directory.path().join(".build");
        std::fs::create_dir_all(&build).unwrap();
        std::fs::create_dir(directory.path().join("refs")).unwrap();
        for name in ["refs/a.bib", "b.bib", "c.bib"] {
            std::fs::write(directory.path().join(name), "").unwrap();
        }
        std::fs::write(build.join("main.aux"), "\\relax\n\\bibdata{refs/a,b}\n").unwrap();
        std::fs::write(build.join("main.bcf"), "<bcf:datasource type=\"file\" datatype=\"bibtex\" glob=\"false\">c.bib</bcf:datasource>").unwrap();
        let found: Vec<PathBuf> = bibliographies(directory.path(), &build, "main");
        assert_eq!(found, [directory.path().join("refs/a.bib"), directory.path().join("b.bib"), directory.path().join("c.bib")]);
        let log = "LaTeX Warning: Citation `knuth84' on page 1 undefined on input line 3.\nPackage natbib Warning: Citation `lamport' on page 2 undefined on input line 9.\nLaTeX Warning: Citation 'bib:latex' on page 1 undefined on input line 4.\nLaTeX Warning: There were undefined references.\n";
        assert_eq!(undefined_citations(log), ["bib:latex", "knuth84", "lamport"]);
    }
}
