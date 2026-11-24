//! Working on a project with others: live sessions (hosting the open
//! project or joining someone else's) and reviews by file. Every editor
//! keeps the shared state as a CRDT on this thread, in step with its text,
//! writes the shared files to its own disk and compiles them itself.

use std::{
    collections::{HashMap, HashSet},
    ops::Range,
    path::{Component, Path, PathBuf},
    time::Duration,
};

use collab::{
    Author, Comment, CommentIntent, Event, Identity, LanBrowser, LanSession, Presence, Project, ReviewFile, ReviewKind, Role, Session, Suggestion,
    SuggestionKind, random_client_id, track,
};
use editor_core::{Decoration, DecorationKind, Selection, TextChange, state::changed_middle};
use gpui::{
    AnyElement, ClipboardItem, Context, Entity, Focusable, FontWeight, KeystrokeEvent, PathPromptOptions, PromptLevel, SharedString, Subscription,
    Task, Window, div, prelude::*, px,
};
use serde::{Deserialize, Serialize};

use super::{Dialog, LeftPanel, Menu, MenuEntry, Workspace, entry, menu_separator};
use crate::{editor_view::EditorView, theme};

/// Files above this size stay out of a session, so that joining stays quick.
const MAX_BLOB_BYTES: usize = 8 * 1024 * 1024;
/// All files that are not text together, which must fit in one message.
const MAX_BLOBS_BYTES: usize = 48 * 1024 * 1024;
/// Text files beyond this, such as large data tables, stay out, so that the
/// whole project fits one message to a newcomer, with blobs and history.
const MAX_TEXT_BYTES: usize = 4 * 1024 * 1024;
const MAX_TEXTS_BYTES: usize = 12 * 1024 * 1024;

const TEXT_EXTENSIONS: &[&str] = &["tex", "bib", "sty", "cls", "bst", "bbx", "cbx", "typ", "md", "txt", "toml", "csv", "tikz", "ltx", "dtx", "ins"];
const BLOB_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "pdf", "eps", "svg"];

/// Colors that read well on both themes, one per person.
const PEOPLE_COLORS: &[u32] = &[0x2f80ed, 0xd9480f, 0x2b8a3e, 0x9c36b5, 0xc2255c, 0x0c8599, 0xe67700, 0x5f3dc4];
const COMMENT_COLOR: u32 = 0xf2c94c;
const DELETION_COLOR: u32 = 0xe5484d;

/// Where a review by file keeps its state, hidden in the project, so that
/// sharing and exports leave it out.
const REVIEW_FOLDER: &str = ".editor";
const REVIEW_STATE: &str = "review.ydoc";
const REVIEW_META: &str = "review.json";
/// The state reaches the disk this long after the last change.
const REVIEW_SAVE_DELAY: Duration = Duration::from_secs(2);

pub(super) struct CollabUi {
    state: Option<CollabState>,
    name_field: Entity<EditorView>,
    code_field: Entity<EditorView>,
    comment: Option<CommentDraft>,
    /// Whether a new session shows up for others on the local network.
    visible_on_lan: bool,
    lan: Option<(LanBrowser, Task<()>)>,
    lan_sessions: Vec<LanSession>,
    error: Option<SharedString>,
}

struct CollabState {
    project: Project,
    /// Missing for a review by file.
    session: Option<Session>,
    review: Option<ReviewMeta>,
    root: PathBuf,
    /// What this editor may do. The host and the author decide on suggestions.
    role: Role,
    decides: bool,
    /// A guest opens the project once the files are on disk.
    opened: bool,
    tracking: bool,
    /// Paths that are part of the shared state, relative to `root`.
    shared: HashSet<String>,
    peers: HashMap<u64, Presence>,
    requests: Vec<(u64, String)>,
    comments: Vec<Comment>,
    suggestions: Vec<Suggestion>,
    /// What goes on with the link, while it is not fine.
    note: Option<SharedString>,
    last_presence: Option<(String, usize, usize)>,
    /// Whose code the panel shows.
    invite_role: Role,
    _poll: Option<Task<()>>,
    save_task: Option<Task<()>>,
}

/// What a review by file remembers between runs of the editor.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct ReviewMeta {
    /// The author decides on suggestions, a reviewer makes them.
    reviewer: bool,
    project: String,
    /// The state vector of the request, which a response builds on.
    base: Vec<u8>,
}

struct CommentDraft {
    path: String,
    range: Range<usize>,
    parent: Option<String>,
    field: Entity<EditorView>,
    _observer: Subscription,
}

enum JoinTarget {
    Code(String),
    Lan(LanSession),
}

impl CollabUi {
    pub(super) fn new(name: &str, cx: &mut Context<Workspace>) -> Self {
        let field = |text: &str, cx: &mut Context<Workspace>| {
            cx.new(|cx| {
                let mut view = EditorView::new(text, None, cx);
                view.set_plain();
                view
            })
        };
        Self {
            state: None,
            name_field: field(name, cx),
            code_field: field("", cx),
            comment: None,
            visible_on_lan: true,
            lan: None,
            lan_sessions: Vec::new(),
            error: None,
        }
    }

    pub(super) fn active(&self) -> bool {
        self.state.is_some()
    }

    /// Where a document belongs in the shared state, if it does.
    fn relative(&self, path: &Path) -> Option<String> {
        let state = self.state.as_ref()?;
        let relative = path.strip_prefix(&state.root).ok()?;
        Some(relative.to_str()?.replace('\\', "/"))
    }
}

/// The user name of the computer, for those who never typed one.
pub(super) fn default_name() -> String {
    std::env::var("USER").or_else(|_| std::env::var("USERNAME")).unwrap_or_else(|_| "Guest".to_string())
}

fn color_for(name: &str) -> u32 {
    let hash = name.bytes().fold(5381u32, |hash, byte| hash.wrapping_mul(33) ^ byte as u32);
    PEOPLE_COLORS[hash as usize % PEOPLE_COLORS.len()]
}

/// A path from someone else, as a path inside `root`. Anything that could
/// leave the folder is refused.
fn inside(root: &Path, relative: &str) -> Option<PathBuf> {
    let relative = Path::new(relative);
    if relative.as_os_str().is_empty() || !relative.components().all(|component| matches!(component, Component::Normal(_))) {
        return None;
    }
    Some(root.join(relative))
}

/// Plays changes onto a text, clipped to what is there, so that a file that
/// changed on disk meanwhile cannot make this fail.
fn replay(text: &mut String, changes: &[TextChange]) {
    for change in changes {
        let clip = |mut offset: usize| {
            offset = offset.min(text.len());
            while !text.is_char_boundary(offset) {
                offset -= 1;
            }
            offset
        };
        let start = clip(change.start);
        let end = clip(change.start + change.removed).max(start);
        text.replace_range(start..end, &change.inserted);
    }
}

fn is_text(path: &str) -> bool {
    let extension = Path::new(path).extension().and_then(|extension| extension.to_str()).unwrap_or_default().to_ascii_lowercase();
    TEXT_EXTENSIONS.contains(&extension.as_str())
}

/// The project's text files and other files worth sharing, as a project.
/// Ignored and hidden files stay out, like build output. Returns what was
/// too large or unreadable too.
fn collect_project(root: &Path, open: &HashMap<PathBuf, String>) -> (Project, HashSet<String>, Vec<String>) {
    let mut project = Project::new(random_client_id());
    let mut shared = HashSet::new();
    let mut skipped = Vec::new();
    let mut blob_bytes = 0;
    let mut text_bytes = 0;
    for entry in ignore::WalkBuilder::new(root).build().flatten() {
        let path = entry.path();
        if !entry.file_type().is_some_and(|kind| kind.is_file()) {
            continue;
        }
        let Some(relative) = path.strip_prefix(root).ok().and_then(Path::to_str).map(|relative| relative.replace('\\', "/")) else {
            continue;
        };
        let extension = path.extension().and_then(|extension| extension.to_str()).unwrap_or_default().to_ascii_lowercase();
        if is_text(&relative) {
            match open.get(path).cloned().or_else(|| std::fs::read_to_string(path).ok()) {
                Some(text) if text.len() <= MAX_TEXT_BYTES && text_bytes + text.len() <= MAX_TEXTS_BYTES => {
                    text_bytes += text.len();
                    project.add_file(&relative, &text);
                    shared.insert(relative);
                }
                _ => skipped.push(relative),
            }
        } else if BLOB_EXTENSIONS.contains(&extension.as_str()) {
            match std::fs::read(path) {
                Ok(bytes) if bytes.len() <= MAX_BLOB_BYTES && blob_bytes + bytes.len() <= MAX_BLOBS_BYTES => {
                    blob_bytes += bytes.len();
                    project.add_blob(&relative, bytes);
                }
                _ => skipped.push(relative),
            }
        }
    }
    project.forget_events();
    (project, shared, skipped)
}

/// A folder for a joined project that does not touch anything already there.
fn fresh_folder(parent: &Path, name: &str) -> PathBuf {
    let name: String = name.chars().map(|ch| if ch.is_alphanumeric() || ch == ' ' || ch == '-' { ch } else { '_' }).collect();
    let name = if name.trim().is_empty() { "Shared project".to_string() } else { name.trim().to_string() };
    let mut folder = parent.join(&name);
    let mut number = 2;
    while folder.exists() {
        folder = parent.join(format!("{name} {number}"));
        number += 1;
    }
    folder
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |duration| duration.as_secs())
}

fn project_name(root: &Path) -> String {
    root.file_name().map_or_else(|| "Project".to_string(), |name| name.to_string_lossy().into_owned())
}

impl CollabState {
    fn new(project: Project, session: Option<Session>, root: PathBuf, role: Role, decides: bool) -> Self {
        Self {
            project,
            session,
            review: None,
            root,
            role,
            decides,
            opened: true,
            tracking: role == Role::Reviewer,
            shared: HashSet::new(),
            peers: HashMap::new(),
            requests: Vec::new(),
            comments: Vec::new(),
            suggestions: Vec::new(),
            note: None,
            last_presence: None,
            invite_role: Role::Editor,
            _poll: None,
            save_task: None,
        }
    }

    fn set_role(&mut self, role: Role) {
        self.role = role;
        if role == Role::Reviewer {
            self.tracking = true;
        }
    }
}

impl Workspace {
    fn identity(&self, cx: &Context<Self>) -> Identity {
        let typed = self.collab.name_field.read(cx).state.buffer().to_string();
        let name = typed.trim();
        let name = if name.is_empty() { default_name() } else { name.chars().take(40).collect() };
        Identity {
            color: color_for(&name),
            name,
        }
    }

    fn author(&self, cx: &Context<Self>) -> Author {
        let identity = self.identity(cx);
        Author {
            name: identity.name,
            color: identity.color,
        }
    }

    fn remember_name(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.identity(cx).name;
        if self.settings.collab_name != name {
            self.update_settings(|settings| settings.collab_name = name, window, cx);
        }
    }

    /// Wakes the UI whenever the session has news, from its own thread.
    fn session_waker(&self, window: &mut Window, cx: &mut Context<Self>) -> (impl Fn() + Send + Sync + 'static, Task<()>) {
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel::<()>();
        let task = cx.spawn_in(window, async move |this, cx| {
            while receiver.recv().await.is_some() {
                // One look covers everything that came in meanwhile.
                while receiver.try_recv().is_ok() {}
                if this.update_in(cx, |this, window, cx| this.poll_collab(window, cx)).is_err() {
                    return;
                }
            }
        });
        (
            move || {
                sender.send(()).ok();
            },
            task,
        )
    }

    /// Text of open documents, which may not be saved yet.
    fn open_texts(&self, cx: &Context<Self>) -> HashMap<PathBuf, String> {
        self.documents
            .iter()
            .filter_map(|document| {
                let view = document.view.read(cx);
                Some((view.path.clone()?, view.state.buffer().to_string()))
            })
            .collect()
    }

    fn busy_with_others(&mut self, cx: &mut Context<Self>) -> bool {
        if self.collab.active() {
            self.set_status("End the current session or review first.", cx);
            self.show_collab_panel(cx);
            return true;
        }
        false
    }

    pub(super) fn host_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy_with_others(cx) {
            return;
        }
        let Some(root) = self.root.clone() else {
            self.set_status("Open a project first.", cx);
            return;
        };
        self.remember_name(window, cx);
        let identity = self.identity(cx);
        let (project, shared, skipped) = collect_project(&root, &self.open_texts(cx));
        let lan_name = self.collab.visible_on_lan.then(|| format!("{} ({})", project_name(&root), identity.name));
        let (wake, poll) = self.session_waker(window, cx);
        match Session::host(identity, &project.encode_state(), lan_name, wake) {
            Ok(session) => {
                let mut state = CollabState::new(project, Some(session), root, Role::Editor, true);
                state.shared = shared;
                state._poll = Some(poll);
                self.collab.state = Some(state);
                self.collab.error = None;
                self.collab.lan = None;
                self.start_change_logs(cx);
                let message = if skipped.is_empty() {
                    "Session started. Send an invite code to the others.".to_string()
                } else {
                    format!("Session started. Not shared, too large or unreadable: {}", super::shortened_list(&skipped))
                };
                self.set_status(message, cx);
                self.show_collab_panel(cx);
            }
            Err(error) => self.collab.error = Some(format!("Cannot start a session: {error}").into()),
        }
        cx.notify();
    }

    pub(super) fn open_join_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy_with_others(cx) {
            return;
        }
        self.collab.error = None;
        self.open_dialog(Dialog::JoinSession, window, cx);
        window.focus(&self.collab.code_field.read(cx).focus_handle(cx), cx);
    }

    fn join_with_code(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let code = self.collab.code_field.read(cx).state.buffer().to_string();
        if code.trim().is_empty() {
            self.collab.error = Some("Paste the invite code first.".into());
            cx.notify();
            return;
        }
        self.pick_folder("Put the shared project here", window, cx, move |this, parent, window, cx| {
            this.join_session(JoinTarget::Code(code.clone()), parent, window, cx)
        });
    }

    /// Asks for a folder, then calls `then` with it.
    fn pick_folder(
        &mut self,
        prompt: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
        then: impl FnOnce(&mut Self, PathBuf, &mut Window, &mut Context<Self>) + 'static,
    ) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(prompt.to_string().into()),
        });
        let handle = window.window_handle();
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let Some(folder) = paths.into_iter().next() else {
                return;
            };
            handle
                .update(cx, |_, window, cx| {
                    this.update(cx, |this, cx| then(this, folder, window, cx)).ok();
                })
                .ok();
        })
        .detach();
    }

    fn join_session(&mut self, target: JoinTarget, parent: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if self.collab.active() {
            return;
        }
        self.remember_name(window, cx);
        let identity = self.identity(cx);
        let name = match &target {
            JoinTarget::Code(_) => "Shared project".to_string(),
            JoinTarget::Lan(session) => session.name.clone(),
        };
        let root = fresh_folder(&parent, &name);
        if let Err(error) = std::fs::create_dir_all(&root) {
            self.collab.error = Some(format!("Cannot create {}: {error}", root.display()).into());
            cx.notify();
            return;
        }
        let (wake, poll) = self.session_waker(window, cx);
        let session = match target {
            JoinTarget::Code(code) => Session::join(&code, identity, wake),
            JoinTarget::Lan(session) => Session::join_lan(session.host, identity, wake),
        };
        match session {
            Ok(session) => {
                let mut state = CollabState::new(Project::new(random_client_id()), Some(session), root, Role::Editor, false);
                state.opened = false;
                state.note = Some("Connecting to the host…".into());
                state._poll = Some(poll);
                self.collab.state = Some(state);
                self.collab.error = None;
                self.collab.lan = None;
                self.collab.code_field.update(cx, |field, _| field.state.reload(""));
                self.dialog = None;
                self.show_collab_panel(cx);
            }
            Err(error) => {
                std::fs::remove_dir(&root).ok();
                self.collab.error = Some(error.into());
            }
        }
        cx.notify();
    }

    /// Leaves a live session, or puts a review by file aside until the
    /// project opens again.
    pub(super) fn leave_session(&mut self, cx: &mut Context<Self>) {
        let Some(mut state) = self.collab.state.take() else {
            return;
        };
        let message = match (&state.session, &state.review) {
            (Some(session), _) if session.is_host() => "The session ended.",
            (Some(_), _) => "You left the session. The files stay on your disk.",
            (None, _) => {
                save_review(&mut state);
                "The review is put aside. It comes back when the project opens."
            }
        };
        drop(state);
        self.collab.comment = None;
        for document in &self.documents {
            document.view.update(cx, |view, cx| {
                view.state.set_change_log(false);
                view.state.set_read_only(false);
                view.set_decorations(Vec::new(), cx);
            });
        }
        self.set_status(message, cx);
        cx.notify();
    }

    fn show_collab_panel(&mut self, cx: &mut Context<Self>) {
        self.show_file_panel = true;
        self.left_panel = LeftPanel::Collab;
        cx.notify();
    }

    /// Opening another project ends the session of this one.
    pub(super) fn collab_root_changing(&mut self, root: &Path, cx: &mut Context<Self>) {
        if self.collab.state.as_ref().is_some_and(|state| state.root != root) {
            self.leave_session(cx);
        }
    }

    /// A project with a review in progress picks it up again.
    pub(super) fn collab_root_opened(&mut self, root: &Path, cx: &mut Context<Self>) {
        if self.collab.active() {
            self.start_change_logs(cx);
            return;
        }
        let folder = root.join(REVIEW_FOLDER);
        let (Ok(state), Ok(meta)) = (std::fs::read(folder.join(REVIEW_STATE)), std::fs::read(folder.join(REVIEW_META))) else {
            return;
        };
        let loaded = serde_json::from_slice::<ReviewMeta>(&meta)
            .map_err(|error| error.to_string())
            .and_then(|meta| Project::from_state(random_client_id(), &state).map(|project| (meta, project)));
        let (meta, mut project) = match loaded {
            Ok(loaded) => loaded,
            Err(error) => {
                self.set_status(format!("Cannot read the review of this project: {error}"), cx);
                return;
            }
        };
        let shared = catch_up(&mut project, root, &self.open_texts(cx));
        let role = if meta.reviewer { Role::Reviewer } else { Role::Editor };
        let mut state = CollabState::new(project, None, root.to_path_buf(), role, !meta.reviewer);
        state.shared = shared;
        state.review = Some(meta);
        state.comments = state.project.comments();
        state.suggestions = state.project.suggestions();
        self.collab.state = Some(state);
        self.start_change_logs(cx);
        let open_items = self.collab.state.as_ref().map_or(0, |state| state.suggestions.len() + state.comments.len());
        self.set_status(format!("This project is in review, with {open_items} suggestions and comments."), cx);
    }

    /// Every shared document reports its edits from now on.
    fn start_change_logs(&mut self, cx: &mut Context<Self>) {
        let views: Vec<_> = self.documents.iter().map(|document| document.view.clone()).collect();
        for view in views {
            self.collab_document_opened(&view, cx);
        }
    }

    /// A document joins the shared state, and a file new to it gets shared.
    pub(super) fn collab_document_opened(&mut self, view: &Entity<EditorView>, cx: &mut Context<Self>) {
        let Some(relative) = view.read(cx).path.as_deref().and_then(|path| self.collab.relative(path)) else {
            return;
        };
        let Some(state) = &mut self.collab.state else {
            return;
        };
        let read_only = state.role == Role::Reader;
        if !state.shared.contains(&relative) {
            // Readers add nothing, and reviewers only suggest.
            if !is_text(&relative) || state.role != Role::Editor {
                return;
            }
            let update = state.project.add_file(&relative, &view.read(cx).state.buffer().to_string());
            state.shared.insert(relative);
            self.share_update(update, cx);
        }
        view.update(cx, |view, _| {
            view.state.set_change_log(true);
            view.state.set_read_only(read_only);
            view.state.take_changes();
        });
        self.refresh_collab_decorations(cx);
    }

    /// Sends a change of the local project to the others, and keeps a
    /// review by file on disk.
    fn share_update(&mut self, update: Vec<u8>, cx: &mut Context<Self>) {
        let Some(state) = &mut self.collab.state else {
            return;
        };
        if let Some(session) = &state.session {
            session.send_update(update);
        }
        if state.review.is_some() && state.save_task.is_none() {
            state.save_task = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(REVIEW_SAVE_DELAY).await;
                this.update(cx, |this, _| {
                    if let Some(state) = &mut this.collab.state {
                        save_review(state);
                    }
                })
                .ok();
            }));
        }
    }

    pub(super) fn collab_edited(&mut self, view: &Entity<EditorView>, cx: &mut Context<Self>) {
        let Some(relative) = view.read(cx).path.as_deref().and_then(|path| self.collab.relative(path)) else {
            return;
        };
        let changes = view.update(cx, |view, _| view.state.take_changes());
        let author = self.author(cx);
        let Some(state) = &mut self.collab.state else {
            return;
        };
        if changes.is_empty() || !state.shared.contains(&relative) {
            return;
        }
        shift_presences(state, &relative, &changes);
        let update = if state.tracking {
            let own: Vec<Range<usize>> = state
                .suggestions
                .iter()
                .filter(|suggestion| suggestion.path == relative && suggestion.author == author.name && suggestion.kind == SuggestionKind::Inserted)
                .map(|suggestion| suggestion.range.clone())
                .collect();
            let tracked = track(&changes, &own);
            if !tracked.restores.is_empty() {
                let before = state.last_presence.as_ref().filter(|(path, ..)| *path == relative).map(|(_, _, head)| *head);
                view.update(cx, |view, cx| {
                    view.state.restore_deleted(&tracked.restores);
                    place_after_struck(view, &tracked.restores, before);
                    cx.notify();
                });
            }
            state.project.edit_tracked(&relative, &tracked.ops, &author)
        } else {
            state.project.edit(&relative, &changes)
        };
        refresh_annotations(state);
        self.share_update(update, cx);
        self.refresh_collab_decorations(cx);
    }

    pub(super) fn collab_cursor(&mut self, view: &Entity<EditorView>, cx: &mut Context<Self>) {
        if self.active_view().as_ref() != Some(view) {
            return;
        }
        let Some(relative) = view.read(cx).path.as_deref().and_then(|path| self.collab.relative(path)) else {
            return;
        };
        let selection = *view.read(cx).state.selections().primary();
        let Some(state) = &mut self.collab.state else {
            return;
        };
        let presence = (relative, selection.anchor, selection.head);
        if state.last_presence.as_ref() != Some(&presence) {
            if let Some(session) = &state.session {
                session.presence(&presence.0, presence.1, presence.2);
            }
            state.last_presence = Some(presence);
        }
    }

    fn poll_collab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(events) = self.collab.state.as_ref().and_then(|state| state.session.as_ref()).map(Session::poll) else {
            return;
        };
        if events.is_empty() {
            return;
        }
        let mut decorations_changed = false;
        let mut ended = None;
        for event in events {
            let author_for_intent = match &event {
                Event::Intent { name, color, .. } => Some(Author { name: name.clone(), color: *color }),
                _ => None,
            };
            let Some(state) = &mut self.collab.state else {
                return;
            };
            match event {
                // Applied at once, so that positions in later events count it in.
                Event::Update(update) => {
                    if let Err(error) = state.project.apply(&update) {
                        self.status = Some(format!("An update from the session did not fit: {error}").into());
                    }
                    self.take_project_changes(window, cx);
                    decorations_changed = true;
                }
                Event::Intent { intent, .. } => {
                    let author = author_for_intent.expect("set above for intents");
                    let update = match intent {
                        CommentIntent::Add { path, range, text, parent } => state.project.add_comment(&path, range, &text, parent, &author),
                        CommentIntent::Resolve { id, resolved } => state.project.set_resolved(&id, resolved),
                    };
                    self.share_update(update, cx);
                    self.take_project_changes(window, cx);
                    decorations_changed = true;
                }
                Event::Presence(presence) => {
                    state.peers.insert(presence.peer, presence);
                    decorations_changed = true;
                }
                Event::PeerLeft(peer) => {
                    decorations_changed |= state.peers.remove(&peer).is_some();
                    state.requests.retain(|(id, _)| *id != peer);
                }
                Event::JoinRequest { peer, name } => {
                    self.status = Some(format!("{name} asks to join the session.").into());
                    state.requests.push((peer, name));
                    self.left_panel = LeftPanel::Collab;
                    self.show_file_panel = true;
                }
                Event::Joined(role) => {
                    state.note = None;
                    state.set_role(role);
                    if role == Role::Reader {
                        for document in &self.documents {
                            document.view.update(cx, |view, _| view.state.set_read_only(true));
                        }
                    }
                }
                Event::Reconnecting => {
                    state.note = Some("The connection broke. Reconnecting…".into());
                    state.peers.clear();
                    decorations_changed = true;
                }
                Event::Warning(warning) => self.status = Some(warning.into()),
                Event::Ended(reason) => ended = Some(reason),
            }
        }
        if decorations_changed {
            self.refresh_collab_decorations(cx);
        }
        if let Some(reason) = ended {
            self.leave_session(cx);
            self.set_status(reason, cx);
        }
        cx.notify();
    }

    /// Brings what changed in the local project into the editor and onto
    /// the disk, and opens a joined project once its files are there.
    fn take_project_changes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(state) = &mut self.collab.state else {
            return;
        };
        let root = state.root.clone();
        let remote = state.project.take_remote();
        let blobs = state.project.take_blobs();
        if state.project.take_annotations_changed() || !state.comments.is_empty() || !state.suggestions.is_empty() {
            refresh_annotations(state);
        }
        for (path, changes) in &remote {
            if let Some(state) = &mut self.collab.state {
                shift_presences(state, path, changes);
            }
            self.apply_collab_changes(&root, path, changes, cx);
        }
        for (path, bytes) in blobs {
            if let Some(target) = inside(&root, &path)
                && !target.exists()
            {
                target.parent().map(std::fs::create_dir_all);
                crate::atomic::write(&target, &bytes).ok();
            }
        }
        if let Some(state) = &mut self.collab.state
            && !state.opened
            && !state.shared.is_empty()
        {
            state.opened = true;
            self.open_root(root, window, cx);
        }
    }

    /// Changes from someone else go into the open document, or else into the
    /// file on disk, which every editor keeps up to date.
    fn apply_collab_changes(&mut self, root: &Path, path: &str, changes: &[TextChange], cx: &mut Context<Self>) {
        let Some(target) = inside(root, path) else {
            return;
        };
        let Some(state) = &mut self.collab.state else {
            return;
        };
        // A file new to the session must not land on a file of the same
        // name that was never shared, such as one too large to share.
        if !state.shared.contains(path) && target.exists() {
            self.status = Some(format!("Kept your own {path}, which someone else also shared.").into());
            return;
        }
        state.shared.insert(path.to_string());
        if let Some(index) = self.document_at(&target, cx) {
            let view = self.documents[index].view.clone();
            view.update(cx, |view, cx| view.apply_remote(changes, cx));
            return;
        }
        let mut text = std::fs::read_to_string(&target).unwrap_or_default();
        replay(&mut text, changes);
        target.parent().map(std::fs::create_dir_all);
        if let Err(error) = crate::atomic::write(&target, text.as_bytes()) {
            self.status = Some(format!("Cannot write {path}: {error}").into());
        }
    }

    /// Shows the others' cursors, comments and suggestions in every document.
    fn refresh_collab_decorations(&mut self, cx: &mut Context<Self>) {
        let views: Vec<_> = self.documents.iter().map(|document| document.view.clone()).collect();
        for view in views {
            let relative = view.read(cx).path.as_deref().and_then(|path| self.collab.relative(path));
            let decorations = match (&self.collab.state, relative) {
                (Some(state), Some(relative)) => decorations_for(state, &relative),
                _ => Vec::new(),
            };
            view.update(cx, |view, cx| view.set_decorations(decorations, cx));
        }
    }

    pub(super) fn collab_tracking(&self) -> bool {
        self.collab.state.as_ref().is_some_and(|state| state.tracking)
    }

    pub(super) fn toggle_tracking(&mut self, cx: &mut Context<Self>) {
        let Some(state) = &mut self.collab.state else {
            self.set_status("Track changes works in a session or a review.", cx);
            return;
        };
        if state.role != Role::Editor {
            self.set_status("Your role makes every edit a suggestion.", cx);
            return;
        }
        state.tracking = !state.tracking;
        let message = if state.tracking { "Your edits are now suggestions for review." } else { "Your edits go straight into the text again." };
        self.set_status(message, cx);
    }

    /// Opens the field for a comment on the selection, or on the line.
    pub(super) fn start_comment(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(view) = self.active_view() else {
            return;
        };
        let Some(path) = view.read(cx).path.as_deref().and_then(|path| self.collab.relative(path)) else {
            self.set_status("Comments work in a session or a review.", cx);
            return;
        };
        let state = &view.read(cx).state;
        let selection = state.selections().primary();
        let range = if selection.is_empty() {
            let line = state.buffer().offset_to_line(selection.head);
            state.buffer().line_start(line)..state.buffer().line_end(line)
        } else {
            selection.start()..selection.end()
        };
        self.open_comment_field(path, range, None, window, cx);
    }

    fn open_comment_field(&mut self, path: String, range: Range<usize>, parent: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let field = cx.new(|cx| {
            let mut view = EditorView::new("", None, cx);
            view.set_plain();
            view
        });
        let _observer = cx.observe(&field, |_, _, cx| cx.notify());
        window.focus(&field.read(cx).focus_handle(cx), cx);
        self.collab.comment = Some(CommentDraft {
            path,
            range,
            parent,
            field,
            _observer,
        });
        self.show_collab_panel(cx);
    }

    fn post_comment(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(draft) = self.collab.comment.take() else {
            return;
        };
        let text = draft.field.read(cx).state.buffer().to_string().trim().to_string();
        self.focus_active(window, cx);
        if !text.is_empty() {
            self.comment_action(
                CommentIntent::Add {
                    path: draft.path,
                    range: draft.range,
                    text,
                    parent: draft.parent,
                },
                window,
                cx,
            );
        }
        cx.notify();
    }

    /// Comments right away, or through the host for a reader.
    fn comment_action(&mut self, intent: CommentIntent, window: &mut Window, cx: &mut Context<Self>) {
        let author = self.author(cx);
        let Some(state) = &mut self.collab.state else {
            return;
        };
        if state.role == Role::Reader {
            if let Some(session) = &state.session {
                session.send_intent(intent);
            }
            return;
        }
        let update = match intent {
            CommentIntent::Add { path, range, text, parent } => state.project.add_comment(&path, range, &text, parent, &author),
            CommentIntent::Resolve { id, resolved } => state.project.set_resolved(&id, resolved),
        };
        self.share_update(update, cx);
        self.take_project_changes(window, cx);
        self.refresh_collab_decorations(cx);
    }

    fn delete_comment(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(state) = &mut self.collab.state else {
            return;
        };
        let update = state.project.delete_comment(id);
        self.share_update(update, cx);
        self.take_project_changes(window, cx);
        self.refresh_collab_decorations(cx);
    }

    fn go_to_shared(&mut self, path: &str, range: Range<usize>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(target) = self.collab.state.as_ref().and_then(|state| inside(&state.root, path)) else {
            return;
        };
        self.open_file(target, window, cx);
        if let Some(view) = self.active_view() {
            view.update(cx, |view, cx| view.select_range(range, window, cx));
        }
    }

    fn decide(&mut self, id: &str, accept: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(state) = &mut self.collab.state else {
            return;
        };
        if !state.decides {
            return;
        }
        let update = if accept { state.project.accept(id) } else { state.project.reject(id) };
        self.share_update(update, cx);
        self.take_project_changes(window, cx);
        self.refresh_collab_decorations(cx);
        cx.notify();
    }

    fn decide_all(&mut self, author: Option<String>, accept: bool, window: &mut Window, cx: &mut Context<Self>) {
        let ids: Vec<String> = self
            .collab
            .state
            .iter()
            .flat_map(|state| state.suggestions.iter())
            .filter(|suggestion| author.as_ref().is_none_or(|author| suggestion.author == *author))
            .map(|suggestion| suggestion.id.clone())
            .collect();
        // From the end, so that one decision does not move the next.
        for id in ids.iter().rev() {
            self.decide(id, accept, window, cx);
        }
    }

    // Reviews by file

    pub(super) fn send_for_review(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy_with_others(cx) {
            return;
        }
        let Some(root) = self.root.clone() else {
            self.set_status("Open a project first.", cx);
            return;
        };
        self.remember_name(window, cx);
        let (project, shared, skipped) = collect_project(&root, &self.open_texts(cx));
        let name = project_name(&root);
        let request = ReviewFile {
            kind: ReviewKind::Request,
            project: name.clone(),
            author: self.identity(cx).name,
            created: now(),
            base: project.state_vector(),
            update: project.encode_state(),
        };
        let meta = ReviewMeta {
            reviewer: false,
            project: name.clone(),
            base: request.base.clone(),
        };
        let mut state = CollabState::new(project, None, root.clone(), Role::Editor, true);
        state.shared = shared;
        state.review = Some(meta);
        save_review(&mut state);
        self.collab.state = Some(state);
        self.start_change_logs(cx);
        self.show_collab_panel(cx);
        let note = if skipped.is_empty() { String::new() } else { format!(" Left out, too large or unreadable: {}.", super::shortened_list(&skipped)) };
        self.save_review_file(request.encode(), format!("{name}.review"), format!("Review request saved. Send it to your reviewers.{note}"), cx);
    }

    /// Writes a review file where the user wants it.
    fn save_review_file(&mut self, bytes: Vec<u8>, name: String, done: String, cx: &mut Context<Self>) {
        let directory = dirs_home();
        let answer = cx.prompt_for_new_path(&directory, Some(&name));
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(target))) = answer.await else {
                return;
            };
            let result = crate::atomic::write(&target, &bytes);
            this.update(cx, |this, cx| match result {
                Ok(()) => this.set_status(done, cx),
                Err(error) => this.set_status(format!("Cannot save {}: {error}", target.display()), cx),
            })
            .ok();
        })
        .detach();
    }

    /// Reads a review file the user picks, then calls `then` with it.
    fn pick_review_file(
        &mut self,
        prompt: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
        then: impl FnOnce(&mut Self, ReviewFile, &mut Window, &mut Context<Self>) + 'static,
    ) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(prompt.to_string().into()),
        });
        let handle = window.window_handle();
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let read = cx
                .background_spawn(async move { std::fs::read(&path).map_err(|error| error.to_string()).and_then(|bytes| ReviewFile::decode(&bytes)) })
                .await;
            handle
                .update(cx, |_, window, cx| {
                    this.update(cx, |this, cx| match read {
                        Ok(file) => then(this, file, window, cx),
                        Err(error) => this.set_status(error, cx),
                    })
                    .ok();
                })
                .ok();
        })
        .detach();
    }

    pub(super) fn open_review_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy_with_others(cx) {
            return;
        }
        self.pick_review_file("Open review", window, cx, |this, file, window, cx| {
            if file.kind != ReviewKind::Request {
                this.set_status("This is an answer to a review. The author imports it into the project.", cx);
                return;
            }
            this.pick_folder("Put the project to review here", window, cx, move |this, parent, window, cx| {
                this.start_review(file, parent, window, cx)
            });
        });
    }

    fn start_review(&mut self, file: ReviewFile, parent: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if self.collab.active() {
            return;
        }
        let mut project = match Project::from_state(random_client_id(), &file.update) {
            Ok(project) => project,
            Err(error) => {
                self.set_status(format!("Cannot read the review: {error}"), cx);
                return;
            }
        };
        let root = fresh_folder(&parent, &format!("{} (review)", file.project));
        let mut shared = HashSet::new();
        let mut written = std::fs::create_dir_all(&root).map_err(|error| error.to_string());
        for (path, text) in project.files() {
            if let Some(target) = inside(&root, &path) {
                target.parent().map(std::fs::create_dir_all);
                written = written.and(crate::atomic::write(&target, text.as_bytes()).map_err(|error| error.to_string()));
                shared.insert(path);
            }
        }
        for (path, bytes) in project.blobs() {
            if let Some(target) = inside(&root, &path) {
                target.parent().map(std::fs::create_dir_all);
                crate::atomic::write(&target, &bytes).ok();
            }
        }
        if let Err(error) = written {
            self.set_status(format!("Cannot write the project to review: {error}"), cx);
            return;
        }
        let mut state = CollabState::new(project, None, root.clone(), Role::Reviewer, false);
        state.shared = shared;
        state.review = Some(ReviewMeta {
            reviewer: true,
            project: file.project.clone(),
            base: file.base,
        });
        state.comments = state.project.comments();
        state.suggestions = state.project.suggestions();
        save_review(&mut state);
        self.collab.state = Some(state);
        self.dialog = None;
        self.open_root(root, window, cx);
        self.show_collab_panel(cx);
        self.set_status(format!("Reviewing {}. Your edits are suggestions. Send the review back when done.", file.project), cx);
    }

    pub(super) fn send_review_back(&mut self, cx: &mut Context<Self>) {
        let name = self.identity(cx).name;
        let Some(state) = &mut self.collab.state else {
            return;
        };
        let Some(meta) = state.review.clone().filter(|meta| meta.reviewer) else {
            self.set_status("Only a reviewer sends a review back.", cx);
            return;
        };
        save_review(state);
        let update = match state.project.diff(&meta.base) {
            Ok(update) => update,
            Err(error) => {
                self.set_status(format!("Cannot write the review: {error}"), cx);
                return;
            }
        };
        let response = ReviewFile {
            kind: ReviewKind::Response,
            project: meta.project.clone(),
            author: name.clone(),
            created: now(),
            base: meta.base,
            update,
        };
        self.save_review_file(response.encode(), format!("{} review by {name}.review", meta.project), "Review saved. Send it back to the author.".to_string(), cx);
    }

    pub(super) fn import_review(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.collab.state.as_ref().is_some_and(|state| state.review.as_ref().is_some_and(|meta| !meta.reviewer)) {
            self.set_status("Send the project for review first, then import the answers.", cx);
            return;
        }
        self.pick_review_file("Import", window, cx, |this, file, window, cx| {
            let Some(state) = &mut this.collab.state else {
                return;
            };
            if file.kind != ReviewKind::Response {
                this.set_status("This is a review request, not an answer to one.", cx);
                return;
            }
            // A copy takes the answer first: a reviewer may only suggest and comment.
            let checked = Project::from_state(random_client_id(), &state.project.encode_state()).and_then(|mut copy| copy.apply_suggestions_only(&file.update));
            if let Err(reason) = checked {
                this.set_status(format!("The review from {} was not imported: {reason}.", file.author), cx);
                return;
            }
            if let Err(error) = state.project.apply(&file.update) {
                this.set_status(format!("Cannot import the review: {error}"), cx);
                return;
            }
            save_review(state);
            this.take_project_changes(window, cx);
            this.refresh_collab_decorations(cx);
            this.set_status(format!("Imported the review from {}.", file.author), cx);
            this.show_collab_panel(cx);
        });
    }

    fn end_review(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let open = self.collab.state.as_ref().map_or(0, |state| state.suggestions.len() + state.comments.len());
        let answer = window.prompt(
            PromptLevel::Warning,
            "End the review?",
            Some(&format!("{open} open suggestions and comments go away. The text stays as it is now.")),
            &["End review", "Cancel"],
            cx,
        );
        cx.spawn(async move |this, cx| {
            if answer.await != Ok(0) {
                return;
            }
            this.update(cx, |this, cx| {
                let Some(state) = this.collab.state.take() else {
                    return;
                };
                let folder = state.root.join(REVIEW_FOLDER);
                std::fs::remove_file(folder.join(REVIEW_STATE)).ok();
                std::fs::remove_file(folder.join(REVIEW_META)).ok();
                std::fs::remove_dir(&folder).ok();
                for document in &this.documents {
                    document.view.update(cx, |view, cx| {
                        view.state.set_change_log(false);
                        view.set_decorations(Vec::new(), cx);
                    });
                }
                this.set_status("The review is over.", cx);
            })
            .ok();
        })
        .detach();
    }

    // Keys and frames

    /// Enter and Escape in the fields of the session. Returns whether the key
    /// was taken.
    pub(super) fn collab_key(&mut self, event: &KeystrokeEvent, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let key = event.keystroke.key.as_str();
        if !matches!(key, "enter" | "escape") {
            return false;
        }
        if let Some(draft) = &self.collab.comment
            && draft.field.read(cx).focus_handle(cx).is_focused(window)
        {
            cx.stop_propagation();
            if key == "enter" {
                self.post_comment(window, cx);
            } else {
                self.collab.comment = None;
                self.focus_active(window, cx);
                cx.notify();
            }
            return true;
        }
        let in_dialog = [&self.collab.code_field, &self.collab.name_field]
            .iter()
            .any(|field| field.read(cx).focus_handle(cx).is_focused(window));
        if in_dialog && self.dialog == Some(Dialog::JoinSession) {
            cx.stop_propagation();
            if key == "enter" {
                self.join_with_code(window, cx);
            } else {
                self.close_dialog(window, cx);
            }
            return true;
        }
        false
    }

    /// Looks for sessions on the local network while the join dialog shows,
    /// and only then.
    pub(super) fn collab_frame(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let browsing = self.dialog == Some(Dialog::JoinSession) && !self.collab.active();
        if browsing == self.collab.lan.is_some() {
            return;
        }
        if !browsing {
            self.collab.lan = None;
            self.collab.lan_sessions.clear();
            return;
        }
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel::<()>();
        let Ok(browser) = LanBrowser::start(move || {
            sender.send(()).ok();
        }) else {
            return;
        };
        let task = cx.spawn_in(window, async move |this, cx| {
            while receiver.recv().await.is_some() {
                while receiver.try_recv().is_ok() {}
                let updated = this.update(cx, |this, cx| {
                    if let Some((browser, _)) = &this.collab.lan {
                        this.collab.lan_sessions = browser.sessions();
                        cx.notify();
                    }
                });
                if updated.is_err() {
                    return;
                }
            }
        });
        self.collab.lan = Some((browser, task));
    }

    pub(super) fn add_collab_entries(&self, menu: Menu, entries: &mut Vec<MenuEntry>) {
        match menu {
            Menu::File => {
                let review = self.collab.state.as_ref().and_then(|state| state.review.as_ref());
                let mut added = vec![
                    entry("Start live session", "", |this, window, cx| this.host_session(window, cx)),
                    entry("Join live session…", "", |this, window, cx| this.open_join_dialog(window, cx)),
                ];
                match review {
                    Some(meta) if meta.reviewer => added.push(entry("Send review back…", "", |this, _, cx| this.send_review_back(cx))),
                    Some(_) => added.push(entry("Import review…", "", |this, window, cx| this.import_review(window, cx))),
                    None => {
                        added.push(entry("Send for review…", "", |this, window, cx| this.send_for_review(window, cx)));
                        added.push(entry("Open review file…", "", |this, window, cx| this.open_review_file(window, cx)));
                    }
                }
                added.push(menu_separator());
                // After "Make a copy" and its separator.
                let at = entries.len().min(5);
                entries.splice(at..at, added);
            }
            Menu::Edit => {
                entries.extend([
                    menu_separator(),
                    entry("  Add comment", "", |this, window, cx| this.start_comment(window, cx)),
                    entry(format!("{} Track my changes", if self.collab_tracking() { "✓" } else { "  " }), "", |this, _, cx| {
                        this.toggle_tracking(cx);
                        cx.notify();
                    }),
                ]);
            }
            _ => {}
        }
    }

    // Rendering

    fn small_button(
        &self,
        id: impl Into<gpui::ElementId>,
        label: impl Into<SharedString>,
        primary: bool,
        cx: &mut Context<Self>,
        handler: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> gpui::Stateful<gpui::Div> {
        div()
            .id(id)
            .flex_none()
            .px(px(8.))
            .py(px(2.))
            .rounded(px(4.))
            .cursor_pointer()
            .bg(theme::color(if primary { theme::palette().accent } else { theme::palette().divider }))
            .when(primary, |style| style.text_color(theme::color(theme::palette().on_accent)))
            .on_click(cx.listener(move |this, _, window, cx| {
                cx.stop_propagation();
                handler(this, window, cx);
            }))
            .child(label.into())
    }

    fn field_box(field: &Entity<EditorView>) -> impl IntoElement {
        div()
            .h(px(26.))
            .px(px(6.))
            .rounded(px(4.))
            .overflow_hidden()
            .bg(theme::color(theme::palette().bg))
            .border_1()
            .border_color(theme::color(theme::palette().divider))
            .child(field.clone())
    }

    fn muted(text: impl Into<SharedString>) -> impl IntoElement {
        div().flex_none().text_color(theme::color(theme::palette().ui_muted)).child(text.into())
    }

    fn heading(text: impl Into<SharedString>) -> impl IntoElement {
        div().flex_none().pt(px(6.)).font_weight(FontWeight::SEMIBOLD).child(text.into())
    }

    fn person(name: &str, color: u32) -> impl IntoElement {
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(6.))
            .child(div().flex_none().size(px(9.)).rounded_full().bg(theme::color(color)))
            .child(div().min_w_0().truncate().child(name.to_string()))
    }

    fn check_row(&self, id: &'static str, on: bool, label: &'static str, cx: &mut Context<Self>, handler: impl Fn(&mut Self, &mut Context<Self>) + 'static) -> impl IntoElement {
        div()
            .id(id)
            .flex()
            .flex_none()
            .items_center()
            .gap(px(6.))
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| {
                handler(this, cx);
                cx.notify();
            }))
            .child(div().flex_none().w(px(14.)).child(if on { "✓" } else { "" }))
            .child(label)
    }

    pub(super) fn render_join_dialog(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut content = vec![
            Self::muted("Edit a project together with others, live and without an account. The project comes straight from the host's computer, end-to-end encrypted.")
                .into_any_element(),
            Self::heading("Your name").into_any_element(),
            Self::field_box(&self.collab.name_field).into_any_element(),
            Self::heading("Invite code").into_any_element(),
            Self::field_box(&self.collab.code_field).into_any_element(),
            div()
                .flex()
                .flex_none()
                .pt(px(4.))
                .child(self.panel_button("collab-join-code", "Join…", true, cx, |this, window, cx| this.join_with_code(window, cx)))
                .into_any_element(),
            Self::heading("On this network").into_any_element(),
        ];
        if self.collab.lan_sessions.is_empty() {
            content.push(Self::muted("No session found yet. Sessions show up here when the host lets the network see them.").into_any_element());
        }
        for (index, session) in self.collab.lan_sessions.iter().enumerate() {
            let target = session.clone();
            content.push(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_between()
                    .gap(px(8.))
                    .py(px(4.))
                    .child(div().min_w_0().truncate().child(session.name.clone()))
                    .child(self.small_button(("collab-lan-join", index), "Ask to join…", false, cx, move |this, window, cx| {
                        let target = target.clone();
                        this.pick_folder("Put the shared project here", window, cx, move |this, parent, window, cx| {
                            this.join_session(JoinTarget::Lan(target), parent, window, cx)
                        });
                    }))
                    .into_any_element(),
            );
        }
        content.push(Self::heading("Not at the same time?").into_any_element());
        content.push(Self::muted("A review file holds the whole project. Whoever reviews it sends back their suggestions and comments as a small file.").into_any_element());
        content.push(
            div()
                .flex()
                .flex_none()
                .child(self.panel_button("collab-open-review", "Open review file…", false, cx, |this, window, cx| {
                    this.dialog = None;
                    this.open_review_file(window, cx);
                }))
                .into_any_element(),
        );
        if let Some(error) = &self.collab.error {
            content.push(div().flex_none().pt(px(6.)).text_color(theme::color(theme::palette().error)).child(error.clone()).into_any_element());
        }
        content
    }

    pub(super) fn render_collab_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let panel = div()
            .id("collab-panel")
            .flex()
            .flex_col()
            .flex_none()
            .w(self.file_panel_width)
            .h_full()
            .gap(px(6.))
            .p(px(8.))
            .overflow_y_scroll();
        let Some(state) = &self.collab.state else {
            return panel
                .child(div().flex_none().font_weight(FontWeight::SEMIBOLD).child("Work together"))
                .child(Self::muted("Share this project with others on your network or over the internet. Nobody needs an account, and no server stores the project."))
                // Windows asks once whether the editor may talk on the network.
                .when(cfg!(windows), |panel| panel.child(Self::muted("The first time, Windows asks whether Editor may use the network. Allow it, so that others can reach you.")))
                .child(Self::heading("Your name"))
                .child(Self::field_box(&self.collab.name_field))
                .child(self.check_row("collab-lan-visible", self.collab.visible_on_lan, "Visible on this network", cx, |this, _| {
                    this.collab.visible_on_lan = !this.collab.visible_on_lan;
                }))
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .flex_none()
                        .gap(px(6.))
                        .pt(px(4.))
                        .child(self.panel_button("collab-host", "Start session", true, cx, |this, window, cx| this.host_session(window, cx)))
                        .child(self.panel_button("collab-join", "Join…", false, cx, |this, window, cx| this.open_join_dialog(window, cx))),
                )
                .child(Self::heading("Review later"))
                .child(Self::muted("Send the project as a review file. Reviewers comment and suggest changes in their own time, and you import their answers."))
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .flex_none()
                        .gap(px(6.))
                        .child(self.panel_button("collab-send-review", "Send for review…", false, cx, |this, window, cx| this.send_for_review(window, cx)))
                        .child(self.panel_button("collab-open-review", "Open review file…", false, cx, |this, window, cx| this.open_review_file(window, cx))),
                )
                .children(self.collab.error.clone().map(|error| div().flex_none().text_color(theme::color(theme::palette().error)).child(error)))
                .into_any_element();
        };
        let title = match (&state.session, &state.review) {
            (Some(_), _) => "Live session",
            (None, Some(meta)) if meta.reviewer => "Reviewing",
            _ => "In review",
        };
        let mut panel = panel.child(div().flex_none().font_weight(FontWeight::SEMIBOLD).child(title));
        if let Some(note) = &state.note {
            panel = panel.child(div().flex_none().text_color(theme::color(theme::palette().warning)).child(note.clone()));
        }
        if let Some(session) = &state.session {
            panel = self.render_live_part(panel, state, session, cx);
        } else if let Some(meta) = &state.review {
            panel = self.render_review_part(panel, meta, cx);
        }
        if state.role != Role::Reader {
            let locked = state.role != Role::Editor;
            panel = panel.child(div().pt(px(6.)).child(self.check_row("collab-tracking", state.tracking, "Track my changes", cx, |this, cx| this.toggle_tracking(cx))));
            if locked {
                panel = panel.child(Self::muted("As a reviewer, every edit of yours is a suggestion."));
            }
        }
        panel = panel.child(self.render_suggestions(state, cx)).child(self.render_comments(state, cx));
        let leave = match (&state.session, &state.review) {
            (Some(session), _) if session.is_host() => "End session",
            (Some(_), _) => "Leave session",
            _ => "Put aside",
        };
        panel
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .flex_none()
                    .gap(px(6.))
                    .pt(px(10.))
                    .child(self.panel_button("collab-leave", leave, false, cx, |this, _, cx| this.leave_session(cx)))
                    .when(state.review.as_ref().is_some_and(|meta| !meta.reviewer), |element| {
                        element.child(self.panel_button("collab-end-review", "End review…", false, cx, |this, window, cx| this.end_review(window, cx)))
                    }),
            )
            .into_any_element()
    }

    fn render_live_part(&self, mut panel: gpui::Stateful<gpui::Div>, state: &CollabState, session: &Session, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        if session.is_host() {
            let role = state.invite_role;
            let code = session.ticket(role).unwrap_or_default().to_string();
            let explain = match role {
                Role::Editor => "Anyone with this code can edit the project while the session runs.",
                Role::Reviewer => "Anyone with this code can suggest changes and comment. You decide on the suggestions.",
                Role::Reader => "Anyone with this code can read along and comment.",
            };
            panel = panel
                .child(Self::heading("Invite as"))
                .child(div().flex().flex_none().gap(px(4.)).children(Role::ALL.into_iter().map(|each| {
                    div()
                        .id(("collab-invite-role", each as usize))
                        .px(px(8.))
                        .py(px(2.))
                        .rounded(px(4.))
                        .cursor_pointer()
                        .bg(theme::color(if each == role { theme::palette().accent_soft } else { theme::palette().divider }))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(state) = &mut this.collab.state {
                                state.invite_role = each;
                            }
                            cx.notify();
                        }))
                        .child(each.label())
                })))
                .child(Self::muted(explain))
                .child(
                    div()
                        .flex_none()
                        .p(px(6.))
                        .rounded(px(4.))
                        .bg(theme::color(theme::palette().bg))
                        .font_family(theme::code_font())
                        .text_size(px(11.))
                        .child(code.clone()),
                )
                .child(div().flex().flex_none().child(self.panel_button("collab-copy", "Copy code", false, cx, move |this, _, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(code.clone()));
                    this.set_status("Invite code copied.", cx);
                })));
        } else {
            panel = panel.child(Self::muted(match state.role {
                Role::Editor => "You edit along with the others.",
                Role::Reviewer => "You suggest changes and comment. The host decides.",
                Role::Reader => "You read along and comment.",
            }));
        }
        for (peer, name) in &state.requests {
            let peer = *peer;
            let mut allow = Vec::new();
            for role in Role::ALL {
                let label = format!("Let in as {}", role.label().to_lowercase());
                allow.push(self.small_button(("collab-allow", peer as usize * 4 + role as usize), label, role == Role::Editor, cx, move |this, _, cx| {
                    this.answer_request(peer, Some(role), cx)
                }));
            }
            panel = panel.child(
                div()
                    .flex()
                    .flex_col()
                    .flex_none()
                    .gap(px(4.))
                    .p(px(6.))
                    .rounded(px(4.))
                    .bg(theme::color(theme::palette().accent_soft))
                    .child(format!("{name} asks to join."))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap(px(4.))
                            .children(allow)
                            .child(self.small_button(("collab-deny", peer as usize), "Decline", false, cx, move |this, _, cx| this.answer_request(peer, None, cx))),
                    ),
            );
        }
        let me = self.identity(cx);
        let mut people: Vec<(String, u32)> = vec![(format!("{} (you, {})", me.name, state.role.label().to_lowercase()), me.color)];
        let mut peers: Vec<&Presence> = state.peers.values().collect();
        peers.sort_by_key(|presence| presence.peer);
        people.extend(peers.into_iter().map(|presence| (format!("{} ({})", presence.name, presence.role.label().to_lowercase()), presence.color)));
        panel.child(Self::heading("People")).children(people.iter().map(|(name, color)| Self::person(name, *color)))
    }

    fn render_review_part(&self, panel: gpui::Stateful<gpui::Div>, meta: &ReviewMeta, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        if meta.reviewer {
            panel
                .child(Self::muted(format!("You review {}. Comment and edit as you like: every edit is a suggestion. Send the review back to the author when you are done, or in between.", meta.project)))
                .child(div().flex().flex_none().child(self.panel_button("collab-send-back", "Send review back…", true, cx, |this, _, cx| this.send_review_back(cx))))
        } else {
            panel
                .child(Self::muted("Reviewers have this project as a file. Keep working meanwhile: their answers merge with your changes."))
                .child(div().flex().flex_none().child(self.panel_button("collab-import", "Import review…", true, cx, |this, window, cx| this.import_review(window, cx))))
        }
    }

    fn answer_request(&mut self, peer: u64, role: Option<Role>, cx: &mut Context<Self>) {
        if let Some(state) = &mut self.collab.state {
            if let Some(session) = &state.session {
                session.admit(peer, role);
            }
            state.requests.retain(|(id, _)| *id != peer);
        }
        cx.notify();
    }

    fn render_suggestions(&self, state: &CollabState, cx: &mut Context<Self>) -> impl IntoElement {
        let decides = state.decides;
        let mut authors: Vec<String> = state.suggestions.iter().map(|suggestion| suggestion.author.clone()).collect();
        authors.sort();
        authors.dedup();
        let mut section = div().flex().flex_col().flex_none().gap(px(4.)).child(Self::heading(format!("Suggestions ({})", state.suggestions.len())));
        if decides && !state.suggestions.is_empty() {
            section = section.child(
                div()
                    .flex()
                    .flex_wrap()
                    .flex_none()
                    .gap(px(4.))
                    .child(self.small_button("collab-accept-all", "Accept all", false, cx, |this, window, cx| this.decide_all(None, true, window, cx)))
                    .child(self.small_button("collab-reject-all", "Reject all", false, cx, |this, window, cx| this.decide_all(None, false, window, cx))),
            );
            if authors.len() > 1 {
                for (index, author) in authors.iter().enumerate() {
                    let (accept, reject) = (author.clone(), author.clone());
                    section = section.child(
                        div()
                            .flex()
                            .flex_wrap()
                            .flex_none()
                            .items_center()
                            .gap(px(4.))
                            .child(div().min_w_0().truncate().child(format!("From {author}:")))
                            .child(self.small_button(("collab-accept-author", index), "Accept", false, cx, move |this, window, cx| {
                                this.decide_all(Some(accept.clone()), true, window, cx)
                            }))
                            .child(self.small_button(("collab-reject-author", index), "Reject", false, cx, move |this, window, cx| {
                                this.decide_all(Some(reject.clone()), false, window, cx)
                            })),
                    );
                }
            }
        }
        if state.suggestions.is_empty() {
            section = section.child(Self::muted("Tracked edits wait here: insertions underlined, deletions struck through, until someone who decides accepts or rejects them."));
        } else {
            section = section.child(Self::muted("The PDF shows struck text until it is accepted."));
        }
        for (index, suggestion) in state.suggestions.iter().enumerate() {
            let (what, excerpt) = match &suggestion.kind {
                SuggestionKind::Inserted => ("Added", self.shared_text(&suggestion.path, suggestion.range.clone(), cx).unwrap_or_default()),
                SuggestionKind::Deleted(text) => ("Deleted", text.clone()),
            };
            let excerpt: String = excerpt.chars().take(80).collect();
            let (path, range) = (suggestion.path.clone(), suggestion.range.clone());
            let (accept, reject) = (suggestion.id.clone(), suggestion.id.clone());
            section = section.child(
                div()
                    .id(("collab-suggestion", index))
                    .flex()
                    .flex_col()
                    .flex_none()
                    .gap(px(2.))
                    .p(px(6.))
                    .rounded(px(4.))
                    .cursor_pointer()
                    .border_l_2()
                    .border_color(theme::color(if what == "Deleted" { DELETION_COLOR } else { suggestion.color }))
                    .hover(|style| style.bg(theme::color(theme::palette().divider)))
                    .on_click(cx.listener(move |this, _, window, cx| this.go_to_shared(&path, range.clone(), window, cx)))
                    .child(Self::muted(format!("{what} by {} in {}", suggestion.author, suggestion.path)))
                    .child(
                        div()
                            .flex_none()
                            .when(what == "Deleted", |style| style.line_through())
                            .child(if excerpt.trim().is_empty() { "(blank)".to_string() } else { excerpt }),
                    )
                    .when(decides, |element| {
                        element.child(
                            div()
                                .flex()
                                .gap(px(4.))
                                .pt(px(2.))
                                .child(self.small_button(("collab-accept", index), "Accept", false, cx, move |this, window, cx| this.decide(&accept, true, window, cx)))
                                .child(self.small_button(("collab-reject", index), "Reject", false, cx, move |this, window, cx| this.decide(&reject, false, window, cx))),
                        )
                    }),
            );
        }
        section
    }

    fn render_comments(&self, state: &CollabState, cx: &mut Context<Self>) -> impl IntoElement {
        let open: Vec<&Comment> = state.comments.iter().filter(|comment| comment.parent.is_none() && !comment.resolved).collect();
        let reader = state.role == Role::Reader;
        let mut section = div().flex().flex_col().flex_none().gap(px(4.)).child(
            div()
                .flex()
                .flex_none()
                .items_center()
                .justify_between()
                .child(Self::heading(format!("Comments ({})", open.len())))
                .child(div().pt(px(6.)).child(self.small_button("collab-comment", "Comment", false, cx, |this, window, cx| this.start_comment(window, cx)))),
        );
        if let Some(draft) = &self.collab.comment {
            let label = if draft.parent.is_some() { "Your reply, Enter to post" } else { "Your comment on the selection, Enter to post" };
            section = section.child(Self::muted(label)).child(Self::field_box(&draft.field));
        }
        if open.is_empty() && self.collab.comment.is_none() {
            section = section.child(Self::muted("Select text and choose Comment to start a thread."));
        }
        let resolved = state.comments.iter().filter(|comment| comment.parent.is_none() && comment.resolved).count();
        for (index, comment) in open.into_iter().enumerate() {
            let quoted: String = self.shared_text(&comment.path, comment.range.clone(), cx).unwrap_or_default().chars().take(60).collect();
            let (path, range) = (comment.path.clone(), comment.range.clone());
            let (reply_path, reply_range, reply_id) = (comment.path.clone(), comment.range.clone(), comment.id.clone());
            let (resolve, delete) = (comment.id.clone(), comment.id.clone());
            let replies = state.comments.iter().filter(|reply| reply.parent.as_deref() == Some(comment.id.as_str()));
            section = section.child(
                div()
                    .id(("collab-thread", index))
                    .flex()
                    .flex_col()
                    .flex_none()
                    .gap(px(3.))
                    .p(px(6.))
                    .rounded(px(4.))
                    .cursor_pointer()
                    .border_l_2()
                    .border_color(theme::color(COMMENT_COLOR))
                    .hover(|style| style.bg(theme::color(theme::palette().divider)))
                    .on_click(cx.listener(move |this, _, window, cx| this.go_to_shared(&path, range.clone(), window, cx)))
                    .when(!quoted.trim().is_empty(), |element| element.child(Self::muted(format!("\u{201c}{}\u{201d}", quoted.trim()))))
                    .child(Self::person(&comment.author, comment.color))
                    .child(div().flex_none().child(comment.text.clone()))
                    .children(replies.map(|reply| {
                        div()
                            .flex()
                            .flex_col()
                            .flex_none()
                            .pl(px(10.))
                            .child(Self::person(&reply.author, reply.color))
                            .child(div().flex_none().child(reply.text.clone()))
                    }))
                    .child(
                        div()
                            .flex()
                            .gap(px(4.))
                            .pt(px(2.))
                            .child(self.small_button(("collab-reply", index), "Reply", false, cx, move |this, window, cx| {
                                this.open_comment_field(reply_path.clone(), reply_range.clone(), Some(reply_id.clone()), window, cx);
                            }))
                            .child(self.small_button(("collab-resolve", index), "Resolve", false, cx, move |this, window, cx| {
                                this.comment_action(
                                    CommentIntent::Resolve {
                                        id: resolve.clone(),
                                        resolved: true,
                                    },
                                    window,
                                    cx,
                                );
                            }))
                            .when(!reader, |element| {
                                element.child(self.small_button(("collab-delete", index), "Delete", false, cx, move |this, window, cx| this.delete_comment(&delete, window, cx)))
                            }),
                    ),
            );
        }
        if resolved > 0 {
            section = section.child(Self::muted(format!("{resolved} resolved")));
        }
        section
    }

    /// The text of a shared file in `range`, while the file is open. Reading
    /// the disk on every frame would cost too much for a quote.
    fn shared_text(&self, path: &str, range: Range<usize>, cx: &Context<Self>) -> Option<String> {
        let target = inside(&self.collab.state.as_ref()?.root, path)?;
        let document = self.documents.iter().find(|document| document.view.read(cx).path.as_deref() == Some(target.as_path()))?;
        let buffer = document.view.read(cx).state.buffer();
        let end = buffer.clip(range.end.min(buffer.len()));
        Some(buffer.slice(buffer.clip(range.start.min(end))..end))
    }
}

/// Takes what changed on disk while the editor was away into a review, as
/// one change per file around what stayed the same. Returns the shared paths.
fn catch_up(project: &mut Project, root: &Path, open: &HashMap<PathBuf, String>) -> HashSet<String> {
    let mut shared = HashSet::new();
    for path in project.paths() {
        let Some(target) = inside(root, &path) else {
            continue;
        };
        let disk = open.get(&target).cloned().or_else(|| std::fs::read_to_string(&target).ok());
        if let Some(disk) = disk {
            let known = project.content(&path);
            if disk != known {
                let middle = changed_middle(&known, &disk);
                let inserted = &disk[middle.start..disk.len() - (known.len() - middle.end)];
                project.edit(&path, &[TextChange::new(middle.start, middle.len(), inserted)]);
            }
        }
        shared.insert(path);
    }
    project.forget_events();
    shared
}

/// Writes a review by file into the project, so that it survives the editor.
fn save_review(state: &mut CollabState) {
    state.save_task = None;
    let Some(meta) = &state.review else {
        return;
    };
    let folder = state.root.join(REVIEW_FOLDER);
    std::fs::create_dir_all(&folder).ok();
    crate::atomic::write(&folder.join(REVIEW_STATE), &state.project.encode_state()).ok();
    crate::atomic::write(&folder.join(REVIEW_META), &serde_json::to_vec(meta).unwrap()).ok();
}

/// The comments and suggestions with where they are now.
fn refresh_annotations(state: &mut CollabState) {
    state.comments = state.project.comments();
    state.suggestions = state.project.suggestions();
}

/// After a deletion that stays, struck through: Backspace keeps the cursor
/// before the struck text, so that it goes on to the left, and deleting
/// forward puts it after, so that it goes on to the right.
fn place_after_struck(view: &mut EditorView, restores: &[TextChange], before: Option<usize>) {
    let head = view.state.selections().primary().head;
    let Some(restore) = restores.iter().find(|restore| restore.start == head) else {
        return;
    };
    let backspace = before == Some(head + restore.inserted.len());
    if !backspace {
        view.state.set_selections(vec![Selection::cursor(head + restore.inserted.len())]);
    }
}

/// Moves the others' cursors along with changes to a file, until they
/// report where they are again.
fn shift_presences(state: &mut CollabState, path: &str, changes: &[TextChange]) {
    for presence in state.peers.values_mut().filter(|presence| presence.path == path) {
        for change in changes {
            let removed = change.start..change.start + change.removed;
            let map = |offset: usize| {
                if offset <= removed.start {
                    offset
                } else if offset >= removed.end {
                    offset - removed.len() + change.inserted.len()
                } else {
                    removed.start + change.inserted.len()
                }
            };
            presence.anchor = map(presence.anchor);
            presence.head = map(presence.head);
        }
    }
}

fn decorations_for(state: &CollabState, path: &str) -> Vec<Decoration> {
    let mut decorations = Vec::new();
    for comment in &state.comments {
        if comment.path == path && comment.parent.is_none() && !comment.resolved && !comment.range.is_empty() {
            decorations.push(Decoration {
                range: comment.range.clone(),
                kind: DecorationKind::Comment,
                color: COMMENT_COLOR,
                label: None,
            });
        }
    }
    for suggestion in &state.suggestions {
        if suggestion.path != path {
            continue;
        }
        let (kind, color) = match suggestion.kind {
            SuggestionKind::Inserted => (DecorationKind::Inserted, suggestion.color),
            SuggestionKind::Deleted(_) => (DecorationKind::Deleted, DELETION_COLOR),
        };
        decorations.push(Decoration {
            range: suggestion.range.clone(),
            kind,
            color,
            label: None,
        });
    }
    for presence in state.peers.values() {
        if presence.path != path {
            continue;
        }
        if presence.anchor != presence.head {
            decorations.push(Decoration {
                range: presence.anchor.min(presence.head)..presence.anchor.max(presence.head),
                kind: DecorationKind::Selection,
                color: presence.color,
                label: None,
            });
        }
        decorations.push(Decoration {
            range: presence.head..presence.head,
            kind: DecorationKind::Caret,
            color: presence.color,
            label: Some(presence.name.clone()),
        });
    }
    decorations
}

fn dirs_home() -> PathBuf {
    crate::platform::home().unwrap_or_else(|| PathBuf::from("."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_from_others_stay_inside_the_project() {
        let root = Path::new("/projects/paper");
        assert_eq!(inside(root, "chapters/intro.tex"), Some(root.join("chapters/intro.tex")));
        for evil in ["../secret.tex", "/etc/passwd", "chapters/../../x", "", "./a"] {
            assert_eq!(inside(root, evil), None, "{evil}");
        }
    }

    #[test]
    fn replay_survives_a_file_that_changed_meanwhile() {
        let mut text = "äb".to_string();
        replay(&mut text, &[TextChange::new(1, 0, "x"), TextChange::new(40, 3, "!")]);
        assert_eq!(text, "xäb!");
    }

    #[test]
    fn a_joined_project_never_lands_in_an_existing_folder() {
        let parent = tempfile::tempdir().unwrap();
        std::fs::create_dir(parent.path().join("Paper")).unwrap();
        assert_eq!(fresh_folder(parent.path(), "Paper"), parent.path().join("Paper 2"));
        assert_eq!(fresh_folder(parent.path(), "a/b"), parent.path().join("a_b"));
    }

    #[test]
    fn the_project_to_share_leaves_hidden_files_out() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("main.tex"), "Text").unwrap();
        std::fs::create_dir(root.path().join(REVIEW_FOLDER)).unwrap();
        std::fs::write(root.path().join(REVIEW_FOLDER).join("notes.txt"), "private").unwrap();
        let (mut project, shared, _) = collect_project(root.path(), &HashMap::new());
        assert_eq!(project.paths(), ["main.tex"]);
        assert_eq!(project.content("main.tex"), "Text");
        assert!(shared.contains("main.tex"));
    }

    #[test]
    fn a_review_catches_up_with_edits_made_outside_and_keeps_its_anchors() {
        let root = tempfile::tempdir().unwrap();
        let mut project = Project::new(1);
        project.add_file("main.tex", "Eins. Zwei. Drei.");
        let author = Author {
            name: "Prof".into(),
            color: 1,
        };
        project.add_comment("main.tex", 12..17, "Hm", None, &author);
        // Another program changed the start of the file.
        std::fs::write(root.path().join("main.tex"), "Null. Eins. Zwei. Drei.").unwrap();
        let shared = catch_up(&mut project, root.path(), &HashMap::new());
        assert!(shared.contains("main.tex"));
        assert_eq!(project.content("main.tex"), "Null. Eins. Zwei. Drei.");
        assert_eq!(project.comments()[0].range, 18..23);
        assert!(project.take_remote().is_empty());
    }
}

