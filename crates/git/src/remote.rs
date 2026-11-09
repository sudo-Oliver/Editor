//! Talking to remotes: fetching, merging what comes in, and telling a
//! missing login apart from other failures.

use std::fmt;

use crate::Repository;

/// Set for the askpass program, so that the editor answers instead of
/// opening a window.
pub const ASKPASS_ENV: &str = "EDITOR_GIT_ASKPASS";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GitError {
    /// The remote at this host wants a login the editor does not have.
    Auth { host: String },
    Other(String),
}

impl fmt::Display for GitError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GitError::Auth { host } => write!(formatter, "{host} needs a login."),
            GitError::Other(message) => formatter.write_str(message),
        }
    }
}

impl From<String> for GitError {
    fn from(message: String) -> Self {
        GitError::Other(message)
    }
}

impl From<GitError> for String {
    fn from(error: GitError) -> Self {
        error.to_string()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PullOutcome {
    UpToDate,
    Merged,
    /// The merge stopped in these files, relative to the repository root.
    Conflicts(Vec<String>),
}

const AUTH_FAILURES: &[&str] = &[
    "Authentication failed",
    "could not read Username",
    "could not read Password",
    "Invalid username or password",
    "HTTP Basic: Access denied",
    "returned error: 401",
    "returned error: 403",
];

/// The host of an HTTPS remote URL.
pub(crate) fn https_host(url: &str) -> Option<String> {
    let rest = url.strip_prefix("https://")?;
    let authority = rest.split('/').next()?;
    let host = authority.rsplit('@').next()?;
    (!host.is_empty()).then(|| host.to_string())
}

impl Repository {
    /// A failed network command, as a login to ask for where that helps.
    pub(crate) fn classify(&self, message: String) -> GitError {
        if !AUTH_FAILURES.iter().any(|failure| message.contains(failure)) {
            return GitError::Other(message);
        }
        match self.remote_url().as_deref().and_then(https_host) {
            Some(host) => GitError::Auth { host },
            None => GitError::Other(message),
        }
    }

    /// The URL of the upstream's remote, or of the only or `origin` remote.
    pub fn remote_url(&self) -> Option<String> {
        let remote = self
            .run(["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{upstream}"])
            .ok()
            .and_then(|upstream| upstream.trim().split('/').next().map(str::to_string))
            .or_else(|| {
                let remotes = self.run(["remote"]).ok()?;
                let remotes: Vec<&str> = remotes.lines().collect();
                match remotes.as_slice() {
                    [only] => Some(only.to_string()),
                    _ if remotes.contains(&"origin") => Some("origin".to_string()),
                    _ => None,
                }
            })?;
        self.run(["remote", "get-url", &remote]).ok().map(|url| url.trim().to_string())
    }

    pub fn has_upstream(&self) -> bool {
        self.run(["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{upstream}"]).is_ok()
    }

    pub fn fetch(&self) -> Result<(), GitError> {
        self.run_on_network(&["fetch", "--prune", "--quiet"]).map(drop)
    }

    /// Merges the upstream in. A merge that stops at conflicts is no error:
    /// the files wait for the user, and `finish_merge` or `abort_merge` ends it.
    pub fn pull_merge(&self) -> Result<PullOutcome, GitError> {
        let before = self.run(["rev-parse", "HEAD"]).unwrap_or_default();
        match self.run_on_network(&["-c", "merge.conflictStyle=diff3", "pull", "--no-rebase", "--no-edit"]) {
            Ok(_) => {
                let after = self.run(["rev-parse", "HEAD"]).unwrap_or_default();
                Ok(if before == after { PullOutcome::UpToDate } else { PullOutcome::Merged })
            }
            Err(error) => {
                let conflicts = self.conflicted_files();
                if self.merge_in_progress() && !conflicts.is_empty() {
                    Ok(PullOutcome::Conflicts(conflicts))
                } else {
                    Err(error)
                }
            }
        }
    }

    pub fn merge_in_progress(&self) -> bool {
        self.run(["rev-parse", "-q", "--verify", "MERGE_HEAD"]).is_ok()
    }

    pub fn conflicted_files(&self) -> Vec<String> {
        self.run(["diff", "--name-only", "--diff-filter=U", "-z"])
            .map(|text| text.split('\0').filter(|path| !path.is_empty()).map(str::to_string).collect())
            .unwrap_or_default()
    }

    pub fn abort_merge(&self) -> Result<(), String> {
        self.run(["merge", "--abort"]).map(drop)
    }

    /// Stages the solved files and records the merge.
    pub fn finish_merge(&self, files: &[String]) -> Result<(), String> {
        if !files.is_empty() {
            let mut args = vec!["--literal-pathspecs", "add", "--"];
            args.extend(files.iter().map(String::as_str));
            self.run(args)?;
        }
        self.run(["commit", "--no-edit"]).map(drop)
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;
    use crate::test_support::{git, repository};

    fn write(directory: &std::path::Path, name: &str, text: &str) {
        fs::write(directory.join(name), text).unwrap();
    }

    /// A remote, a clone of it with one commit, and a second clone.
    fn remote_and_clones() -> (tempfile::TempDir, Repository, Repository) {
        let base = tempdir().unwrap();
        let remote = base.path().join("remote.git");
        git(base.path(), &["init", "-q", "--bare", "-b", "main", remote.to_str().unwrap()]);
        let first = base.path().join("first");
        fs::create_dir(&first).unwrap();
        let mine = repository(&first);
        write(&first, "a.tex", "one\ntwo\nthree\n");
        git(&first, &["add", "."]);
        git(&first, &["commit", "-q", "-m", "start"]);
        git(&first, &["remote", "add", "origin", remote.to_str().unwrap()]);
        mine.push().unwrap();
        git(base.path(), &["clone", "-q", remote.to_str().unwrap(), "second"]);
        let second = base.path().join("second");
        crate::test_support::configure(&second);
        let theirs = crate::test_support::program().discover(&second).unwrap().unwrap();
        (base, mine, theirs)
    }

    #[test]
    fn fetch_and_merge_bring_in_what_others_pushed() {
        let (_base, mine, theirs) = remote_and_clones();
        write(theirs.root(), "b.tex", "new\n");
        git(theirs.root(), &["add", "."]);
        git(theirs.root(), &["commit", "-q", "-m", "theirs"]);
        theirs.push().unwrap();
        mine.fetch().unwrap();
        assert_eq!(mine.status().unwrap().behind, 1);
        assert_eq!(mine.pull_merge().unwrap(), PullOutcome::Merged);
        assert_eq!(mine.pull_merge().unwrap(), PullOutcome::UpToDate);
        assert!(mine.root().join("b.tex").exists());
    }

    #[test]
    fn diverged_edits_of_one_line_stop_at_a_conflict() {
        let (_base, mine, theirs) = remote_and_clones();
        write(theirs.root(), "a.tex", "one\nTHEIRS\nthree\n");
        git(theirs.root(), &["commit", "-q", "-am", "theirs"]);
        theirs.push().unwrap();
        write(mine.root(), "a.tex", "one\nMINE\nthree\n");
        git(mine.root(), &["commit", "-q", "-am", "mine"]);

        assert_eq!(mine.pull_merge().unwrap(), PullOutcome::Conflicts(vec!["a.tex".to_string()]));
        assert!(mine.merge_in_progress());
        let text = fs::read_to_string(mine.root().join("a.tex")).unwrap();
        assert!(text.contains("<<<<<<<") && text.contains("|||||||") && text.contains(">>>>>>>"));

        mine.abort_merge().unwrap();
        assert!(!mine.merge_in_progress());
        assert_eq!(mine.pull_merge().unwrap(), PullOutcome::Conflicts(vec!["a.tex".to_string()]));
        write(mine.root(), "a.tex", "one\nBOTH\nthree\n");
        mine.finish_merge(&["a.tex".to_string()]).unwrap();
        assert!(!mine.merge_in_progress());
        assert_eq!(mine.status().unwrap().ahead, 2);
    }

    #[test]
    fn a_missing_login_names_the_host() {
        let directory = tempdir().unwrap();
        let repository = repository(directory.path());
        git(directory.path(), &["remote", "add", "origin", "https://ada@git.example.org/thesis.git"]);
        assert_eq!(
            repository.classify("fatal: Authentication failed for 'https://git.example.org/thesis.git/'".to_string()),
            GitError::Auth {
                host: "git.example.org".to_string()
            }
        );
        assert!(matches!(repository.classify("fatal: unable to access".to_string()), GitError::Other(_)));
        assert_eq!(https_host("git@github.com:a/b.git"), None);
    }
}
