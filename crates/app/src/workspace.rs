use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};

use build::{BuildEvent, BuildManager, BuildOutcome, BuildRequest, Config, Diagnostic, Engine, Severity, Trigger, find_main};
use addon_api::{ActiveFile, DiagnosticInfo, Reply, Request, SearchHit};
use editor_core::{
    Language, Mark, MarkSeverity,
    snippet::Snippet,
    vim::{self, Effect, Key, Leader, Mode},
};
use gpui::{
    AnyElement, Anchor, App, ClickEvent, Context, CursorStyle, Entity, FocusHandle, Focusable, FontWeight, Keystroke,
    KeystrokeEvent, MouseButton, MouseMoveEvent, PathPromptOptions, Pixels, Point, Render, ScrollHandle, SharedString,
    Subscription, Task, Window, anchored, black, deferred, div, point, prelude::*, px, svg,
};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use synctex::SyncTex;

use crate::{
    actions::*,
    addon_panel::{AddonPanel, AddonPanelEvent},
    addons::{self, Installed},
    editor_view::{EditorEvent, EditorView, FormatKind},
    file_tree::{self, TreeEntry},
    git_panel::{self, GitPanel, GitPanelEvent},
    outline::{self, OutlineItem},
    image_view::{ImageView, ImageViewEvent, include_snippet, is_image, relative_path},
    pdf_view::{PdfView, PdfViewEvent, ZOOM_STEP},
    picker::{self, Picker, PickerKind},
    problems::{self, Problem},
    settings::{self, Settings},
    shortcuts,
    snippets,
    status_bar::{self, StatusInfo},
    theme,
};

const TOP_BAR_HEIGHT: f32 = 40.0;
const RAIL_WIDTH: f32 = 40.0;
const DIVIDER_WIDTH: f32 = 7.0;
const TAB_BAR_HEIGHT: f32 = 36.0;
const TOOLBAR_HEIGHT: f32 = 32.0;
const SYMBOL_CELL: f32 = 32.0;
const SYMBOL_COLUMNS: f32 = 11.0;
const TABLE_GRID: usize = 8;
const PREVIEW_TOOLBAR_HEIGHT: f32 = 32.0;
const MIN_PANE_WIDTH: f32 = 160.0;
const OUTLINE_INSET: f32 = 8.0;
const OUTLINE_INDENT: f32 = 16.0;
const OUTLINE_CHEVRON: f32 = 16.0;
const FOLLOW_DEBOUNCE: Duration = Duration::from_millis(150);
/// Oldest output is dropped beyond this size.
const MAX_LOG_BYTES: usize = 2 << 20;
const MAX_LOG_LINES_SHOWN: usize = 2000;
const MAX_DIAGNOSTICS_SHOWN: usize = 200;
/// Beyond this many errors the later ones are echoes of the first, and marking
/// them would paint the whole document red.
const MAX_ERRORS_MARKED: usize = 5;
const DIALOG_WIDTH: f32 = 640.0;
const DIALOG_HEIGHT: f32 = 560.0;
const PICKER_WIDTH: f32 = 720.0;
const PICKER_HEIGHT: f32 = 440.0;
/// A pause in typing before the project is searched for the query.
const PICKER_SEARCH_DELAY: Duration = Duration::from_millis(120);

#[cfg(target_os = "macos")]
const TRAFFIC_LIGHT_SPACE: f32 = 78.0;
#[cfg(not(target_os = "macos"))]
const TRAFFIC_LIGHT_SPACE: f32 = 8.0;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Divider {
    FilePanel,
    Preview,
    Outline,
}

/// What the left panel shows.
#[derive(Clone, Copy, PartialEq, Eq)]
enum LeftPanel {
    Files,
    Git,
    /// The list of installed add-ons.
    Addons,
    /// An index into `addon_panels`.
    Addon(usize),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Menu {
    File,
    Edit,
    Insert,
    View,
    Format,
    Help,
    Layout,
    Compile,
    Symbols,
    Table,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SettingsPage {
    Editor,
    Compiler,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Dialog {
    Settings(SettingsPage),
    Shortcuts,
}

/// How the PDF on screen relates to the last build.
#[derive(Clone, Copy, PartialEq, Eq)]
enum PreviewState {
    Current,
    /// The build reported errors but still produced a PDF.
    Incomplete,
    /// The build produced no PDF, so this is the last good one.
    LastGood,
    /// The build produced no PDF, and there is none to fall back on.
    Empty,
}

type MenuHandler = Rc<dyn Fn(&mut Workspace, &mut Window, &mut Context<Workspace>)>;

struct MenuEntry {
    label: String,
    shortcut: &'static str,
    handler: MenuHandler,
}

fn entry(
    label: impl Into<String>,
    shortcut: &'static str,
    handler: impl Fn(&mut Workspace, &mut Window, &mut Context<Workspace>) + 'static,
) -> MenuEntry {
    MenuEntry {
        label: label.into(),
        shortcut,
        handler: Rc::new(handler),
    }
}

struct OpenDocument {
    view: Entity<EditorView>,
    _subscription: Subscription,
    _observer: Subscription,
}

/// The panel of an add-on that is turned on.
struct AddonEntry {
    panel: Entity<AddonPanel>,
    _subscription: Subscription,
}

/// What a discard of add-on edits has to take back.
#[derive(Default)]
struct TrackedEdits {
    originals: HashMap<PathBuf, String>,
    created: Vec<PathBuf>,
}

struct ImagePreview {
    view: Entity<ImageView>,
    _subscription: Subscription,
}

pub struct Workspace {
    root: Option<PathBuf>,
    entries: Vec<TreeEntry>,
    collapsed: HashSet<PathBuf>,
    documents: Vec<OpenDocument>,
    active: Option<usize>,
    outline: Vec<OutlineItem>,
    outline_version: (usize, u64),
    show_file_panel: bool,
    left_panel: LeftPanel,
    git_panel: Entity<GitPanel>,
    _git_subscription: Subscription,
    installed_addons: Vec<Installed>,
    addon_panels: Vec<AddonEntry>,
    tracked_edits: HashMap<String, TrackedEdits>,
    /// Add-on requests that wait for the running build to end.
    compile_waiters: Vec<(Entity<AddonPanel>, u64)>,
    show_preview: bool,
    show_logs: bool,
    show_outline: bool,
    file_panel_width: Pixels,
    preview_width: Pixels,
    outline_height: Pixels,
    dragging: Option<Divider>,
    open_menu: Option<Menu>,
    /// Where the open menu appears, below the button that opened it.
    menu_origin: Point<Pixels>,
    symbol_group: usize,
    /// What the hovered palette entry inserts.
    symbol_hover: Option<&'static str>,
    /// The hovered size of the table grid, as rows and columns.
    table_hover: (usize, usize),
    /// Held by the backdrop of an open menu, so that keys do not reach the editor.
    menu_focus: FocusHandle,
    status: Option<SharedString>,
    pdf: Entity<PdfView>,
    build: BuildManager,
    config: Config,
    building: bool,
    build_log: String,
    diagnostics: Vec<Diagnostic>,
    problems: Vec<Problem>,
    show_raw_log: bool,
    /// The user closed the error card of the current build.
    error_card_dismissed: bool,
    preview_state: PreviewState,
    /// LaTeX state from a failed or stopped run must not reach the next run.
    clean_next_build: bool,
    engine: Option<Engine>,
    synctex: Option<SyncTex>,
    sync_on_click: bool,
    /// The line of the active document that the PDF last followed.
    follow_line: Option<usize>,
    follow_task: Option<Task<()>>,
    /// Shown instead of the editor. The tabs stay as they are.
    image_preview: Option<ImagePreview>,
    last_pdf: Option<PathBuf>,
    last_main: Option<PathBuf>,
    /// The main file of the PDF on screen.
    pdf_main: Option<PathBuf>,
    settings: Settings,
    settings_path: Option<PathBuf>,
    dialog: Option<Dialog>,
    dialog_focus: FocusHandle,
    picker: Option<Picker>,
    picker_focus: FocusHandle,
    picker_scroll: ScrollHandle,
    picker_files_task: Option<Task<()>>,
    tree_focus: FocusHandle,
    tree_scroll: ScrollHandle,
    tree_cursor: usize,
    tree_g_pending: bool,
    tree_leader: Option<String>,
    git_branch: Option<String>,
    /// The diagnostic that `]d` and `[d` jumped to last.
    error_cursor: Option<usize>,
    _key_subscription: Subscription,
    _quit_subscription: Subscription,
    build_task: Option<Task<()>>,
    debounce_task: Option<Task<()>>,
    synctex_task: Option<Task<()>>,
    _pdf_subscription: Subscription,
    _watcher: Option<RecommendedWatcher>,
    _watch_task: Option<Task<()>>,
}

impl Workspace {
    pub fn new(root: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let pdf = cx.new(PdfView::new);
        let pdf_subscription = cx.subscribe_in(&pdf, window, |this, _, event, window, cx| match event {
            PdfViewEvent::BackwardSearch { page, x, y } => this.backward_search(*page, *x, *y, window, cx),
            PdfViewEvent::Failed(message) => this.set_status(message.clone(), cx),
        });
        // Edits live in memory only, so closing must not lose them.
        let quit_subscription = cx.on_app_quit(|this, cx| {
            this.save_all(cx);
            async {}
        });
        let workspace_handle = cx.entity().downgrade();
        window.on_window_should_close(cx, move |_, cx| {
            workspace_handle.update(cx, |this, cx| this.save_all(cx)).unwrap_or(true)
        });
        let key_handle = cx.entity().downgrade();
        let key_subscription = cx.intercept_keystrokes(move |event, window, cx| {
            key_handle.update(cx, |this, cx| this.intercept_key(event, window, cx)).ok();
        });
        let workspace_handle = cx.entity().downgrade();
        let git_panel = cx.new(|cx| GitPanel::new(workspace_handle, cx));
        let git_subscription = cx.subscribe_in(&git_panel, window, |this, _, event, window, cx| match event {
            GitPanelEvent::OpenFile(path) => this.open_file(path.clone(), window, cx),
            GitPanelEvent::PickBranch => this.open_picker(PickerKind::Branches, window, cx),
            GitPanelEvent::Changed(branch) => {
                this.git_branch = branch.clone().or_else(|| this.root.as_deref().and_then(git_branch));
                this.refresh_from_disk(cx);
            }
        });
        let settings_path = settings::default_path();
        let settings = settings_path.as_deref().map(Settings::load).unwrap_or_default();
        let mut workspace = Self {
            root: None,
            entries: Vec::new(),
            collapsed: HashSet::new(),
            documents: Vec::new(),
            active: None,
            outline: Vec::new(),
            outline_version: (usize::MAX, 0),
            show_file_panel: true,
            left_panel: LeftPanel::Files,
            git_panel,
            _git_subscription: git_subscription,
            installed_addons: Vec::new(),
            addon_panels: Vec::new(),
            tracked_edits: HashMap::new(),
            compile_waiters: Vec::new(),
            show_preview: true,
            show_logs: false,
            show_outline: true,
            file_panel_width: px(217.5),
            preview_width: px(612.5),
            outline_height: px(320.),
            dragging: None,
            open_menu: None,
            menu_origin: point(px(0.), px(0.)),
            symbol_group: 0,
            symbol_hover: None,
            table_hover: (1, 1),
            menu_focus: cx.focus_handle(),
            status: None,
            pdf,
            build: BuildManager::new(),
            config: Config::default(),
            building: false,
            build_log: String::new(),
            diagnostics: Vec::new(),
            problems: Vec::new(),
            show_raw_log: false,
            error_card_dismissed: false,
            preview_state: PreviewState::Current,
            clean_next_build: false,
            engine: None,
            synctex: None,
            sync_on_click: true,
            follow_line: None,
            follow_task: None,
            image_preview: None,
            last_pdf: None,
            last_main: None,
            pdf_main: None,
            settings,
            settings_path,
            dialog: None,
            dialog_focus: cx.focus_handle(),
            picker: None,
            picker_focus: cx.focus_handle(),
            picker_scroll: ScrollHandle::new(),
            picker_files_task: None,
            tree_focus: cx.focus_handle(),
            tree_scroll: ScrollHandle::new(),
            tree_cursor: 0,
            tree_g_pending: false,
            tree_leader: None,
            git_branch: None,
            error_cursor: None,
            _key_subscription: key_subscription,
            _quit_subscription: quit_subscription,
            build_task: None,
            debounce_task: None,
            synctex_task: None,
            _pdf_subscription: pdf_subscription,
            _watcher: None,
            _watch_task: None,
        };
        workspace.reload_addons(cx);
        if let Some(root) = root {
            workspace.open_root(root, window, cx);
        }
        workspace
    }

    fn open_root(&mut self, root: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.entries = file_tree::scan(&root, &self.collapsed);
        self.documents.clear();
        self.active = None;
        self.image_preview = None;
        self.start_watching(&root, cx);
        self.git_branch = git_branch(&root);
        self.root = Some(root.clone());
        self.git_panel.update(cx, |panel, cx| panel.set_root(Some(root.clone()), cx));
        self.tracked_edits.clear();
        for entry in &self.addon_panels {
            entry.panel.update(cx, |panel, cx| panel.set_root(Some(root.clone()), cx));
        }
        self.config = match Config::load(&root) {
            Ok(config) => config,
            Err(error) => {
                self.set_status(error, cx);
                Config::default()
            }
        };

        let first = ["main.tex", "main.typ", "README.md"]
            .iter()
            .map(|name| root.join(name))
            .find(|path| path.is_file())
            .or_else(|| {
                self.entries
                    .iter()
                    .find(|entry| {
                        !entry.is_dir && Language::from_path(&entry.path) != Language::Plain
                    })
                    .map(|entry| entry.path.clone())
            });
        if let Some(path) = first {
            self.open_file(path, window, cx);
            self.recompile(cx);
        }
        cx.notify();
    }

    fn start_watching(&mut self, root: &Path, cx: &mut Context<Self>) {
        let (sender, receiver) = mpsc::channel::<()>();
        let watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
            // Build output would otherwise wake the tree on every compile.
            let relevant = event.is_ok_and(|event| event.paths.iter().any(|path| git_panel::is_watched_path(path)));
            if relevant {
                sender.send(()).ok();
            }
        });
        let Ok(mut watcher) = watcher else {
            self.status = Some("File watching is unavailable.".into());
            return;
        };
        if watcher.watch(root, RecursiveMode::Recursive).is_err() {
            self.status = Some("File watching is unavailable.".into());
            return;
        }
        self._watcher = Some(watcher);
        self._watch_task = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(250)).await;
                let mut changed = false;
                loop {
                    match receiver.try_recv() {
                        Ok(()) => changed = true,
                        Err(mpsc::TryRecvError::Empty) => break,
                        Err(mpsc::TryRecvError::Disconnected) => return,
                    }
                }
                if changed
                    && this
                        .update(cx, |this, cx| this.refresh_from_disk(cx))
                        .is_err()
                {
                    return;
                }
            }
        }));
    }

    fn refresh_from_disk(&mut self, cx: &mut Context<Self>) {
        if let Some(root) = &self.root {
            self.entries = file_tree::scan(root, &self.collapsed);
        }
        for document in &self.documents {
            document.view.update(cx, |view, cx| view.reload_from_disk(cx));
        }
        self.git_panel.update(cx, |panel, cx| panel.refresh(cx));
        cx.notify();
    }

    fn open_file(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if is_image(&path) {
            self.open_image(path, window, cx);
            return;
        }
        if let Some(index) = self
            .documents
            .iter()
            .position(|document| document.view.read(cx).path.as_deref() == Some(&path))
        {
            self.activate(index, window, cx);
            return;
        }
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) => {
                self.status = Some(format!("Cannot open {}: {error}", path.display()).into());
                cx.notify();
                return;
            }
        };
        self.add_document(path, &text, cx);
        self.activate(self.documents.len() - 1, window, cx);
        self.apply_marks(cx);
    }

    /// Opens a tab without switching to it.
    fn add_document(&mut self, path: PathBuf, text: &str, cx: &mut Context<Self>) {
        let settings = self.settings;
        let view = cx.new(|cx| {
            let mut view = EditorView::new(text, Some(path), cx);
            view.set_vim_mode(settings.vim_mode, settings.smooth_cursor, cx);
            view
        });
        let subscription = cx.subscribe(&view, |this, _, event, cx| match event {
            EditorEvent::Edited => {
                this.schedule_type_build(cx);
                cx.notify();
            }
            EditorEvent::CursorClicked { line } => {
                if this.sync_on_click {
                    this.forward_search(*line, false, cx);
                }
            }
        });
        let observer = cx.observe(&view, |this, view, cx| this.cursor_changed(&view, cx));
        self.documents.push(OpenDocument {
            view,
            _subscription: subscription,
            _observer: observer,
        });
    }

    fn open_image(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let active = self.active_view().and_then(|view| view.read(cx).path.clone());
        let main = self.root.as_deref().and_then(|root| find_main(root, &self.config, active.as_deref()));
        let engine = self.engine.or_else(|| Engine::for_path(main.as_deref()?));
        let directory = main.as_deref().and_then(Path::parent).or(self.root.as_deref());
        let relative = directory.map_or_else(|| path.display().to_string(), |directory| relative_path(directory, &path));
        let include = include_snippet(engine, &relative);
        let view = cx.new(|cx| ImageView::new(path, include, cx));
        let subscription = cx.subscribe(&view, |this, _, event, cx| match event {
            ImageViewEvent::Status(message) => this.set_status(message.clone(), cx),
        });
        let focus = view.read(cx).focus_handle().clone();
        window.focus(&focus, cx);
        self.image_preview = Some(ImagePreview { view, _subscription: subscription });
        cx.notify();
    }

    fn activate(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.image_preview = None;
        self.follow_line = None;
        self.active = Some(index);
        let view = self.documents[index].view.clone();
        window.focus(&view.read(cx).focus_handle(cx), cx);
        self.compile_if_document_changed(cx);
        cx.notify();
    }

    fn close_document(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let view = self.documents[index].view.clone();
        if let Err(error) = view.update(cx, |view, cx| view.save(cx)) {
            self.status = Some(format!("Cannot save: {error}").into());
            cx.notify();
            return;
        }
        self.remove_document(index, window, cx);
    }

    fn remove_document(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.documents.remove(index);
        self.active = if self.documents.is_empty() {
            None
        } else {
            Some(index.min(self.documents.len() - 1))
        };
        if let Some(active) = self.active {
            self.activate(active, window, cx);
        } else if self.settings.vim_mode {
            self.focus_tree(window, cx);
        }
        cx.notify();
    }

    fn active_view(&self) -> Option<Entity<EditorView>> {
        self.active
            .and_then(|index| self.documents.get(index))
            .map(|document| document.view.clone())
    }

    fn with_active(&self, cx: &mut Context<Self>, f: impl FnOnce(&mut EditorView, &mut Context<EditorView>)) {
        if let Some(view) = self.active_view() {
            view.update(cx, f);
        }
    }

    fn save_active(&mut self, cx: &mut Context<Self>) {
        let Some(view) = self.active_view() else {
            return;
        };
        match view.update(cx, |view, cx| view.save(cx)) {
            Ok(true) if self.config.trigger == Trigger::Save => self.recompile(cx),
            Ok(_) => {}
            Err(error) => self.set_status(format!("Cannot save: {error}"), cx),
        }
    }

    fn set_status(&mut self, message: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.status = Some(message.into());
        cx.notify();
    }

    fn prompt_open_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Open".into()),
        });
        let handle = window.window_handle();
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            handle
                .update(cx, |_, window, cx| {
                    this.update(cx, |this, cx| this.open_root(path, window, cx)).ok();
                })
                .ok();
        })
        .detach();
    }

    // Add-ons

    /// Reads the add-on folder and keeps panels only for what is turned on.
    fn reload_addons(&mut self, cx: &mut Context<Self>) {
        let Some(directory) = addons::default_directory() else {
            return;
        };
        let (installed, errors) = addons::load_all(&directory);
        if let Some(error) = errors.into_iter().next() {
            self.set_status(error, cx);
        }
        let shown = match self.left_panel {
            LeftPanel::Addon(index) => self.addon_panels.get(index).map(|entry| entry.panel.read(cx).add_on.manifest.id.clone()),
            _ => None,
        };
        let mut old = std::mem::take(&mut self.addon_panels);
        for add_on in installed.iter().filter(|add_on| add_on.enabled) {
            let kept = old.iter().position(|entry| entry.panel.read(cx).add_on == *add_on);
            let entry = match kept {
                Some(index) => old.remove(index),
                None => self.new_addon_entry(add_on.clone(), cx),
            };
            self.addon_panels.push(entry);
        }
        if matches!(self.left_panel, LeftPanel::Addon(_)) {
            let index = self.addon_panels.iter().position(|entry| Some(&entry.panel.read(cx).add_on.manifest.id) == shown.as_ref());
            self.left_panel = index.map_or(LeftPanel::Files, LeftPanel::Addon);
        }
        self.installed_addons = installed;
        cx.notify();
    }

    fn new_addon_entry(&self, add_on: Installed, cx: &mut Context<Self>) -> AddonEntry {
        let root = self.root.clone();
        let panel = cx.new(|_| AddonPanel::new(add_on, root));
        let subscription = cx.subscribe(&panel, |this, panel, event, cx| {
            let AddonPanelEvent::Request { id, request } = event;
            this.handle_addon_request(panel, *id, request.clone(), cx);
        });
        AddonEntry { panel, _subscription: subscription }
    }

    fn set_addon_enabled(&mut self, add_on: &Installed, enabled: bool, cx: &mut Context<Self>) {
        if let Err(error) = addons::set_enabled(add_on, enabled) {
            self.set_status(format!("Cannot change {}: {error}", add_on.manifest.name), cx);
        }
        self.reload_addons(cx);
    }

    fn remove_addon(&mut self, add_on: &Installed, cx: &mut Context<Self>) {
        if let Err(error) = addons::remove(add_on) {
            self.set_status(format!("Cannot remove {}: {error}", add_on.manifest.name), cx);
        }
        self.reload_addons(cx);
    }

    fn prompt_install_addon(&mut self, cx: &mut Context<Self>) {
        let Some(directory) = addons::default_directory() else {
            return self.set_status("The add-on folder is unknown, because HOME is not set.", cx);
        };
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Install".into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let Some(source) = paths.into_iter().next() else {
                return;
            };
            let result = cx.background_spawn(async move { addons::install_from(&source, &directory) }).await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(manifest) => this.set_status(format!("Installed {}. Turn it on to use it.", manifest.name), cx),
                    Err(error) => this.set_status(error, cx),
                }
                this.reload_addons(cx);
            })
            .ok();
        })
        .detach();
    }

    fn addon_file(&self, relative: &str) -> Result<PathBuf, String> {
        let root = self.root.as_deref().ok_or("No project is open.")?;
        addons::resolve_path(root, relative)
    }

    fn document_at(&self, path: &Path, cx: &App) -> Option<usize> {
        let wanted = path.canonicalize().ok();
        self.documents.iter().position(|document| {
            let open = document.view.read(cx).path.as_deref();
            open == Some(path) || (wanted.is_some() && open.and_then(|open| open.canonicalize().ok()) == wanted)
        })
    }

    fn relative_to_project(&self, path: &Path) -> String {
        let root = self.root.as_deref().and_then(|root| root.canonicalize().ok());
        let real = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        root.and_then(|root| real.strip_prefix(root).ok().map(|inner| inner.display().to_string()))
            .unwrap_or_else(|| path.display().to_string())
    }

    fn diagnostic_infos(&self) -> Vec<DiagnosticInfo> {
        self.diagnostics
            .iter()
            .map(|diagnostic| DiagnosticInfo {
                path: diagnostic.file.as_deref().map(|file| self.relative_to_project(file)),
                line: diagnostic.line,
                error: diagnostic.severity == Severity::Error,
                message: diagnostic.message.clone(),
            })
            .collect()
    }

    fn reply_to_compile_waiters(&mut self, result: impl Fn() -> Result<Reply, String>, cx: &mut Context<Self>) {
        for (panel, id) in std::mem::take(&mut self.compile_waiters) {
            panel.update(cx, |panel, _| panel.reply(id, result()));
        }
    }

    fn handle_addon_request(&mut self, panel: Entity<AddonPanel>, id: u64, request: Request, cx: &mut Context<Self>) {
        let add_on = panel.read(cx).add_on.manifest.id.clone();
        let result = match request {
            Request::ListFiles | Request::Search { .. } => return self.answer_from_project_scan(panel, id, request, cx),
            Request::GetSecret { key } => {
                return self.answer_in_background(panel, id, cx, move || {
                    addons::get_secret(&add_on, &key).map(|value| Reply::Secret { value })
                });
            }
            Request::SetSecret { key, value } => {
                return self.answer_in_background(panel, id, cx, move || {
                    addons::set_secret(&add_on, &key, &value).map(|()| Reply::Done)
                });
            }
            Request::Compile => {
                self.recompile(cx);
                if self.building {
                    self.compile_waiters.push((panel, id));
                    return;
                }
                let reason = self.status.as_ref().map_or_else(|| "The build did not start.".to_string(), ToString::to_string);
                Err(reason)
            }
            Request::ReadFile { path } => self.addon_read(&path, cx).map(|text| Reply::Text { text }),
            Request::Diagnostics => Ok(Reply::Diagnostics { diagnostics: self.diagnostic_infos() }),
            Request::ActiveFile => Ok(Reply::Active { file: self.active_file_info(cx) }),
            Request::EditFile { path, old, new } => self.addon_edit(&add_on, &path, &old, &new, cx).map(|()| Reply::Done),
            Request::CreateFile { path, text } => self.addon_create(&add_on, &path, &text, cx).map(|()| Reply::Done),
            Request::BeginEdits => {
                self.tracked_edits.insert(add_on, TrackedEdits::default());
                Ok(Reply::Done)
            }
            Request::DiscardEdits => {
                self.discard_addon_edits(&add_on, cx);
                Ok(Reply::Done)
            }
        };
        panel.update(cx, |panel, _| panel.reply(id, result));
    }

    fn answer_in_background(
        &self,
        panel: Entity<AddonPanel>,
        id: u64,
        cx: &mut Context<Self>,
        work: impl FnOnce() -> Result<Reply, String> + Send + 'static,
    ) {
        cx.spawn(async move |_, cx| {
            let result = cx.background_spawn(async move { work() }).await;
            panel.update(cx, |panel, _| panel.reply(id, result));
        })
        .detach();
    }

    fn answer_from_project_scan(&self, panel: Entity<AddonPanel>, id: u64, request: Request, cx: &mut Context<Self>) {
        let root = self.root.clone();
        self.answer_in_background(panel, id, cx, move || {
            let root = root.ok_or("No project is open.")?;
            let files: Vec<PathBuf> = file_tree::scan(&root, &HashSet::new())
                .into_iter()
                .filter(|entry| !entry.is_dir)
                .map(|entry| entry.path)
                .collect();
            match request {
                Request::Search { query } => {
                    let hits = picker::search_text(&files, &root, &query, &AtomicBool::new(false))
                        .into_iter()
                        .map(|item| SearchHit {
                            path: item.label,
                            line: item.position.map_or(1, |(line, _)| line + 1),
                            text: item.detail.unwrap_or_default(),
                        })
                        .collect();
                    Ok(Reply::Hits { hits })
                }
                _ => Ok(Reply::Files { files: files.iter().map(|file| picker::relative_label(file, &root)).collect() }),
            }
        });
    }

    fn addon_read(&self, relative: &str, cx: &App) -> Result<String, String> {
        let path = self.addon_file(relative)?;
        match self.document_at(&path, cx) {
            Some(index) => Ok(self.documents[index].view.read(cx).state.buffer().to_string()),
            None => addons::read_file_limited(&path),
        }
    }

    fn active_file_info(&self, cx: &App) -> Option<ActiveFile> {
        let view = self.active_view()?;
        let view = view.read(cx);
        let path = view.path.as_deref()?;
        let selection = view.state.selections().primary().range();
        Some(ActiveFile {
            path: self.relative_to_project(path),
            line: view.cursor_position().0,
            selection: view.state.buffer().slice(selection),
        })
    }

    fn addon_edit(&mut self, add_on: &str, relative: &str, old: &str, new: &str, cx: &mut Context<Self>) -> Result<(), String> {
        let path = self.addon_file(relative)?;
        let index = match self.document_at(&path, cx) {
            Some(index) => index,
            None => {
                let text = addons::read_file_limited(&path)?;
                self.add_document(path.clone(), &text, cx);
                self.documents.len() - 1
            }
        };
        if self.active.is_none() {
            self.active = Some(index);
        }
        let view = self.documents[index].view.clone();
        let text = view.read(cx).state.buffer().to_string();
        let range = addons::find_unique(&text, old)?;
        self.tracked_edits.entry(add_on.to_string()).or_default().originals.entry(path).or_insert(text);
        view.update(cx, |view, cx| view.edit_from_outside(range, new, cx));
        cx.notify();
        Ok(())
    }

    fn addon_create(&mut self, add_on: &str, relative: &str, text: &str, cx: &mut Context<Self>) -> Result<(), String> {
        let path = self.addon_file(relative)?;
        if path.exists() {
            return Err(format!("{relative} already exists."));
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| format!("Cannot create {relative}: {error}"))?;
        }
        std::fs::write(&path, "").map_err(|error| format!("Cannot create {relative}: {error}"))?;
        self.tracked_edits.entry(add_on.to_string()).or_default().created.push(path.clone());
        self.add_document(path, "", cx);
        let index = self.documents.len() - 1;
        if self.active.is_none() {
            self.active = Some(index);
        }
        self.documents[index].view.update(cx, |view, cx| view.edit_from_outside(0..0, text, cx));
        self.refresh_from_disk(cx);
        Ok(())
    }

    /// Puts back the text from before the first edit, as one step that undo can
    /// reverse. Edits by the user in those files since then go with it.
    fn discard_addon_edits(&mut self, add_on: &str, cx: &mut Context<Self>) {
        let Some(tracked) = self.tracked_edits.remove(add_on) else {
            return;
        };
        for (path, original) in tracked.originals {
            if let Some(index) = self.document_at(&path, cx) {
                self.documents[index].view.update(cx, |view, cx| {
                    let length = view.state.buffer().len();
                    view.edit_from_outside(0..length, &original, cx);
                });
            }
        }
        for path in tracked.created {
            if let Some(index) = self.document_at(&path, cx) {
                self.documents.remove(index);
                self.active = match self.active {
                    Some(active) if active == index => self.documents.len().checked_sub(1),
                    Some(active) if active > index => Some(active - 1),
                    other => other,
                };
            }
            std::fs::remove_file(&path).ok();
        }
        self.refresh_from_disk(cx);
    }

    /// The compilers read files, so every open document is saved first.
    pub(crate) fn save_all(&mut self, cx: &mut Context<Self>) -> bool {
        for index in 0..self.documents.len() {
            let view = self.documents[index].view.clone();
            if let Err(error) = view.update(cx, |view, cx| view.save(cx)) {
                self.set_status(format!("Cannot save: {error}"), cx);
                return false;
            }
        }
        true
    }

    fn recompile(&mut self, cx: &mut Context<Self>) {
        self.debounce_task = None;
        let Some(root) = self.root.clone() else {
            self.set_status("Open a folder first.", cx);
            return;
        };
        if !self.save_all(cx) {
            return;
        }
        let active = self.active_view().and_then(|view| view.read(cx).path.clone());
        let Some(main) = build::find_main(&root, &self.config, active.as_deref()) else {
            self.set_status("This file type cannot be compiled.", cx);
            return;
        };
        let Some(engine) = self.config.engine.or_else(|| Engine::for_path(&main)) else {
            self.set_status(format!("No compiler for {}.", main.display()), cx);
            return;
        };
        self.engine = Some(engine);
        self.last_main = Some(main.clone());
        // Starting over stops a running build, which can leave broken files behind.
        let clean = std::mem::take(&mut self.clean_next_build) || self.building;
        self.building = true;
        self.build_log.clear();
        self.status = Some(format!("Compiling with {}…", engine.label()).into());
        let mut events = self.build.start(BuildRequest { root, main, engine, clean });
        self.build_task = Some(cx.spawn(async move |this, cx| {
            while let Some(event) = events.recv().await {
                if this.update(cx, |this, cx| this.handle_build_event(event, cx)).is_err() {
                    return;
                }
            }
        }));
        cx.notify();
    }

    fn recompile_from_scratch(&mut self, cx: &mut Context<Self>) {
        self.clean_next_build = true;
        self.recompile(cx);
    }

    /// A folder of standalone notes has one PDF per file, so switching files
    /// must not leave the PDF of the previous file on screen.
    fn compile_if_document_changed(&mut self, cx: &mut Context<Self>) {
        let (Some(root), Some(last_main)) = (&self.root, &self.last_main) else {
            return;
        };
        let active = self.active_view().and_then(|view| view.read(cx).path.clone());
        let main = build::find_main(root, &self.config, active.as_deref());
        if main.is_some_and(|main| main != *last_main) {
            self.recompile(cx);
        }
    }

    fn schedule_type_build(&mut self, cx: &mut Context<Self>) {
        if self.config.trigger != Trigger::Type {
            return;
        }
        let delay = Duration::from_millis(self.config.debounce_ms);
        self.debounce_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(delay).await;
            this.update(cx, |this, cx| this.recompile(cx)).ok();
        }));
    }

    fn handle_build_event(&mut self, event: BuildEvent, cx: &mut Context<Self>) {
        match event {
            BuildEvent::Started => {}
            BuildEvent::Output(line) => {
                self.build_log.push_str(&line);
                if self.build_log.len() > MAX_LOG_BYTES {
                    let mut cut = self.build_log.len() - MAX_LOG_BYTES / 2;
                    while !self.build_log.is_char_boundary(cut) {
                        cut += 1;
                    }
                    self.build_log.drain(..cut);
                }
                if self.show_logs {
                    cx.notify();
                }
            }
            BuildEvent::Cancelled => {
                self.building = false;
                self.clean_next_build = true;
                self.set_status("Build stopped.", cx);
                self.reply_to_compile_waiters(|| Err("The build was stopped.".to_string()), cx);
            }
            BuildEvent::Finished(outcome) => self.finish_build(outcome, cx),
        }
    }

    fn finish_build(&mut self, outcome: BuildOutcome, cx: &mut Context<Self>) {
        self.building = false;
        self.status = Some(
            format!(
                "{} in {:.1} s",
                if outcome.success { "Compiled" } else { "Failed" },
                outcome.duration.as_secs_f32(),
            )
            .into(),
        );
        self.clean_next_build = !outcome.success;
        self.diagnostics = outcome.diagnostics;
        self.diagnostics.sort_by_key(|diagnostic| diagnostic.severity != Severity::Error);
        self.problems = problems::problems(&self.diagnostics);
        self.error_cursor = None;
        self.error_card_dismissed = false;
        self.apply_marks(cx);
        let (success, diagnostics) = (outcome.success, self.diagnostic_infos());
        self.reply_to_compile_waiters(|| Ok(Reply::Compiled { success, diagnostics: diagnostics.clone() }), cx);
        let same_document = self.pdf_main == self.last_main;
        self.preview_state = match (outcome.pdf, outcome.last_good_pdf) {
            (Some(pdf), _) => {
                self.last_pdf = Some(pdf.clone());
                self.pdf_main = self.last_main.clone();
                self.pdf.update(cx, |view, cx| view.load_file(pdf, cx));
                if outcome.success { PreviewState::Current } else { PreviewState::Incomplete }
            }
            // The PDF on screen stays. It only changes for one of this document.
            (None, _) if same_document => PreviewState::LastGood,
            (None, Some(pdf)) => {
                self.last_pdf = Some(pdf.clone());
                self.pdf_main = self.last_main.clone();
                self.pdf.update(cx, |view, cx| view.load_file(pdf, cx));
                PreviewState::LastGood
            }
            (None, None) => {
                self.pdf_main = None;
                self.last_pdf = None;
                self.pdf.update(cx, |view, cx| view.clear(cx));
                PreviewState::Empty
            }
        };
        self.synctex_task = outcome.synctex.map(|path| {
            cx.spawn(async move |this, cx| {
                let synctex = cx.background_spawn(async move { SyncTex::open(&path) }).await;
                this.update(cx, |this, cx| match synctex {
                    Ok(synctex) => {
                        this.synctex = Some(synctex);
                        this.follow_cursor(cx);
                    }
                    Err(error) => {
                        this.synctex = None;
                        this.set_status(format!("Cannot read SyncTeX data: {error}"), cx);
                    }
                })
                .ok();
            })
        });
        cx.notify();
    }

    /// Turns diagnostics into squiggles in the open documents.
    fn apply_marks(&mut self, cx: &mut Context<Self>) {
        let errors = self.error_count();
        for document in &self.documents {
            document.view.update(cx, |view, cx| {
                let Some(path) = view.path.clone() else {
                    return;
                };
                let buffer = view.state.buffer();
                let shown = if errors > MAX_ERRORS_MARKED { 1 } else { self.diagnostics.len() };
                let marks = self
                    .diagnostics
                    .iter()
                    .take(shown)
                    .filter(|diagnostic| diagnostic.file.as_deref() == Some(path.as_path()))
                    .filter_map(|diagnostic| {
                        let line = diagnostic.line?.checked_sub(1)?;
                        if line >= buffer.line_count() {
                            return None;
                        }
                        let text = buffer.line_text(line);
                        let start = buffer.line_start(line);
                        let indent = text.len() - text.trim_start().len();
                        let range = match diagnostic.column {
                            Some(column) => {
                                let from = start + byte_column(&text, column);
                                match diagnostic.length {
                                    Some(length) => from..start + byte_column(&text, column + length),
                                    None => from..from,
                                }
                            }
                            None => start + indent..buffer.line_end(line),
                        };
                        let range = if range.is_empty() {
                            let word = buffer.word_range(range.start);
                            if word.is_empty() { range.start..buffer.next_boundary(range.start) } else { word }
                        } else {
                            range
                        };
                        Some(Mark {
                            range,
                            severity: match diagnostic.severity {
                                Severity::Error => MarkSeverity::Error,
                                Severity::Warning => MarkSeverity::Warning,
                            },
                            message: diagnostic.message.clone(),
                        })
                    })
                    .collect();
                view.set_marks(marks, cx);
            });
        }
    }

    /// Explains why PDF navigation is off. `None` means it is available.
    fn synctex_unavailable_reason(&self) -> Option<String> {
        match self.engine {
            Some(engine) if !engine.has_synctex() => Some(format!(
                "PDF navigation works for LaTeX only. {} produces no SyncTeX data.",
                engine.label()
            )),
            _ if self.synctex.is_none() => Some("PDF navigation needs a finished LaTeX build.".into()),
            _ => None,
        }
    }

    fn forward_search(&mut self, line: usize, report: bool, cx: &mut Context<Self>) {
        if let Some(reason) = self.synctex_unavailable_reason() {
            if report {
                self.set_status(reason, cx);
            }
            return;
        }
        let path = self.active_view().and_then(|view| view.read(cx).path.clone());
        let location = path.and_then(|path| self.synctex.as_ref()?.forward(&path, line + 1));
        match location {
            Some(location) => self.pdf.update(cx, |view, cx| view.reveal(location, cx)),
            None if report => self.set_status("This line has no position in the PDF.", cx),
            None => {}
        }
    }

    /// Runs whenever the document notifies, which covers every way of moving the
    /// cursor, so it has to stay cheap until the line changes.
    fn cursor_changed(&mut self, view: &Entity<EditorView>, cx: &mut Context<Self>) {
        if !self.settings.auto_scroll_pdf || self.active_view().as_ref() != Some(view) {
            return;
        }
        let state = &view.read(cx).state;
        let line = state.buffer().offset_to_line(state.selections().primary().head);
        if self.follow_line == Some(line) {
            return;
        }
        self.follow_line = Some(line);
        self.follow_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(FOLLOW_DEBOUNCE).await;
            this.update(cx, |this, cx| this.follow_cursor(cx)).ok();
        }));
    }

    fn follow_cursor(&mut self, cx: &mut Context<Self>) {
        if !self.settings.auto_scroll_pdf || self.synctex_unavailable_reason().is_some() {
            return;
        }
        let Some(view) = self.active_view() else {
            return;
        };
        let view = view.read(cx);
        let line = view.state.buffer().offset_to_line(view.state.selections().primary().head);
        let location = view.path.as_ref().and_then(|path| self.synctex.as_ref()?.forward(path, line + 1));
        if let Some(location) = location {
            self.pdf.update(cx, |pdf, cx| pdf.follow(location, cx));
        }
    }

    fn toggle_auto_scroll(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.update_settings(|settings| settings.auto_scroll_pdf = !settings.auto_scroll_pdf, window, cx);
        if self.settings.auto_scroll_pdf {
            match self.synctex_unavailable_reason() {
                Some(reason) => self.set_status(reason, cx),
                None => self.follow_cursor(cx),
            }
        }
    }

    fn forward_from_cursor(&mut self, cx: &mut Context<Self>) {
        let Some(view) = self.active_view() else {
            return;
        };
        let state = &view.read(cx).state;
        let line = state.buffer().offset_to_line(state.selections().primary().head);
        self.forward_search(line, true, cx);
    }

    fn backward_search(&mut self, page: usize, x: f32, y: f32, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(reason) = self.synctex_unavailable_reason() {
            self.set_status(reason, cx);
            return;
        }
        let Some(location) = self.synctex.as_ref().and_then(|synctex| synctex.backward(page, x, y)) else {
            self.set_status("This position has no source line.", cx);
            return;
        };
        let line = location.line.saturating_sub(1);
        self.open_file(location.file, window, cx);
        // The PDF is already there, following this jump would pull it away from the click.
        self.follow_line = Some(line);
        self.follow_task = None;
        if let Some(view) = self.active_view() {
            view.update(cx, |view, cx| view.go_to_line(line, 0, window, cx));
        }
    }

    fn go_to_diagnostic(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(diagnostic) = self.diagnostics.get(index).cloned() else {
            return;
        };
        let (Some(file), Some(line)) = (diagnostic.file, diagnostic.line) else {
            return;
        };
        self.open_file(file, window, cx);
        if let Some(view) = self.active_view() {
            let line = line.saturating_sub(1);
            let column = byte_column(&view.read(cx).state.buffer().line_text(line), diagnostic.column.unwrap_or(1));
            view.update(cx, |view, cx| view.go_to_line(line, column, window, cx));
        }
    }

    // Settings and dialogs

    fn update_settings(&mut self, update: impl FnOnce(&mut Settings), window: &mut Window, cx: &mut Context<Self>) {
        update(&mut self.settings);
        if let Some(path) = &self.settings_path
            && let Err(error) = self.settings.save(path)
        {
            self.set_status(format!("Cannot save settings: {error}"), cx);
        }
        let settings = self.settings;
        for document in &self.documents {
            document.view.update(cx, |view, cx| view.set_vim_mode(settings.vim_mode, settings.smooth_cursor, cx));
        }
        if !settings.vim_mode && self.tree_focus.is_focused(window) {
            self.focus_active(window, cx);
        }
        cx.notify();
    }

    fn open_dialog(&mut self, dialog: Dialog, window: &mut Window, cx: &mut Context<Self>) {
        self.open_menu = None;
        self.dialog = Some(dialog);
        window.focus(&self.dialog_focus, cx);
        cx.notify();
    }

    fn close_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.dialog = None;
        self.focus_active(window, cx);
        cx.notify();
    }

    fn focus_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(view) = self.active_view() {
            window.focus(&view.read(cx).focus_handle(cx), cx);
        } else if self.settings.vim_mode {
            window.focus(&self.tree_focus, cx);
        }
    }

    // Vim keys

    /// Runs before the key bindings. An open picker takes its keys in any mode. Apart
    /// from that, nothing changes with vim mode off.
    fn intercept_key(&mut self, event: &KeystrokeEvent, window: &mut Window, cx: &mut Context<Self>) {
        if event.keystroke.modifiers.platform {
            return;
        }
        if self.picker.is_some() {
            cx.stop_propagation();
            self.picker_key(&event.keystroke, window, cx);
            return;
        }
        if !self.settings.vim_mode {
            return;
        }
        // An open menu or dialog holds the focus and gets `Esc` itself.
        if self.open_menu.is_some() || self.dialog.is_some() {
            return;
        }
        // These keep their own key bindings, also with no document open.
        if self.pdf.read(cx).is_focused(window) || self.image_preview.is_some() {
            return;
        }
        // The commit message is a text field with its own keys.
        if self.git_panel.read(cx).contains_focus(window, cx)
            || self.addon_panels.iter().any(|entry| entry.panel.read(cx).contains_focus(window, cx))
        {
            return;
        }
        let view = self.active_view();
        if self.tree_focus.is_focused(window) || view.is_none() {
            cx.stop_propagation();
            self.tree_key(&event.keystroke, window, cx);
            return;
        }
        let Some(view) = view else {
            return;
        };
        if !view.read(cx).focus_handle(cx).is_focused(window) {
            return;
        }
        let takes_keys = view.read(cx).vim_takes_keys();
        let Some(key) = translate_key(&event.keystroke, takes_keys) else {
            if takes_keys {
                cx.stop_propagation();
            }
            return;
        };
        if !takes_keys && key != Key::Esc {
            view.update(cx, |view, _| view.vim_record_insert(key));
            return;
        }
        // `Esc` belongs to the input method while it composes.
        if !takes_keys && view.read(cx).is_composing() {
            return;
        }
        cx.stop_propagation();
        let wants_clipboard = matches!(key, Key::Char('p' | 'P' | '.'));
        let clipboard = wants_clipboard.then(|| cx.read_from_clipboard().and_then(|item| item.text())).flatten();
        let effects = view.update(cx, |view, cx| view.vim_key(key, clipboard, cx));
        for effect in effects {
            self.apply_effect(effect, window, cx);
        }
        cx.notify();
    }

    fn apply_effect(&mut self, effect: Effect, window: &mut Window, cx: &mut Context<Self>) {
        match effect {
            // The editor handles these two itself.
            Effect::Yank(_) | Effect::Scroll(_) => {}
            Effect::Save => self.save_active(cx),
            Effect::Close | Effect::SaveAndClose => {
                if let Some(index) = self.active {
                    self.close_document(index, window, cx);
                }
            }
            Effect::CloseWithoutSaving => {
                if let Some(index) = self.active {
                    self.remove_document(index, window, cx);
                }
            }
            Effect::FocusTree => self.focus_tree(window, cx),
            Effect::ToggleTree => {
                if self.show_file_panel {
                    self.show_file_panel = false;
                    self.focus_active(window, cx);
                } else {
                    self.focus_tree(window, cx);
                }
            }
            Effect::RevealInTree => {
                if let Some(path) = self.active_view().and_then(|view| view.read(cx).path.clone()) {
                    for ancestor in path.ancestors().skip(1) {
                        self.collapsed.remove(ancestor);
                    }
                    self.refresh_from_disk(cx);
                }
                self.focus_tree(window, cx);
            }
            Effect::TogglePreview => self.show_preview = !self.show_preview,
            Effect::FindFiles => self.open_picker(PickerKind::Files, window, cx),
            Effect::FindText => self.open_picker(PickerKind::Text, window, cx),
            Effect::ShowError => {
                if self.problems.is_empty() {
                    self.set_status("No errors.", cx);
                } else {
                    self.show_preview = true;
                    self.show_logs = false;
                    self.error_card_dismissed = false;
                }
            }
            Effect::NextError => self.step_error(true, window, cx),
            Effect::PreviousError => self.step_error(false, window, cx),
            Effect::PreviousTab => self.step_tab(false, window, cx),
            Effect::NextTab => self.step_tab(true, window, cx),
        }
    }

    fn step_error(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let located: Vec<usize> = self
            .diagnostics
            .iter()
            .enumerate()
            .filter(|(_, diagnostic)| diagnostic.file.is_some() && diagnostic.line.is_some())
            .map(|(index, _)| index)
            .collect();
        if located.is_empty() {
            self.set_status("No errors.", cx);
            return;
        }
        let position = self.error_cursor.and_then(|cursor| located.iter().position(|index| *index == cursor));
        let next = match position {
            Some(position) if forward => (position + 1) % located.len(),
            Some(position) => (position + located.len() - 1) % located.len(),
            None if forward => 0,
            None => located.len() - 1,
        };
        self.error_cursor = Some(located[next]);
        self.go_to_diagnostic(located[next], window, cx);
    }

    fn step_tab(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let count = self.documents.len();
        if let (Some(active), true) = (self.active, count > 0) {
            let next = if forward { (active + 1) % count } else { (active + count - 1) % count };
            self.activate(next, window, cx);
        }
    }

    // File tree keys

    fn focus_tree(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.show_file_panel = true;
        self.left_panel = LeftPanel::Files;
        let active = self.active_view().and_then(|view| view.read(cx).path.clone());
        if let Some(index) = active.and_then(|path| self.entries.iter().position(|entry| entry.path == path)) {
            self.tree_cursor = index;
        }
        self.tree_scroll.scroll_to_item(self.tree_cursor);
        self.tree_g_pending = false;
        self.tree_leader = None;
        window.focus(&self.tree_focus, cx);
        cx.notify();
    }

    fn move_tree_cursor(&mut self, index: usize, cx: &mut Context<Self>) {
        self.tree_cursor = index.min(self.entries.len().saturating_sub(1));
        self.tree_scroll.scroll_to_item(self.tree_cursor);
        cx.notify();
    }

    fn tree_key(&mut self, keystroke: &Keystroke, window: &mut Window, cx: &mut Context<Self>) {
        let Some(key) = translate_key(keystroke, true) else {
            return;
        };
        if let Some(mut prefix) = self.tree_leader.take() {
            if let Key::Char(ch) = key {
                prefix.push(ch);
                match vim::leader_lookup(&prefix) {
                    Leader::Effect(effect) => self.apply_effect(effect, window, cx),
                    Leader::Prefix => self.tree_leader = Some(prefix),
                    Leader::Unknown => {}
                }
            }
            cx.notify();
            return;
        }
        let g_pending = std::mem::take(&mut self.tree_g_pending);
        match key {
            Key::Char('j') => self.move_tree_cursor(self.tree_cursor + 1, cx),
            Key::Char('k') => self.move_tree_cursor(self.tree_cursor.saturating_sub(1), cx),
            Key::Char('g') if g_pending => self.move_tree_cursor(0, cx),
            Key::Char('g') => self.tree_g_pending = true,
            Key::Char('G') => self.move_tree_cursor(usize::MAX, cx),
            Key::Char('l') | Key::Enter => self.open_tree_entry(window, cx),
            Key::Char('h') => self.close_tree_folder(cx),
            Key::Char(' ') => self.tree_leader = Some(String::new()),
            Key::Tab => self.focus_active(window, cx),
            // `q` does nothing in the nvim-tree config either.
            _ => {}
        }
        cx.notify();
    }

    fn open_tree_entry(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(entry) = self.entries.get(self.tree_cursor).cloned() else {
            return;
        };
        if !entry.is_dir {
            self.open_file(entry.path, window, cx);
        } else {
            if !self.collapsed.remove(&entry.path) {
                self.collapsed.insert(entry.path);
            }
            self.refresh_from_disk(cx);
        }
    }

    /// Closes the folder under the cursor, or else the one that contains it.
    fn close_tree_folder(&mut self, cx: &mut Context<Self>) {
        let Some(entry) = self.entries.get(self.tree_cursor).cloned() else {
            return;
        };
        let folder = if entry.is_dir && !self.collapsed.contains(&entry.path) {
            entry.path
        } else {
            match entry.path.parent() {
                Some(parent) if Some(parent) != self.root.as_deref() => parent.to_path_buf(),
                _ => return,
            }
        };
        if let Some(index) = self.entries.iter().position(|entry| entry.path == folder) {
            self.tree_cursor = index;
        }
        self.collapsed.insert(folder);
        self.refresh_from_disk(cx);
        self.tree_scroll.scroll_to_item(self.tree_cursor);
    }

    // Picker

    fn open_picker(&mut self, kind: PickerKind, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.root.clone() else {
            self.set_status("Open a folder first.", cx);
            return;
        };
        let scan_root = root.clone();
        let language = self.active_language(cx);
        let repository = self.git_panel.read(cx).repository();
        if kind == PickerKind::Branches && repository.is_none() {
            return;
        }
        self.picker = Some(Picker {
            kind,
            query: String::new(),
            items: Vec::new(),
            selected: 0,
            root,
            files: Arc::new(Vec::new()),
            entries: Arc::new(Vec::new()),
            cancel: Arc::new(AtomicBool::new(false)),
            task: None,
        });
        window.focus(&self.picker_focus, cx);
        self.picker_files_task = Some(cx.spawn(async move |this, cx| {
            let (files, entries) = cx
                .background_spawn(async move {
                    if let Some(repository) = repository.filter(|_| kind == PickerKind::Branches) {
                        return (Vec::new(), picker::branch_items(repository.branches().unwrap_or_default()));
                    }
                    let files = file_tree::scan(&scan_root, &HashSet::new())
                        .into_iter()
                        .filter(|entry| !entry.is_dir)
                        .map(|entry| entry.path)
                        .filter(|path| match kind {
                            PickerKind::Files | PickerKind::Text | PickerKind::Branches => true,
                            PickerKind::Figures => language.is_some_and(|language| snippets::is_figure_file(language, path)),
                            PickerKind::Citations => path.extension().is_some_and(|extension| extension == "bib"),
                        })
                        .collect::<Vec<_>>();
                    let entries = if kind == PickerKind::Citations { picker::read_citations(&files) } else { Vec::new() };
                    (files, entries)
                })
                .await;
            this.update(cx, |this, cx| {
                if let Some(picker) = &mut this.picker {
                    picker.files = Arc::new(files);
                    picker.entries = Arc::new(entries);
                }
                this.refresh_picker(cx);
            })
            .ok();
        }));
        cx.notify();
    }

    fn close_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(picker) = self.picker.take() {
            picker.cancel.store(true, Ordering::Relaxed);
        }
        self.picker_files_task = None;
        self.focus_active(window, cx);
        cx.notify();
    }

    fn refresh_picker(&mut self, cx: &mut Context<Self>) {
        let Some(picker) = &mut self.picker else {
            return;
        };
        match picker.kind {
            PickerKind::Files | PickerKind::Figures => {
                picker.items = picker::match_files(&picker.files, &picker.root, &picker.query);
                picker.selected = 0;
            }
            PickerKind::Citations => {
                picker.items = picker::match_entries(&picker.entries, &picker.query);
                picker.selected = 0;
            }
            PickerKind::Branches => {
                picker.items = picker::match_branches(&picker.entries, &picker.query);
                picker.selected = 0;
            }
            PickerKind::Text => {
                picker.cancel.store(true, Ordering::Relaxed);
                let cancel = Arc::new(AtomicBool::new(false));
                picker.cancel = cancel.clone();
                let (files, root, query) = (picker.files.clone(), picker.root.clone(), picker.query.clone());
                picker.task = Some(cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(PICKER_SEARCH_DELAY).await;
                    let stop = cancel.clone();
                    let items = cx
                        .background_spawn(async move { picker::search_text(&files, &root, &query, &stop) })
                        .await;
                    if cancel.load(Ordering::Relaxed) {
                        return;
                    }
                    this.update(cx, |this, cx| {
                        if let Some(picker) = &mut this.picker {
                            picker.items = items;
                            picker.selected = 0;
                        }
                        cx.notify();
                    })
                    .ok();
                }));
            }
        }
        cx.notify();
    }

    fn move_picker(&mut self, forward: bool, cx: &mut Context<Self>) {
        let Some(picker) = &mut self.picker else {
            return;
        };
        let count = picker.items.len();
        if count > 0 {
            picker.selected = if forward { (picker.selected + 1) % count } else { (picker.selected + count - 1) % count };
            self.picker_scroll.scroll_to_item(picker.selected);
        }
        cx.notify();
    }

    fn accept_picker(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some((kind, item)) = self.picker.as_ref().and_then(|picker| Some((picker.kind, picker.items.get(index).cloned()?))) else {
            return;
        };
        self.close_picker(window, cx);
        match kind {
            PickerKind::Files | PickerKind::Text => {}
            PickerKind::Figures => return self.insert_figure(&item.path, cx),
            PickerKind::Branches => {
                if self.save_all(cx) {
                    let create = item.detail.as_deref() == Some(picker::CREATE_BRANCH);
                    self.git_panel.update(cx, |panel, cx| {
                        panel.run(cx, move |repository| {
                            if create { repository.create_branch(&item.label) } else { repository.switch(&item.label) }
                        });
                    });
                }
                return;
            }
            PickerKind::Citations => {
                if let Some(language) = self.active_language(cx) {
                    self.insert_snippet(&snippets::citation(language, &item.label), false, cx);
                }
                return;
            }
        }
        self.open_file(item.path, window, cx);
        if let (Some((line, column)), Some(view)) = (item.position, self.active_view()) {
            view.update(cx, |view, cx| view.go_to_line(line, column, window, cx));
        }
    }

    fn insert_figure(&mut self, path: &Path, cx: &mut Context<Self>) {
        let Some(language) = self.active_language(cx) else {
            return;
        };
        let active = self.active_view().and_then(|view| view.read(cx).path.clone());
        // Typst looks next to the current file, LaTeX next to the main file.
        let main = match language {
            Language::Typst => active,
            _ => self.root.as_deref().and_then(|root| find_main(root, &self.config, active.as_deref())),
        };
        let directory = main.as_deref().and_then(Path::parent).or(self.root.as_deref());
        let relative = directory.map_or_else(|| path.display().to_string(), |directory| relative_path(directory, path));
        self.insert_snippet(&snippets::figure(language, &relative), false, cx);
    }

    fn insert_snippet(&mut self, template: &str, math: bool, cx: &mut Context<Self>) {
        self.with_active(cx, |view, cx| view.insert_snippet(template, math, cx));
        cx.notify();
    }

    /// The language of the document that toolbar snippets go into.
    fn active_language(&self, cx: &App) -> Option<Language> {
        if self.image_preview.is_some() {
            return None;
        }
        self.active_view().map(|view| view.read(cx).state.language())
    }

    fn picker_key(&mut self, keystroke: &Keystroke, window: &mut Window, cx: &mut Context<Self>) {
        match keystroke.key.as_str() {
            "escape" => return self.close_picker(window, cx),
            "enter" => {
                let selected = self.picker.as_ref().map_or(0, |picker| picker.selected);
                return self.accept_picker(selected, window, cx);
            }
            "down" => return self.move_picker(true, cx),
            "up" => return self.move_picker(false, cx),
            "n" if keystroke.modifiers.control => return self.move_picker(true, cx),
            "p" if keystroke.modifiers.control => return self.move_picker(false, cx),
            "backspace" => {
                if let Some(picker) = &mut self.picker {
                    picker.query.pop();
                }
                return self.refresh_picker(cx);
            }
            _ => {}
        }
        let Some(text) = keystroke.key_char.as_deref().filter(|_| !keystroke.modifiers.control) else {
            return;
        };
        if text.chars().all(|ch| !ch.is_control())
            && let Some(picker) = &mut self.picker
        {
            picker.query.push_str(text);
            self.refresh_picker(cx);
        }
    }

    fn toggle_logs(&mut self) {
        self.show_logs = !self.show_logs;
    }

    fn toggle_menu(&mut self, menu: Menu, position: Point<Pixels>, bottom: f32, window: &mut Window, cx: &mut Context<Self>) {
        if self.open_menu == Some(menu) {
            self.close_menu(window, cx);
        } else {
            self.open_menu = Some(menu);
            self.symbol_hover = None;
            self.table_hover = (1, 1);
            self.menu_origin = point(position.x - px(12.), px(bottom));
            window.focus(&self.menu_focus, cx);
            cx.notify();
        }
    }

    fn close_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_menu = None;
        if let Some(view) = self.active_view() {
            window.focus(&view.read(cx).focus_handle(cx), cx);
        }
        cx.notify();
    }

    fn error_count(&self) -> usize {
        self.diagnostics.iter().filter(|diagnostic| diagnostic.severity == Severity::Error).count()
    }

    fn set_engine(&mut self, engine: Engine, cx: &mut Context<Self>) {
        self.config.engine = Some(engine);
        self.recompile(cx);
    }

    fn refresh_outline(&mut self, cx: &mut Context<Self>) {
        let Some(view) = self.active_view() else {
            self.outline.clear();
            return;
        };
        let state = &view.read(cx).state;
        let key = (self.active.unwrap_or(usize::MAX), state.syntax().applied_results());
        if key != self.outline_version {
            let mut items = outline::outline(state.syntax().headings(), state.language());
            if key.0 == self.outline_version.0 {
                outline::carry_collapsed(&self.outline, &mut items);
            }
            self.outline = items;
            self.outline_version = key;
        }
    }

    fn on_root_mouse_move(&mut self, event: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(divider) = self.dragging else {
            return;
        };
        if !event.dragging() {
            self.dragging = None;
            return;
        }
        let viewport = window.viewport_size();
        match divider {
            Divider::FilePanel => {
                let max = viewport.width - px(RAIL_WIDTH + MIN_PANE_WIDTH * 2.);
                self.file_panel_width = (event.position.x - px(RAIL_WIDTH)).clamp(px(120.), max);
            }
            Divider::Preview => {
                let max = viewport.width - px(RAIL_WIDTH + MIN_PANE_WIDTH);
                self.preview_width =
                    (viewport.width - event.position.x - px(DIVIDER_WIDTH)).clamp(px(MIN_PANE_WIDTH), max);
            }
            Divider::Outline => {
                let max = viewport.height - px(TOP_BAR_HEIGHT + 80.);
                self.outline_height = (viewport.height - event.position.y).clamp(px(60.), max);
            }
        }
        cx.notify();
    }

    fn menu_entries(&self, menu: Menu) -> Vec<MenuEntry> {
        let check = |on: bool, label: &str| format!("{} {label}", if on { "✓" } else { "  " });
        match menu {
            Menu::File => vec![
                entry("Open folder…", "⌘O", |this, window, cx| this.prompt_open_folder(window, cx)),
                entry("Save", "⌘S", |this, _, cx| this.save_active(cx)),
                entry("Recompile", "⌘↩", |this, _, cx| this.recompile(cx)),
            ],
            Menu::Edit => vec![
                entry("Undo", "⌘Z", |this, _, cx| this.with_active(cx, |view, cx| view.undo_edit(cx))),
                entry("Redo", "⇧⌘Z", |this, _, cx| this.with_active(cx, |view, cx| view.redo_edit(cx))),
            ],
            Menu::Insert => vec![
                entry("Inline math", "", |this, _, cx| {
                    this.with_active(cx, |view, cx| view.format(FormatKind::Math, cx))
                }),
                entry("Link", "", |this, _, cx| {
                    this.with_active(cx, |view, cx| view.format(FormatKind::Link, cx))
                }),
            ],
            Menu::View | Menu::Layout => vec![
                entry("Toggle file panel", "", |this, _, cx| {
                    this.show_file_panel = !this.show_file_panel;
                    cx.notify();
                }),
                entry("Toggle PDF preview", "", |this, _, cx| {
                    this.show_preview = !this.show_preview;
                    cx.notify();
                }),
                entry("Toggle logs", "", |this, _, cx| {
                    this.toggle_logs();
                    cx.notify();
                }),
            ],
            Menu::Format => vec![
                entry("Bold", "⌘B", |this, _, cx| {
                    this.with_active(cx, |view, cx| view.format(FormatKind::Bold, cx))
                }),
                entry("Italic", "⌘I", |this, _, cx| {
                    this.with_active(cx, |view, cx| view.format(FormatKind::Italic, cx))
                }),
            ],
            Menu::Symbols | Menu::Table => Vec::new(),
            Menu::Help => vec![entry("Keyboard shortcuts", "", |this, window, cx| {
                this.open_dialog(Dialog::Shortcuts, window, cx);
            })],
            Menu::Compile => {
                let trigger = self.config.trigger;
                let engine = self.config.engine;
                let debounce = self.config.debounce_ms;
                vec![
                    entry("Recompile", "⌘↩", |this, _, cx| this.recompile(cx)),
                    entry("Recompile from scratch", "", |this, _, cx| this.recompile_from_scratch(cx)),
                    entry("Stop build", "", |this, _, _| this.build.cancel()),
                    entry(check(trigger == Trigger::Save, "Compile on save"), "", |this, _, cx| {
                        this.config.trigger = Trigger::Save;
                        cx.notify();
                    }),
                    entry(
                        check(trigger == Trigger::Type, &format!("Compile on type, autosaves after {debounce} ms")),
                        "",
                        |this, _, cx| {
                            this.config.trigger = Trigger::Type;
                            cx.notify();
                        },
                    ),
                    entry(check(self.sync_on_click, "Sync PDF on click"), "", |this, _, cx| {
                        this.sync_on_click = !this.sync_on_click;
                        cx.notify();
                    }),
                    entry(check(engine == Some(Engine::PdfLatex), "LaTeX: pdfLaTeX"), "", |this, _, cx| {
                        this.set_engine(Engine::PdfLatex, cx);
                    }),
                    entry(check(engine == Some(Engine::LuaLatex), "LaTeX: LuaLaTeX"), "", |this, _, cx| {
                        this.set_engine(Engine::LuaLatex, cx);
                    }),
                    entry(check(engine == Some(Engine::Tectonic), "LaTeX: Tectonic"), "", |this, _, cx| {
                        this.set_engine(Engine::Tectonic, cx);
                    }),
                    entry(check(engine.is_none(), "Pick compiler from file type"), "", |this, _, cx| {
                        this.config.engine = None;
                        this.recompile(cx);
                    }),
                ]
            }
        }
    }

    // Rendering helpers

    fn icon(name: &'static str, size: f32, color: u32) -> impl IntoElement {
        svg()
            .path(format!("icons/{name}.svg"))
            .size(px(size))
            .flex_none()
            .text_color(theme::color(color))
    }

    fn icon_button(
        &self,
        id: &'static str,
        name: &'static str,
        cx: &mut Context<Self>,
        handler: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> impl IntoElement {
        div()
            .id(id)
            .flex()
            .items_center()
            .justify_center()
            .size(px(26.))
            .rounded(px(4.))
            .cursor_pointer()
            .hover(|style| style.bg(theme::color(theme::DIVIDER)))
            .on_click(cx.listener(move |this, _, window, cx| handler(this, window, cx)))
            .child(Self::icon(name, 16., theme::UI_TEXT))
    }

    fn menu_button(&self, menu: Menu, label: &'static str, cx: &mut Context<Self>) -> impl IntoElement {
        let open = self.open_menu == Some(menu);
        div()
            .id(label)
            .px(px(9.))
            .h(px(28.))
            .flex()
            .items_center()
            .rounded(px(4.))
            .cursor_pointer()
            .when(open, |style| style.bg(theme::color(theme::PANEL)))
            .hover(|style| style.bg(theme::color(theme::PANEL)))
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                this.toggle_menu(menu, event.position(), TOP_BAR_HEIGHT, window, cx);
            }))
            .child(label)
            .when(open, |element| element.child(self.render_menu(menu, cx)))
    }

    fn render_menu(&self, menu: Menu, cx: &mut Context<Self>) -> impl IntoElement {
        let items = self.menu_entries(menu);
        deferred(
            anchored()
                .anchor(Anchor::TopLeft)
                .position(self.menu_origin)
                .child(
                    div()
                        .id("menu")
                        .occlude()
                        .min_w(px(220.))
                        .py(px(4.))
                        .rounded(px(4.))
                        .bg(theme::color(theme::PANEL))
                        .border_1()
                        .border_color(theme::color(theme::DIVIDER))
                        .shadow_lg()
                        .children(items.into_iter().enumerate().map(|(index, item)| {
                            let handler = item.handler.clone();
                            div()
                                .id(index)
                                .flex()
                                .justify_between()
                                .gap(px(24.))
                                .px(px(12.))
                                .py(px(5.))
                                .cursor_pointer()
                                .hover(|style| style.bg(theme::color(theme::GREEN_DARK)))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.close_menu(window, cx);
                                    handler(this, window, cx);
                                    cx.notify();
                                }))
                                .child(item.label.clone())
                                .child(
                                    div()
                                        .text_color(theme::color(theme::UI_MUTED))
                                        .child(item.shortcut),
                                )
                        })),
                ),
        )
    }

    fn render_top_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let title = self
            .root
            .as_deref()
            .and_then(|root| root.file_name())
            .map_or_else(|| "No project".to_string(), |name| name.to_string_lossy().into_owned());
        div()
            .id("top-bar")
            .relative()
            .flex()
            .flex_none()
            .items_center()
            .justify_between()
            .h(px(TOP_BAR_HEIGHT))
            .pl(px(TRAFFIC_LIGHT_SPACE))
            .pr(px(10.))
            .border_b_1()
            .border_color(theme::color(theme::PANEL))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(2.))
                    .child(self.menu_button(Menu::File, "File", cx))
                    .child(self.menu_button(Menu::Edit, "Edit", cx))
                    .child(self.menu_button(Menu::Insert, "Insert", cx))
                    .child(self.menu_button(Menu::View, "View", cx))
                    .child(self.menu_button(Menu::Format, "Format", cx))
                    .child(self.menu_button(Menu::Help, "Help", cx)),
            )
            .child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .flex()
                    .justify_center()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(title),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .child(self.menu_button(Menu::Layout, "Layout", cx)),
            )
    }

    fn render_rail(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let rail_button = |id: &'static str, icon: &'static str, panel: LeftPanel, cx: &mut Context<Self>| {
            let on = self.show_file_panel && self.left_panel == panel;
            div()
                .id(id)
                .flex()
                .items_center()
                .justify_center()
                .size(px(32.))
                .rounded(px(4.))
                .cursor_pointer()
                .when(on, |style| style.bg(theme::color(theme::GREEN_DARK)))
                .hover(|style| style.bg(theme::color(theme::DIVIDER)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.show_file_panel = !(this.show_file_panel && this.left_panel == panel);
                    this.left_panel = panel;
                    cx.notify();
                }))
                .child(Self::icon(icon, 18., theme::UI_TEXT))
        };
        div()
            .flex()
            .flex_none()
            .flex_col()
            .justify_between()
            .items_center()
            .w(px(RAIL_WIDTH))
            .h_full()
            .py(px(4.))
            .border_r_1()
            .border_color(theme::color(theme::PANEL))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(6.))
                    .child(rail_button("rail-files", "file", LeftPanel::Files, cx))
                    .child(rail_button("rail-git", "git-branch", LeftPanel::Git, cx))
                    .child(rail_button("rail-addons", "addon", LeftPanel::Addons, cx))
                    .children(self.addon_panels.iter().enumerate().map(|(index, entry)| {
                        let panel = LeftPanel::Addon(index);
                        let on = self.show_file_panel && self.left_panel == panel;
                        let icon = entry.panel.read(cx).add_on.icon();
                        let path: SharedString = icon.map_or_else(|| "icons/puzzle.svg".to_string(), |icon| icon.display().to_string()).into();
                        div()
                            .id(("rail-addon", index))
                            .flex()
                            .items_center()
                            .justify_center()
                            .size(px(32.))
                            .rounded(px(4.))
                            .cursor_pointer()
                            .when(on, |style| style.bg(theme::color(theme::GREEN_DARK)))
                            .hover(|style| style.bg(theme::color(theme::DIVIDER)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.show_file_panel = !(this.show_file_panel && this.left_panel == panel);
                                this.left_panel = panel;
                                cx.notify();
                            }))
                            .child(svg().path(path).size(px(18.)).flex_none().text_color(theme::color(theme::UI_TEXT)))
                    })),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(6.))
                    .child(self.icon_button("rail-logs", "log", cx, |this, _, cx| {
                        this.toggle_logs();
                        cx.notify();
                    }))
                    .child(self.icon_button("rail-help", "help", cx, |this, window, cx| {
                        this.open_dialog(Dialog::Shortcuts, window, cx);
                    }))
                    .child(self.icon_button("rail-settings", "settings", cx, |this, window, cx| {
                        this.open_dialog(Dialog::Settings(SettingsPage::Editor), window, cx);
                    })),
            )
    }

    fn file_icon(entry: &TreeEntry) -> &'static str {
        if entry.is_dir {
            return "folder";
        }
        match entry.path.extension().and_then(|extension| extension.to_str()) {
            Some("png" | "jpg" | "jpeg" | "gif" | "svg" | "pdf" | "eps") => "image",
            Some("bib") => "book",
            _ => "file",
        }
    }

    fn render_file_tree(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tree_cursor = self.tree_focus.is_focused(window).then_some(self.tree_cursor);
        let active_path = self
            .active_view()
            .and_then(|view| view.read(cx).path.clone());
        div()
            .id("file-tree")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_focus(&self.tree_focus)
            .track_scroll(&self.tree_scroll)
            .px(px(4.))
            .children(self.entries.iter().enumerate().map(|(index, entry)| {
                let selected = active_path.as_deref() == Some(entry.path.as_path());
                let path = entry.path.clone();
                let is_dir = entry.is_dir;
                let collapsed = self.collapsed.contains(&entry.path);
                div()
                    .id(index)
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .h(px(32.))
                    .pl(px(8. + entry.depth as f32 * 14.))
                    .rounded(px(4.))
                    .cursor_pointer()
                    .when(selected, |style| {
                        style
                            .bg(theme::color(theme::GREEN_DARK))
                            .font_weight(FontWeight::SEMIBOLD)
                    })
                    .when(!selected, |style| {
                        style.hover(|style| style.bg(theme::color(theme::PANEL)))
                    })
                    .when(tree_cursor == Some(index), |style| {
                        style.border_1().border_color(theme::color(theme::CARET))
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if is_dir {
                            if !this.collapsed.remove(&path) {
                                this.collapsed.insert(path.clone());
                            }
                            this.refresh_from_disk(cx);
                        } else {
                            this.open_file(path.clone(), window, cx);
                        }
                    }))
                    .when(is_dir, |element| {
                        element.child(Self::icon(
                            if collapsed { "chevron-right" } else { "chevron-down" },
                            12.,
                            theme::UI_TEXT,
                        ))
                    })
                    .child(Self::icon(Self::file_icon(entry), 16., theme::UI_TEXT))
                    .child(div().truncate().child(entry.name.clone()))
            }))
    }

    fn render_outline(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("outline")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .children(outline::visible(&self.outline).map(|(index, item)| {
                let line = item.line;
                div()
                    .id(index)
                    .relative()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .h(px(32.))
                    .pl(px(OUTLINE_INSET + item.depth as f32 * OUTLINE_INDENT))
                    .pr(px(8.))
                    .cursor_pointer()
                    .hover(|style| style.bg(theme::color(theme::PANEL)))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if let Some(view) = this.active_view() {
                            view.update(cx, |view, cx| view.go_to_line(line, 0, window, cx));
                        }
                    }))
                    // Rows touch, so the guides of neighbours join into one line.
                    .children((0..item.depth).map(|depth| {
                        div()
                            .absolute()
                            .top_0()
                            .bottom_0()
                            .left(px(OUTLINE_INSET + OUTLINE_CHEVRON / 2. + depth as f32 * OUTLINE_INDENT))
                            .w(px(1.))
                            .bg(theme::color(theme::DIVIDER))
                    }))
                    .child(
                        div()
                            .id(("outline-toggle", index))
                            .flex()
                            .flex_none()
                            .items_center()
                            .justify_center()
                            .size(px(OUTLINE_CHEVRON))
                            .when(item.has_children, |toggle| {
                                toggle
                                    .child(Self::icon(
                                        if item.collapsed { "chevron-right" } else { "chevron-down" },
                                        12.,
                                        theme::UI_TEXT,
                                    ))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.outline[index].collapsed ^= true;
                                        cx.notify();
                                    }))
                            }),
                    )
                    .child(div().truncate().child(item.title.clone()))
            }))
    }

    fn panel_header(
        &self,
        id: &'static str,
        title: &'static str,
        open: bool,
        cx: &mut Context<Self>,
        toggle: impl Fn(&mut Self, &mut Context<Self>) + 'static,
    ) -> gpui::Stateful<gpui::Div> {
        div()
            .id(id)
            .flex()
            .flex_none()
            .items_center()
            .gap(px(6.))
            .h(px(32.))
            .px(px(10.))
            .font_weight(FontWeight::SEMIBOLD)
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| {
                toggle(this, cx);
                cx.notify();
            }))
            .child(Self::icon(
                if open { "chevron-down" } else { "chevron-right" },
                12.,
                theme::UI_TEXT,
            ))
            .child(title)
    }

    fn render_file_panel(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let show_outline = self.show_outline;
        div()
            .flex()
            .flex_none()
            .flex_col()
            .w(self.file_panel_width)
            .h_full()
            .child(self.panel_header("tree-header", "File tree", true, cx, |_, _| {}))
            .child(self.render_file_tree(window, cx))
            .child(self.divider_horizontal(cx))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_none()
                    .when(show_outline, |style| style.h(self.outline_height))
                    .child(self.panel_header(
                        "outline-header",
                        "File outline",
                        show_outline,
                        cx,
                        |this, _| this.show_outline = !this.show_outline,
                    ))
                    .when(show_outline, |element| element.child(self.render_outline(cx))),
            )
    }

    fn divider_horizontal(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("outline-divider")
            .flex_none()
            .h(px(6.))
            .bg(theme::color(theme::DIVIDER))
            .cursor(CursorStyle::ResizeUpDown)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.dragging = Some(Divider::Outline);
                    cx.notify();
                }),
            )
    }

    fn divider_vertical(&self, id: &'static str, divider: Divider, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id(id)
            .flex_none()
            .w(px(DIVIDER_WIDTH))
            .h_full()
            .bg(theme::color(theme::DIVIDER))
            .cursor(CursorStyle::ResizeLeftRight)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    this.dragging = Some(divider);
                    cx.notify();
                }),
            )
    }

    fn render_tabs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_none()
            .items_end()
            .h(px(TAB_BAR_HEIGHT))
            .bg(theme::color(theme::PANEL))
            .pl(px(4.))
            .gap(px(2.))
            .children(self.documents.iter().enumerate().map(|(index, document)| {
                let view = document.view.read(cx);
                let active = self.active == Some(index);
                let dirty = view.state.is_dirty();
                div()
                    .id(("tab", index))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .h(px(32.))
                    .px(px(8.))
                    .rounded_t(px(4.))
                    .cursor_pointer()
                    .when(active, |style| style.bg(theme::color(theme::BG)))
                    .on_click(cx.listener(move |this, _, window, cx| this.activate(index, window, cx)))
                    .child(Self::icon("file", 14., theme::UI_TEXT))
                    .child(view.title())
                    .when(dirty, |element| {
                        element.child(div().size(px(6.)).rounded_full().bg(theme::color(theme::UI_TEXT)))
                    })
                    .child(
                        div()
                            .id(("tab-close", index))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.close_document(index, window, cx);
                            }))
                            .child(Self::icon("close", 12., theme::UI_MUTED)),
                    )
            }))
    }

    fn format_button(
        &self,
        id: &'static str,
        kind: FormatKind,
        cx: &mut Context<Self>,
        child: impl IntoElement,
    ) -> impl IntoElement {
        div()
            .id(id)
            .flex()
            .items_center()
            .justify_center()
            .size(px(26.))
            .rounded(px(4.))
            .cursor_pointer()
            .hover(|style| style.bg(theme::color(theme::DIVIDER)))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.with_active(cx, |view, cx| view.format(kind, cx));
            }))
            .child(child)
    }

    /// A toolbar button that opens a popup below the toolbar.
    fn popup_button(
        &self,
        id: &'static str,
        icon: &'static str,
        menu: Menu,
        cx: &mut Context<Self>,
        popup: impl FnOnce(&Self, &mut Context<Self>) -> AnyElement,
    ) -> impl IntoElement {
        let open = self.open_menu == Some(menu);
        let element = open.then(|| popup(self, cx));
        div()
            .id(id)
            .flex()
            .items_center()
            .justify_center()
            .size(px(26.))
            .rounded(px(4.))
            .cursor_pointer()
            .when(open, |style| style.bg(theme::color(theme::DIVIDER)))
            .hover(|style| style.bg(theme::color(theme::DIVIDER)))
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                let bottom = TOP_BAR_HEIGHT + TAB_BAR_HEIGHT + TOOLBAR_HEIGHT;
                this.toggle_menu(menu, event.position(), bottom, window, cx);
            }))
            .child(Self::icon(icon, 16., theme::UI_TEXT))
            .children(element)
    }

    fn render_popup(&self, id: &'static str, content: impl IntoElement) -> AnyElement {
        deferred(
            anchored().anchor(Anchor::TopLeft).position(self.menu_origin).child(
                div()
                    .id(id)
                    .occlude()
                    .p(px(8.))
                    .rounded(px(4.))
                    .bg(theme::color(theme::PANEL))
                    .border_1()
                    .border_color(theme::color(theme::DIVIDER))
                    .shadow_lg()
                    .child(content),
            ),
        )
        .into_any_element()
    }

    fn render_symbol_palette(&self, language: Language, cx: &mut Context<Self>) -> AnyElement {
        let group = &snippets::GROUPS[self.symbol_group];
        let tabs = snippets::GROUPS.iter().enumerate().map(|(index, tab)| {
            div()
                .id(("symbol-group", index))
                .px(px(8.))
                .py(px(3.))
                .rounded(px(4.))
                .cursor_pointer()
                .when(index == self.symbol_group, |style| style.bg(theme::color(theme::GREEN_DARK)))
                .when(index != self.symbol_group, |style| style.hover(|style| style.bg(theme::color(theme::DIVIDER))))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.symbol_group = index;
                    this.symbol_hover = None;
                    cx.notify();
                }))
                .child(tab.name)
        });
        let cells = group.symbols.iter().enumerate().map(|(index, symbol)| {
            let template = symbol.template(language);
            div()
                .id(("symbol", index))
                .flex()
                .items_center()
                .justify_center()
                .size(px(SYMBOL_CELL))
                .rounded(px(4.))
                .cursor_pointer()
                .font_family(theme::CODE_FONT)
                .hover(|style| style.bg(theme::color(theme::DIVIDER)))
                .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                    if *hovered {
                        this.symbol_hover = Some(template);
                    } else if this.symbol_hover == Some(template) {
                        this.symbol_hover = None;
                    }
                    cx.notify();
                }))
                // The palette stays open, so that several symbols can follow.
                .on_click(cx.listener(move |this, _, _, cx| this.insert_snippet(template, true, cx)))
                .child(symbol.label)
        });
        let preview = self.symbol_hover.map_or_else(String::new, |template| Snippet::parse(template, "").text.replace('\n', " "));
        self.render_popup(
            "symbol-palette",
            div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .w(px(SYMBOL_CELL * SYMBOL_COLUMNS))
                .child(div().flex().flex_wrap().children(tabs))
                .child(div().flex().flex_wrap().children(cells))
                .child(
                    div()
                        .h(px(18.))
                        .truncate()
                        .font_family(theme::CODE_FONT)
                        .text_size(px(theme::CODE_FONT_SIZE))
                        .text_color(theme::color(theme::UI_MUTED))
                        .child(preview),
                ),
        )
    }

    fn render_table_grid(&self, language: Language, cx: &mut Context<Self>) -> AnyElement {
        let (hover_rows, hover_columns) = self.table_hover;
        let rows = (1..=TABLE_GRID).map(|row| {
            div().flex().gap(px(2.)).children((1..=TABLE_GRID).map(|column| {
                let selected = row <= hover_rows && column <= hover_columns;
                div()
                    .id(("table-cell", (row - 1) * TABLE_GRID + column - 1))
                    .size(px(18.))
                    .rounded(px(2.))
                    .cursor_pointer()
                    .border_1()
                    .border_color(theme::color(theme::DIVIDER))
                    .when(selected, |style| style.bg(theme::color(theme::GREEN_DARK)))
                    .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                        if *hovered && this.table_hover != (row, column) {
                            this.table_hover = (row, column);
                            cx.notify();
                        }
                    }))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.close_menu(window, cx);
                        this.insert_snippet(&snippets::table(language, row, column), false, cx);
                    }))
            }))
        });
        self.render_popup(
            "table-grid",
            div()
                .flex()
                .flex_col()
                .gap(px(2.))
                .children(rows)
                .child(div().pt(px(4.)).text_color(theme::color(theme::UI_MUTED)).child(format!("{hover_rows} × {hover_columns}"))),
        )
    }

    fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let separator = || div().w(px(1.)).h(px(18.)).mx(px(4.)).bg(theme::color(theme::DIVIDER));
        let language = self.active_language(cx);
        let markup = matches!(language, Some(Language::Latex | Language::Typst));
        let formats = matches!(language, Some(Language::Latex | Language::Typst | Language::Markdown));
        div()
            .flex()
            .flex_none()
            .items_center()
            .h(px(TOOLBAR_HEIGHT))
            .px(px(6.))
            .bg(theme::color(theme::PANEL))
            .child(self.icon_button("undo", "undo", cx, |this, _, cx| {
                this.with_active(cx, |view, cx| view.undo_edit(cx));
            }))
            .child(self.icon_button("redo", "redo", cx, |this, _, cx| {
                this.with_active(cx, |view, cx| view.redo_edit(cx));
            }))
            .when(formats, |element| {
                element
                    .child(separator())
                    .child(self.format_button(
                        "bold",
                        FormatKind::Bold,
                        cx,
                        div().font_weight(FontWeight::BOLD).child("B"),
                    ))
                    .child(self.format_button(
                        "italic",
                        FormatKind::Italic,
                        cx,
                        div().italic().child("I"),
                    ))
                    .child(separator())
            })
            .when(markup, |element| {
                element.child(self.format_button(
                    "math",
                    FormatKind::Math,
                    cx,
                    Self::icon("math", 16., theme::UI_TEXT),
                ))
            })
            .when(formats, |element| {
                element.child(self.format_button(
                    "link",
                    FormatKind::Link,
                    cx,
                    Self::icon("link", 16., theme::UI_TEXT),
                ))
            })
            .when_some(language.filter(|_| markup), |element, language| {
                element
                    .child(separator())
                    .child(self.popup_button("symbols", "sigma", Menu::Symbols, cx, move |this, cx| {
                        this.render_symbol_palette(language, cx)
                    }))
                    .child(self.popup_button("table", "table", Menu::Table, cx, move |this, cx| {
                        this.render_table_grid(language, cx)
                    }))
                    .child(self.icon_button("figure", "image", cx, |this, window, cx| {
                        this.open_picker(PickerKind::Figures, window, cx);
                    }))
                    .child(self.icon_button("citation", "quote", cx, |this, window, cx| {
                        this.open_picker(PickerKind::Citations, window, cx);
                    }))
            })
    }

    fn render_editor_pane(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let active = self.active_view();
        let body: AnyElement = match (&self.image_preview, active) {
            (Some(preview), _) => preview.view.clone().into_any_element(),
            (None, Some(view)) => view.into_any_element(),
            (None, None) => div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_color(theme::color(theme::UI_MUTED))
                .child(if self.root.is_some() {
                    "Select a file in the file tree."
                } else {
                    "Open a folder to start."
                })
                .into_any_element(),
        };
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .h_full()
            .child(self.render_tabs(cx))
            .child(self.render_toolbar(cx))
            .child(div().flex_1().min_h_0().child(body))
            .when(self.error_count() > 0, |element| element.child(self.render_error_strip(cx)))
    }

    /// Stays below the editor, so the error is visible while fixing it.
    fn render_error_strip(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let first = self.problems.first();
        let label = match first {
            Some(Problem { location: Some(location), title, .. }) => format!("{location}: {title}"),
            Some(problem) => problem.title.clone(),
            None => String::new(),
        };
        div()
            .id("error-strip")
            .flex()
            .flex_none()
            .items_center()
            .gap(px(8.))
            .h(px(28.))
            .px(px(10.))
            .bg(theme::color(theme::ERROR_PANEL))
            .border_t_1()
            .border_color(theme::color(theme::ERROR))
            .cursor_pointer()
            .on_click(cx.listener(|this, _, window, cx| this.go_to_diagnostic(0, window, cx)))
            .child(div().flex_1().min_w_0().truncate().child(label))
            .child(div().flex_none().text_color(theme::color(theme::UI_MUTED)).child("Go to line"))
    }

    fn render_preview_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let open = self.open_menu == Some(Menu::Compile);
        let errors = self.error_count();
        let warnings = self.diagnostics.len() - errors;
        let pdf = self.pdf.read(cx);
        let page_label = if pdf.has_document() {
            format!("{} / {}", pdf.current_page() + 1, pdf.page_count())
        } else {
            String::new()
        };
        let zoom_label = format!("{}%", pdf.zoom_percent());
        div()
            .flex()
            .flex_none()
            .items_center()
            .justify_between()
            .gap(px(8.))
            .h(px(PREVIEW_TOOLBAR_HEIGHT))
            .px(px(8.))
            .bg(theme::color(theme::PANEL))
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(4.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .h(px(24.))
                            .rounded(px(12.))
                            .bg(theme::color(theme::GREEN))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(
                                div()
                                    .id("recompile")
                                    .px(px(14.))
                                    .cursor_pointer()
                                    .on_click(cx.listener(|this, _, _, cx| this.recompile(cx)))
                                    .child(if self.building { "Compiling…" } else { "Recompile" }),
                            )
                            .child(
                                div()
                                    .id("compile-options")
                                    .relative()
                                    .flex()
                                    .items_center()
                                    .h_full()
                                    .pl(px(6.))
                                    .pr(px(8.))
                                    .border_l_1()
                                    .border_color(theme::color(theme::GREEN_DARK))
                                    .cursor_pointer()
                                    .on_click(cx.listener(|this, event: &ClickEvent, window, cx| {
                                        let bottom = TOP_BAR_HEIGHT + PREVIEW_TOOLBAR_HEIGHT;
                                        this.toggle_menu(Menu::Compile, event.position(), bottom, window, cx);
                                    }))
                                    .child(Self::icon("chevron-down", 12., theme::UI_TEXT))
                                    .when(open, |element| element.child(self.render_menu(Menu::Compile, cx))),
                            ),
                    )
                    .child(self.icon_button("view-logs", "log", cx, |this, _, cx| {
                        this.toggle_logs();
                        cx.notify();
                    }))
                    .when(errors > 0, |element| {
                        element.child(
                            div()
                                .px(px(5.))
                                .rounded(px(8.))
                                .bg(theme::color(theme::ERROR))
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(errors.to_string()),
                        )
                    })
                    .when(warnings > 0, |element| {
                        element.child(
                            div()
                                .px(px(5.))
                                .rounded(px(8.))
                                .bg(theme::color(theme::WARNING))
                                .text_color(theme::color(theme::BG))
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(warnings.to_string()),
                        )
                    })
                    .child(self.icon_button("go-to-pdf", "arrow-right", cx, |this, _, cx| {
                        this.forward_from_cursor(cx);
                    })),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(theme::color(theme::UI_MUTED))
                    .child(self.status.clone().unwrap_or_default()),
            )
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(2.))
                    .child(self.icon_button("page-prev", "chevron-up", cx, |this, _, cx| {
                        this.pdf.update(cx, |view, cx| {
                            let page = view.current_page().saturating_sub(1);
                            view.go_to_page(page, cx);
                        });
                    }))
                    .child(self.icon_button("page-next", "chevron-down", cx, |this, _, cx| {
                        this.pdf.update(cx, |view, cx| {
                            let page = view.current_page() + 1;
                            view.go_to_page(page, cx);
                        });
                    }))
                    .child(div().px(px(4.)).child(page_label))
                    .child(self.icon_button("zoom-out", "minus", cx, |this, _, cx| {
                        this.pdf.update(cx, |view, cx| view.zoom_by(1. / ZOOM_STEP, cx));
                    }))
                    .child(self.icon_button("zoom-in", "plus", cx, |this, _, cx| {
                        this.pdf.update(cx, |view, cx| view.zoom_by(ZOOM_STEP, cx));
                    }))
                    .child(
                        div()
                            .id("zoom-fit")
                            .px(px(4.))
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.pdf.update(cx, |view, cx| view.set_fit_width(cx));
                            }))
                            .child(zoom_label),
                    )
                    .child(
                        div()
                            .rounded(px(4.))
                            .when(self.settings.auto_scroll_pdf, |element| element.bg(theme::color(theme::GREEN_DARK)))
                            .child(self.icon_button("auto-scroll-pdf", "position", cx, |this, window, cx| {
                                this.toggle_auto_scroll(window, cx);
                            })),
                    )
                    .child(self.icon_button("invert", "contrast", cx, |this, _, cx| {
                        this.pdf.update(cx, |view, cx| view.toggle_invert(cx));
                    }))
                    .child(self.icon_button("reveal-pdf", "download", cx, |this, _, cx| {
                        match this.last_pdf.clone() {
                            Some(path) => cx.reveal_path(&path),
                            None => this.set_status("No PDF yet.", cx),
                        }
                    })),
            )
    }

    fn panel_button(
        &self,
        id: &'static str,
        label: impl Into<SharedString>,
        primary: bool,
        cx: &mut Context<Self>,
        handler: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> impl IntoElement {
        div()
            .id(id)
            .flex_none()
            .px(px(10.))
            .py(px(4.))
            .rounded(px(4.))
            .cursor_pointer()
            .bg(theme::color(if primary { theme::GREEN } else { theme::DIVIDER }))
            .font_weight(FontWeight::SEMIBOLD)
            .on_click(cx.listener(move |this, _, window, cx| handler(this, window, cx)))
            .child(label.into())
    }

    /// The title, position, source and advice of the error to fix first.
    fn render_first_error(&self, problem: &Problem, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(div().text_size(px(15.)).font_weight(FontWeight::SEMIBOLD).child(problem.title.clone()))
            .when_some(problem.location.clone(), |element, location| {
                element.child(div().text_color(theme::color(theme::UI_MUTED)).child(location))
            })
            .when(!problem.excerpt.is_empty(), |element| {
                element.child(
                    div()
                        .p(px(8.))
                        .rounded(px(4.))
                        .bg(theme::color(theme::BG))
                        .font_family(theme::CODE_FONT)
                        .text_size(px(theme::CODE_FONT_SIZE))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .children(problem.excerpt.iter().flat_map(|line| {
                            let number = format!("{:>5}  ", line.number);
                            let text = div()
                                .when(!line.target, |style| style.text_color(theme::color(theme::UI_MUTED)))
                                .when(line.target, |style| style.font_weight(FontWeight::SEMIBOLD))
                                .child(format!("{number}{}", line.text));
                            // Non-breaking spaces, because plain ones collapse in a text run.
                            let marker = problem.marker.filter(|_| line.target).map(|(before, width)| {
                                div()
                                    .text_color(theme::color(theme::ERROR))
                                    .font_weight(FontWeight::BOLD)
                                    .child(format!("{}{}", "\u{a0}".repeat(number.len() + before), "^".repeat(width)))
                            });
                            std::iter::once(text).chain(marker)
                        })),
                )
            })
            .when_some(problem.hint.clone(), |element, hint| {
                element.children(hint.lines().map(|line| div().child(line.to_string())).collect::<Vec<_>>())
            })
            .when_some(problem.compiler_message.clone(), |element, message| {
                element.child(
                    div()
                        .text_size(px(11.))
                        .text_color(theme::color(theme::UI_MUTED))
                        .child(format!("Compiler message: {message}")),
                )
            })
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(6.))
                    .when(problem.location.is_some(), |element| {
                        element.child(self.panel_button("go-to-error", "Go to line", true, cx, |this, window, cx| {
                            this.go_to_diagnostic(0, window, cx);
                        }))
                    })
                    .child(self.panel_button("all-details", "All details", false, cx, |this, _, cx| {
                        this.show_logs = true;
                        cx.notify();
                    }))
                    .child(self.panel_button("dismiss-error", "Hide", false, cx, |this, _, cx| {
                        this.error_card_dismissed = true;
                        cx.notify();
                    })),
            )
    }

    fn preview_message(&self) -> &'static str {
        match self.preview_state {
            PreviewState::Incomplete => "Errors found. This PDF may be incomplete.",
            PreviewState::LastGood => "Build failed. Showing the last good PDF.",
            PreviewState::Empty => "Build failed. There is no PDF yet.",
            PreviewState::Current => "This build has errors.",
        }
    }

    /// Lies above the PDF, so the last good PDF stays visible below it.
    fn render_error_card(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let errors = self.error_count();
        div()
            .id("error-card")
            .flex()
            .flex_col()
            .flex_none()
            .max_h(gpui::relative(0.6))
            .overflow_y_scroll()
            .gap(px(8.))
            .p(px(12.))
            .bg(theme::color(theme::ERROR_PANEL))
            .border_b_2()
            .border_color(theme::color(theme::ERROR))
            .child(div().font_weight(FontWeight::SEMIBOLD).child(self.preview_message()))
            .when_some(self.problems.first(), |element, problem| {
                element.child(self.render_first_error(problem, cx))
            })
            .when(errors > 1, |element| {
                element.child(div().text_color(theme::color(theme::UI_MUTED)).child(format!(
                    "{} more. Fix this one first and save, because later errors often come from it.",
                    errors - 1
                )))
            })
    }

    /// A one-line reminder after the user hides the card.
    fn render_error_banner(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("error-banner")
            .flex()
            .flex_none()
            .items_center()
            .h(px(28.))
            .px(px(10.))
            .bg(theme::color(theme::ERROR_PANEL))
            .cursor_pointer()
            .on_click(cx.listener(|this, _, _, cx| {
                this.error_card_dismissed = false;
                cx.notify();
            }))
            .child(div().truncate().child(self.preview_message()))
    }

    fn render_problem_row(&self, index: usize, problem: &Problem, cx: &mut Context<Self>) -> impl IntoElement {
        let color = match self.diagnostics[index].severity {
            Severity::Error => theme::ERROR,
            Severity::Warning => theme::WARNING,
        };
        div()
            .id(("diagnostic", index))
            .flex()
            .flex_col()
            .p(px(6.))
            .mb(px(4.))
            .rounded(px(4.))
            .border_l_2()
            .border_color(theme::color(color))
            .bg(theme::color(theme::PANEL))
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, window, cx| this.go_to_diagnostic(index, window, cx)))
            .child(problem.title.clone())
            .child(
                div()
                    .text_color(theme::color(theme::UI_MUTED))
                    .child(problem.location.clone().unwrap_or_default()),
            )
    }

    fn render_logs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let errors = self.error_count();
        let warnings = self.diagnostics.len() - errors;
        let headline = match (errors, warnings) {
            (0, 0) => "No errors and no warnings.".to_string(),
            (0, warnings) => format!("No errors, {warnings} warnings. The PDF is complete."),
            (errors, warnings) => format!("{errors} errors, {warnings} warnings."),
        };
        let lines: Vec<&str> = self.build_log.lines().collect();
        let shown = &lines[lines.len().saturating_sub(MAX_LOG_LINES_SHOWN)..];
        div()
            .id("logs")
            .size_full()
            .overflow_y_scroll()
            .bg(theme::color(theme::BG))
            .p(px(10.))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .justify_between()
                    .gap(px(6.))
                    .mb(px(10.))
                    .child(div().font_weight(FontWeight::SEMIBOLD).child(headline))
                    .child(
                        div()
                            .flex()
                            .gap(px(6.))
                            .child(self.panel_button("back-to-pdf", "Back to PDF", true, cx, |this, _, cx| {
                                this.show_logs = false;
                                cx.notify();
                            }))
                            .child(self.panel_button("from-scratch", "Recompile from scratch", false, cx, |this, _, cx| {
                                this.recompile_from_scratch(cx);
                            })),
                    ),
            )
            .children(
                self.problems
                    .iter()
                    .take(MAX_DIAGNOSTICS_SHOWN)
                    .enumerate()
                    .map(|(index, problem)| self.render_problem_row(index, problem, cx).into_any_element()),
            )
            .when(self.diagnostics.len() > MAX_DIAGNOSTICS_SHOWN, |element| {
                element.child(
                    div()
                        .mb(px(4.))
                        .text_color(theme::color(theme::UI_MUTED))
                        .child(format!("{} more are not shown.", self.diagnostics.len() - MAX_DIAGNOSTICS_SHOWN)),
                )
            })
            .child(div().mt(px(8.)).child(self.panel_button(
                "raw-log",
                if self.show_raw_log { "Hide raw log" } else { "Show raw log" },
                false,
                cx,
                |this, _, cx| {
                    this.show_raw_log = !this.show_raw_log;
                    cx.notify();
                },
            )))
            .when(self.show_raw_log, |element| {
                element.child(
                    div()
                        .mt(px(8.))
                        .font_family(theme::CODE_FONT)
                        .text_size(px(11.))
                        .text_color(theme::color(theme::UI_MUTED))
                        .children(shown.iter().map(|line| div().child(line.to_string()))),
                )
            })
    }

    fn render_status_bar(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.settings.vim_mode {
            return None;
        }
        let view = self.active_view();
        let view = view.as_ref().map(|view| view.read(cx));
        let errors = self.error_count();
        let (line, column) = view.map_or((1, 1), EditorView::cursor_position);
        let pending = match &self.tree_leader {
            Some(prefix) => format!("Space {prefix}"),
            None => view.map_or_else(String::new, EditorView::vim_pending_keys),
        };
        let info = StatusInfo {
            mode: view.and_then(EditorView::vim_mode).unwrap_or(Mode::Normal),
            pending,
            file: view.map(EditorView::title),
            file_icon: status_bar::file_icon(
                view.and_then(|view| view.path.as_deref()?.extension()?.to_str()),
            ),
            dirty: view.is_some_and(|view| view.state.is_dirty()),
            branch: self.git_branch.clone(),
            errors,
            warnings: self.diagnostics.len() - errors,
            project: self
                .root
                .as_deref()
                .and_then(|root| root.file_name())
                .map_or_else(String::new, |name| name.to_string_lossy().into_owned()),
            line,
            column,
            command_line: view.and_then(EditorView::vim_command_line),
        };
        Some(status_bar::render(info).into_any_element())
    }

    fn toggle_switch(&self, id: &'static str, on: bool, enabled: bool, cx: &mut Context<Self>, handler: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static) -> impl IntoElement {
        div()
            .id(id)
            .flex()
            .flex_none()
            .items_center()
            .w(px(34.))
            .h(px(20.))
            .p(px(2.))
            .rounded(px(10.))
            .when(on, |style| style.justify_end())
            .bg(theme::color(if on { theme::GREEN } else { theme::DIVIDER }))
            .when(enabled, |style| {
                style
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, window, cx| handler(this, window, cx)))
            })
            .when(!enabled, |style| style.opacity(0.4))
            .child(div().size(px(16.)).rounded_full().bg(theme::color(theme::UI_TEXT)))
    }

    /// All choices stay visible, so one click is enough to change them.
    fn segmented(&self, id: &'static str, options: Vec<(&'static str, bool, MenuHandler)>, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_none()
            .p(px(2.))
            .gap(px(2.))
            .rounded(px(6.))
            .bg(theme::color(theme::BG))
            .children(options.into_iter().enumerate().map(|(index, (label, selected, handler))| {
                div()
                    .id((id, index))
                    .px(px(10.))
                    .py(px(3.))
                    .rounded(px(4.))
                    .cursor_pointer()
                    .when(selected, |style| style.bg(theme::color(theme::GREEN_DARK)).font_weight(FontWeight::SEMIBOLD))
                    .when(!selected, |style| style.hover(|style| style.bg(theme::color(theme::DIVIDER))))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        handler(this, window, cx);
                        cx.notify();
                    }))
                    .child(label)
            }))
    }

    fn setting_row(title: &'static str, description: impl Into<SharedString>, control: impl IntoElement) -> impl IntoElement {
        div()
            .flex()
            .flex_none()
            .items_center()
            .justify_between()
            .gap(px(16.))
            .py(px(10.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(px(2.))
                    .child(div().font_weight(FontWeight::SEMIBOLD).child(title))
                    .child(div().text_color(theme::color(theme::UI_MUTED)).child(description.into())),
            )
            .child(control)
    }

    fn render_editor_settings(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let vim_on = self.settings.vim_mode;
        let handler = |vim_mode: bool| -> MenuHandler {
            Rc::new(move |this, window, cx| this.update_settings(|settings| settings.vim_mode = vim_mode, window, cx))
        };
        vec![
            Self::setting_row(
                "Keybindings",
                "Vim adds modes, Space commands, a status bar and keyboard control of the file tree.",
                self.segmented("keybindings", vec![("None", !vim_on, handler(false)), ("Vim", vim_on, handler(true))], cx),
            )
            .into_any_element(),
            Self::setting_row(
                "Smooth cursor",
                if vim_on {
                    "Draws a short trail when the block cursor jumps."
                } else {
                    "Needs the Vim keybindings."
                },
                self.toggle_switch("smooth-cursor", vim_on && self.settings.smooth_cursor, vim_on, cx, |this, window, cx| {
                    this.update_settings(|settings| settings.smooth_cursor = !settings.smooth_cursor, window, cx);
                }),
            )
            .into_any_element(),
            Self::setting_row(
                "Follow cursor in PDF",
                "Scrolls the PDF along with the editor cursor. Needs SyncTeX, so LaTeX only.",
                self.toggle_switch("auto-scroll-pdf-setting", self.settings.auto_scroll_pdf, true, cx, |this, window, cx| {
                    this.toggle_auto_scroll(window, cx);
                }),
            )
            .into_any_element(),
        ]
    }

    fn render_addons_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let rows: Vec<AnyElement> = self
            .installed_addons
            .iter()
            .enumerate()
            .map(|(index, add_on)| {
                let manifest = &add_on.manifest;
                let permissions = manifest.permissions.iter().map(|permission| permission.description()).collect::<Vec<_>>().join(", ");
                let (for_toggle, for_remove) = (add_on.clone(), add_on.clone());
                let enabled = add_on.enabled;
                div()
                    .flex()
                    .flex_col()
                    .flex_none()
                    .gap(px(4.))
                    .py(px(10.))
                    .border_t_1()
                    .border_color(theme::color(theme::PANEL))
                    .child(div().font_weight(FontWeight::SEMIBOLD).child(format!("{} {}", manifest.name, manifest.version)))
                    .child(div().text_color(theme::color(theme::UI_MUTED)).child(format!("By {}", manifest.author)))
                    .child(div().text_color(theme::color(theme::UI_MUTED)).child(if permissions.is_empty() {
                        "Asks for no permissions.".to_string()
                    } else {
                        permissions
                    }))
                    .child(
                        div()
                            .flex()
                            .gap(px(6.))
                            .pt(px(4.))
                            .child(
                                div()
                                    .id(("addon-toggle", index))
                                    .flex_none()
                                    .px(px(8.))
                                    .py(px(3.))
                                    .rounded(px(4.))
                                    .cursor_pointer()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .bg(theme::color(if enabled { theme::GREEN_DARK } else { theme::DIVIDER }))
                                    .on_click(cx.listener(move |this, _, _, cx| this.set_addon_enabled(&for_toggle, !enabled, cx)))
                                    .child(if enabled { "On" } else { "Off" }),
                            )
                            .child(
                                div()
                                    .id(("addon-remove", index))
                                    .flex_none()
                                    .px(px(8.))
                                    .py(px(3.))
                                    .rounded(px(4.))
                                    .cursor_pointer()
                                    .bg(theme::color(theme::DIVIDER))
                                    .on_click(cx.listener(move |this, _, _, cx| this.remove_addon(&for_remove, cx)))
                                    .child("Remove"),
                            ),
                    )
                    .into_any_element()
            })
            .collect();
        div()
            .id("addons-panel")
            .flex()
            .flex_col()
            .flex_none()
            .w(self.file_panel_width)
            .h_full()
            .gap(px(8.))
            .p(px(8.))
            .overflow_y_scroll()
            .child(div().flex_none().font_weight(FontWeight::SEMIBOLD).child("Add-ons"))
            .child(
                div()
                    .flex_none()
                    .text_color(theme::color(theme::UI_MUTED))
                    .child("Add-ons run with your user rights and have no sandbox, so turn on only those you trust. Permissions limit their requests to the editor, not the program itself."),
            )
            .child(
                div().flex().flex_none().child(self.panel_button("addon-install", "Install from folder", false, cx, |this, _, cx| {
                    this.prompt_install_addon(cx);
                })),
            )
            .when(rows.is_empty(), |element| element.child(div().flex_none().child("No add-ons are installed.")))
            .children(rows)
    }

    fn render_compiler_settings(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let trigger = self.config.trigger;
        let engine = self.config.engine;
        let set_trigger = |trigger: Trigger| -> MenuHandler {
            Rc::new(move |this, _, _| this.config.trigger = trigger)
        };
        let set_engine = |engine: Option<Engine>| -> MenuHandler {
            Rc::new(move |this, _, cx| {
                this.config.engine = engine;
                this.recompile(cx);
            })
        };
        vec![
            Self::setting_row(
                "Compile",
                "Applies until the app closes. Set trigger in .editor.toml to keep it.",
                self.segmented(
                    "compile-trigger",
                    vec![
                        ("On save", trigger == Trigger::Save, set_trigger(Trigger::Save)),
                        ("While typing", trigger == Trigger::Type, set_trigger(Trigger::Type)),
                    ],
                    cx,
                ),
            )
            .into_any_element(),
            Self::setting_row(
                "LaTeX compiler",
                "Applies until the app closes. Set engine in .editor.toml to keep it.",
                self.segmented(
                    "compile-engine",
                    vec![
                        ("Auto", engine.is_none(), set_engine(None)),
                        ("pdfLaTeX", engine == Some(Engine::PdfLatex), set_engine(Some(Engine::PdfLatex))),
                        ("LuaLaTeX", engine == Some(Engine::LuaLatex), set_engine(Some(Engine::LuaLatex))),
                        ("Tectonic", engine == Some(Engine::Tectonic), set_engine(Some(Engine::Tectonic))),
                    ],
                    cx,
                ),
            )
            .into_any_element(),
            Self::setting_row(
                "Sync PDF on click",
                "Clicking a line in the editor shows it in the PDF. LaTeX only.",
                self.toggle_switch("sync-on-click", self.sync_on_click, true, cx, |this, _, cx| {
                    this.sync_on_click = !this.sync_on_click;
                    cx.notify();
                }),
            )
            .into_any_element(),
        ]
    }

    fn render_shortcuts(&self) -> Vec<AnyElement> {
        let mut elements: Vec<AnyElement> = shortcuts::sections(self.settings.vim_mode)
            .into_iter()
            .map(|section| {
                div()
                    .flex()
                    .flex_col()
                    .flex_none()
                    .gap(px(4.))
                    .mb(px(14.))
                    .child(div().font_weight(FontWeight::SEMIBOLD).mb(px(2.)).child(section.title))
                    .children(section.rows.into_iter().map(|(keys, label)| {
                        div()
                            .flex()
                            .items_center()
                            .gap(px(12.))
                            .child(
                                div()
                                    .w(px(170.))
                                    .flex_none()
                                    .font_family(theme::CODE_FONT)
                                    .text_size(px(theme::CODE_FONT_SIZE))
                                    .text_color(theme::color(theme::CYAN))
                                    .child(keys),
                            )
                            .child(div().flex_1().min_w_0().child(label))
                    }))
                    .into_any_element()
            })
            .collect();
        if !self.settings.vim_mode {
            elements.push(
                div()
                    .flex_none()
                    .text_color(theme::color(theme::UI_MUTED))
                    .child("Coming from Vim? Turn on the Vim keybindings in the settings.")
                    .into_any_element(),
            );
        }
        elements
    }

    fn settings_tab(&self, page: SettingsPage, current: SettingsPage, label: &'static str, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id(label)
            .flex()
            .flex_none()
            .items_center()
            .h(px(32.))
            .px(px(10.))
            .rounded(px(4.))
            .cursor_pointer()
            .when(page == current, |style| style.bg(theme::color(theme::GREEN_DARK)).font_weight(FontWeight::SEMIBOLD))
            .when(page != current, |style| style.hover(|style| style.bg(theme::color(theme::DIVIDER))))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.dialog = Some(Dialog::Settings(page));
                cx.notify();
            }))
            .child(label)
    }

    fn render_dialog(&self, dialog: Dialog, cx: &mut Context<Self>) -> impl IntoElement {
        let title = match dialog {
            Dialog::Settings(_) => "Settings",
            Dialog::Shortcuts => "Keyboard shortcuts",
        };
        let content = match dialog {
            Dialog::Settings(SettingsPage::Editor) => self.render_editor_settings(cx),
            Dialog::Settings(SettingsPage::Compiler) => self.render_compiler_settings(cx),
            Dialog::Shortcuts => self.render_shortcuts(),
        };
        div()
            .id("dialog-backdrop")
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(black().opacity(0.5))
            .occlude()
            .track_focus(&self.dialog_focus)
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| this.close_dialog(window, cx)))
            .child(
                div()
                    .id("dialog")
                    .occlude()
                    .flex()
                    .flex_col()
                    .w(px(DIALOG_WIDTH))
                    .h(px(DIALOG_HEIGHT))
                    .max_h(gpui::relative(0.9))
                    .rounded(px(6.))
                    .bg(theme::color(theme::PANEL))
                    .border_1()
                    .border_color(theme::color(theme::DIVIDER))
                    .shadow_lg()
                    .child(
                        div()
                            .flex()
                            .flex_none()
                            .items_center()
                            .justify_between()
                            .h(px(52.))
                            .px(px(16.))
                            .border_b_1()
                            .border_color(theme::color(theme::DIVIDER))
                            .child(div().text_size(px(17.)).font_weight(FontWeight::SEMIBOLD).child(title))
                            .child(self.icon_button("dialog-close", "close", cx, |this, window, cx| {
                                this.close_dialog(window, cx);
                            })),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_1()
                            .min_h_0()
                            .when_some(
                                match dialog {
                                    Dialog::Settings(page) => Some(page),
                                    Dialog::Shortcuts => None,
                                },
                                |element, page| {
                                    element.child(
                                        div()
                                            .flex()
                                            .flex_col()
                                            .flex_none()
                                            .w(px(170.))
                                            .gap(px(2.))
                                            .p(px(6.))
                                            .border_r_1()
                                            .border_color(theme::color(theme::DIVIDER))
                                            .child(self.settings_tab(SettingsPage::Editor, page, "Editor", cx))
                                            .child(self.settings_tab(SettingsPage::Compiler, page, "Compiler", cx)),
                                    )
                                },
                            )
                            .child(
                                div()
                                    .id("dialog-content")
                                    .flex()
                                    .flex_col()
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_y_scroll()
                                    .px(px(16.))
                                    .py(px(10.))
                                    .children(content),
                            ),
                    ),
            )
    }

    fn render_picker(&self, picker: &Picker, cx: &mut Context<Self>) -> impl IntoElement {
        let title = match picker.kind {
            PickerKind::Files => "Find file",
            PickerKind::Text => "Find text",
            PickerKind::Figures => "Insert figure",
            PickerKind::Citations => "Insert citation",
            PickerKind::Branches => "Switch branch",
        };
        let empty = match (picker.kind, picker.query.is_empty()) {
            (PickerKind::Text, true) => "Type to search the project.",
            (PickerKind::Figures, true) => "No images in this project.",
            (PickerKind::Citations, true) => "No .bib entries in this project.",
            (PickerKind::Branches, true) => "Type a name to create a branch.",
            _ => "No results.",
        };
        div()
            .id("picker-backdrop")
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .flex()
            .justify_center()
            .pt(px(TOP_BAR_HEIGHT + 40.))
            .bg(black().opacity(0.4))
            .occlude()
            .track_focus(&self.picker_focus)
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| this.close_picker(window, cx)))
            .child(
                div()
                    .id("picker")
                    .occlude()
                    .flex()
                    .flex_col()
                    .w(px(PICKER_WIDTH))
                    .h(px(PICKER_HEIGHT))
                    .max_h(gpui::relative(0.8))
                    .rounded(px(6.))
                    .bg(theme::color(theme::PANEL))
                    .border_1()
                    .border_color(theme::color(theme::DIVIDER))
                    .shadow_lg()
                    .child(
                        div()
                            .flex()
                            .flex_none()
                            .items_center()
                            .gap(px(10.))
                            .h(px(36.))
                            .px(px(12.))
                            .border_b_1()
                            .border_color(theme::color(theme::DIVIDER))
                            .child(div().flex_none().text_color(theme::color(theme::UI_MUTED)).child(title))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .font_family(theme::CODE_FONT)
                                    .child(picker.query.clone())
                                    .child(div().w(px(1.5)).h(px(15.)).bg(theme::color(theme::CARET))),
                            ),
                    )
                    .child(
                        div()
                            .id("picker-results")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .track_scroll(&self.picker_scroll)
                            .when(picker.items.is_empty(), |element| {
                                element.child(div().p(px(12.)).text_color(theme::color(theme::UI_MUTED)).child(empty))
                            })
                            .children(picker.items.iter().enumerate().map(|(index, item)| {
                                let location = match item.position {
                                    Some((line, _)) => format!("{}:{}", item.label, line + 1),
                                    None => item.label.clone(),
                                };
                                div()
                                    .id(("picker-item", index))
                                    .flex()
                                    .items_center()
                                    .gap(px(12.))
                                    .h(px(26.))
                                    .px(px(12.))
                                    .cursor_pointer()
                                    .when(index == picker.selected, |style| style.bg(theme::color(theme::GREEN_DARK)))
                                    .when(index != picker.selected, |style| style.hover(|style| style.bg(theme::color(theme::DIVIDER))))
                                    .on_click(cx.listener(move |this, _, window, cx| this.accept_picker(index, window, cx)))
                                    .child(div().flex_none().max_w(px(320.)).truncate().child(location))
                                    .when_some(item.detail.clone(), |element, detail| {
                                        element.child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .truncate()
                                                .font_family(theme::CODE_FONT)
                                                .text_size(px(theme::CODE_FONT_SIZE))
                                                .text_color(theme::color(theme::UI_MUTED))
                                                .child(detail),
                                        )
                                    })
                            })),
                    ),
            )
    }

    fn render_preview(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let has_errors = self.error_count() > 0;
        let body: AnyElement = if self.show_logs {
            self.render_logs(cx).into_any_element()
        } else {
            div()
                .flex()
                .flex_col()
                .size_full()
                .when(has_errors && !self.error_card_dismissed, |element| element.child(self.render_error_card(cx)))
                .when(has_errors && self.error_card_dismissed, |element| element.child(self.render_error_banner(cx)))
                .child(div().flex_1().min_h_0().child(self.pdf.clone()))
                .into_any_element()
        };
        div()
            .flex()
            .flex_col()
            .flex_none()
            .w(self.preview_width)
            .h_full()
            .bg(theme::color(theme::PANEL))
            .child(self.render_preview_toolbar(cx))
            .child(div().flex_1().min_h_0().child(body))
    }
}

/// Reads the branch without running git, as the status bar does this for every project.
fn git_branch(root: &Path) -> Option<String> {
    let head = std::fs::read_to_string(root.join(".git").join("HEAD")).ok()?;
    let head = head.trim();
    Some(match head.strip_prefix("ref: refs/heads/") {
        Some(branch) => branch.to_string(),
        None => head.chars().take(7).collect(),
    })
}

/// Turns a keystroke into a vim key. Characters come from `key_char`, which
/// respects the keyboard layout: on a German layout, `/` is Shift+7 and `{` is Alt+8.
fn translate_key(keystroke: &Keystroke, map_navigation: bool) -> Option<Key> {
    let name = keystroke.key.as_str();
    if keystroke.modifiers.control {
        let mut chars = name.chars();
        return match (chars.next(), chars.next()) {
            (Some(ch @ ('d' | 'u' | 'r')), None) => Some(Key::Ctrl(ch)),
            _ => None,
        };
    }
    let named = match name {
        "escape" => Some(Key::Esc),
        "enter" => Some(Key::Enter),
        "tab" if !keystroke.modifiers.shift => Some(Key::Tab),
        "backspace" => Some(Key::Backspace),
        "delete" => Some(Key::Delete),
        _ => None,
    };
    if named.is_some() {
        return named;
    }
    let navigation = match name {
        "left" => Some(Key::Char('h')),
        "right" => Some(Key::Char('l')),
        "up" => Some(Key::Char('k')),
        "down" => Some(Key::Char('j')),
        "home" => Some(Key::Char('0')),
        "end" => Some(Key::Char('$')),
        "pageup" => Some(Key::Ctrl('u')),
        "pagedown" => Some(Key::Ctrl('d')),
        _ => None,
    };
    if navigation.is_some() {
        return navigation.filter(|_| map_navigation);
    }
    let text = keystroke.key_char.as_deref().unwrap_or(name);
    let mut chars = text.chars();
    let ch = match (chars.next(), chars.next()) {
        (Some(ch), None) => ch,
        _ if name == "space" => ' ',
        _ => return None,
    };
    let ch = if keystroke.key_char.is_none() && keystroke.modifiers.shift { ch.to_ascii_uppercase() } else { ch };
    (!ch.is_control()).then_some(Key::Char(ch))
}

/// Turns a one-based character column into a byte offset within the line.
fn byte_column(text: &str, column: usize) -> usize {
    let text = text.trim_end_matches(['\n', '\r']);
    text.char_indices().nth(column.saturating_sub(1)).map_or(text.len(), |(index, _)| index)
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.refresh_outline(cx);
        self.tree_cursor = self.tree_cursor.min(self.entries.len().saturating_sub(1));
        let dragging = self.dragging;
        let show_preview = self.show_preview;
        let show_file_panel = self.show_file_panel;
        // Git runs only while its panel shows.
        let git_visible = show_file_panel && self.left_panel == LeftPanel::Git;
        self.git_panel.update(cx, |panel, cx| panel.set_visible(git_visible, cx));
        for index in 0..self.addon_panels.len() {
            let visible = show_file_panel && self.left_panel == LeftPanel::Addon(index);
            self.addon_panels[index].panel.clone().update(cx, |panel, cx| panel.set_visible(visible, cx));
        }
        div()
            .id("workspace")
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .bg(theme::color(theme::BG))
            .text_color(theme::color(theme::UI_TEXT))
            .text_size(px(theme::UI_FONT_SIZE))
            .when(dragging.is_some(), |style| {
                style.cursor(match dragging {
                    Some(Divider::Outline) => CursorStyle::ResizeUpDown,
                    _ => CursorStyle::ResizeLeftRight,
                })
            })
            .on_mouse_move(cx.listener(Self::on_root_mouse_move))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.dragging = None;
                    cx.notify();
                }),
            )
            .on_action(cx.listener(|this, _: &OpenFolder, window, cx| this.prompt_open_folder(window, cx)))
            .on_action(cx.listener(|this, _: &Save, _, cx| this.save_active(cx)))
            .on_action(cx.listener(|this, _: &Recompile, _, cx| this.recompile(cx)))
            .on_action(cx.listener(|this, _: &CloseTab, window, cx| {
                if this.image_preview.take().is_some() {
                    this.focus_active(window, cx);
                    cx.notify();
                } else if let Some(index) = this.active {
                    this.close_document(index, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &ToggleFileTree, _, cx| {
                this.show_file_panel = !this.show_file_panel;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &TogglePreview, _, cx| {
                this.show_preview = !this.show_preview;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ToggleLogs, _, cx| {
                this.toggle_logs();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &CloseMenu, window, cx| {
                if this.open_menu.is_some() {
                    this.close_menu(window, cx);
                } else if this.dialog.is_some() {
                    this.close_dialog(window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &OpenSettings, window, cx| {
                this.open_dialog(Dialog::Settings(SettingsPage::Editor), window, cx);
            }))
            .on_action(|_: &Quit, _, cx: &mut App| cx.quit())
            .child(self.render_top_bar(cx))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(self.render_rail(cx))
                    .when(show_file_panel, |element| {
                        let panel = match self.left_panel {
                            LeftPanel::Files => self.render_file_panel(window, cx).into_any_element(),
                            LeftPanel::Git => div()
                                .flex_none()
                                .w(self.file_panel_width)
                                .h_full()
                                .child(self.git_panel.clone())
                                .into_any_element(),
                            LeftPanel::Addons => self.render_addons_panel(cx).into_any_element(),
                            LeftPanel::Addon(index) => div()
                                .flex_none()
                                .w(self.file_panel_width)
                                .h_full()
                                .children(self.addon_panels.get(index).map(|entry| entry.panel.clone()))
                                .into_any_element(),
                        };
                        element
                            .child(panel)
                            .child(self.divider_vertical("file-divider", Divider::FilePanel, cx))
                    })
                    .child(self.render_editor_pane(cx))
                    .when(show_preview, |element| {
                        element
                            .child(self.divider_vertical("preview-divider", Divider::Preview, cx))
                            .child(self.render_preview(cx))
                    }),
            )
            .children(self.render_status_bar(cx))
            .when_some(self.dialog, |element, dialog| element.child(self.render_dialog(dialog, cx)))
            .when_some(self.picker.as_ref(), |element, picker| element.child(self.render_picker(picker, cx)))
            // Below the menus, which are deferred, and above everything else.
            .when(self.open_menu.is_some(), |element| {
                element.child(
                    div()
                        .id("menu-backdrop")
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full()
                        .occlude()
                        .track_focus(&self.menu_focus)
                        .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| this.close_menu(window, cx)))
                        .on_mouse_down(MouseButton::Right, cx.listener(|this, _, window, cx| this.close_menu(window, cx))),
                )
            })
    }
}
