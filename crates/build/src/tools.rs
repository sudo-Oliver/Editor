use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    sync::RwLock,
};

#[cfg(not(windows))]
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
    directories.extend(extra_directories());
    directories.extend(ADDED_DIRECTORIES.read().map(|added| added.clone()).unwrap_or_default());
    directories
}

/// The usual install places, for apps started without the shell's `PATH`.
#[cfg(not(windows))]
fn extra_directories() -> Vec<PathBuf> {
    let mut directories: Vec<PathBuf> = EXTRA_DIRECTORIES.iter().map(PathBuf::from).collect();
    if let Some(home) = std::env::var_os("HOME") {
        directories.push(Path::new(&home).join(".cargo/bin"));
    }
    directories
}

/// The usual install places on Windows, which installers do not always add
/// to the `PATH` an app started from the Start menu sees: TeX Live, MiKTeX
/// for everyone or for one user, and Git.
#[cfg(windows)]
fn extra_directories() -> Vec<PathBuf> {
    let variable = |name: &str| std::env::var_os(name).map(PathBuf::from);
    let mut directories = Vec::new();
    // TeX Live puts each year in its own folder; the newest wins. Read once,
    // since every search for a program comes here.
    static TEXLIVE: std::sync::OnceLock<Vec<PathBuf>> = std::sync::OnceLock::new();
    directories.extend(TEXLIVE.get_or_init(|| {
        let mut years: Vec<PathBuf> = std::fs::read_dir("C:\\texlive").into_iter().flatten().flatten().map(|entry| entry.path()).filter(|path| path.join("bin\\windows").is_dir()).collect();
        years.sort();
        years.into_iter().rev().map(|year| year.join("bin\\windows")).collect()
    }).iter().cloned());
    for base in [variable("ProgramFiles"), variable("LOCALAPPDATA").map(|local| local.join("Programs"))].into_iter().flatten() {
        directories.push(base.join("MiKTeX\\miktex\\bin\\x64"));
        directories.push(base.join("Git\\cmd"));
        directories.push(base.join("Pandoc"));
    }
    if let Some(local) = variable("LOCALAPPDATA") {
        // Where winget links what it installs for one user, and Pandoc's own installer.
        directories.push(local.join("Microsoft\\WinGet\\Links"));
        directories.push(local.join("Pandoc"));
    }
    if let Some(profile) = variable("USERPROFILE") {
        directories.push(profile.join(".cargo\\bin"));
    }
    // What installers added to the PATH since the app started, such as winget,
    // which puts a program's own folder there when it cannot make a link.
    directories.extend(registry_path());
    directories
}

/// The `PATH` of this user and of the system as the registry has it now, not
/// as it was when the app started.
#[cfg(windows)]
fn registry_path() -> Vec<PathBuf> {
    use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_EXPAND_SZ, RRF_RT_REG_SZ, RegGetValueW};
    let wide = |text: &str| text.encode_utf16().chain([0]).collect::<Vec<u16>>();
    let value = wide("Path");
    let mut directories = Vec::new();
    for (key, subkey) in [(HKEY_CURRENT_USER, "Environment"), (HKEY_LOCAL_MACHINE, "SYSTEM\\CurrentControlSet\\Control\\Session Manager\\Environment")] {
        let subkey = wide(subkey);
        let mut buffer = vec![0u16; 16_384];
        let mut bytes = (buffer.len() * 2) as u32;
        // SAFETY: both names end in a zero, and `bytes` is the size of `buffer`.
        // A REG_EXPAND_SZ value comes back with its %VARIABLES% expanded.
        let status = unsafe {
            RegGetValueW(key, subkey.as_ptr(), value.as_ptr(), RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ, std::ptr::null_mut(), buffer.as_mut_ptr().cast(), &mut bytes)
        };
        if status != 0 {
            continue;
        }
        // An expanded value can be shorter than the size Windows reports.
        let filled = &buffer[..(bytes as usize / 2).min(buffer.len())];
        let text = String::from_utf16_lossy(&filled[..filled.iter().position(|&unit| unit == 0).unwrap_or(filled.len())]);
        directories.extend(text.split(';').map(str::trim).filter(|part| !part.is_empty()).map(PathBuf::from));
    }
    directories
}

/// Turns `\\?\C:\x` into `C:\x`, and `\\?\UNC\server\x` into `\\server\x`.
/// `canonicalize` gives the long form on Windows, which TeX, Typst and Pandoc
/// refuse and which never equals a path as typed. Elsewhere it does nothing.
pub fn plain_path(path: PathBuf) -> PathBuf {
    let Some(text) = path.to_str() else {
        return path;
    };
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    match text.strip_prefix(r"\\?\") {
        // Only a drive path; others need the long form.
        Some(rest) if rest.as_bytes().get(1) == Some(&b':') => PathBuf::from(rest),
        _ => path,
    }
}

/// Debug builds only: `EDITOR_PRETEND_FRESH=1` hides the tools of this Mac,
/// to try the setup as on a new one.
pub fn pretend_fresh() -> bool {
    cfg!(debug_assertions) && std::env::var_os("EDITOR_PRETEND_FRESH").is_some()
}

/// Searches `directory` too, after everything else.
pub fn add_directory(directory: PathBuf) {
    if let Ok(mut added) = ADDED_DIRECTORIES.write()
        && !added.contains(&directory) {
            added.push(directory);
        }
}

/// Stops searching `directory`, for example after its tool was removed.
pub fn remove_directory(directory: &Path) {
    if let Ok(mut added) = ADDED_DIRECTORIES.write() {
        added.retain(|known| known != directory);
    }
}

/// Finds a program by its name. On Windows, `pdflatex` is `pdflatex.exe`, and
/// some programs are batch files, as `PATHEXT` lists them.
pub fn find_tool(name: &str) -> Option<PathBuf> {
    let names = program_names(name);
    search_path().into_iter().find_map(|directory| names.iter().map(|name| directory.join(name)).find(|path| path.is_file()))
}

#[cfg(not(windows))]
fn program_names(name: &str) -> Vec<String> {
    vec![name.to_string()]
}

#[cfg(windows)]
fn program_names(name: &str) -> Vec<String> {
    if Path::new(name).extension().is_some() {
        return vec![name.to_string()];
    }
    let extensions = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_string());
    extensions.split(';').filter(|extension| !extension.is_empty()).map(|extension| format!("{name}{}", extension.to_ascii_lowercase())).collect()
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
        // A program on Windows has an extension, which find_tool adds.
        let file = format!("{name}{}", std::env::consts::EXE_SUFFIX);
        std::fs::write(directory.path().join(&file), "").unwrap();
        assert_eq!(find_tool(name), None);

        add_directory(directory.path().to_path_buf());
        add_directory(directory.path().to_path_buf());
        assert_eq!(find_tool(name), Some(directory.path().join(&file)));
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

#[cfg(test)]
mod plain_tests {
    use super::*;

    #[test]
    fn long_windows_paths_become_plain() {
        assert_eq!(plain_path(PathBuf::from(r"\\?\C:\Users\Ada\Thesis")), PathBuf::from(r"C:\Users\Ada\Thesis"));
        assert_eq!(plain_path(PathBuf::from(r"\\?\UNC\server\share\x")), PathBuf::from(r"\\server\share\x"));
        assert_eq!(plain_path(PathBuf::from("/Users/ada/Thesis")), PathBuf::from("/Users/ada/Thesis"));
    }
}
