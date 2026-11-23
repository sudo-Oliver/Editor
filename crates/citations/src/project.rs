//! Which files make one document: the main file, everything it includes, and
//! the bibliographies they declare, found where LaTeX, BibTeX, biber or Typst
//! would find them.

use std::{
    collections::HashSet,
    ops::Range,
    path::{Component, Path, PathBuf},
    sync::Arc,
};

use crate::scan::{BibKind, IncludeKind, Scan};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Language {
    Latex,
    Typst,
}

impl Language {
    pub fn of(path: &Path) -> Option<Self> {
        match path.extension()?.to_str()? {
            "tex" | "ltx" | "sty" | "cls" => Some(Self::Latex),
            "typ" => Some(Self::Typst),
            _ => None,
        }
    }
}

/// Which program reads the bibliography, which decides what is a mistake.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Backend {
    /// BibTeX, for `\bibliography` and natbib, or biblatex with `backend=bibtex`.
    #[default]
    Bibtex,
    /// biblatex's default.
    Biber,
    /// Typst reads `.bib` and Hayagriva itself.
    Typst,
}

#[derive(Clone, Debug)]
pub struct Source {
    pub path: PathBuf,
    pub language: Language,
    pub scan: Arc<Scan>,
}

/// A bibliography a source declares, and the file it is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bibliography {
    /// The file that declares it, and the range of its name there.
    pub declared_in: PathBuf,
    pub range: Range<usize>,
    pub name: String,
    /// `None` when it is not on the disk, or not a file at all.
    pub path: Option<PathBuf>,
    /// A URL or a format other than BibTeX: nothing to read, nothing missing.
    pub elsewhere: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Project {
    pub main: Option<PathBuf>,
    /// The main file first, then what it includes, each once.
    pub sources: Vec<Source>,
    pub bibliographies: Vec<Bibliography>,
    /// No source declares a bibliography, so every `.bib` of the project counts.
    pub every_bib: bool,
    pub backend: Backend,
    pub biblatex: bool,
    pub cleveref: bool,
    /// `\externaldocument`: the `.aux` names whose labels count, with their prefix.
    pub external: Vec<(PathBuf, String)>,
}

/// What a project needs from outside: the scans of its files, whether a file
/// exists, and where TeX keeps a bibliography of the system, such as `IEEEabrv.bib`.
pub struct Env<'a> {
    pub root: &'a Path,
    pub scan: &'a mut dyn FnMut(&Path, Language) -> Option<Arc<Scan>>,
    pub exists: &'a dyn Fn(&Path) -> bool,
    pub system_bib: &'a dyn Fn(&str) -> Option<PathBuf>,
}

/// Follows the includes of `main`. `extra` are files that count even when
/// nothing includes them yet, such as the one being written.
pub fn build(main: Option<&Path>, extra: &[PathBuf], env: &mut Env) -> Project {
    let mut project = Project { main: main.map(Path::to_path_buf), ..Project::default() };
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let main_dir = main.and_then(Path::parent).unwrap_or(env.root).to_path_buf();
    let mut queue: Vec<PathBuf> = main.into_iter().map(Path::to_path_buf).chain(extra.iter().cloned()).collect();
    queue.reverse();
    while let Some(path) = queue.pop() {
        let path = normalize(&path);
        if !seen.insert(path.clone()) {
            continue;
        }
        let Some(language) = Language::of(&path) else {
            continue;
        };
        let Some(scan) = (env.scan)(&path, language) else {
            continue;
        };
        let file_dir = path.parent().unwrap_or(env.root).to_path_buf();
        // Depth first, in the order of the text.
        let mut found = Vec::new();
        for include in &scan.includes {
            if let Some(target) = resolve_include(&include.path, &include.kind, &main_dir, &file_dir, env) {
                found.push(target);
            }
        }
        queue.extend(found.into_iter().rev());
        for source in &scan.bibliographies {
            let elsewhere = source.remote || source.foreign;
            let bib = if elsewhere { None } else { resolve_bib(&source.name, source.kind, &main_dir, &file_dir, env) };
            project.bibliographies.push(Bibliography { declared_in: path.clone(), range: source.range.clone(), name: source.name.clone(), path: bib, elsewhere });
        }
        for package in &scan.packages {
            match package.name.as_str() {
                name if name == "biblatex" || name.starts_with("biblatex-") => {
                    project.biblatex = true;
                    let options: String = package.options.split_whitespace().collect();
                    project.backend = match options.split(',').find_map(|option| option.strip_prefix("backend=")) {
                        Some("bibtex" | "bibtex8" | "bibtexu") => Backend::Bibtex,
                        _ => Backend::Biber,
                    };
                }
                "cleveref" => project.cleveref = true,
                _ => {}
            }
        }
        for external in &scan.external_documents {
            project.external.push((main_dir.join(format!("{}.aux", external.name)), external.prefix.clone()));
        }
        if language == Language::Typst {
            project.backend = Backend::Typst;
        }
        project.sources.push(Source { path, language, scan });
    }
    project.every_bib = project.bibliographies.is_empty() && project.sources.iter().all(|source| source.scan.bibitems.is_empty());
    project
}

/// Where an include points, trying what TeX tries: the folder of the main
/// file first, as TeX runs there, then the folder of the including file.
fn resolve_include(name: &str, kind: &IncludeKind, main_dir: &Path, file_dir: &Path, env: &Env) -> Option<PathBuf> {
    let candidates: Vec<PathBuf> = match kind {
        IncludeKind::Input | IncludeKind::Subfile => [main_dir, file_dir].iter().flat_map(|dir| with_tex(&dir.join(name))).collect(),
        IncludeKind::Include => [main_dir, file_dir].iter().map(|dir| add_tex(&dir.join(name))).collect(),
        IncludeKind::Import { directory, relative } => {
            let base = if *relative { file_dir } else { main_dir };
            with_tex(&base.join(directory).join(name))
        }
        IncludeKind::Typst => vec![match name.strip_prefix('/') {
            Some(from_root) => env.root.join(from_root),
            None => file_dir.join(name),
        }],
    };
    candidates.into_iter().map(|path| normalize(&path)).find(|path| (env.exists)(path))
}

/// `name.tex`, then `name` as it is, as `\input` tries them.
fn with_tex(path: &Path) -> Vec<PathBuf> {
    if path.extension().is_some_and(|extension| extension == "tex") { vec![path.to_path_buf()] } else { vec![add_tex(path), path.to_path_buf()] }
}

fn add_tex(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".tex");
    PathBuf::from(name)
}

fn resolve_bib(name: &str, kind: BibKind, main_dir: &Path, file_dir: &Path, env: &Env) -> Option<PathBuf> {
    let file = match kind {
        BibKind::Bibliography if !name.ends_with(".bib") => format!("{name}.bib"),
        _ => name.to_string(),
    };
    if kind == BibKind::Typst {
        let path = match file.strip_prefix('/') {
            Some(from_root) => env.root.join(from_root),
            None => file_dir.join(&file),
        };
        let path = normalize(&path);
        return (env.exists)(&path).then_some(path);
    }
    if Path::new(&file).is_absolute() {
        let path = normalize(Path::new(&file));
        return (env.exists)(&path).then_some(path);
    }
    [main_dir, file_dir].iter().map(|dir| normalize(&dir.join(&file))).find(|path| (env.exists)(path)).or_else(|| (env.system_bib)(&file))
}

/// Takes out `.` and `..` without asking the disk, so that one file has one name.
pub fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other),
        }
    }
    out
}
