//! Talks to the `git` program. Every call blocks, so callers use a background thread.

mod actions;
mod history;
mod log;
mod remote;
mod status;

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
};

pub use history::CommittedFile;
pub use log::{Commit, GraphRow, HeadSummary, Lanes, PAGE_SIZE};
pub use remote::{ASKPASS_ENV, GitError, PullOutcome};
pub use status::{FileChange, Status};

const MAX_ERROR_LINES: usize = 6;

/// How to start `git`, independent of a repository.
#[derive(Clone, Debug)]
pub struct Program {
    pub path: PathBuf,
    /// `PATH` for git, so that it finds `git-lfs` and credential helpers.
    pub path_env: OsString,
    /// A program that answers git's questions for a user name or password,
    /// from tokens the user gave the editor.
    pub askpass: Option<PathBuf>,
}

impl Program {
    fn command(&self, directory: &Path) -> Command {
        let mut command = Command::new(&self.path);
        command
            .current_dir(directory)
            .env("PATH", &self.path_env)
            // Nobody could answer a prompt.
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_EDITOR", "true")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // Reading must leave the index alone, or the file watcher would
            // see the write and read again, without end.
            .args(["--no-optional-locks", "-c", "core.fsmonitor=false", "-c", "core.quotePath=false"]);
        command
    }

    /// The repository that contains `directory`, if any.
    pub fn discover(&self, directory: &Path) -> Result<Option<Repository>, String> {
        let output = self
            .command(directory)
            .args(["rev-parse", "--show-toplevel"])
            .output()
            .map_err(|error| format!("Cannot start git: {error}"))?;
        if !output.status.success() {
            let message = String::from_utf8_lossy(&output.stderr);
            return if message.contains("not a git repository") {
                Ok(None)
            } else {
                Err(short_error(&output))
            };
        }
        let root = String::from_utf8_lossy(&output.stdout).trim_end_matches(['\n', '\r']).to_string();
        Ok(Some(Repository { program: self.clone(), root: PathBuf::from(root) }))
    }
}

/// A repository. Commands run in its top directory, where paths in `Status` start.
#[derive(Clone, Debug)]
pub struct Repository {
    program: Program,
    root: PathBuf,
}

impl Repository {
    pub fn root(&self) -> &Path {
        &self.root
    }

    fn command(&self) -> Command {
        self.program.command(&self.root)
    }

    fn run<I, S>(&self, args: I) -> Result<String, String>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<std::ffi::OsStr>,
    {
        Self::finish(self.command().args(args))
    }

    /// For commands that reach a remote. A dead connection then ends in an error
    /// instead of waiting forever, and a missing login in `GitError::Auth`.
    fn run_on_network(&self, args: &[&str]) -> Result<String, GitError> {
        let mut command = self.command();
        command.args(["-c", "http.lowSpeedLimit=1000", "-c", "http.lowSpeedTime=60"]);
        // These settings beat the user's own, so they stay out when the user has some.
        if std::env::var_os("GIT_SSH_COMMAND").is_none() && self.run(["config", "--get", "core.sshCommand"]).is_err() {
            command.env("GIT_SSH_COMMAND", "ssh -o BatchMode=yes -o ConnectTimeout=20 -o ServerAliveInterval=15 -o ServerAliveCountMax=4");
        }
        if let Some(askpass) = &self.program.askpass {
            command.env("GIT_ASKPASS", askpass).env(remote::ASKPASS_ENV, "1");
        }
        Self::finish(command.args(args)).map_err(|error| self.classify(error))
    }

    fn finish(command: &mut Command) -> Result<String, String> {
        let output = command.output().map_err(|error| format!("Cannot start git: {error}"))?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).into_owned())
        } else {
            Err(short_error(&output))
        }
    }
}

fn short_error(output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let text = if stderr.trim().is_empty() { stdout.trim() } else { stderr.trim() };
    text.lines().take(MAX_ERROR_LINES).collect::<Vec<_>>().join("\n")
}

#[cfg(test)]
pub(crate) mod test_support {
    use std::path::Path;

    use super::*;

    pub fn program() -> Program {
        Program { path: PathBuf::from("git"), path_env: std::env::var_os("PATH").unwrap_or_default(), askpass: None }
    }

    /// Runs git outside the code under test, without the user's configuration.
    pub fn git(directory: &Path, args: &[&str]) -> String {
        let output = Command::new("git").args(args).current_dir(directory).envs(identity()).output().unwrap();
        assert!(output.status.success(), "{args:?}: {}", String::from_utf8_lossy(&output.stderr));
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    fn identity() -> [(&'static str, &'static str); 6] {
        [
            ("GIT_CONFIG_GLOBAL", "/dev/null"),
            ("GIT_CONFIG_NOSYSTEM", "1"),
            ("GIT_AUTHOR_NAME", "Ada"),
            ("GIT_AUTHOR_EMAIL", "ada@example.com"),
            ("GIT_COMMITTER_NAME", "Ada"),
            ("GIT_COMMITTER_EMAIL", "ada@example.com"),
        ]
    }

    /// A repository whose own config holds what the isolated tests need.
    pub fn repository(directory: &Path) -> Repository {
        git(directory, &["init", "-q", "-b", "main"]);
        configure(directory);
        program().discover(directory).unwrap().unwrap()
    }

    pub fn configure(directory: &Path) {
        git(directory, &["config", "user.name", "Ada"]);
        git(directory, &["config", "user.email", "ada@example.com"]);
    }
}
