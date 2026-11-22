//! Where the editor keeps its own files, and how much they take. Everything
//! lives in two folders named after the bundle id, as macOS expects, so that
//! cleanup tools and uninstallers find all of it in one place:
//!
//! - `~/Library/Application Support/io.github.sudo-oliver.editor`: settings,
//!   file history, crash recovery copies, installed tools, add-ons, templates.
//! - `~/Library/Caches/io.github.sudo-oliver.editor`: downloads and caches,
//!   which may disappear at any time and come back when needed.
//!
//! Nothing grows without a limit, and the settings show and clear each part.

use std::{
    fs, io,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

pub const BUNDLE_ID: &str = "io.github.sudo-oliver.editor";
/// A download nobody continued for this long is not coming back.
const STALE_DOWNLOAD: Duration = Duration::from_secs(14 * 86_400);

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// `EDITOR_HOME` puts everything in one folder, for tests and for trying things out.
fn override_dir(part: &str) -> Option<PathBuf> {
    std::env::var_os("EDITOR_HOME").map(|home| PathBuf::from(home).join(part))
}

/// What belongs to the user and must stay.
pub fn data_dir() -> Option<PathBuf> {
    if let Some(dir) = override_dir("data") {
        return Some(dir);
    }
    if cfg!(target_os = "macos") {
        return Some(home()?.join("Library/Application Support").join(BUNDLE_ID));
    }
    let config = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).or_else(|| Some(home()?.join(".config")))?;
    Some(config.join("editor"))
}

/// What can be made or fetched again.
pub fn cache_dir() -> Option<PathBuf> {
    if let Some(dir) = override_dir("cache") {
        return Some(dir);
    }
    if cfg!(target_os = "macos") {
        return Some(home()?.join("Library/Caches").join(BUNDLE_ID));
    }
    let cache = std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from).or_else(|| Some(home()?.join(".cache")))?;
    Some(cache.join("editor"))
}

/// Package downloads, kept between tries so that a second try continues.
pub fn downloads_dir() -> Option<PathBuf> {
    Some(cache_dir()?.join("downloads"))
}

/// Moves the folders of earlier versions, `~/.config/editor` and
/// `~/.cache/editor`, to their places. A rename on the same disk is instant,
/// and anything already at the new place stays.
pub fn migrate() {
    if !cfg!(target_os = "macos") || std::env::var_os("EDITOR_HOME").is_some() {
        return;
    }
    let Some(home) = home() else {
        return;
    };
    let config = std::env::var_os("XDG_CONFIG_HOME").map_or_else(|| home.join(".config"), PathBuf::from).join("editor");
    let cache = std::env::var_os("XDG_CACHE_HOME").map_or_else(|| home.join(".cache"), PathBuf::from).join("editor");
    if let Some(data) = data_dir() {
        // The old tools folder held the package downloads too.
        if let Some(downloads) = downloads_dir() {
            move_into(&config.join("tools/downloads"), &downloads);
        }
        move_into(&config, &data);
    }
    if let Some(target) = cache_dir() {
        move_into(&cache, &target);
    }
}

/// Moves `from` to `to`, or its entries one by one when `to` exists. Entries
/// that exist on both sides stay where they are.
fn move_into(from: &Path, to: &Path) {
    if !from.is_dir() {
        return;
    }
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent).ok();
    }
    if fs::rename(from, to).is_ok() {
        return;
    }
    let Ok(entries) = fs::read_dir(from) else {
        return;
    };
    for entry in entries.flatten() {
        let target = to.join(entry.file_name());
        if !target.exists() {
            fs::create_dir_all(to).ok();
            fs::rename(entry.path(), &target).ok();
        }
    }
    // Only goes away when everything moved.
    fs::remove_dir(from).ok();
}

/// Removes what a crash or force quit left behind: half-finished tool
/// downloads and unpacks, half-copied add-ons, and package downloads nobody
/// continued for two weeks. Runs once per start, away from typing.
pub fn sweep() {
    if let Some(tools) = data_dir().map(|data| data.join("tools")) {
        remove_matching(&tools, |name| name.starts_with(".download-") || name.starts_with(".unpack-"));
    }
    if let Some(addons) = data_dir().map(|data| data.join("addons")) {
        remove_matching(&addons, |name| name.starts_with(".installing-"));
    }
    if let Some(downloads) = downloads_dir()
        && let Ok(entries) = fs::read_dir(&downloads)
    {
        for entry in entries.flatten() {
            let stale = entry.metadata().and_then(|metadata| metadata.modified()).is_ok_and(|modified| {
                SystemTime::now().duration_since(modified).is_ok_and(|age| age > STALE_DOWNLOAD)
            });
            if stale {
                fs::remove_file(entry.path()).ok();
            }
        }
    }
    if let Some(history) = crate::history::directory() {
        crate::history::sweep(&history);
    }
}

fn remove_matching(folder: &Path, matches: impl Fn(&str) -> bool) {
    let Ok(entries) = fs::read_dir(folder) else {
        return;
    };
    for entry in entries.flatten() {
        if matches(&entry.file_name().to_string_lossy()) {
            let path = entry.path();
            if path.is_dir() { fs::remove_dir_all(&path).ok() } else { fs::remove_file(&path).ok() };
        }
    }
}

/// The bytes the files under `path` take, not following links.
pub fn size_of(path: &Path) -> u64 {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return 0;
    };
    if !metadata.is_dir() {
        return metadata.len();
    }
    fs::read_dir(path).into_iter().flatten().flatten().map(|entry| size_of(&entry.path())).sum()
}

/// Empties a folder the app can make again. A missing folder is already empty.
pub fn clear(path: &Path) -> io::Result<()> {
    match fs::remove_dir_all(path) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

/// What each part of the two folders takes, for the storage settings.
#[derive(Clone, Default)]
pub struct Usage {
    pub history: u64,
    pub recovery: u64,
    pub tools: u64,
    pub addons: u64,
    pub templates: u64,
    pub downloads: u64,
    /// The cache folder but for the downloads.
    pub caches: u64,
    /// Both folders, also what no part names, such as the settings.
    pub total: u64,
    /// The build output of the open project.
    pub build: Option<u64>,
}

/// Walks all the folders, which takes a while for gigabytes of tools, so
/// never on the UI thread.
pub fn measure(build: Option<&Path>) -> Usage {
    let size = |path: Option<PathBuf>| path.as_deref().map_or(0, size_of);
    let downloads = size(downloads_dir());
    Usage {
        history: crate::history::directory().as_deref().map_or(0, crate::history::size),
        recovery: size(crate::recovery::directory()),
        tools: size(crate::toolchain::tools_directory()),
        addons: size(crate::addons::default_directory()),
        templates: size(crate::projects::library_path()),
        downloads,
        caches: size(cache_dir()).saturating_sub(downloads),
        total: size(data_dir()) + size(cache_dir()),
        build: build.map(size_of),
    }
}

/// Empties the cache folder but for the downloads, which are cleared on their own.
pub fn clear_caches() -> io::Result<()> {
    cache_dir().map_or(Ok(()), |cache| clear_but_downloads(&cache))
}

fn clear_but_downloads(cache: &Path) -> io::Result<()> {
    let Ok(entries) = fs::read_dir(cache) else {
        return Ok(());
    };
    for entry in entries.flatten() {
        if entry.file_name() == "downloads" {
            continue;
        }
        if entry.file_type()?.is_dir() {
            clear(&entry.path())?;
        } else if let Err(error) = fs::remove_file(entry.path())
            && error.kind() != io::ErrorKind::NotFound
        {
            return Err(error);
        }
    }
    Ok(())
}

/// Short sizes for people: "0 KB", "820 KB", "12.4 MB", "6.9 GB".
pub fn human(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    let bytes = bytes as f64;
    if bytes < KB * KB {
        format!("{:.0} KB", (bytes / KB).ceil())
    } else if bytes < KB * KB * KB {
        format!("{:.1} MB", bytes / KB / KB)
    } else {
        format!("{:.1} GB", bytes / KB / KB / KB)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moving_keeps_what_is_there_and_takes_the_rest() {
        let root = tempfile::tempdir().unwrap();
        let (from, to) = (root.path().join("old"), root.path().join("new"));
        fs::create_dir_all(from.join("history")).unwrap();
        fs::write(from.join("settings.toml"), "old").unwrap();
        fs::write(from.join("history/a"), "a").unwrap();
        fs::create_dir_all(&to).unwrap();
        fs::write(to.join("settings.toml"), "new").unwrap();
        move_into(&from, &to);
        assert_eq!(fs::read_to_string(to.join("settings.toml")).unwrap(), "new");
        assert_eq!(fs::read_to_string(to.join("history/a")).unwrap(), "a");
        // The old settings had nowhere to go, so the old folder stays with them.
        assert!(from.join("settings.toml").exists());
    }

    #[test]
    fn moving_to_a_new_place_is_one_rename() {
        let root = tempfile::tempdir().unwrap();
        let (from, to) = (root.path().join("old"), root.path().join("a/b/new"));
        fs::create_dir_all(&from).unwrap();
        fs::write(from.join("x"), "x").unwrap();
        move_into(&from, &to);
        assert!(!from.exists());
        assert_eq!(fs::read_to_string(to.join("x")).unwrap(), "x");
        assert_eq!(size_of(&to), 1);
    }

    #[test]
    fn clearing_caches_keeps_the_downloads() {
        let cache = tempfile::tempdir().unwrap();
        fs::create_dir_all(cache.path().join("downloads")).unwrap();
        fs::create_dir_all(cache.path().join("other")).unwrap();
        fs::write(cache.path().join("downloads/MacTeX.pkg"), "half").unwrap();
        fs::write(cache.path().join("other/x"), "x").unwrap();
        fs::write(cache.path().join("themes.json"), "{}").unwrap();
        clear_but_downloads(cache.path()).unwrap();
        assert_eq!(size_of(cache.path()), 4);
        assert!(!cache.path().join("other").exists());
        assert!(!cache.path().join("themes.json").exists());
        clear(&cache.path().join("downloads")).unwrap();
        clear(&cache.path().join("downloads")).unwrap();
        assert_eq!(size_of(cache.path()), 0);
    }

    #[test]
    fn sizes_read_well() {
        assert_eq!(human(0), "0 KB");
        assert_eq!(human(1), "1 KB");
        assert_eq!(human(5 * 1024 * 1024 + 300_000), "5.3 MB");
        assert_eq!(human(7_380_000_000), "6.9 GB");
    }
}
