//! The projects the user knows and the templates new ones start from. Both live in
//! the config folder of the user, next to the settings.

use std::{
    fs, io,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

use crate::picker::fuzzy_score;

/// Neither copied into a template nor out of one.
const SKIPPED: [&str; 4] = [".build", ".git", ".DS_Store", TEMPLATE_FILE];
const TEMPLATE_FILE: &str = "template.toml";
/// Deep enough for chapters in folders, short of walking a whole disk.
const LANGUAGE_DEPTH: usize = 3;

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TemplateLanguage {
    Latex,
    Typst,
    #[default]
    Other,
}

impl TemplateLanguage {
    pub fn label(self) -> &'static str {
        match self {
            Self::Latex => "LaTeX",
            Self::Typst => "Typst",
            Self::Other => "Other",
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct ProjectEntry {
    pub path: PathBuf,
    pub title: String,
    /// Seconds since the Unix epoch.
    pub last_opened: u64,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(default)]
pub struct Registry {
    /// Where the last new project went, for the next one.
    pub default_location: Option<PathBuf>,
    /// The most recently opened first.
    pub projects: Vec<ProjectEntry>,
}

impl Registry {
    /// A missing or broken file means no projects, so that the hub always opens.
    pub fn load(path: &Path) -> Self {
        fs::read_to_string(path).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let text = serde_json::to_string_pretty(self).map_err(io::Error::other)?;
        crate::atomic::write(path, text.as_bytes())
    }

    /// Puts a project first, and adds it with its folder name when it is new.
    pub fn touch(&mut self, path: &Path, now: u64) {
        let entry = match self.projects.iter().position(|entry| entry.path == path) {
            Some(index) => {
                let mut entry = self.projects.remove(index);
                entry.last_opened = now;
                entry
            }
            None => ProjectEntry { path: path.to_path_buf(), title: folder_name(path), last_opened: now },
        };
        self.projects.insert(0, entry);
    }

    pub fn rename(&mut self, path: &Path, title: &str) {
        let title = title.trim();
        if let Some(entry) = self.projects.iter_mut().find(|entry| entry.path == path)
            && !title.is_empty()
        {
            entry.title = title.to_string();
        }
    }

    pub fn remove(&mut self, path: &Path) {
        self.projects.retain(|entry| entry.path != path);
    }

    pub fn location(&self) -> PathBuf {
        self.default_location.clone().unwrap_or_else(|| home().join("Documents").join("EditorProjects"))
    }

    /// The indices of the projects that match, best first. An empty query keeps
    /// the order of last use.
    pub fn filter(&self, query: &str) -> Vec<usize> {
        if query.trim().is_empty() {
            return (0..self.projects.len()).collect();
        }
        let mut scored: Vec<(i64, usize)> = self
            .projects
            .iter()
            .enumerate()
            .filter_map(|(index, entry)| {
                let title = fuzzy_score(query, &entry.title);
                let path = fuzzy_score(query, &entry.path.to_string_lossy());
                // The title is what people remember, so it counts more.
                title.map(|score| score + 1000).max(path).map(|score| (score, index))
            })
            .collect();
        scored.sort_by(|left, right| right.0.cmp(&left.0).then(left.1.cmp(&right.1)));
        scored.into_iter().map(|(_, index)| index).collect()
    }
}

pub fn registry_path() -> Option<PathBuf> {
    Some(crate::settings::config_dir()?.join("projects.json"))
}

pub fn library_path() -> Option<PathBuf> {
    Some(crate::settings::config_dir()?.join("templates"))
}

pub fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |time| time.as_secs())
}

fn home() -> PathBuf {
    std::env::var_os("HOME").map_or_else(|| PathBuf::from("/"), PathBuf::from)
}

fn folder_name(path: &Path) -> String {
    path.file_name().map_or_else(|| path.display().to_string(), |name| name.to_string_lossy().into_owned())
}

/// The path with the home folder as `~`.
pub fn tilde(path: &Path) -> String {
    match path.strip_prefix(home()) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".to_string(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

/// How long ago, in the largest unit that fits.
pub fn ago(now: u64, then: u64) -> String {
    let seconds = now.saturating_sub(then);
    let (count, unit) = match seconds {
        0..60 => return "Just now".to_string(),
        60..3600 => (seconds / 60, "minute"),
        3600..86_400 => (seconds / 3600, "hour"),
        86_400..2_592_000 => (seconds / 86_400, "day"),
        2_592_000..31_536_000 => (seconds / 2_592_000, "month"),
        _ => (seconds / 31_536_000, "year"),
    };
    format!("{count} {unit}{} ago", if count == 1 { "" } else { "s" })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TemplateSource {
    /// Ships with the editor and cannot be removed.
    Builtin(&'static [(&'static str, &'static str)]),
    Folder(PathBuf),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Template {
    pub name: String,
    pub language: TemplateLanguage,
    pub description: String,
    pub source: TemplateSource,
}

#[derive(Deserialize, Serialize)]
struct TemplateFile {
    name: String,
    #[serde(default)]
    language: TemplateLanguage,
    #[serde(default)]
    description: String,
}

const EMPTY_LATEX: &str = "\\documentclass{article}

\\title{Title}
\\author{Author}

\\begin{document}
\\maketitle

\\section{Introduction}

\\end{document}
";

const EMPTY_TYPST: &str = "#set page(paper: \"a4\")
#set heading(numbering: \"1.\")

#align(center, text(17pt)[*Title*])

= Introduction
";

fn builtins() -> Vec<Template> {
    let template = |name: &str, language, description: &str, files| Template {
        name: name.to_string(),
        language,
        description: description.to_string(),
        source: TemplateSource::Builtin(files),
    };
    vec![
        template("Empty LaTeX", TemplateLanguage::Latex, "A main.tex with an article to start from.", &[("main.tex", EMPTY_LATEX)]),
        template("Empty Typst", TemplateLanguage::Typst, "A main.typ with a title and a heading.", &[("main.typ", EMPTY_TYPST)]),
        template("Empty project", TemplateLanguage::Other, "No files at all.", &[]),
    ]
}

/// The built-in templates, then those of the user by name.
pub fn load_templates(library: Option<&Path>) -> Vec<Template> {
    let mut own: Vec<Template> = library
        .and_then(|library| fs::read_dir(library).ok())
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .map(|dir| {
            let file = fs::read_to_string(dir.join(TEMPLATE_FILE)).ok().and_then(|text| toml::from_str::<TemplateFile>(&text).ok());
            match file {
                Some(file) => Template { name: file.name, language: file.language, description: file.description, source: TemplateSource::Folder(dir) },
                // A folder dropped into the library by hand still counts.
                None => Template {
                    name: folder_name(&dir),
                    language: detect_language(&dir),
                    description: String::new(),
                    source: TemplateSource::Folder(dir),
                },
            }
        })
        .collect();
    own.sort_by_key(|template| template.name.to_lowercase());
    let mut templates = builtins();
    templates.extend(own);
    templates
}

/// Copies a project into the library and returns the new template folder.
pub fn save_template(from: &Path, library: &Path, name: &str, description: &str) -> io::Result<PathBuf> {
    let name = name.trim();
    if name.is_empty() {
        return Err(io::Error::other("The template needs a name."));
    }
    let base = slug(name);
    let mut dir = library.join(&base);
    let mut number = 2;
    while dir.exists() {
        dir = library.join(format!("{base}-{number}"));
        number += 1;
    }
    copy_tree(from, &dir)?;
    let file = TemplateFile { name: name.to_string(), language: detect_language(from), description: description.trim().to_string() };
    write_template_file(&dir, &file)?;
    Ok(dir)
}

pub fn rename_template(dir: &Path, name: &str) -> io::Result<()> {
    let name = name.trim();
    if name.is_empty() {
        return Err(io::Error::other("The template needs a name."));
    }
    let mut file = fs::read_to_string(dir.join(TEMPLATE_FILE))
        .ok()
        .and_then(|text| toml::from_str::<TemplateFile>(&text).ok())
        .unwrap_or_else(|| TemplateFile { name: String::new(), language: detect_language(dir), description: String::new() });
    file.name = name.to_string();
    write_template_file(dir, &file)
}

fn write_template_file(dir: &Path, file: &TemplateFile) -> io::Result<()> {
    let text = toml::to_string(file).map_err(io::Error::other)?;
    crate::atomic::write(&dir.join(TEMPLATE_FILE), text.as_bytes())
}

/// A folder name from a template name, without characters file systems dislike.
fn slug(name: &str) -> String {
    let slug: String = name
        .chars()
        .map(|ch| if ch.is_alphanumeric() || ch == '-' || ch == '_' { ch } else { '-' })
        .collect();
    let slug = slug.trim_matches('-').to_string();
    if slug.is_empty() { "template".to_string() } else { slug }
}

/// Copies a folder, leaving out build output, git history and template metadata.
pub fn copy_tree(from: &Path, to: &Path) -> io::Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let name = entry.file_name();
        if SKIPPED.iter().any(|skipped| name == *skipped) {
            continue;
        }
        let source = entry.path();
        let target = to.join(&name);
        // A template inside its own project must not copy itself forever.
        if to.starts_with(&source) {
            continue;
        }
        if entry.file_type()?.is_dir() {
            copy_tree(&source, &target)?;
        } else {
            fs::copy(&source, &target)?;
        }
    }
    Ok(())
}

/// Typst wins with a `main.typ`, LaTeX with a `main.tex`, otherwise any source counts.
pub fn detect_language(dir: &Path) -> TemplateLanguage {
    if dir.join("main.typ").is_file() {
        return TemplateLanguage::Typst;
    }
    if dir.join("main.tex").is_file() {
        return TemplateLanguage::Latex;
    }
    let (mut tex, mut typ) = (false, false);
    scan_sources(dir, LANGUAGE_DEPTH, &mut tex, &mut typ);
    match (tex, typ) {
        (true, _) => TemplateLanguage::Latex,
        (false, true) => TemplateLanguage::Typst,
        (false, false) => TemplateLanguage::Other,
    }
}

fn scan_sources(dir: &Path, depth: usize, tex: &mut bool, typ: &mut bool) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if SKIPPED.iter().any(|skipped| entry.file_name() == *skipped) {
            continue;
        }
        if path.is_dir() {
            if depth > 0 {
                scan_sources(&path, depth - 1, tex, typ);
            }
            continue;
        }
        match path.extension().and_then(|extension| extension.to_str()) {
            Some("tex") => *tex = true,
            Some("typ") => *typ = true,
            _ => {}
        }
        if *tex {
            return;
        }
    }
}

/// Keeps build output out of git, without touching other lines.
pub fn ensure_gitignore(dir: &Path) -> io::Result<()> {
    let path = dir.join(".gitignore");
    let text = fs::read_to_string(&path).unwrap_or_default();
    if text.lines().any(|line| matches!(line.trim(), ".build" | ".build/" | "/.build" | "/.build/")) {
        return Ok(());
    }
    let separator = if text.is_empty() || text.ends_with('\n') { "" } else { "\n" };
    fs::write(&path, format!("{text}{separator}.build/\n"))
}

/// The folder a new project goes to, or why it cannot.
pub fn new_project_dir(location: &Path, name: &str) -> Result<PathBuf, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("The project needs a name.".to_string());
    }
    if name.contains(['/', '\\']) || name == "." || name == ".." {
        return Err("A project name cannot contain a slash.".to_string());
    }
    let dir = location.join(name);
    let occupied = fs::read_dir(&dir).is_ok_and(|mut entries| entries.next().is_some());
    if occupied || dir.is_file() {
        return Err(format!("{} already exists.", tilde(&dir)));
    }
    Ok(dir)
}

pub fn create_project(template: &Template, dir: &Path) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    match &template.source {
        TemplateSource::Builtin(files) => {
            for (name, text) in *files {
                fs::write(dir.join(name), text)?;
            }
        }
        TemplateSource::Folder(from) => copy_tree(from, dir)?,
    }
    ensure_gitignore(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str, title: &str, last_opened: u64) -> ProjectEntry {
        ProjectEntry { path: PathBuf::from(path), title: title.to_string(), last_opened }
    }

    #[test]
    fn a_missing_or_broken_registry_is_empty() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("projects.json");
        assert_eq!(Registry::load(&path), Registry::default());
        fs::write(&path, "{ not json").unwrap();
        assert_eq!(Registry::load(&path), Registry::default());
    }

    #[test]
    fn the_registry_survives_a_restart() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("nested").join("projects.json");
        let registry = Registry { default_location: Some("/tmp/projects".into()), projects: vec![entry("/a", "A", 3)] };
        registry.save(&path).unwrap();
        assert_eq!(Registry::load(&path), registry);
    }

    #[test]
    fn touching_moves_a_project_first_without_doubling_it() {
        let mut registry = Registry::default();
        registry.touch(Path::new("/work/thesis"), 1);
        registry.touch(Path::new("/work/paper"), 2);
        registry.touch(Path::new("/work/thesis"), 3);
        assert_eq!(registry.projects, [entry("/work/thesis", "thesis", 3), entry("/work/paper", "paper", 2)]);
    }

    #[test]
    fn renaming_and_removing_change_only_the_list() {
        let mut registry = Registry { default_location: None, projects: vec![entry("/a", "a", 1), entry("/b", "b", 1)] };
        registry.rename(Path::new("/a"), "  Dissertation ");
        registry.rename(Path::new("/b"), "   ");
        registry.remove(Path::new("/b"));
        assert_eq!(registry.projects, [entry("/a", "Dissertation", 1)]);
    }

    #[test]
    fn the_filter_prefers_titles_and_keeps_order_without_a_query() {
        let registry = Registry {
            default_location: None,
            projects: vec![entry("/kit/dissertation", "Thesis", 3), entry("/notes/math", "Math notes", 2), entry("/x/y", "Other", 1)],
        };
        assert_eq!(registry.filter(""), [0, 1, 2]);
        assert_eq!(registry.filter("math"), [1]);
        assert_eq!(registry.filter("kit"), [0]);
        assert!(registry.filter("zzz").is_empty());
    }

    #[test]
    fn copies_leave_out_build_output_git_and_template_metadata() {
        let directory = tempfile::tempdir().unwrap();
        let from = directory.path().join("from");
        for dir in ["chapters", ".build", ".git"] {
            fs::create_dir_all(from.join(dir)).unwrap();
        }
        fs::write(from.join("main.tex"), "x").unwrap();
        fs::write(from.join("chapters").join("one.tex"), "y").unwrap();
        fs::write(from.join(".build").join("main.pdf"), "z").unwrap();
        fs::write(from.join(TEMPLATE_FILE), "name = \"t\"").unwrap();
        let to = directory.path().join("to");
        copy_tree(&from, &to).unwrap();
        assert!(to.join("main.tex").is_file());
        assert!(to.join("chapters").join("one.tex").is_file());
        assert!(!to.join(".build").exists());
        assert!(!to.join(".git").exists());
        assert!(!to.join(TEMPLATE_FILE).exists());
    }

    #[test]
    fn the_language_comes_from_the_main_file_or_any_source() {
        let directory = tempfile::tempdir().unwrap();
        let dir = |name: &str, files: &[&str]| {
            let dir = directory.path().join(name);
            for file in files {
                let path = dir.join(file);
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::write(path, "").unwrap();
            }
            fs::create_dir_all(&dir).unwrap();
            dir
        };
        assert_eq!(detect_language(&dir("latex", &["chapters/intro.tex"])), TemplateLanguage::Latex);
        assert_eq!(detect_language(&dir("typst", &["paper.typ"])), TemplateLanguage::Typst);
        assert_eq!(detect_language(&dir("mixed", &["main.typ", "figure.tex"])), TemplateLanguage::Typst);
        assert_eq!(detect_language(&dir("empty", &[])), TemplateLanguage::Other);
    }

    #[test]
    fn the_gitignore_gets_the_build_folder_once() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(".gitignore");
        ensure_gitignore(directory.path()).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), ".build/\n");
        ensure_gitignore(directory.path()).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), ".build/\n");
        fs::write(&path, "*.aux").unwrap();
        ensure_gitignore(directory.path()).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "*.aux\n.build/\n");
    }

    #[test]
    fn a_new_project_needs_a_free_folder_and_a_plain_name() {
        let directory = tempfile::tempdir().unwrap();
        let location = directory.path();
        assert!(new_project_dir(location, " ").is_err());
        assert!(new_project_dir(location, "a/b").is_err());
        assert_eq!(new_project_dir(location, "Thesis").unwrap(), location.join("Thesis"));
        fs::create_dir_all(location.join("Taken")).unwrap();
        fs::write(location.join("Taken").join("main.tex"), "").unwrap();
        assert!(new_project_dir(location, "Taken").is_err());
        // An empty folder is fine, as a file dialog may have made it.
        fs::create_dir_all(location.join("Empty")).unwrap();
        assert!(new_project_dir(location, "Empty").is_ok());
    }

    #[test]
    fn templates_round_trip_through_the_library() {
        let directory = tempfile::tempdir().unwrap();
        let project = directory.path().join("project");
        fs::create_dir_all(&project).unwrap();
        fs::write(project.join("main.typ"), "= Hi").unwrap();
        let library = directory.path().join("library");
        let saved = save_template(&project, &library, "My Paper", "Two columns").unwrap();
        let again = save_template(&project, &library, "My Paper", "").unwrap();
        assert_ne!(saved, again);
        rename_template(&again, "Slides").unwrap();
        let own: Vec<Template> = load_templates(Some(&library)).into_iter().filter(|template| matches!(template.source, TemplateSource::Folder(_))).collect();
        assert_eq!(own.iter().map(|template| template.name.as_str()).collect::<Vec<_>>(), ["My Paper", "Slides"]);
        assert!(own.iter().all(|template| template.language == TemplateLanguage::Typst));
        assert_eq!(own[0].description, "Two columns");

        let target = directory.path().join("new");
        create_project(&own[0], &target).unwrap();
        assert_eq!(fs::read_to_string(target.join("main.typ")).unwrap(), "= Hi");
        assert!(!target.join(TEMPLATE_FILE).exists());
        assert!(target.join(".gitignore").is_file());
    }

    #[test]
    fn ages_read_like_a_person_would_say_them() {
        assert_eq!(ago(100, 90), "Just now");
        assert_eq!(ago(3600 * 5, 0), "5 hours ago");
        assert_eq!(ago(86_400, 0), "1 day ago");
        assert_eq!(ago(86_400 * 400, 0), "1 year ago");
    }
}
