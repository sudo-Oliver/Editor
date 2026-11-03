use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

const EXTRA_DIRECTORIES: &[&str] = &["/opt/homebrew/bin", "/usr/local/bin", "/Library/TeX/texbin"];

/// Directories to search. Apps started from Finder miss the shell's `PATH`, so
/// the usual install locations are added.
fn search_path() -> Vec<PathBuf> {
    let mut directories: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default();
    directories.extend(EXTRA_DIRECTORIES.iter().map(PathBuf::from));
    if let Some(home) = std::env::var_os("HOME") {
        directories.push(Path::new(&home).join(".cargo/bin"));
    }
    directories
}

pub fn find_tool(name: &str) -> Option<PathBuf> {
    search_path()
        .into_iter()
        .map(|directory| directory.join(name))
        .find(|path| path.is_file())
}

/// `PATH` for child processes, so that for example Pandoc finds Typst.
pub(crate) fn child_path() -> OsString {
    std::env::join_paths(search_path()).unwrap_or_default()
}
