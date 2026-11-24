//! What differs between macOS and Windows, in one place, so that the rest of
//! the app reads the same on both.

use std::path::{Path, PathBuf};

/// The user's home folder: `HOME` on macOS, the profile folder on Windows.
pub fn home() -> Option<PathBuf> {
    #[cfg(windows)]
    let home = std::env::var_os("USERPROFILE");
    #[cfg(not(windows))]
    let home = std::env::var_os("HOME");
    home.filter(|home| !home.is_empty()).map(PathBuf::from)
}

/// A command for `program` that opens no console window on Windows, where
/// every console program would otherwise flash one up.
pub fn command(program: impl AsRef<std::ffi::OsStr>) -> std::process::Command {
    let mut command = std::process::Command::new(program);
    build::no_window(&mut command);
    command
}

/// How the system's file manager is called, for "Show in Finder" and the like.
pub const FILE_MANAGER: &str = if cfg!(windows) { "Explorer" } else { "Finder" };

/// How people call this kind of computer, as in "on this Mac".
pub const COMPUTER: &str = if cfg!(windows) { "PC" } else { "Mac" };

/// Megabytes free on the disk that holds `path`.
pub fn free_megabytes(path: &Path) -> Option<u64> {
    imp::free_bytes(path).map(|bytes| bytes / 1_000_000)
}

/// The system and its version, such as `macOS 15.1 (aarch64)` or
/// `Windows 10.0.22631 (x86_64)`. Read once, as it starts a program.
pub fn system() -> String {
    static SYSTEM: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    SYSTEM
        .get_or_init(|| {
            let arch = std::env::consts::ARCH;
            match imp::version() {
                Some(version) => format!("{} {version} ({arch})", imp::NAME),
                None => format!("{} ({arch})", imp::NAME),
            }
        })
        .clone()
}

/// Turns `\\?\C:\x` into `C:\x`. `canonicalize` gives the long form on Windows,
/// which TeX, Typst and Pandoc refuse and which never equals a path as typed.
pub fn plain_path(path: PathBuf) -> PathBuf {
    build::plain_path(path)
}

#[cfg(not(windows))]
mod imp {
    use std::{os::unix::ffi::OsStrExt, path::Path, process::Command};

    pub const NAME: &str = if cfg!(target_os = "macos") { "macOS" } else { "Linux" };

    pub fn free_bytes(path: &Path) -> Option<u64> {
        let path = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
        let mut stats: libc::statvfs = unsafe { std::mem::zeroed() };
        // SAFETY: statvfs fills the struct it is given from a valid C string.
        if unsafe { libc::statvfs(path.as_ptr(), &mut stats) } != 0 {
            return None;
        }
        Some(stats.f_bavail as u64 * stats.f_frsize as u64)
    }

    pub fn version() -> Option<String> {
        if !cfg!(target_os = "macos") {
            return None;
        }
        let output = Command::new("sw_vers").arg("-productVersion").output().ok()?;
        Some(String::from_utf8_lossy(&output.stdout).trim().to_string()).filter(|version| !version.is_empty())
    }
}

#[cfg(windows)]
mod imp {
    use std::{os::windows::ffi::OsStrExt, path::Path, process::Command};

    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;

    pub const NAME: &str = "Windows";

    pub fn free_bytes(path: &Path) -> Option<u64> {
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let mut free = 0u64;
        // SAFETY: a nul-terminated path and a valid out pointer; the others may be null.
        let read = unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut free, std::ptr::null_mut(), std::ptr::null_mut()) };
        (read != 0).then_some(free)
    }

    /// `ver` says `Microsoft Windows [Version 10.0.22631.4602]`.
    pub fn version() -> Option<String> {
        let mut command = Command::new("cmd");
        command.args(["/C", "ver"]);
        let output = build::no_window(&mut command).output().ok()?;
        let text = String::from_utf8_lossy(&output.stdout);
        let start = text.find("Version ")? + "Version ".len();
        let version: String = text[start..].chars().take_while(|ch| ch.is_ascii_digit() || *ch == '.').collect();
        (!version.is_empty()).then_some(version)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_system_is_named_and_the_home_has_room() {
        assert!(system().contains(std::env::consts::ARCH));
        let home = home().unwrap();
        assert!(free_megabytes(&home).is_some_and(|free| free > 0));
    }
}
