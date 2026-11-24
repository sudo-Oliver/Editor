//! The `editor` command for the terminal, like `code` for VS Code. The script
//! ships inside the app bundle, and installing it links it into a folder that
//! the shell searches. Homebrew's cask links it the same way.

use std::{
    io::ErrorKind,
    path::{Path, PathBuf},
    process::Command,
};

const NAME: &str = "editor";

/// Folders on the default `PATH` of macOS or of Homebrew, in the order tried.
const FOLDERS: &[&str] = &["/opt/homebrew/bin", "/usr/local/bin"];

/// The script inside the running app. Builds made with cargo have none.
pub fn script() -> Option<PathBuf> {
    let binary = std::env::current_exe().ok()?;
    binary.parent()?.join("../Resources/bin").join(NAME).canonicalize().ok()
}

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Installed(PathBuf),
    AlreadyThere(PathBuf),
    /// No folder could be written without an administrator.
    NeedsAdmin,
}

/// Whether `link` is a command from some copy of this app, which may be
/// replaced, for example after the app moved.
fn ours(link: &Path) -> bool {
    std::fs::read_link(link).is_ok_and(|target| target.ends_with(Path::new("Contents/Resources/bin").join(NAME)))
}

/// Links `script` into the first of `folders` that exists and can be written.
pub fn install_into(script: &Path, folders: &[PathBuf]) -> Result<Outcome, String> {
    for folder in folders.iter().filter(|folder| folder.is_dir()) {
        let link = folder.join(NAME);
        if link.symlink_metadata().is_ok() {
            if link.canonicalize().is_ok_and(|target| target == script) {
                return Ok(Outcome::AlreadyThere(link));
            }
            if !ours(&link) {
                return Err(format!("{} already exists and belongs to another program, so it was left alone.", link.display()));
            }
            match std::fs::remove_file(&link) {
                Err(error) if error.kind() == ErrorKind::PermissionDenied => continue,
                Err(error) => return Err(format!("Cannot replace {}: {error}", link.display())),
                Ok(()) => {}
            }
        }
        // Windows gets the command from its installer instead.
        #[cfg(not(unix))]
        return Err(format!("Cannot link {} here.", link.display()));
        #[cfg(unix)]
        match std::os::unix::fs::symlink(script, &link) {
            Ok(()) => return Ok(Outcome::Installed(link)),
            Err(error) if error.kind() == ErrorKind::PermissionDenied => continue,
            Err(error) => return Err(format!("Cannot create {}: {error}", link.display())),
        }
    }
    Ok(Outcome::NeedsAdmin)
}

/// Installs the command, asking macOS for an administrator only when no
/// folder on the `PATH` can be written otherwise. Blocks.
pub fn install() -> Result<Outcome, String> {
    let script = script().ok_or_else(|| "Only the installed app has the editor command. Install Editor from its dmg first.".to_string())?;
    let folders: Vec<PathBuf> = FOLDERS.iter().map(PathBuf::from).collect();
    match install_into(&script, &folders)? {
        Outcome::NeedsAdmin => {}
        outcome => return Ok(outcome),
    }
    // The path goes in as an argument, so that nothing in it can change the command.
    let status = Command::new("osascript")
        .args([
            "-e",
            "on run argv",
            "-e",
            "do shell script \"mkdir -p /usr/local/bin && ln -sf \" & quoted form of item 1 of argv & \" /usr/local/bin/editor\" with prompt \"Editor wants to add the editor command for the Terminal.\" with administrator privileges",
            "-e",
            "end run",
        ])
        .arg(&script)
        .status()
        .map_err(|error| format!("Cannot ask for the password: {error}"))?;
    if status.success() {
        Ok(Outcome::Installed(PathBuf::from("/usr/local/bin").join(NAME)))
    } else {
        Err("The editor command was not added.".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(unix)]
    fn the_command_goes_into_the_first_folder_that_works() {
        let directory = tempfile::tempdir().unwrap();
        let app = directory.path().join("Editor.app/Contents/Resources/bin");
        std::fs::create_dir_all(&app).unwrap();
        let script = app.join(NAME);
        std::fs::write(&script, "").unwrap();
        let script = script.canonicalize().unwrap();
        let (missing, bin) = (directory.path().join("missing"), directory.path().join("bin"));
        std::fs::create_dir_all(&bin).unwrap();
        let folders = [missing, bin.clone()];

        assert_eq!(install_into(&script, &folders), Ok(Outcome::Installed(bin.join(NAME))));
        assert_eq!(install_into(&script, &folders), Ok(Outcome::AlreadyThere(bin.join(NAME))));

        // A link left by a copy of the app elsewhere is replaced.
        std::fs::remove_file(bin.join(NAME)).unwrap();
        std::os::unix::fs::symlink("/Volumes/Old/Editor.app/Contents/Resources/bin/editor", bin.join(NAME)).unwrap();
        assert_eq!(install_into(&script, &folders), Ok(Outcome::Installed(bin.join(NAME))));

        // Someone else's program of the same name is not.
        std::fs::remove_file(bin.join(NAME)).unwrap();
        std::fs::write(bin.join(NAME), "#!/bin/sh").unwrap();
        assert!(install_into(&script, &folders).is_err());
    }

    #[test]
    fn without_a_folder_an_administrator_is_needed() {
        let directory = tempfile::tempdir().unwrap();
        assert_eq!(install_into(directory.path(), &[directory.path().join("missing")]), Ok(Outcome::NeedsAdmin));
    }
}
