//! What a commit holds, and files as they were in a commit.

use crate::Repository;

impl Repository {
    /// The text of a file as of the last commit. `None` for a file that is
    /// new, binary or not in a commit yet.
    pub fn file_at_head(&self, path: &str) -> Option<String> {
        let blob = self.run(["show", &format!("HEAD:{path}")]).ok()?;
        (!blob.contains('\0')).then_some(blob)
    }

    /// What a commit says and changes, with its patch.
    pub fn commit_patch(&self, oid: &str) -> Result<String, String> {
        check_oid(oid)?;
        self.run(["show", "--stat", "--patch", "-M", "--format=medium", oid])
    }
}

/// A commit id from the log. Anything else could read as an option.
fn check_oid(oid: &str) -> Result<(), String> {
    if oid.is_empty() || !oid.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return Err(format!("\"{oid}\" is not a commit."));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use crate::test_support::{git, repository};

    #[test]
    fn commits_tell_their_patches() {
        let directory = tempdir().unwrap();
        let repository = repository(directory.path());
        fs::write(directory.path().join("a.tex"), "one\n").unwrap();
        assert_eq!(repository.file_at_head("a.tex"), None);
        git(directory.path(), &["add", "."]);
        git(directory.path(), &["commit", "-q", "-m", "a"]);
        fs::write(directory.path().join("a.tex"), "one\ntwo\n").unwrap();
        fs::write(directory.path().join("b.tex"), "b\n").unwrap();
        git(directory.path(), &["add", "."]);
        git(directory.path(), &["commit", "-q", "-m", "b"]);

        assert_eq!(repository.file_at_head("a.tex").as_deref(), Some("one\ntwo\n"));
        let oid = repository.log(0).unwrap()[0].hash.clone();
        let patch = repository.commit_patch(&oid).unwrap();
        assert!(patch.contains("+two") && patch.contains("b.tex"));
        assert!(repository.commit_patch("--all").is_err());
    }
}
