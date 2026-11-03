use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use ignore::WalkBuilder;

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
            name != ".git" && name != ".build" && name != ".DS_Store"
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

#[cfg(test)]
mod tests {
    use super::*;

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
