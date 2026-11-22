//! Replaces files in one step, so that a crash or a full disk during a write
//! leaves the old file whole and never a cut-off one.

use std::{
    fs::{self, File},
    io::{self, Write},
    path::Path,
};

use tempfile::NamedTempFile;

/// Hidden from the file tree and from exports, also if a crash leaves one behind.
pub const TEMP_PREFIX: &str = ".export-";

pub fn write(path: &Path, contents: &[u8]) -> io::Result<()> {
    // A link stays a link, and the file behind it changes.
    let target = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    // Renaming would replace a read-only file without asking.
    if fs::metadata(&target).is_ok_and(|metadata| metadata.permissions().readonly()) {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, "the file is read-only"));
    }
    let folder = target.parent().filter(|folder| !folder.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let mut temp = match tempfile::Builder::new().prefix(TEMP_PREFIX).tempfile_in(folder) {
        Ok(temp) => temp,
        // Some folders allow changing a file but not adding one.
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => return fs::write(&target, contents),
        Err(error) => return Err(error),
    };
    temp.write_all(contents)?;
    finish(temp, &target)
}

/// Puts a finished temp file in place, in one step. Temp files are private, so
/// the result takes the rights of the file it replaces or normal ones.
pub fn finish(temp: NamedTempFile, target: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let permissions = fs::metadata(target).map_or_else(|_| fs::Permissions::from_mode(0o644), |metadata| metadata.permissions());
        fs::set_permissions(temp.path(), permissions)?;
    }
    // Other programs write by path, so the file is opened again for the sync.
    File::open(temp.path())?.sync_all()?;
    temp.persist(target).map_err(|error| error.error)?;
    if let Some(folder) = target.parent()
        && let Ok(folder) = File::open(folder)
    {
        folder.sync_all().ok();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(folder: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(folder).unwrap().map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        names
    }

    #[test]
    fn writes_new_and_existing_files_and_leaves_no_temp_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("a.tex");
        write(&path, b"one").unwrap();
        write(&path, b"two").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"two");
        assert_eq!(names(directory.path()), ["a.tex"]);
    }

    #[test]
    fn a_failed_write_keeps_the_old_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("a.tex");
        write(&path, b"old").unwrap();
        let mut rights = fs::metadata(&path).unwrap().permissions();
        rights.set_readonly(true);
        fs::set_permissions(&path, rights).unwrap();
        assert_eq!(write(&path, b"new").unwrap_err().kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(fs::read(&path).unwrap(), b"old");
    }

    #[cfg(unix)]
    #[test]
    fn keeps_links_and_rights() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let real = directory.path().join("real.tex");
        let link = directory.path().join("link.tex");
        fs::write(&real, "old").unwrap();
        fs::set_permissions(&real, fs::Permissions::from_mode(0o640)).unwrap();
        std::os::unix::fs::symlink(&real, &link).unwrap();
        write(&link, b"new").unwrap();
        assert!(fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
        assert_eq!(fs::read(&real).unwrap(), b"new");
        assert_eq!(fs::metadata(&real).unwrap().permissions().mode() & 0o777, 0o640);
    }

    /// Needs a nearly full disk, such as a small RAM disk:
    ///
    ///   EDITOR_FULL_DISK=/Volumes/Tiny cargo test -p app full_disk -- --ignored
    #[test]
    #[ignore]
    fn a_full_disk_keeps_the_old_file() {
        let Some(folder) = std::env::var_os("EDITOR_FULL_DISK").map(std::path::PathBuf::from) else {
            return;
        };
        let path = folder.join("thesis.tex");
        write(&path, b"the old text").unwrap();
        // Fill the rest of the disk.
        let filler = folder.join("filler");
        let mut file = File::create(&filler).unwrap();
        let block = vec![b'x'; 64 * 1024];
        while file.write_all(&block).is_ok() {}
        drop(file);
        let error = write(&path, &vec![b'y'; 4 * 1024 * 1024]).unwrap_err();
        assert_eq!(fs::read(&path).unwrap(), b"the old text", "{error}");
        assert_eq!(names(&folder).iter().filter(|name| name.starts_with(TEMP_PREFIX)).count(), 0, "no temp file stays behind");
        fs::remove_file(&filler).unwrap();
        fs::remove_file(&path).unwrap();
    }
}
