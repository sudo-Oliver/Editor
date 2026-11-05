use std::{
    collections::HashSet,
    fs,
    io::{self, ErrorKind, Write},
    path::{Component, Path, PathBuf},
};

use ignore::WalkBuilder;

const COPY_NAME_LIMIT: usize = 999;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeEntry {
    pub path: PathBuf,
    pub name: String,
    pub depth: usize,
    pub is_dir: bool,
}

/// Lists the project, honoring `.gitignore`. Directories come before files, and
/// children of collapsed directories are left out.
pub fn scan(root: &Path, collapsed: &HashSet<PathBuf>) -> Vec<TreeEntry> {
    let mut directories: Vec<TreeEntry> = Vec::new();
    let walker = WalkBuilder::new(root)
        .hidden(false)
        .require_git(false)
        .filter_entry(|entry| {
            let name = entry.file_name();
            name != ".git"
                && name != ".build"
                && name != ".DS_Store"
                && !name.to_string_lossy().starts_with(crate::atomic::TEMP_PREFIX)
        })
        .build();
    for entry in walker.flatten() {
        if entry.depth() == 0 {
            continue;
        }
        directories.push(TreeEntry {
            path: entry.path().to_path_buf(),
            name: entry.file_name().to_string_lossy().into_owned(),
            depth: entry.depth() - 1,
            is_dir: entry.file_type().is_some_and(|kind| kind.is_dir()),
        });
    }
    directories.sort_by_cached_key(|entry| tree_key(entry, root));
    directories
        .into_iter()
        .filter(|entry| {
            !entry
                .path
                .ancestors()
                .skip(1)
                .any(|ancestor| collapsed.contains(ancestor))
        })
        .collect()
}

/// Sorts per path component, so that each directory sits above its own children
/// and above files on the same level.
fn tree_key(entry: &TreeEntry, root: &Path) -> Vec<(bool, String)> {
    let relative = entry.path.strip_prefix(root).unwrap_or(&entry.path);
    let count = relative.components().count();
    relative
        .components()
        .enumerate()
        .map(|(index, component)| {
            let is_file = index + 1 == count && !entry.is_dir;
            (is_file, component.as_os_str().to_string_lossy().to_lowercase())
        })
        .collect()
}

/// The folder that a new entry goes to: the selected folder, or the folder of the
/// selected file, or else the project.
pub fn creation_folder(selected: Option<&Path>, root: &Path) -> PathBuf {
    let Some(selected) = selected.filter(|path| path.starts_with(root)) else {
        return root.to_path_buf();
    };
    if selected.is_dir() {
        return selected.to_path_buf();
    }
    selected.parent().filter(|parent| parent.starts_with(root)).unwrap_or(root).to_path_buf()
}

/// Turns a typed name into a path below `parent`. `parts/intro.tex` is fine, but a
/// name cannot leave the folder.
pub fn new_entry_path(parent: &Path, name: &str) -> Result<PathBuf, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("Enter a name.".to_string());
    }
    if !Path::new(name).components().all(|component| matches!(component, Component::Normal(_))) {
        return Err("Use a name inside this folder.".to_string());
    }
    Ok(parent.join(name))
}

fn display_name(path: &Path) -> String {
    path.file_name().map_or_else(|| path.display().to_string(), |name| name.to_string_lossy().into_owned())
}

/// Never replaces anything that is already there.
pub fn create_file(path: &Path) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    match fs::OpenOptions::new().write(true).create_new(true).open(path) {
        Ok(_) => Ok(()),
        Err(error) if error.kind() == ErrorKind::AlreadyExists => Err(format!("{} already exists.", display_name(path))),
        Err(error) => Err(error.to_string()),
    }
}

pub fn create_folder(path: &Path) -> Result<(), String> {
    if path.exists() {
        return Err(format!("{} already exists.", display_name(path)));
    }
    fs::create_dir_all(path).map_err(|error| error.to_string())
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct CopyReport {
    pub copied: usize,
    /// Names that the folder already has, which stay as they are.
    pub existing: Vec<String>,
    /// Names with the reason they could not be copied.
    pub failed: Vec<String>,
}

fn copy_new(source: &Path, target: &Path) -> io::Result<()> {
    let mut from = fs::File::open(source)?;
    let mut to = fs::OpenOptions::new().write(true).create_new(true).open(target)?;
    io::copy(&mut from, &mut to).map(|_| ()).inspect_err(|_| {
        // A partial copy must not look like the file.
        fs::remove_file(target).ok();
    })
}

/// Copies files into `folder` and keeps whatever already has that name.
pub fn copy_into(folder: &Path, sources: &[PathBuf]) -> CopyReport {
    let mut report = CopyReport::default();
    for source in sources {
        let name = display_name(source);
        match copy_new(source, &folder.join(&name)) {
            Ok(()) => report.copied += 1,
            Err(error) if error.kind() == ErrorKind::AlreadyExists => report.existing.push(name),
            Err(error) => report.failed.push(format!("{name}: {error}")),
        }
    }
    report
}

/// `a.tex` becomes `a-copy.tex`, then `a-copy-2.tex`. No blank, as LaTeX can fail on one.
fn copy_name(path: &Path, attempt: usize) -> PathBuf {
    let stem = path.file_stem().map_or_else(String::new, |stem| stem.to_string_lossy().into_owned());
    let number = if attempt > 1 { format!("-{attempt}") } else { String::new() };
    let name = match path.extension() {
        Some(extension) => format!("{stem}-copy{number}.{}", extension.to_string_lossy()),
        None => format!("{stem}-copy{number}"),
    };
    path.with_file_name(name)
}

/// Creates a copy beside `source`, from `text` when given and else from the file.
/// An existing file never gets replaced, and a failed copy leaves nothing behind.
pub fn make_copy(source: &Path, text: Option<&str>) -> io::Result<PathBuf> {
    for attempt in 1..=COPY_NAME_LIMIT {
        let target = copy_name(source, attempt);
        let mut file = match fs::OpenOptions::new().write(true).create_new(true).open(&target) {
            Ok(file) => file,
            Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        };
        let written = match text {
            Some(text) => file.write_all(text.as_bytes()),
            None => fs::File::open(source).and_then(|mut input| io::copy(&mut input, &mut file)).map(|_| ()),
        };
        if let Err(error) = written.and_then(|()| file.sync_all()) {
            fs::remove_file(&target).ok();
            return Err(error);
        }
        return Ok(target);
    }
    Err(io::Error::new(ErrorKind::AlreadyExists, "too many copies of this file"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copies_count_up_and_keep_the_original() {
        let folder = tempfile::tempdir().unwrap();
        let source = folder.path().join("a.tex");
        fs::write(&source, "disk").unwrap();
        assert_eq!(make_copy(&source, Some("buffer")).unwrap(), folder.path().join("a-copy.tex"));
        assert_eq!(make_copy(&source, None).unwrap(), folder.path().join("a-copy-2.tex"));
        assert_eq!(fs::read_to_string(folder.path().join("a-copy.tex")).unwrap(), "buffer");
        assert_eq!(fs::read_to_string(folder.path().join("a-copy-2.tex")).unwrap(), "disk");
        assert_eq!(fs::read_to_string(&source).unwrap(), "disk");
    }

    #[test]
    fn a_copy_name_handles_files_without_an_extension() {
        assert_eq!(copy_name(Path::new("/p/Makefile"), 1), Path::new("/p/Makefile-copy"));
        assert_eq!(copy_name(Path::new("/p/.env"), 2), Path::new("/p/.env-copy-2"));
        assert_eq!(copy_name(Path::new("/p/a.b.tex"), 3), Path::new("/p/a.b-copy-3.tex"));
    }


    #[test]
    fn new_entries_go_to_the_selected_folder() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir(root.join("parts")).unwrap();
        fs::write(root.join("parts/a.tex"), "").unwrap();
        assert_eq!(creation_folder(None, root), root);
        assert_eq!(creation_folder(Some(&root.join("parts")), root), root.join("parts"));
        assert_eq!(creation_folder(Some(&root.join("parts/a.tex")), root), root.join("parts"));
        assert_eq!(creation_folder(Some(Path::new("/elsewhere")), root), root);
    }

    #[test]
    fn a_name_cannot_leave_the_folder() {
        let parent = Path::new("/project");
        assert_eq!(new_entry_path(parent, " a.tex "), Ok(parent.join("a.tex")));
        assert_eq!(new_entry_path(parent, "parts/a.tex"), Ok(parent.join("parts/a.tex")));
        for name in ["", "  ", "../a.tex", "parts/../../a.tex", "/etc/a.tex", "."] {
            assert!(new_entry_path(parent, name).is_err(), "{name}");
        }
    }

    #[test]
    fn creating_keeps_what_exists() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("parts/a.tex");
        create_file(&file).unwrap();
        fs::write(&file, "text").unwrap();
        assert_eq!(create_file(&file), Err("a.tex already exists.".to_string()));
        assert_eq!(fs::read_to_string(&file).unwrap(), "text");
        assert!(create_folder(&dir.path().join("parts")).is_err());
        create_folder(&dir.path().join("figures/plots")).unwrap();
        assert!(dir.path().join("figures/plots").is_dir());
    }

    #[test]
    fn copying_keeps_what_exists_and_reports_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        let (from, to) = (dir.path().join("from"), dir.path().join("to"));
        fs::create_dir_all(&from).unwrap();
        fs::create_dir_all(&to).unwrap();
        fs::write(from.join("new.bib"), "new").unwrap();
        fs::write(from.join("old.bib"), "new").unwrap();
        fs::write(to.join("old.bib"), "old").unwrap();
        let report = copy_into(&to, &[from.join("new.bib"), from.join("old.bib"), from.join("gone.bib")]);
        assert_eq!(report.copied, 1);
        assert_eq!(report.existing, ["old.bib"]);
        assert_eq!(report.failed.len(), 1);
        assert!(report.failed[0].starts_with("gone.bib: "));
        assert_eq!(fs::read_to_string(to.join("new.bib")).unwrap(), "new");
        assert_eq!(fs::read_to_string(to.join("old.bib")).unwrap(), "old");
        assert!(!to.join("gone.bib").exists());
    }

    #[test]
    fn sorts_directories_first_and_hides_collapsed_children() {
        let root = std::env::temp_dir().join(format!("tree-test-{}", std::process::id()));
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::create_dir_all(root.join(".build")).unwrap();
        std::fs::write(root.join("a.tex"), "").unwrap();
        std::fs::write(root.join("sub/b.tex"), "").unwrap();
        std::fs::write(root.join(".build/c.log"), "").unwrap();

        let names = |collapsed: &HashSet<PathBuf>| -> Vec<String> {
            scan(&root, collapsed).into_iter().map(|entry| entry.name).collect()
        };
        assert_eq!(names(&HashSet::new()), vec!["sub", "b.tex", "a.tex"]);
        assert_eq!(names(&HashSet::from([root.join("sub")])), vec!["sub", "a.tex"]);
        std::fs::remove_dir_all(&root).unwrap();
    }
}
