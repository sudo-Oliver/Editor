use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    sync::RwLock,
};

const EXTRA_DIRECTORIES: &[&str] = &["/opt/homebrew/bin", "/usr/local/bin", "/Library/TeX/texbin"];

/// Folders of tools that the app installed itself. They come last, so that a
/// TeX distribution or a Typst the user installed wins.
static ADDED_DIRECTORIES: RwLock<Vec<PathBuf>> = RwLock::new(Vec::new());

/// Where the compilers the app starts keep what they download, with the
/// variables that tell them.
static CACHE_DIRECTORIES: RwLock<Vec<(&'static str, PathBuf)>> = RwLock::new(Vec::new());

/// Directories to search. Apps started from Finder miss the shell's `PATH`, so
/// the usual install locations are added.
fn search_path() -> Vec<PathBuf> {
    if pretend_fresh() {
        let mut directories: Vec<PathBuf> = ["/usr/bin", "/bin", "/usr/sbin", "/sbin"].into_iter().map(PathBuf::from).collect();
        directories.extend(ADDED_DIRECTORIES.read().map(|added| added.clone()).unwrap_or_default());
        return directories;
    }
    let mut directories: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default();
    directories.extend(EXTRA_DIRECTORIES.iter().map(PathBuf::from));
    if let Some(home) = std::env::var_os("HOME") {
        directories.push(Path::new(&home).join(".cargo/bin"));
    }
    directories.extend(ADDED_DIRECTORIES.read().map(|added| added.clone()).unwrap_or_default());
    directories
}

/// Debug builds only: `EDITOR_PRETEND_FRESH=1` hides the tools of this Mac,
/// to try the setup as on a new one.
pub fn pretend_fresh() -> bool {
    cfg!(debug_assertions) && std::env::var_os("EDITOR_PRETEND_FRESH").is_some()
}

/// Searches `directory` too, after everything else.
pub fn add_directory(directory: PathBuf) {
    if let Ok(mut added) = ADDED_DIRECTORIES.write() {
        if !added.contains(&directory) {
            added.push(directory);
        }
    }
}

/// Stops searching `directory`, for example after its tool was removed.
pub fn remove_directory(directory: &Path) {
    if let Ok(mut added) = ADDED_DIRECTORIES.write() {
        added.retain(|known| known != directory);
    }
}

pub fn find_tool(name: &str) -> Option<PathBuf> {
    search_path()
        .into_iter()
        .map(|directory| directory.join(name))
        .find(|path| path.is_file())
}

/// `PATH` for child processes, so that for example Pandoc finds Typst.
pub fn child_path() -> OsString {
    std::env::join_paths(search_path()).unwrap_or_default()
}

/// Makes the compilers the app starts keep their packages in `tectonic` and
/// `typst`, where the app can show and clear them, instead of in caches of
/// their own. What they downloaded before stays where it is.
pub fn set_cache_directories(tectonic: PathBuf, typst: PathBuf) {
    if let Ok(mut caches) = CACHE_DIRECTORIES.write() {
        *caches = vec![("TECTONIC_CACHE_DIR", tectonic), ("TYPST_PACKAGE_CACHE_PATH", typst)];
    }
}

/// The environment for child processes: `PATH`, and the cache folders once set.
pub fn child_env() -> Vec<(&'static str, OsString)> {
    let caches = CACHE_DIRECTORIES.read().map(|caches| caches.clone()).unwrap_or_default();
    let mut variables = vec![("PATH", child_path())];
    variables.extend(caches.into_iter().map(|(name, directory)| (name, directory.into_os_string())));
    variables
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn added_directories_come_last_and_can_go() {
        let directory = tempfile::tempdir().unwrap();
        let name = "editor-test-tool-that-exists-nowhere-else";
        std::fs::write(directory.path().join(name), "").unwrap();
        assert_eq!(find_tool(name), None);

        add_directory(directory.path().to_path_buf());
        add_directory(directory.path().to_path_buf());
        assert_eq!(find_tool(name), Some(directory.path().join(name)));
        let path = search_path();
        assert_eq!(path.last(), Some(&directory.path().to_path_buf()));
        assert_eq!(path.iter().filter(|entry| *entry == directory.path()).count(), 1);

        remove_directory(directory.path());
        assert_eq!(find_tool(name), None);
    }

    #[test]
    fn children_get_the_cache_folders() {
        assert!(child_env().iter().any(|(name, _)| *name == "PATH"));
        set_cache_directories(PathBuf::from("/caches/tectonic"), PathBuf::from("/caches/typst"));
        let variables = child_env();
        assert!(variables.contains(&("TECTONIC_CACHE_DIR", OsString::from("/caches/tectonic"))));
        assert!(variables.contains(&("TYPST_PACKAGE_CACHE_PATH", OsString::from("/caches/typst"))));
    }
}
