use std::{fs, io::Write, path::Path, process::Stdio};

use crate::{FileChange, GitError, Program, Repository, short_error, status::WHOLE_REPOSITORY};

const BUILD_IGNORE: &str = ".build/";

impl Program {
    /// Makes `directory` a repository and keeps build output out of it.
    pub fn init(&self, directory: &Path) -> Result<Repository, String> {
        let output = self
            .command(directory)
            .arg("init")
            .output()
            .map_err(|error| format!("Cannot start git: {error}"))?;
        if !output.status.success() {
            return Err(short_error(&output));
        }
        ignore_build_output(&directory.join(".gitignore")).map_err(|error| format!("Cannot write .gitignore: {error}"))?;
        self.discover(directory)?.ok_or_else(|| "git init did not create a repository.".to_string())
    }

    /// A setting of the user's own git, such as `user.email`, if set.
    pub fn global(&self, key: &str) -> Option<String> {
        let home = home()?;
        let output = self.command(&home).args(["config", "--global", "--get", key]).output().ok()?;
        let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
        (output.status.success() && !value.is_empty()).then_some(value)
    }

    /// Sets a setting of the user's own git, unless the user already has one.
    pub fn set_global_if_missing(&self, key: &str, value: &str) -> Result<(), String> {
        if self.global(key).is_some() {
            return Ok(());
        }
        let home = home().ok_or("Cannot find the home folder.")?;
        let output = self.command(&home).args(["config", "--global", key, value]).output().map_err(|error| format!("Cannot start git: {error}"))?;
        if output.status.success() { Ok(()) } else { Err(short_error(&output)) }
    }
}

/// Adds the build folder to an ignore file, once.
fn ignore_build_output(path: &Path) -> std::io::Result<()> {
    let existing = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error),
    };
    if existing.lines().any(|line| line.trim() == BUILD_IGNORE) {
        return Ok(());
    }
    let separator = if existing.is_empty() || existing.ends_with('\n') { "" } else { "\n" };
    fs::write(path, format!("{existing}{separator}{BUILD_IGNORE}\n"))
}

impl Repository {
    pub fn stage(&self, file: &FileChange) -> Result<(), String> {
        self.run(["--literal-pathspecs", "add", "--", &file.path]).map(drop)
    }

    pub fn stage_all(&self) -> Result<(), String> {
        let mut args = vec!["add", "--all", "--"];
        args.extend(WHOLE_REPOSITORY);
        self.run(args).map(drop)
    }

    /// Works before the first commit as well, unlike `git restore --staged`.
    pub fn unstage(&self, file: &FileChange) -> Result<(), String> {
        let mut args = vec!["--literal-pathspecs", "reset", "-q", "--", &file.path];
        args.extend(file.old_path.as_deref());
        self.run(args).map(drop)
    }

    pub fn unstage_all(&self) -> Result<(), String> {
        self.run(["reset", "-q"]).map(drop)
    }

    /// Keeps build output out of other git programs too. It goes into the
    /// repository's own exclude file, since a tracked `.gitignore` is the user's.
    pub fn ignore_build_output_locally(&self) -> Result<(), String> {
        let ignored = self.command().args(["check-ignore", "-q", BUILD_IGNORE]).output().map_err(|error| format!("Cannot start git: {error}"))?;
        if ignored.status.success() {
            return Ok(());
        }
        let exclude = self.root.join(self.run(["rev-parse", "--git-path", "info/exclude"])?.trim_end_matches(['\n', '\r']));
        exclude
            .parent()
            .map_or(Ok(()), fs::create_dir_all)
            .and_then(|()| ignore_build_output(&exclude))
            .map_err(|error| format!("Cannot write {}: {error}", exclude.display()))
    }

    pub fn commit(&self, message: &str) -> Result<(), String> {
        let mut child = self
            .command()
            .args(["commit", "-F", "-"])
            .stdin(Stdio::piped())
            .spawn()
            .map_err(|error| format!("Cannot start git: {error}"))?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(message.as_bytes()).map_err(|error| format!("Cannot pass the message: {error}"))?;
        }
        let output = child.wait_with_output().map_err(|error| format!("Cannot run git: {error}"))?;
        if output.status.success() { Ok(()) } else { Err(short_error(&output)) }
    }

    /// The local branches, by name.
    pub fn branches(&self) -> Result<Vec<String>, String> {
        let text = self.run(["for-each-ref", "--format=%(refname:short)", "--sort=refname", "refs/heads"])?;
        Ok(text.lines().map(ToString::to_string).collect())
    }

    pub fn switch(&self, branch: &str) -> Result<(), String> {
        check_branch_name(branch)?;
        self.run(["switch", branch]).map(drop)
    }

    pub fn create_branch(&self, branch: &str) -> Result<(), String> {
        check_branch_name(branch)?;
        self.run(["switch", "-c", branch]).map(drop)
    }

    /// Adds `origin`, for a repository that has no remote yet.
    pub fn add_origin(&self, url: &str) -> Result<(), String> {
        self.run(["remote", "add", "origin", url]).map(drop)
    }

    pub fn has_remote(&self) -> bool {
        self.run(["remote"]).is_ok_and(|remotes| !remotes.trim().is_empty())
    }

    /// Publishes the branch to the only remote or `origin` on the first push.
    pub fn push(&self) -> Result<(), GitError> {
        if self.has_upstream() {
            return self.run_on_network(&["push"]).map(drop).map_err(explain_rejection);
        }
        let remotes = self.run(["remote"])?;
        let remotes: Vec<&str> = remotes.lines().collect();
        let remote = match remotes.as_slice() {
            [] => return Err("This repository has no remote. Add one with `git remote add`.".to_string().into()),
            [only] => *only,
            _ if remotes.contains(&"origin") => "origin",
            _ => return Err("This branch has no upstream. Set one with `git push -u`.".to_string().into()),
        };
        self.run_on_network(&["push", "-u", remote, "HEAD"]).map(drop).map_err(explain_rejection)
    }

    /// Only fast-forwards, for pulls nobody asked for: a merge may need
    /// decisions, which `pull_merge` leaves to the user.
    pub fn pull(&self) -> Result<(), GitError> {
        self.run_on_network(&["pull", "--ff-only"]).map(drop).map_err(|error| {
            let counts = self.run(["rev-list", "--left-right", "--count", "HEAD...@{upstream}"]).unwrap_or_default();
            let diverged = counts.split_whitespace().filter_map(|count| count.parse::<usize>().ok()).all(|count| count > 0);
            if !counts.is_empty() && diverged {
                "The branch and its upstream have diverged. Pull to merge them.".to_string().into()
            } else {
                error
            }
        })
    }
}

/// The remote has commits that are not here, which git says in its own words.
fn explain_rejection(error: GitError) -> GitError {
    match error {
        GitError::Other(message) if message.contains("[rejected]") || message.contains("fetch first") || message.contains("non-fast-forward") => {
            GitError::Other("The remote has commits that are not here yet. Pull to bring them in, then push.".to_string())
        }
        error => error,
    }
}

/// A leading `-` would read as an option, and git checks the rest itself.
fn check_branch_name(name: &str) -> Result<(), String> {
    if name.is_empty() || name.starts_with('-') {
        return Err(format!("\"{name}\" is not a valid branch name."));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;
    use crate::test_support::{configure, git, program, repository};

    fn write(directory: &Path, name: &str, text: &str) {
        fs::write(directory.join(name), text).unwrap();
    }

    fn file<'a>(status: &'a crate::Status, path: &str) -> &'a FileChange {
        status.files.iter().find(|file| file.path == path).unwrap()
    }

    #[test]
    fn missing_repository_is_not_an_error() {
        let directory = tempdir().unwrap();
        assert!(program().discover(directory.path()).unwrap().is_none());
    }

    #[test]
    fn init_ignores_build_output_once() {
        let directory = tempdir().unwrap();
        write(directory.path(), ".gitignore", "*.log");
        program().init(directory.path()).unwrap();
        program().init(directory.path()).unwrap();
        assert_eq!(fs::read_to_string(directory.path().join(".gitignore")).unwrap(), "*.log\n.build/\n");
    }

    #[test]
    fn build_output_is_ignored_locally_once() {
        let directory = tempdir().unwrap();
        let ignoring = repository(directory.path());
        write(directory.path(), ".gitignore", "*.log");
        ignoring.ignore_build_output_locally().unwrap();
        ignoring.ignore_build_output_locally().unwrap();
        let exclude = fs::read_to_string(directory.path().join(".git/info/exclude")).unwrap();
        assert_eq!(exclude.lines().filter(|line| *line == BUILD_IGNORE).count(), 1);
        assert_eq!(fs::read_to_string(directory.path().join(".gitignore")).unwrap(), "*.log");

        // Ignored already, so the exclude file stays as it is.
        let other = tempdir().unwrap();
        let ignored = repository(other.path());
        write(other.path(), ".gitignore", ".build/\n");
        let before = fs::read_to_string(other.path().join(".git/info/exclude")).unwrap_or_default();
        ignored.ignore_build_output_locally().unwrap();
        assert_eq!(fs::read_to_string(other.path().join(".git/info/exclude")).unwrap_or_default(), before);
    }

    #[test]
    fn stage_unstage_and_commit_before_the_first_commit() {
        let directory = tempdir().unwrap();
        let repository = repository(directory.path());
        write(directory.path(), "ä b.tex", "x");
        write(directory.path(), "[glob].tex", "x");

        let status = repository.status().unwrap();
        assert_eq!(status.oid, None);
        assert_eq!(file(&status, "ä b.tex").unstaged, Some('?'));

        repository.stage(file(&status, "[glob].tex")).unwrap();
        repository.stage(file(&status, "ä b.tex")).unwrap();
        let status = repository.status().unwrap();
        assert_eq!(file(&status, "ä b.tex").staged, Some('A'));

        repository.unstage(file(&status, "ä b.tex")).unwrap();
        let status = repository.status().unwrap();
        assert_eq!(file(&status, "ä b.tex").staged, None);
        assert_eq!(file(&status, "[glob].tex").staged, Some('A'));

        repository.unstage_all().unwrap();
        assert!(repository.status().unwrap().files.iter().all(|file| file.staged.is_none()));
        repository.stage(file(&status, "[glob].tex")).unwrap();

        repository.commit("first\n\nbody").unwrap();
        let log = repository.log(0).unwrap();
        assert_eq!((log.len(), log[0].subject.as_str(), log[0].author.as_str()), (1, "first", "Ada"));
        assert_eq!(git(directory.path(), &["log", "-1", "--format=%b"]).trim(), "body");
    }

    #[test]
    fn build_output_stays_out_of_status_and_stage_all() {
        let directory = tempdir().unwrap();
        let repository = repository(directory.path());
        fs::create_dir_all(directory.path().join("sub/.build")).unwrap();
        fs::create_dir_all(directory.path().join(".build")).unwrap();
        write(directory.path(), "sub/.build/a.pdf", "x");
        write(directory.path(), ".build/b.pdf", "x");
        write(directory.path(), "a.tex", "x");
        let status = repository.status().unwrap();
        assert_eq!(status.files.iter().map(|file| file.path.as_str()).collect::<Vec<_>>(), vec!["a.tex"]);
        repository.stage_all().unwrap();
        repository.commit("all").unwrap();
        assert_eq!(git(directory.path(), &["ls-files"]).trim(), "a.tex");
    }

    #[test]
    fn subdirectory_finds_the_repository_and_paths_start_at_its_root() {
        let directory = tempdir().unwrap();
        repository(directory.path());
        fs::create_dir(directory.path().join("sub")).unwrap();
        write(directory.path(), "sub/a.tex", "x");
        let nested = program().discover(&directory.path().join("sub")).unwrap().unwrap();
        assert_eq!(nested.status().unwrap().files[0].path, "sub/a.tex");
        nested.stage_all().unwrap();
    }

    #[test]
    fn rename_and_unstage_restore_both_names() {
        let directory = tempdir().unwrap();
        let repository = repository(directory.path());
        write(directory.path(), "old.tex", "some content that git can recognize\n");
        repository.stage_all().unwrap();
        repository.commit("add").unwrap();
        git(directory.path(), &["mv", "old.tex", "new.tex"]);
        let status = repository.status().unwrap();
        let renamed = file(&status, "new.tex");
        assert_eq!((renamed.staged, renamed.old_path.as_deref()), (Some('R'), Some("old.tex")));
        repository.unstage(renamed).unwrap();
        let status = repository.status().unwrap();
        assert_eq!(file(&status, "old.tex").unstaged, Some('D'));
        assert_eq!(file(&status, "new.tex").unstaged, Some('?'));
    }

    #[test]
    fn branches_switch_and_keep_changes_safe() {
        let directory = tempdir().unwrap();
        let repository = repository(directory.path());
        write(directory.path(), "a.tex", "base\n");
        repository.stage_all().unwrap();
        repository.commit("base").unwrap();

        repository.create_branch("chapter").unwrap();
        write(directory.path(), "a.tex", "chapter\n");
        repository.stage_all().unwrap();
        repository.commit("chapter").unwrap();
        assert_eq!(repository.status().unwrap().branch.as_deref(), Some("chapter"));
        assert_eq!(repository.branches().unwrap(), vec!["chapter", "main"]);

        write(directory.path(), "a.tex", "unsaved work\n");
        assert!(repository.switch("main").is_err());
        assert_eq!(fs::read_to_string(directory.path().join("a.tex")).unwrap(), "unsaved work\n");

        git(directory.path(), &["checkout", "--", "a.tex"]);
        repository.switch("main").unwrap();
        assert_eq!(fs::read_to_string(directory.path().join("a.tex")).unwrap(), "base\n");
        assert!(repository.create_branch("--force").is_err());
        assert!(repository.switch("").is_err());

        let log = repository.log(0).unwrap();
        assert_eq!(log.len(), 2);
    }

    #[test]
    fn push_and_pull_against_a_bare_remote() {
        let directory = tempdir().unwrap();
        let remote = directory.path().join("remote.git");
        let first = directory.path().join("first");
        let second = directory.path().join("second");
        fs::create_dir_all(&remote).unwrap();
        fs::create_dir_all(&first).unwrap();
        git(&remote, &["init", "-q", "--bare", "-b", "main"]);
        let repo = repository(&first);

        write(&first, "a.tex", "1\n");
        repo.stage_all().unwrap();
        repo.commit("one").unwrap();
        assert!(repo.push().unwrap_err().to_string().contains("no remote"));

        git(&first, &["remote", "add", "origin", remote.to_str().unwrap()]);
        repo.push().unwrap();
        assert_eq!(repo.status().unwrap().upstream.as_deref(), Some("origin/main"));

        git(directory.path(), &["clone", "-q", remote.to_str().unwrap(), "second"]);
        configure(&second);
        let other = program().discover(&second).unwrap().unwrap();
        write(&second, "a.tex", "2\n");
        other.stage_all().unwrap();
        other.commit("two").unwrap();
        assert_eq!(other.status().unwrap().ahead, 1);
        other.push().unwrap();

        repo.pull().unwrap();
        assert_eq!(fs::read_to_string(first.join("a.tex")).unwrap(), "2\n");

        write(&first, "b.tex", "here\n");
        repo.stage_all().unwrap();
        repo.commit("here").unwrap();
        write(&second, "c.tex", "there\n");
        other.stage_all().unwrap();
        other.commit("there").unwrap();
        other.push().unwrap();

        let error = repo.pull().unwrap_err().to_string();
        assert!(error.contains("diverged"), "{error}");
        assert!(!first.join("c.tex").exists());
        assert!(repo.push().is_err());
    }

    #[test]
    fn conflict_shows_up_as_conflicted() {
        let directory = tempdir().unwrap();
        let repository = repository(directory.path());
        write(directory.path(), "a.tex", "base\n");
        repository.stage_all().unwrap();
        repository.commit("base").unwrap();
        repository.create_branch("other").unwrap();
        write(directory.path(), "a.tex", "other\n");
        repository.stage_all().unwrap();
        repository.commit("other").unwrap();
        repository.switch("main").unwrap();
        write(directory.path(), "a.tex", "main\n");
        repository.stage_all().unwrap();
        repository.commit("main").unwrap();

        let merge = std::process::Command::new("git").args(["merge", "other"]).current_dir(directory.path()).output().unwrap();
        assert!(!merge.status.success());
        assert!(file(&repository.status().unwrap(), "a.tex").conflicted);
    }

    #[test]
    fn reading_status_leaves_the_index_alone() {
        let directory = tempdir().unwrap();
        let repository = repository(directory.path());
        write(directory.path(), "a.tex", "x");
        repository.stage_all().unwrap();
        repository.commit("a").unwrap();
        // A changed time makes plain `git status` rewrite the index.
        std::thread::sleep(std::time::Duration::from_millis(1100));
        write(directory.path(), "a.tex", "x");
        let index = directory.path().join(".git/index");
        let before = fs::metadata(&index).unwrap().modified().unwrap();
        repository.status().unwrap();
        assert_eq!(fs::metadata(&index).unwrap().modified().unwrap(), before);
    }
}

/// The user's home folder, where `git config --global` runs.
fn home() -> Option<std::path::PathBuf> {
    let name = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var_os(name).filter(|home| !home.is_empty()).map(std::path::PathBuf::from)
}
