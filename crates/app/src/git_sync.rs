//! Git beyond the panel: colors in the file tree, marks next to changed
//! lines, a commit message to suggest, syncing in the background, logins,
//! conflicts and what a commit changed. The config of a repository can make
//! git start programs, so all of it waits until the user trusts the
//! repository by opening the git panel for it once.

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use editor_core::{Decoration, DecorationKind, conflicts::Choice};
use git::{GitError, Repository, Status};
use gpui::{AnyElement, Context, Entity, Focusable, FontWeight, SharedString, Task, Window, div, prelude::*, px};

use super::{Dialog, LeftPanel, Workspace};
use crate::{editor_view::EditorView, git_login, git_panel, theme};

/// How often the background sync looks for new commits.
const SYNC_INTERVAL: Duration = Duration::from_secs(5 * 60);
/// The longest wait after failures, such as a long time offline.
const SYNC_INTERVAL_MAX: Duration = Duration::from_secs(30 * 60);
const ADDED_COLOR: u32 = 0x2b8a3e;
const REMOVED_COLOR: u32 = 0xe5484d;

#[derive(Default)]
pub(super) struct GitUi {
    repository: Option<Repository>,
    /// What git says about each changed file, by absolute path.
    files: HashMap<PathBuf, char>,
    /// Folders that hold a changed file.
    folders: HashSet<PathBuf>,
    status_task: Option<Task<()>>,
    status_again: bool,
    sync_task: Option<Task<()>>,
    /// What the background sync last had to say, while it matters.
    note: Option<SharedString>,
    sign_in: Option<SignIn>,
    commit_view: Option<(SharedString, Entity<EditorView>)>,
}

struct SignIn {
    host: String,
    user: Entity<EditorView>,
    token: Entity<EditorView>,
    error: Option<SharedString>,
}

/// The letter of a changed file as the tree shows it: conflicts first, then
/// what is not committed yet.
fn tree_letter(file: &git::FileChange) -> char {
    if file.conflicted {
        return 'C';
    }
    match (file.unstaged, file.staged) {
        (Some('?'), _) => 'U',
        (Some('D'), _) | (_, Some('D')) => 'D',
        (Some(_), _) => 'M',
        (None, Some('A')) => 'A',
        (None, Some(_)) => 'S',
        (None, None) => 'M',
    }
}

fn letter_color(letter: char) -> u32 {
    match letter {
        'C' | 'D' => theme::palette().error,
        'U' | 'A' => theme::palette().st_green,
        'S' => theme::palette().accent,
        _ => theme::palette().warning,
    }
}

/// A commit message from what changed: the sections whose lines changed in
/// open documents, or else the names of the files.
pub(super) fn suggest_message(sections: &[String], added: &[String], changed: &[String]) -> Option<String> {
    let list = |names: &[String]| match names {
        [] => String::new(),
        [one] => one.clone(),
        [one, two] => format!("{one} and {two}"),
        [one, rest @ ..] => format!("{one} and {} more", rest.len()),
    };
    if let [first, rest @ ..] = sections {
        return Some(match rest.len() {
            0 => format!("Update {first}"),
            1 => format!("Update {first} and {}", rest[0]),
            more => format!("Update {first} and {more} more sections"),
        });
    }
    match (added.is_empty(), changed.is_empty()) {
        (true, true) => None,
        (false, true) => Some(format!("Add {}", list(added))),
        (true, false) => Some(format!("Update {}", list(changed))),
        (false, false) => Some(format!("Update {}, add {}", list(changed), list(added))),
    }
}

/// The patch of a commit, with added and removed lines colored.
fn patch_decorations(patch: &str) -> Vec<Decoration> {
    let mut decorations = Vec::new();
    let mut offset = 0;
    let mut in_patch = false;
    for line in patch.split_inclusive('\n') {
        let text = line.trim_end_matches('\n');
        if text.starts_with("diff --git") {
            in_patch = true;
        }
        let color = match text.as_bytes().first() {
            Some(b'+') if in_patch && !text.starts_with("+++") => Some(ADDED_COLOR),
            Some(b'-') if in_patch && !text.starts_with("---") => Some(REMOVED_COLOR),
            _ => None,
        };
        if let Some(color) = color
            && !text.is_empty()
        {
            decorations.push(Decoration {
                range: offset..offset + text.len(),
                kind: DecorationKind::Selection,
                color,
                label: None,
            });
        }
        offset += line.len();
    }
    decorations
}

impl Workspace {
    fn git_trusted(&self, root: &Path) -> bool {
        self.settings.trusted_git_roots.iter().any(|trusted| trusted == root)
    }

    /// The git panel was opened for this project, which trusts its repository.
    pub(super) fn trust_git_root(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.root.clone() else {
            return;
        };
        if self.git_trusted(&root) {
            return;
        }
        self.update_settings(|settings| settings.trusted_git_roots.push(root.clone()), window, cx);
        self.git_root_opened(cx);
    }

    /// Starts the git features for a new project, if its repository is trusted.
    pub(super) fn git_root_opened(&mut self, cx: &mut Context<Self>) {
        self.git = GitUi::default();
        let Some(root) = self.root.clone().filter(|root| self.git_trusted(root)) else {
            return;
        };
        let Some(program) = git_panel::program() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let repository = cx.background_spawn(async move { program.discover(&root).ok().flatten() }).await;
            this.update(cx, |this, cx| {
                if this.root.as_deref().is_some_and(|root| repository.as_ref().is_some_and(|repository| root.starts_with(repository.root()))) {
                    this.git.repository = repository;
                    this.refresh_git(cx);
                    this.start_sync(cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// Reads the status again, for the tree, the marks and the suggestion.
    pub(super) fn refresh_git(&mut self, cx: &mut Context<Self>) {
        let Some(repository) = self.git.repository.clone() else {
            return;
        };
        if self.git.status_task.is_some() {
            self.git.status_again = true;
            return;
        }
        self.git.status_task = Some(cx.spawn(async move |this, cx| {
            let status = cx.background_spawn({
                let repository = repository.clone();
                async move { repository.status() }
            })
            .await;
            this.update(cx, |this, cx| {
                this.git.status_task = None;
                if let Ok(status) = status {
                    this.apply_git_status(&repository, &status, cx);
                }
                if std::mem::take(&mut this.git.status_again) {
                    this.refresh_git(cx);
                }
            })
            .ok();
        }));
    }

    fn apply_git_status(&mut self, repository: &Repository, status: &Status, cx: &mut Context<Self>) {
        let mut files = HashMap::new();
        let mut folders = HashSet::new();
        for file in &status.files {
            let path = repository.root().join(&file.path);
            for folder in path.ancestors().skip(1) {
                if !folders.insert(folder.to_path_buf()) || folder == repository.root() {
                    break;
                }
            }
            files.insert(path, tree_letter(file));
        }
        let changed = files != self.git.files;
        self.git.files = files;
        self.git.folders = folders;
        if changed {
            self.refresh_diff_bases(cx);
        }
        self.update_suggestion(cx);
        cx.notify();
    }

    /// Gives every open document the text it had in the last commit.
    pub(super) fn refresh_diff_bases(&mut self, cx: &mut Context<Self>) {
        let views: Vec<_> = self.documents.iter().map(|document| document.view.clone()).collect();
        for view in views {
            self.load_diff_base(&view, cx);
        }
    }

    pub(super) fn load_diff_base(&mut self, view: &Entity<EditorView>, cx: &mut Context<Self>) {
        let Some(repository) = self.git.repository.clone() else {
            return;
        };
        let Some(path) = view.read(cx).path.clone() else {
            return;
        };
        let Ok(relative) = path.strip_prefix(repository.root()).map(|relative| relative.to_string_lossy().replace('\\', "/")) else {
            return;
        };
        // A file git does not know yet is all new.
        let untracked = self.git.files.get(&path) == Some(&'U');
        let view = view.downgrade();
        cx.spawn(async move |_, cx| {
            let base = if untracked {
                Some(String::new())
            } else {
                cx.background_spawn(async move { repository.file_at_head(&relative) }).await
            };
            view.update(cx, |view, cx| view.set_diff_base(base.map(Arc::from), cx)).ok();
        })
        .detach();
    }

    fn update_suggestion(&mut self, cx: &mut Context<Self>) {
        let mut sections = Vec::new();
        for document in &self.documents {
            let view = document.view.read(cx);
            let headings = view.state.syntax().headings();
            for hunk in view.diff_hunks() {
                // The heading that holds the first changed line, or the line just after a deletion.
                let line = hunk.lines.start;
                if let Some(heading) = headings.iter().rev().find(|heading| heading.line <= line)
                    && !sections.contains(&heading.title)
                {
                    sections.push(heading.title.clone());
                }
            }
        }
        let name = |path: &PathBuf| path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
        let mut added: Vec<String> = self.git.files.iter().filter(|(_, letter)| matches!(letter, 'U' | 'A')).map(|(path, _)| name(path)).collect();
        let mut changed: Vec<String> = self.git.files.iter().filter(|(_, letter)| matches!(letter, 'M' | 'S' | 'D')).map(|(path, _)| name(path)).collect();
        added.sort();
        changed.sort();
        let suggestion = suggest_message(&sections, &added, &changed);
        self.git_panel.update(cx, |panel, cx| panel.set_suggestion(suggestion, cx));
    }

    /// Fetches now and then, and fast-forwards when nothing local is in the
    /// way. Never pushes: that waits for the user.
    fn start_sync(&mut self, cx: &mut Context<Self>) {
        self.git.sync_task = Some(cx.spawn(async move |this, cx| {
            let mut wait = SYNC_INTERVAL;
            loop {
                cx.background_executor().timer(wait).await;
                let Ok(Some((repository, may_pull))) = this.update(cx, |this, cx| {
                    let dirty = this.documents.iter().any(|document| document.view.read(cx).state.is_dirty());
                    this.settings.git_background_sync.then(|| this.git.repository.clone().map(|repository| (repository, !dirty))).flatten()
                }) else {
                    if this.upgrade().is_none() {
                        return;
                    }
                    continue;
                };
                let result = cx.background_spawn(async move { sync_once(&repository, may_pull) }).await;
                let Ok(()) = this.update(cx, |this, cx| {
                    this.git.note = match &result {
                        Ok(SyncResult::Pulled(count)) => {
                            this.set_status(format!("Pulled {count} new commits."), cx);
                            None
                        }
                        Ok(SyncResult::Waiting(count)) => Some(format!("{count} new commits on the remote. Pull to bring them in.").into()),
                        Ok(SyncResult::Nothing) => None,
                        Err(GitError::Auth { host }) => Some(format!("Sign in to {host} to sync. Push or pull once to do it.").into()),
                        Err(GitError::Other(_)) => this.git.note.clone(),
                    };
                    this.refresh_git(cx);
                    this.git_panel.update(cx, |panel, cx| panel.refresh(cx));
                    cx.notify();
                }) else {
                    return;
                };
                // Offline or failing, it tries less often.
                wait = if result.is_ok() { SYNC_INTERVAL } else { (wait * 2).min(SYNC_INTERVAL_MAX) };
            }
        }));
    }

    pub(super) fn git_note(&self) -> Option<SharedString> {
        self.git.note.clone()
    }

    // Logins

    pub(super) fn open_sign_in(&mut self, host: String, window: &mut Window, cx: &mut Context<Self>) {
        let field = |cx: &mut Context<Self>| {
            cx.new(|cx| {
                let mut view = EditorView::new("", None, cx);
                view.set_plain();
                view
            })
        };
        let (user, token) = (field(cx), field(cx));
        self.git.sign_in = Some(SignIn { host, user, token: token.clone(), error: None });
        self.open_dialog(Dialog::GitSignIn, window, cx);
        window.focus(&token.read(cx).focus_handle(cx), cx);
    }

    fn save_sign_in(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(sign_in) = &self.git.sign_in else {
            return;
        };
        let host = sign_in.host.clone();
        let user = sign_in.user.read(cx).state.buffer().to_string();
        let token = sign_in.token.read(cx).state.buffer().to_string().trim().to_string();
        if token.is_empty() {
            if let Some(sign_in) = &mut self.git.sign_in {
                sign_in.error = Some("Paste a token first.".into());
            }
            cx.notify();
            return;
        }
        let handle = window.window_handle();
        cx.spawn(async move |this, cx| {
            let stored = cx.background_spawn(async move { git_login::store(&host, &user, &token) }).await;
            handle
                .update(cx, |_, window, cx| {
                    this.update(cx, |this, cx| match stored {
                        Ok(()) => {
                            this.git.sign_in = None;
                            this.close_dialog(window, cx);
                            let panel = this.git_panel.clone();
                            cx.defer(move |cx| panel.update(cx, |panel, cx| panel.retry_network(cx)));
                        }
                        Err(error) => {
                            if let Some(sign_in) = &mut this.git.sign_in {
                                sign_in.error = Some(error.into());
                            }
                            cx.notify();
                        }
                    })
                    .ok();
                })
                .ok();
        })
        .detach();
    }

    pub(super) fn render_sign_in(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Some(sign_in) = &self.git.sign_in else {
            return Vec::new();
        };
        let field = |view: &Entity<EditorView>| {
            div()
                .h(px(26.))
                .px(px(6.))
                .rounded(px(4.))
                .overflow_hidden()
                .bg(theme::color(theme::palette().bg))
                .border_1()
                .border_color(theme::color(theme::palette().divider))
                .child(view.clone())
                .into_any_element()
        };
        let muted = |text: String| div().flex_none().text_color(theme::color(theme::palette().ui_muted)).child(text).into_any_element();
        let heading = |text: &'static str| div().flex_none().pt(px(6.)).font_weight(FontWeight::SEMIBOLD).child(text).into_any_element();
        let mut content = vec![
            muted(format!(
                "{} wants a login for this repository. Create a personal access token with access to repositories in your account settings there, and paste it here. It stays in the system keychain.",
                sign_in.host
            )),
            heading("User name (optional)"),
            field(&sign_in.user),
            heading("Token"),
            field(&sign_in.token),
            div()
                .flex()
                .flex_none()
                .pt(px(6.))
                .child(self.panel_button("git-sign-in-save", "Save and try again", true, cx, |this, window, cx| this.save_sign_in(window, cx)))
                .into_any_element(),
        ];
        if let Some(error) = &sign_in.error {
            content.push(div().flex_none().text_color(theme::color(theme::palette().error)).child(error.clone()).into_any_element());
        }
        content
    }

    pub(super) fn sign_in_fields_focused(&self, window: &Window, cx: &gpui::App) -> bool {
        self.git
            .sign_in
            .as_ref()
            .is_some_and(|sign_in| sign_in.user.read(cx).focus_handle(cx).is_focused(window) || sign_in.token.read(cx).focus_handle(cx).is_focused(window))
    }

    pub(super) fn confirm_sign_in(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.save_sign_in(window, cx);
    }

    // Commits

    pub(super) fn show_commit(&mut self, title: &str, patch: &str, window: &mut Window, cx: &mut Context<Self>) {
        let decorations = patch_decorations(patch);
        let view = cx.new(|cx| {
            let mut view = EditorView::new(patch, None, cx);
            view.state.set_read_only(true);
            view.set_decorations(decorations, cx);
            view
        });
        self.git.commit_view = Some((title.to_string().into(), view));
        self.open_dialog(Dialog::CommitDiff, window, cx);
    }

    pub(super) fn commit_title(&self) -> SharedString {
        self.git.commit_view.as_ref().map_or_else(|| "Commit".into(), |(title, _)| title.clone())
    }

    pub(super) fn render_commit_view(&self) -> Vec<AnyElement> {
        self.git
            .commit_view
            .as_ref()
            .map(|(_, view)| vec![div().flex_1().min_h(px(400.)).h_full().child(view.clone()).into_any_element()])
            .unwrap_or_default()
    }

    // Conflicts

    /// Opens the first conflicted file after a merge stopped.
    pub(super) fn show_conflicts(&mut self, files: &[PathBuf], window: &mut Window, cx: &mut Context<Self>) {
        if let Some(first) = files.first() {
            self.open_file(first.clone(), window, cx);
        }
        self.refresh_git(cx);
    }

    /// The conflict at or after the cursor of the active document, with its
    /// number and the count.
    fn current_conflict(&self, cx: &gpui::App) -> Option<(Entity<EditorView>, usize, usize)> {
        let view = self.active_view()?;
        let state = view.read(cx);
        let conflicts = state.conflicts();
        if conflicts.is_empty() {
            return None;
        }
        let cursor = state.state.selections().primary().head;
        let index = conflicts.iter().position(|conflict| conflict.whole.end > cursor).unwrap_or(0);
        Some((view.clone(), index, conflicts.len()))
    }

    fn resolve_current(&mut self, choice: Choice, window: &mut Window, cx: &mut Context<Self>) {
        let Some((view, index, count)) = self.current_conflict(cx) else {
            return;
        };
        view.update(cx, |view, cx| view.resolve_conflict(index, choice, cx));
        if count == 1 {
            let name = view.read(cx).title();
            self.set_status(format!("All conflicts in {name} are solved. Finish the merge in the git panel."), cx);
        } else {
            self.next_conflict(window, cx);
        }
    }

    fn next_conflict(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((view, index, _)) = self.current_conflict(cx) else {
            return;
        };
        let conflicts = view.read(cx).conflicts();
        let cursor = view.read(cx).state.selections().primary().head;
        let target = if conflicts[index].whole.start > cursor { index } else { (index + 1) % conflicts.len() };
        let start = conflicts[target].whole.start;
        view.update(cx, |view, cx| view.select_range(start..start, window, cx));
    }

    pub(super) fn render_conflict_strip(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (_, index, count) = self.current_conflict(cx)?;
        let button = |id: &'static str, label: &'static str, cx: &mut Context<Self>, handler: fn(&mut Self, &mut Window, &mut Context<Self>)| {
            div()
                .id(id)
                .flex_none()
                .px(px(8.))
                .py(px(2.))
                .rounded(px(4.))
                .cursor_pointer()
                .bg(theme::color(theme::palette().bg))
                .hover(|style| style.bg(theme::color(theme::palette().panel)))
                .on_click(cx.listener(move |this, _, window, cx| handler(this, window, cx)))
                .child(label)
        };
        Some(
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap(px(6.))
                .h(px(32.))
                .px(px(10.))
                .bg(theme::color(theme::palette().divider))
                .child(div().flex_1().min_w_0().truncate().font_weight(FontWeight::SEMIBOLD).child(format!("Conflict {} of {count}", index + 1)))
                .child(button("conflict-ours", "Keep mine", cx, |this, window, cx| this.resolve_current(Choice::Ours, window, cx)))
                .child(button("conflict-theirs", "Keep theirs", cx, |this, window, cx| this.resolve_current(Choice::Theirs, window, cx)))
                .child(button("conflict-both", "Keep both", cx, |this, window, cx| this.resolve_current(Choice::Both, window, cx)))
                .child(button("conflict-next", "Next", cx, |this, window, cx| this.next_conflict(window, cx)))
                .into_any_element(),
        )
    }

    // Tree

    /// The letter and color of a file or folder in the tree.
    pub(super) fn tree_git_mark(&self, path: &Path, is_dir: bool) -> Option<(char, u32)> {
        if is_dir {
            return self.git.folders.contains(path).then(|| ('•', theme::palette().warning));
        }
        self.git.files.get(path).map(|letter| (*letter, letter_color(*letter)))
    }

    /// Keys of the commit message and the login. Returns whether one was taken.
    pub(super) fn git_key(&mut self, event: &gpui::KeystrokeEvent, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let key = event.keystroke.key.as_str();
        let modifiers = event.keystroke.modifiers;
        if self.git_panel.read(cx).contains_focus(window, cx) {
            if key == "enter" && modifiers.platform {
                cx.stop_propagation();
                // The panel saves through the workspace, which is busy until this returns.
                let panel = self.git_panel.clone();
                cx.defer(move |cx| panel.update(cx, |panel, cx| panel.commit_and_maybe_push(true, cx)));
                return true;
            }
            if key == "tab" && !modifiers.shift && self.git_panel.update(cx, |panel, cx| panel.take_suggestion(cx)) {
                cx.stop_propagation();
                return true;
            }
            return false;
        }
        if key == "enter" && self.dialog == Some(Dialog::GitSignIn) && self.sign_in_fields_focused(window, cx) {
            cx.stop_propagation();
            self.confirm_sign_in(window, cx);
            return true;
        }
        false
    }

    pub(super) fn open_git_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.show_file_panel = true;
        self.left_panel = LeftPanel::Git;
        self.git_panel.update(cx, |panel, cx| panel.focus_commit_box(window, cx));
        cx.notify();
    }
}

enum SyncResult {
    Nothing,
    Pulled(usize),
    /// New commits wait, which a fast-forward cannot bring in now.
    Waiting(usize),
}

fn sync_once(repository: &Repository, may_pull: bool) -> Result<SyncResult, GitError> {
    if !repository.has_upstream() {
        return Ok(SyncResult::Nothing);
    }
    repository.fetch()?;
    let status = repository.status()?;
    if status.behind == 0 {
        return Ok(SyncResult::Nothing);
    }
    if !may_pull || status.ahead > 0 {
        return Ok(SyncResult::Waiting(status.behind));
    }
    match repository.pull() {
        Ok(()) => Ok(SyncResult::Pulled(status.behind)),
        // Local changes in the way of a fast-forward.
        Err(GitError::Other(_)) => Ok(SyncResult::Waiting(status.behind)),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| name.to_string()).collect()
    }

    #[test]
    fn suggestions_name_sections_first_then_files() {
        assert_eq!(suggest_message(&names(&["Introduction"]), &[], &names(&["main.tex"])).as_deref(), Some("Update Introduction"));
        assert_eq!(suggest_message(&names(&["Methods", "Results"]), &[], &[]).as_deref(), Some("Update Methods and Results"));
        assert_eq!(suggest_message(&names(&["A", "B", "C"]), &[], &[]).as_deref(), Some("Update A and 2 more sections"));
        assert_eq!(suggest_message(&[], &names(&["frog.jpg"]), &[]).as_deref(), Some("Add frog.jpg"));
        assert_eq!(suggest_message(&[], &[], &names(&["main.tex", "sample.bib"])).as_deref(), Some("Update main.tex and sample.bib"));
        assert_eq!(suggest_message(&[], &names(&["a.png"]), &names(&["x.tex", "y.tex", "z.tex"])).as_deref(), Some("Update x.tex and 2 more, add a.png"));
        assert_eq!(suggest_message(&[], &[], &[]), None);
    }

    #[test]
    fn patches_color_their_own_lines_only() {
        let patch = "commit abc\n--- not a removal\n\ndiff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -1 +1 @@\n-old\n+new\n context\n";
        let decorations = patch_decorations(patch);
        let colored: Vec<&str> = decorations.iter().map(|decoration| &patch[decoration.range.clone()]).collect();
        assert_eq!(colored, ["-old", "+new"]);
    }

    fn git(directory: &Path, args: &[&str]) {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(directory)
            .envs([
                ("GIT_CONFIG_GLOBAL", "/dev/null"),
                ("GIT_CONFIG_NOSYSTEM", "1"),
                ("GIT_AUTHOR_NAME", "Ada"),
                ("GIT_AUTHOR_EMAIL", "ada@example.com"),
                ("GIT_COMMITTER_NAME", "Ada"),
                ("GIT_COMMITTER_EMAIL", "ada@example.com"),
            ])
            .output()
            .unwrap();
        assert!(output.status.success(), "{args:?}: {}", String::from_utf8_lossy(&output.stderr));
    }

    #[test]
    fn sync_pulls_only_when_nothing_is_in_the_way() {
        let base = tempfile::tempdir().unwrap();
        let (remote, mine, theirs) = (base.path().join("remote.git"), base.path().join("mine"), base.path().join("theirs"));
        git(base.path(), &["init", "-q", "--bare", "-b", "main", remote.to_str().unwrap()]);
        git(base.path(), &["clone", "-q", remote.to_str().unwrap(), mine.to_str().unwrap()]);
        std::fs::write(mine.join("a.tex"), "a\n").unwrap();
        git(&mine, &["add", "."]);
        git(&mine, &["commit", "-q", "-m", "a"]);
        git(&mine, &["push", "-q", "-u", "origin", "main"]);
        git(base.path(), &["clone", "-q", remote.to_str().unwrap(), theirs.to_str().unwrap()]);
        let commit_theirs = |name: &str| {
            std::fs::write(theirs.join(name), "x\n").unwrap();
            git(&theirs, &["add", "."]);
            git(&theirs, &["commit", "-q", "-m", name]);
            git(&theirs, &["push", "-q"]);
        };
        let repository = git_panel::program().unwrap().discover(&mine).unwrap().unwrap();

        commit_theirs("b.tex");
        // Unsaved documents keep the files as they are.
        assert!(matches!(sync_once(&repository, false), Ok(SyncResult::Waiting(1))));
        assert!(matches!(sync_once(&repository, true), Ok(SyncResult::Pulled(1))));
        assert!(mine.join("b.tex").exists());
        assert!(matches!(sync_once(&repository, true), Ok(SyncResult::Nothing)));

        // Local commits make it a merge, which waits for the user.
        commit_theirs("c.tex");
        std::fs::write(mine.join("d.tex"), "d\n").unwrap();
        git(&mine, &["add", "."]);
        git(&mine, &["commit", "-q", "-m", "d"]);
        assert!(matches!(sync_once(&repository, true), Ok(SyncResult::Waiting(1))));
        assert!(!mine.join("c.tex").exists());
    }
}

