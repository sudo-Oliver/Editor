//! The panel for staging, committing, branches and history. Git runs only
//! while the panel is visible, because status can start programs named in the
//! config of a repository.

use std::{
    ops::Range,
    path::{Path, PathBuf},
};

use build::{child_path, find_tool};
use git::{Commit, FileChange, GraphRow, Lanes, PAGE_SIZE, Program, Repository, Status};
use gpui::{
    AnyElement, Bounds, Context, Entity, EventEmitter, Focusable, FontWeight, PathBuilder, Pixels, Render,
    SharedString, Task, UniformListScrollHandle, WeakEntity, Window, canvas, div, fill, point, prelude::*, px, size, svg,
    uniform_list,
};

use crate::{actions::Recompile, editor_view::EditorView, theme, workspace::Workspace};

const COMMIT_ROW_HEIGHT: f32 = 38.0;
const LANE_WIDTH: f32 = 10.0;
const MAX_LANES_SHOWN: usize = 8;
/// A folder of generated files should not cost thousands of rows.
const MAX_FILES_SHOWN: usize = 500;
const MAX_REFS_SHOWN: usize = 2;
/// How close to the end of the list the next page starts to load.
const LOAD_MARGIN: usize = 40;
const LANE_COLORS: [u32; 6] = [theme::CYAN, theme::PINK, theme::ORANGE, theme::ST_GREEN, theme::CARET, theme::ST_INSERT];

type Handler = Box<dyn Fn(&mut GitPanel, &mut Context<GitPanel>)>;
/// The icon and the action of a button in a list row.
type RowAction = Option<(&'static str, Handler)>;

pub enum GitPanelEvent {
    OpenFile(PathBuf),
    PickBranch,
    /// Git changed files or the branch.
    Changed(Option<String>),
}

enum RepoState {
    /// Nothing has been read yet.
    Unknown,
    NoProject,
    GitMissing,
    NoRepository,
    Ready(Repository),
}

pub struct GitPanel {
    workspace: WeakEntity<Workspace>,
    root: Option<PathBuf>,
    program: Option<Program>,
    state: RepoState,
    status: Status,
    /// Why the last action failed. Stays until the next action.
    error: Option<String>,
    /// Why reading failed. Goes with the next read that works.
    read_error: Option<String>,
    busy: bool,
    visible: bool,
    commit_box: Entity<EditorView>,
    show_history: bool,
    commits: Vec<Commit>,
    rows: Vec<GraphRow>,
    lanes: Lanes,
    /// How many lanes the loaded rows use.
    lanes_used: usize,
    /// Equals the refs of the loaded history. A difference reloads it.
    history_refs: Option<String>,
    history_done: bool,
    history_task: Option<Task<()>>,
    history_scroll: UniformListScrollHandle,
    refresh_task: Option<Task<()>>,
    refresh_again: bool,
    action_task: Option<Task<()>>,
}

impl EventEmitter<GitPanelEvent> for GitPanel {}

impl GitPanel {
    pub fn new(workspace: WeakEntity<Workspace>, cx: &mut Context<Self>) -> Self {
        let commit_box = cx.new(|cx| {
            let mut view = EditorView::new("", None, cx);
            view.set_plain();
            view
        });
        cx.observe(&commit_box, |_, _, cx| cx.notify()).detach();
        Self {
            workspace,
            root: None,
            program: None,
            state: RepoState::Unknown,
            status: Status::default(),
            error: None,
            read_error: None,
            busy: false,
            visible: false,
            commit_box,
            show_history: true,
            commits: Vec::new(),
            rows: Vec::new(),
            lanes: Lanes::default(),
            lanes_used: 1,
            history_refs: None,
            history_done: false,
            history_task: None,
            history_scroll: UniformListScrollHandle::new(),
            refresh_task: None,
            refresh_again: false,
            action_task: None,
        }
    }

    pub fn set_root(&mut self, root: Option<PathBuf>, cx: &mut Context<Self>) {
        self.root = root;
        self.state = RepoState::Unknown;
        self.status = Status::default();
        self.error = None;
        self.read_error = None;
        self.reset_history();
        self.refresh(cx);
    }

    pub fn set_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if self.visible != visible {
            self.visible = visible;
            self.refresh(cx);
        }
    }

    pub fn repository(&self) -> Option<Repository> {
        match &self.state {
            RepoState::Ready(repository) => Some(repository.clone()),
            _ => None,
        }
    }

    pub fn contains_focus(&self, window: &Window, cx: &gpui::App) -> bool {
        self.commit_box.read(cx).focus_handle(cx).is_focused(window)
    }

    fn reset_history(&mut self) {
        self.commits.clear();
        self.rows.clear();
        self.lanes = Lanes::default();
        self.lanes_used = 1;
        self.history_refs = None;
        self.history_done = false;
        self.history_task = None;
    }

    /// Reads the repository again. Calls during a read lead to one more read.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        if !self.visible {
            return;
        }
        if self.refresh_task.is_some() {
            self.refresh_again = true;
            return;
        }
        let Some(root) = self.root.clone() else {
            self.state = RepoState::NoProject;
            cx.notify();
            return;
        };
        if self.program.is_none() {
            self.program = find_tool("git").map(|path| Program { path, path_env: child_path() });
        }
        let Some(program) = self.program.clone() else {
            self.state = RepoState::GitMissing;
            cx.notify();
            return;
        };
        self.refresh_task = Some(cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async move { read_repository(&program, &root) }).await;
            this.update(cx, |this, cx| {
                this.refresh_task = None;
                this.apply_read(result, cx);
                if std::mem::take(&mut this.refresh_again) {
                    this.refresh(cx);
                }
            })
            .ok();
        }));
    }

    fn apply_read(&mut self, result: Result<Option<RepositoryRead>, String>, cx: &mut Context<Self>) {
        self.read_error = result.as_ref().err().cloned();
        match result {
            Err(_) => {}
            Ok(None) => {
                self.state = RepoState::NoRepository;
                self.status = Status::default();
                self.reset_history();
            }
            Ok(Some(read)) => {
                let branch_changed = self.status.branch != read.status.branch;
                self.state = RepoState::Ready(read.repository);
                self.status = read.status;
                if self.history_refs.as_deref() != Some(&read.refs) {
                    self.reset_history();
                    self.history_refs = Some(read.refs);
                    self.load_history(cx);
                }
                if branch_changed {
                    cx.emit(GitPanelEvent::Changed(self.status.branch.clone()));
                }
            }
        }
        cx.notify();
    }

    /// Loads the next page, unless one is on its way or none is left.
    fn load_history(&mut self, cx: &mut Context<Self>) {
        if self.history_done || self.history_task.is_some() || !self.show_history {
            return;
        }
        let Some(repository) = self.repository() else {
            return;
        };
        // A branch without commits has no history for `git log` to read.
        if self.status.oid.is_none() {
            self.history_done = true;
            return;
        }
        let skip = self.commits.len();
        self.history_task = Some(cx.spawn(async move |this, cx| {
            let page = cx.background_spawn(async move { repository.log(skip) }).await;
            this.update(cx, |this, cx| {
                this.history_task = None;
                match page {
                    Ok(page) => {
                        this.history_done = page.len() < PAGE_SIZE;
                        for commit in &page {
                            let row = this.lanes.next_row(&commit.hash, &commit.parents);
                            let widest = row.through.iter().chain(&row.incoming).chain(&row.outgoing).chain([&row.column]).max();
                            this.lanes_used = this.lanes_used.max(widest.map_or(1, |lane| lane + 1));
                            this.rows.push(row);
                        }
                        this.commits.extend(page);
                    }
                    Err(error) => {
                        this.history_done = true;
                        this.error = Some(error);
                    }
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// Open documents go to disk first, because git reads files from there.
    fn save_documents(&self, cx: &mut Context<Self>) -> bool {
        self.workspace.update(cx, |workspace, cx| workspace.save_all(cx)).unwrap_or(false)
    }

    /// Runs one change at a time, in the background.
    pub fn run(&mut self, cx: &mut Context<Self>, action: impl FnOnce(&Repository) -> Result<(), String> + Send + 'static) {
        if self.busy {
            return;
        }
        let Some(repository) = self.repository() else {
            return;
        };
        self.busy = true;
        self.error = None;
        cx.notify();
        self.action_task = Some(cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async move { action(&repository) }).await;
            this.update(cx, |this, cx| {
                this.finish_action(result, cx);
            })
            .ok();
        }));
    }

    fn finish_action(&mut self, result: Result<(), String>, cx: &mut Context<Self>) {
        self.busy = false;
        self.action_task = None;
        self.error = result.err();
        self.refresh(cx);
        cx.emit(GitPanelEvent::Changed(self.status.branch.clone()));
        cx.notify();
    }

    fn change_with_saved_files(
        &mut self,
        cx: &mut Context<Self>,
        action: impl FnOnce(&Repository) -> Result<(), String> + Send + 'static,
    ) {
        if self.save_documents(cx) {
            self.run(cx, action);
        }
    }

    fn init(&mut self, cx: &mut Context<Self>) {
        let (Some(program), Some(root)) = (self.program.clone(), self.root.clone()) else {
            return;
        };
        if self.busy {
            return;
        }
        self.busy = true;
        self.error = None;
        self.action_task = Some(cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async move { program.init(&root).map(drop) }).await;
            this.update(cx, |this, cx| this.finish_action(result, cx)).ok();
        }));
        cx.notify();
    }

    fn commit_message(&self, cx: &gpui::App) -> String {
        self.commit_box.read(cx).state.buffer().to_string().trim().to_string()
    }

    fn can_commit(&self, cx: &gpui::App) -> bool {
        !self.busy && self.status.files.iter().any(|file| file.staged.is_some()) && !self.commit_message(cx).is_empty()
    }

    fn commit(&mut self, cx: &mut Context<Self>) {
        if !self.can_commit(cx) || !self.save_documents(cx) {
            return;
        }
        let Some(repository) = self.repository() else {
            return;
        };
        let message = self.commit_message(cx);
        self.busy = true;
        self.error = None;
        self.action_task = Some(cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async move { repository.commit(&message) }).await;
            this.update(cx, |this, cx| {
                // A failed commit keeps the message for the next try.
                if result.is_ok() {
                    this.commit_box.update(cx, |view, cx| view.clear(cx));
                }
                this.finish_action(result, cx);
            })
            .ok();
        }));
        cx.notify();
    }

    fn icon(name: &'static str, size: f32, color: u32) -> impl IntoElement {
        svg().path(format!("icons/{name}.svg")).size(px(size)).flex_none().text_color(theme::color(color))
    }

    fn icon_button(
        &self,
        id: impl Into<gpui::ElementId>,
        icon: &'static str,
        cx: &mut Context<Self>,
        handler: impl Fn(&mut Self, &mut Context<Self>) + 'static,
    ) -> gpui::Stateful<gpui::Div> {
        let busy = self.busy;
        div()
            .id(id)
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(px(22.))
            .rounded(px(4.))
            .when(!busy, |style| style.cursor_pointer().hover(|style| style.bg(theme::color(theme::DIVIDER))))
            .when(busy, |style| style.opacity(0.5))
            .on_click(cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                if !this.busy {
                    handler(this, cx);
                }
            }))
            .child(Self::icon(icon, 14., theme::UI_TEXT))
    }

    fn render_head(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let branch: SharedString = match (&self.status.branch, &self.status.oid) {
            (Some(branch), _) => branch.clone().into(),
            (None, Some(oid)) => oid.chars().take(7).collect::<String>().into(),
            (None, None) => "No branch".into(),
        };
        let counts = (self.status.ahead > 0 || self.status.behind > 0)
            .then(|| format!("↑{} ↓{}", self.status.ahead, self.status.behind));
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(2.))
            .h(px(32.))
            .px(px(6.))
            .child(
                div()
                    .id("git-branch")
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .items_center()
                    .gap(px(5.))
                    .h(px(24.))
                    .px(px(4.))
                    .rounded(px(4.))
                    .cursor_pointer()
                    .font_weight(FontWeight::SEMIBOLD)
                    .hover(|style| style.bg(theme::color(theme::DIVIDER)))
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(GitPanelEvent::PickBranch)))
                    .child(Self::icon("git-branch", 14., theme::UI_TEXT))
                    .child(div().min_w_0().truncate().child(branch)),
            )
            .children(counts.map(|counts| div().flex_none().px(px(3.)).text_color(theme::color(theme::UI_MUTED)).child(counts)))
            .child(self.icon_button("git-pull", "download", cx, |this, cx| {
                this.change_with_saved_files(cx, Repository::pull);
            }))
            .child(self.icon_button("git-push", "upload", cx, |this, cx| {
                this.run(cx, Repository::push);
            }))
            .child(self.icon_button("git-refresh", "refresh", cx, |this, cx| this.refresh(cx)))
    }

    fn render_group_title(
        &self,
        id: &'static str,
        title: &'static str,
        count: usize,
        action: RowAction,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .justify_between()
            .h(px(26.))
            .pl(px(10.))
            .pr(px(6.))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(theme::color(theme::UI_MUTED))
            .child(format!("{title} ({count})"))
            .children(action.map(|(icon, handler)| {
                self.icon_button(id, icon, cx, move |this, cx| handler(this, cx))
            }))
    }

    fn render_file(
        &self,
        id: &'static str,
        index: usize,
        file: &FileChange,
        letter: char,
        action: RowAction,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let (directory, name) = split_path(&file.path);
        let path = self.repository().map(|repository| repository.root().join(&file.path));
        let deleted = letter == 'D';
        let color = match letter {
            'A' | '?' => theme::ST_GREEN,
            'D' | 'U' => theme::ST_RED,
            'R' | 'C' => theme::CYAN,
            _ => theme::ORANGE,
        };
        div()
            .id((id, index))
            .flex()
            .items_center()
            .gap(px(6.))
            .h(px(24.))
            .pl(px(10.))
            .pr(px(6.))
            .when(!deleted, |style| style.cursor_pointer())
            .hover(|style| style.bg(theme::color(theme::PANEL)))
            .on_click(cx.listener(move |_, _, _, cx| {
                if let Some(path) = path.clone().filter(|_| !deleted) {
                    cx.emit(GitPanelEvent::OpenFile(path));
                }
            }))
            .child(div().flex_none().w(px(10.)).font_family(theme::CODE_FONT).text_color(theme::color(color)).child(letter.to_string()))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .gap(px(5.))
                    .child(div().flex_none().max_w(gpui::relative(0.75)).truncate().child(name))
                    .child(div().min_w_0().truncate().text_color(theme::color(theme::UI_MUTED)).child(directory)),
            )
            .children(action.map(|(icon, handler)| {
                self.icon_button(("git-file-action", index_of(id, index)), icon, cx, move |this, cx| handler(this, cx))
            }))
    }

    fn render_changes(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let conflicts: Vec<&FileChange> = self.status.files.iter().filter(|file| file.conflicted).collect();
        let staged: Vec<&FileChange> = self.status.files.iter().filter(|file| file.staged.is_some()).collect();
        let changed: Vec<&FileChange> = self.status.files.iter().filter(|file| file.unstaged.is_some()).collect();
        let nothing = conflicts.is_empty() && staged.is_empty() && changed.is_empty();

        let mut list = div()
            .id("git-changes")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll();
        if nothing {
            list = list.child(div().p(px(10.)).text_color(theme::color(theme::UI_MUTED)).child("No changes."));
        }
        if !conflicts.is_empty() {
            list = list.child(self.render_group_title("git-conflicts", "Conflicts", conflicts.len(), None, cx));
            for (index, file) in conflicts.iter().take(MAX_FILES_SHOWN).enumerate() {
                list = list.child(self.render_file("git-conflict", index, file, 'U', None, cx));
            }
        }
        if !staged.is_empty() {
            let unstage_all: Handler = Box::new(|this, cx| this.run(cx, Repository::unstage_all));
            list = list.child(self.render_group_title("git-unstage-all", "Staged", staged.len(), Some(("minus", unstage_all)), cx));
            for (index, file) in staged.iter().take(MAX_FILES_SHOWN).enumerate() {
                let owned = (*file).clone();
                let unstage: Handler = Box::new(move |this, cx| {
                    let file = owned.clone();
                    this.run(cx, move |repository| repository.unstage(&file));
                });
                let letter = file.staged.unwrap_or('M');
                list = list.child(self.render_file("git-staged", index, file, letter, Some(("minus", unstage)), cx));
            }
        }
        if !changed.is_empty() {
            let stage_all: Handler = Box::new(|this, cx| this.change_with_saved_files(cx, Repository::stage_all));
            list = list.child(self.render_group_title("git-stage-all", "Changes", changed.len(), Some(("plus", stage_all)), cx));
            for (index, file) in changed.iter().take(MAX_FILES_SHOWN).enumerate() {
                let owned = (*file).clone();
                let stage: Handler = Box::new(move |this, cx| {
                    let file = owned.clone();
                    this.change_with_saved_files(cx, move |repository| repository.stage(&file));
                });
                let letter = file.unstaged.unwrap_or('M');
                list = list.child(self.render_file("git-changed", index, file, letter, Some(("plus", stage)), cx));
            }
        }
        let hidden = conflicts.len().max(staged.len()).max(changed.len()).saturating_sub(MAX_FILES_SHOWN);
        if hidden > 0 {
            list = list.child(div().p(px(10.)).text_color(theme::color(theme::UI_MUTED)).child(format!("{hidden} more not shown.")));
        }
        list
    }

    fn render_commit_box(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let empty = self.commit_box.read(cx).state.buffer().is_empty();
        let enabled = self.can_commit(cx);
        div()
            .flex()
            .flex_col()
            .flex_none()
            .gap(px(6.))
            .p(px(8.))
            .border_t_1()
            .border_color(theme::color(theme::PANEL))
            .child(
                div()
                    .relative()
                    .h(px(62.))
                    .rounded(px(4.))
                    .border_1()
                    .border_color(theme::color(theme::DIVIDER))
                    .overflow_hidden()
                    .child(self.commit_box.clone())
                    .when(empty, |element| {
                        element.child(
                            div()
                                .absolute()
                                .top_0()
                                .left(px(6.))
                                .font_family(theme::CODE_FONT)
                                .text_size(px(theme::CODE_FONT_SIZE))
                                .line_height(px(theme::CODE_LINE_HEIGHT))
                                .text_color(theme::color(theme::UI_MUTED))
                                .child("Commit message"),
                        )
                    }),
            )
            .child(
                div()
                    .id("git-commit")
                    .flex()
                    .items_center()
                    .justify_center()
                    .h(px(26.))
                    .rounded(px(4.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .bg(theme::color(if enabled { theme::GREEN } else { theme::DIVIDER }))
                    .when(enabled, |style| style.cursor_pointer())
                    .when(!enabled, |style| style.text_color(theme::color(theme::UI_MUTED)))
                    .on_click(cx.listener(|this, _, _, cx| this.commit(cx)))
                    .child("Commit"),
            )
    }

    fn render_commit(&self, index: usize, graph_width: f32) -> AnyElement {
        let commit = &self.commits[index];
        let row = self.rows[index].clone();
        let pills = commit.refs.iter().take(MAX_REFS_SHOWN).map(|name| {
            let name = name.strip_prefix("HEAD -> ").unwrap_or(name).to_string();
            div()
                .flex_none()
                .max_w(px(90.))
                .truncate()
                .px(px(4.))
                .rounded(px(3.))
                .bg(theme::color(theme::GREEN_DARK))
                .text_size(px(11.))
                .child(name)
        });
        div()
            .id(("git-commit-row", index))
            .flex()
            .w_full()
            .h(px(COMMIT_ROW_HEIGHT))
            .pr(px(8.))
            .child(
                canvas(
                    |_, _, _| {},
                    move |bounds, _, window, _| paint_graph_row(&row, bounds, window),
                )
                .flex_none()
                .w(px(graph_width))
                .h_full(),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .justify_center()
                    .child(
                        div()
                            .flex()
                            .min_w_0()
                            .items_center()
                            .gap(px(4.))
                            .children(pills)
                            .child(div().flex_1().min_w_0().truncate().child(commit.subject.clone())),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_size(px(11.))
                            .text_color(theme::color(theme::UI_MUTED))
                            .child(format!("{} · {}", commit.author, commit.age)),
                    ),
            )
            .into_any_element()
    }

    fn render_history(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let open = self.show_history;
        let graph_width = LANE_WIDTH * self.lanes_used.min(MAX_LANES_SHOWN) as f32 + 8.;
        div()
            .flex()
            .flex_col()
            .when(open, |style| style.flex_1().min_h_0())
            .when(!open, |style| style.flex_none())
            .border_t_1()
            .border_color(theme::color(theme::PANEL))
            .child(
                div()
                    .id("git-history-header")
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(6.))
                    .h(px(30.))
                    .px(px(10.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.show_history = !this.show_history;
                        this.load_history(cx);
                        cx.notify();
                    }))
                    .child(Self::icon(if open { "chevron-down" } else { "chevron-right" }, 12., theme::UI_TEXT))
                    .child("History"),
            )
            .when(open, |element| {
                element.child(
                    uniform_list(
                        "git-history",
                        self.commits.len(),
                        cx.processor(move |this, range: Range<usize>, _, cx| {
                            if range.end + LOAD_MARGIN >= this.commits.len() {
                                this.load_history(cx);
                            }
                            range.map(|index| this.render_commit(index, graph_width)).collect::<Vec<_>>()
                        }),
                    )
                    .flex_1()
                    .min_h_0()
                    .track_scroll(&self.history_scroll),
                )
            })
    }

    fn render_message(&self, message: &'static str, action: Option<(&'static str, gpui::Stateful<gpui::Div>)>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .p(px(10.))
            .child(div().text_color(theme::color(theme::UI_MUTED)).child(message))
            .children(action.map(|(label, button)| button.child(label)))
    }
}

struct RepositoryRead {
    repository: Repository,
    status: Status,
    refs: String,
}

fn read_repository(program: &Program, root: &Path) -> Result<Option<RepositoryRead>, String> {
    let Some(repository) = program.discover(root)? else {
        return Ok(None);
    };
    let status = repository.status()?;
    let refs = repository.refs_fingerprint()?;
    Ok(Some(RepositoryRead { repository, status, refs }))
}

/// Whether a change on disk can matter to the tree, open files or git. Of the
/// internals of `.git`, only what git status and history depend on counts,
/// which leaves out lock files and objects.
pub fn is_watched_path(path: &Path) -> bool {
    let mut parts = path.components().map(|part| part.as_os_str());
    while let Some(part) = parts.next() {
        if part == ".build" {
            return false;
        }
        if part == ".git" {
            return parts.next().is_some_and(|inner| {
                inner == "HEAD" || inner == "index" || inner == "FETCH_HEAD" || inner == "packed-refs" || inner == "refs"
            }) && path.extension().is_none_or(|extension| extension != "lock");
        }
    }
    true
}

fn split_path(path: &str) -> (String, String) {
    match path.rsplit_once('/') {
        Some((directory, name)) => (directory.to_string(), name.to_string()),
        None => (String::new(), path.to_string()),
    }
}

/// Keeps the action buttons of the file groups apart.
fn index_of(group: &str, index: usize) -> usize {
    let offset = match group {
        "git-staged" => 0,
        "git-changed" => MAX_FILES_SHOWN + 1,
        _ => (MAX_FILES_SHOWN + 1) * 2,
    };
    offset + index
}

fn lane_x(lane: usize, bounds: &Bounds<Pixels>) -> Pixels {
    bounds.left() + px(LANE_WIDTH * lane as f32 + LANE_WIDTH / 2. + 4.)
}

fn lane_color(lane: usize) -> gpui::Hsla {
    theme::color(LANE_COLORS[lane % LANE_COLORS.len()])
}

fn paint_graph_row(row: &GraphRow, bounds: Bounds<Pixels>, window: &mut Window) {
    let (top, bottom) = (bounds.top(), bounds.bottom());
    let middle = top + bounds.size.height / 2.;
    let shown = |lane: &usize| *lane < MAX_LANES_SHOWN;
    let dot = point(lane_x(row.column, &bounds), middle);
    let mut line = |from: gpui::Point<Pixels>, to: gpui::Point<Pixels>, lane: usize| {
        let mut path = PathBuilder::stroke(px(1.5));
        path.move_to(from);
        path.line_to(to);
        if let Ok(path) = path.build() {
            window.paint_path(path, lane_color(lane));
        }
    };
    for lane in row.through.iter().filter(|lane| shown(lane)) {
        let x = lane_x(*lane, &bounds);
        line(point(x, top), point(x, bottom), *lane);
    }
    if row.column < MAX_LANES_SHOWN {
        for lane in row.incoming.iter().filter(|lane| shown(lane)) {
            line(point(lane_x(*lane, &bounds), top), dot, *lane);
        }
        for lane in row.outgoing.iter().filter(|lane| shown(lane)) {
            line(dot, point(lane_x(*lane, &bounds), bottom), *lane);
        }
        let radius = px(3.5);
        let corner = point(dot.x - radius, dot.y - radius);
        window.paint_quad(fill(Bounds::new(corner, size(radius * 2., radius * 2.)), lane_color(row.column)).corner_radii(radius));
    }
}

impl Render for GitPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let panel = div().flex().flex_col().size_full();
        let failed = self.error.is_some() || self.read_error.is_some();
        let error = self.error.clone().or_else(|| self.read_error.clone()).map(|error| {
            div()
                .flex_none()
                .max_h(px(120.))
                .overflow_hidden()
                .p(px(8.))
                .bg(theme::color(theme::ERROR_PANEL))
                .text_size(px(12.))
                .child(error)
        });
        match &self.state {
            RepoState::Unknown => panel
                .when(!failed, |panel| panel.child(self.render_message("Reading the repository…", None)))
                .children(error)
                .into_any_element(),
            RepoState::NoProject => panel.child(self.render_message("Open a folder to start.", None)).into_any_element(),
            RepoState::GitMissing => panel.child(self.render_message("Git was not found.", None)).into_any_element(),
            RepoState::NoRepository => {
                let button = div()
                    .id("git-init")
                    .flex_none()
                    .px(px(10.))
                    .py(px(5.))
                    .rounded(px(4.))
                    .text_center()
                    .cursor_pointer()
                    .bg(theme::color(theme::GREEN))
                    .font_weight(FontWeight::SEMIBOLD)
                    .on_click(cx.listener(|this, _, _, cx| this.init(cx)));
                panel
                    .child(self.render_message("This folder is not a Git repository.", Some(("Initialize repository", button))))
                    .children(error)
                    .into_any_element()
            }
            RepoState::Ready(_) => panel
                // The commit box is the only thing that takes focus here.
                .on_action(cx.listener(|this, _: &Recompile, _, cx| this.commit(cx)))
                .child(self.render_head(cx))
                .children(error)
                .child(self.render_changes(cx))
                .child(self.render_commit_box(cx))
                .child(self.render_history(cx))
                .into_any_element(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn watcher_skips_build_output_and_git_internals() {
        assert!(is_watched_path(Path::new("/p/main.tex")));
        assert!(is_watched_path(Path::new("/p/.git/HEAD")));
        assert!(is_watched_path(Path::new("/p/.git/index")));
        assert!(is_watched_path(Path::new("/p/.git/refs/heads/main")));
        assert!(is_watched_path(Path::new("/p/.git/packed-refs")));
        assert!(!is_watched_path(Path::new("/p/.git/index.lock")));
        assert!(!is_watched_path(Path::new("/p/.git/refs/heads/main.lock")));
        assert!(!is_watched_path(Path::new("/p/.git/objects/ab/cd")));
        assert!(!is_watched_path(Path::new("/p/.git")));
        assert!(!is_watched_path(Path::new("/p/.build/main.pdf")));
    }

    #[test]
    fn paths_split_into_directory_and_name() {
        assert_eq!(split_path("a/b/c.tex"), ("a/b".to_string(), "c.tex".to_string()));
        assert_eq!(split_path("c.tex"), (String::new(), "c.tex".to_string()));
    }
}
