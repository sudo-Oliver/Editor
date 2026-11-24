//! Keeps a copy of unsaved text outside the project, so that a crash loses at
//! most a few seconds of work and nothing ever lands in a repository.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct Snapshot {
    pub path: PathBuf,
    /// The program that wrote this. While it runs, the text is not left over.
    pid: u32,
    pub text: String,
}

pub fn directory() -> Option<PathBuf> {
    Some(crate::settings::default_path()?.parent()?.join("recovery"))
}

fn file_for(directory: &Path, path: &Path) -> PathBuf {
    directory.join(format!("{}.json", crate::history::stable_hash(path)))
}

pub fn write(directory: &Path, path: &Path, text: &str) -> io::Result<()> {
    create_private(directory)?;
    let snapshot = Snapshot { path: path.to_path_buf(), pid: std::process::id(), text: text.to_string() };
    // Temp files are readable only by their owner, and the rename keeps that.
    let temp = tempfile::NamedTempFile::new_in(directory)?;
    serde_json::to_writer(io::BufWriter::new(temp.as_file()), &snapshot)?;
    temp.as_file().sync_all()?;
    temp.persist(file_for(directory, path)).map_err(|error| error.error)?;
    Ok(())
}

fn create_private(directory: &Path) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    // On Windows the folder inherits the rights of the user's data folder.
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(directory)
}

pub fn remove(directory: &Path, path: &Path) {
    fs::remove_file(file_for(directory, path)).ok();
}

/// What a crash left behind in this project. Copies that say nothing new go away.
pub fn pending(directory: &Path, root: &Path) -> Vec<Snapshot> {
    let Ok(files) = fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut snapshots = Vec::new();
    for file in files.filter_map(Result::ok).map(|file| file.path()) {
        if file.extension().is_none_or(|extension| extension != "json") {
            // A temp file of a write that a crash stopped.
            fs::remove_file(&file).ok();
            continue;
        }
        let Some(snapshot) = fs::read(&file).ok().and_then(|bytes| serde_json::from_slice::<Snapshot>(&bytes).ok()) else {
            fs::remove_file(&file).ok();
            continue;
        };
        if !snapshot.path.starts_with(root) || process_runs(snapshot.pid) {
            continue;
        }
        if crate::text_file::read(&snapshot.path).is_ok_and(|disk| disk.text == snapshot.text) {
            fs::remove_file(&file).ok();
            continue;
        }
        snapshots.push(snapshot);
    }
    snapshots.sort_by(|left, right| left.path.cmp(&right.path));
    snapshots
}

fn process_runs(pid: u32) -> bool {
    pid == std::process::id() || build::process_running(pid)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A number no process has, as if the writer had crashed.
    const GONE: u32 = i32::MAX as u32 - 1;

    fn left_behind(directory: &Path, path: &Path, text: &str) {
        let snapshot = Snapshot { path: path.to_path_buf(), pid: GONE, text: text.to_string() };
        create_private(directory).unwrap();
        fs::write(file_for(directory, path), serde_json::to_vec(&snapshot).unwrap()).unwrap();
    }

    #[test]
    fn offers_only_what_a_crash_left_in_this_project() {
        let directory = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let changed = project.path().join("changed.tex");
        let same = project.path().join("same.tex");
        let gone = project.path().join("gone.tex");
        fs::write(&changed, "old").unwrap();
        fs::write(&same, "same").unwrap();
        left_behind(directory.path(), &changed, "new");
        left_behind(directory.path(), &same, "same");
        left_behind(directory.path(), &gone, "lost");
        left_behind(directory.path(), Path::new("/other/project/a.tex"), "other");
        fs::write(directory.path().join(".tmpABC"), "cut off").unwrap();
        fs::write(directory.path().join("broken.json"), "{").unwrap();

        let snapshots = pending(directory.path(), project.path());
        let paths: Vec<&Path> = snapshots.iter().map(|snapshot| snapshot.path.as_path()).collect();
        assert_eq!(paths, [changed.as_path(), gone.as_path()]);
        assert_eq!(snapshots[0].text, "new");

        assert!(!file_for(directory.path(), &same).exists());
        assert!(!directory.path().join(".tmpABC").exists());
        assert!(!directory.path().join("broken.json").exists());
        assert!(file_for(directory.path(), Path::new("/other/project/a.tex")).exists());
    }

    #[test]
    fn a_running_program_keeps_its_copies() {
        let directory = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let file = project.path().join("main.tex");
        write(directory.path(), &file, "ä unsaved").unwrap();
        assert!(pending(directory.path(), project.path()).is_empty());
        assert!(file_for(directory.path(), &file).exists());
        remove(directory.path(), &file);
        assert!(!file_for(directory.path(), &file).exists());
    }

    #[test]
    #[cfg(unix)]
    fn copies_stay_private() {
        use std::os::unix::fs::PermissionsExt;
        let parent = tempfile::tempdir().unwrap();
        let directory = parent.path().join("recovery");
        let file = Path::new("/project/main.tex");
        write(&directory, file, "text").unwrap();
        assert_eq!(fs::metadata(&directory).unwrap().permissions().mode() & 0o777, 0o700);
        assert_eq!(fs::metadata(file_for(&directory, file)).unwrap().permissions().mode() & 0o777, 0o600);
    }
}
