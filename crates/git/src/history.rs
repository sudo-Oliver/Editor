//! What a commit holds, and files as they were in a commit.

use crate::Repository;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommittedFile {
    /// `A`, `M`, `D`, `R` and so on.
    pub status: char,
    /// Relative to the repository root.
    pub path: String,
}

impl Repository {
    /// The text of a file as of the last commit. `None` for a file that is
    /// new, binary or not in a commit yet.
    pub fn file_at_head(&self, path: &str) -> Option<String> {
        let blob = self.run(["show", &format!("HEAD:{path}")]).ok()?;
        (!blob.contains('\0')).then_some(blob)
    }

    pub fn commit_files(&self, oid: &str) -> Result<Vec<CommittedFile>, String> {
        check_oid(oid)?;
        let text = self.run(["show", "--name-status", "--format=", "-z", "-M", oid])?;
        let mut files = Vec::new();
        let mut fields = text.split('\0').filter(|field| !field.is_empty());
        while let Some(status) = fields.next() {
            let letter = status.chars().next().unwrap_or('M');
            // A rename or copy lists the old path first.
            if matches!(letter, 'R' | 'C') {
                fields.next();
            }
            if let Some(path) = fields.next() {
                files.push(CommittedFile {
                    status: letter,
                    path: path.to_string(),
                });
            }
        }
        Ok(files)
    }

    /// What a commit says and changes, with its patch.
    pub fn commit_patch(&self, oid: &str) -> Result<String, String> {
        check_oid(oid)?;
        self.run(["show", "--stat", "--patch", "-M", "--format=medium", oid])
    }

    /// The patch that a commit made to one file.
    pub fn commit_diff(&self, oid: &str, path: &str) -> Result<String, String> {
        check_oid(oid)?;
        self.run(["--literal-pathspecs", "show", "--format=", "--patch", "-M", oid, "--", path])
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
    fn commits_tell_their_files_and_patches() {
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
        let files = repository.commit_files(&oid).unwrap();
        let summary: Vec<(char, &str)> = files.iter().map(|file| (file.status, file.path.as_str())).collect();
        assert_eq!(summary, [('M', "a.tex"), ('A', "b.tex")]);
        assert!(repository.commit_diff(&oid, "a.tex").unwrap().contains("+two"));
        assert!(repository.commit_files("--all").is_err());
    }
}
