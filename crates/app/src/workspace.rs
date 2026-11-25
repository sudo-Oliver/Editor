use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet, VecDeque},
    ops::Range,
    path::{Path, PathBuf},
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use build::{BuildEvent, BuildManager, BuildOutcome, BuildRequest, Config, Diagnostic, Engine, Fix, Loaded, Severity, Trigger, find_main};
use addon_api::{ActiveFile, DiagnosticInfo, Reply, Request, SearchHit};
use editor_core::{
    Selection, Symbol, SymbolKind,
    Language, Mark, MarkSeverity, MarkSource,
    snippet::Snippet,
    vim::{self, Effect, Key, Leader, Mode},
    vim_map::{self, Mapper, Mappings, Step},
    word_count::{self, WordCount},
};
use gpui::{
    AnyElement, Anchor, App, AsyncWindowContext, Bounds, ClickEvent, ClipboardItem, Context, CursorStyle, Entity, FocusHandle, Focusable, FontWeight, Keystroke,
    KeystrokeEvent, MouseButton, MouseDownEvent, MouseMoveEvent, PathPromptOptions, Pixels, Point, Render, ScrollHandle, ScrollStrategy, SharedString, UniformListScrollHandle,
    EntityId, PromptLevel, Subscription, Task, WeakEntity, Window, anchored, black, canvas, deferred, div, fill, img, point, prelude::*, px, size, uniform_list, svg,
};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use synctex::SyncTex;

use crate::{
    actions::*,
    addon_panel::{AddonPanel, AddonPanelEvent, doi_suggestion},
    addon_reports,
    addons::{self, Installed},
    bib,
    completion::Extra,
    marketplace::{self, CatalogEntry},
    material_icons,
    editor_view::{EditorEvent, EditorView, FormatKind, is_save_conflict},
    export::{self, DraftFormat, PdfState},
    file_tree::{self, TreeEntry},
    git_panel::{self, GitPanel, GitPanelEvent},
    home_view::{HomeEvent, HomeView},
    outline::{self, LINE as OUTLINE_LINE, OutlineItem, PILL as OUTLINE_PILL, SPINE as OUTLINE_SPINE},
    image_view::{ImageView, ImageViewEvent, include_snippet, is_image, relative_path},
    keymap::{self, Keymap},
    markdown_preview::{MarkdownPreview, MarkdownPreviewEvent},
    pdf_view::{PdfView, PdfViewEvent, ZOOM_STEP},
    nvim_themes, package_commands,
    picker::{self, FontSlot, Picker, PickerItem, PickerKind},
    package_check::{self, PackageHint},
    problem_report,
    problems::{self, Problem},
    recovery,
    settings::{self, Settings},
    storage,
    setup::{SetupEvent, SetupView},
    projects::{self, TemplateLanguage},
    tour::{self, Tour, TourAnchor},
    shortcuts,
    snippets,
    status_bar::{self, StatusInfo},
    history,
    texlive::{self, TexLive, TexLiveEvent},
    text_file,
    theme,
    toolchain::{self, Tool, Toolchain, ToolchainEvent},
    updater::{self, Updater},
    window_controls,
    app_location::{self, Mover, MoverEvent},
};

#[cfg(feature = "collab")]
#[path = "collab_session.rs"]
mod collab_session;
#[path = "problem_dialog.rs"]
mod problem_dialog;
#[path = "git_sync.rs"]
mod git_sync;
#[path = "addon_host.rs"]
mod addon_host;
#[path = "review.rs"]
mod review;
#[path = "citation_index.rs"]
mod citation_index;

const RECOVERY_DELAY: Duration = Duration::from_secs(1);
const TOP_BAR_HEIGHT: f32 = 40.0;
const RAIL_WIDTH: f32 = 40.0;
const DIVIDER_WIDTH: f32 = 7.0;
const TAB_BAR_HEIGHT: f32 = 36.0;
const TAB_SLANT: f32 = 12.0;
const TAB_GAP: f32 = 1.5;
const TAB_HEIGHT: f32 = 32.0;
const TOOLBAR_HEIGHT: f32 = 32.0;
/// The toolbar floats in this much of the editor's paper above the text.
const TOOLBAR_ROW_HEIGHT: f32 = TOOLBAR_HEIGHT + 14.0;
/// Corners of the floating toolbar and of the spots inside it.
const TOOLBAR_RADIUS: f32 = 12.0;
const TOOL_RADIUS: f32 = 8.0;
const BREADCRUMB_HEIGHT: f32 = 24.0;
/// The editor in focus mode stays this wide, for lines that read well.
const FOCUS_WIDTH: f32 = 860.0;
const SYMBOL_CELL: f32 = 32.0;
const SYMBOL_COLUMNS: f32 = 11.0;
const TABLE_GRID: usize = 8;
const PREVIEW_TOOLBAR_HEIGHT: f32 = 32.0;
const MIN_PANE_WIDTH: f32 = 160.0;
/// Changes on disk come in bursts; one refresh takes them all.
const WATCH_SETTLE: Duration = Duration::from_millis(250);
/// The rows of the file tree: room to breathe, and one step of indent per level.
const TREE_ROW: f32 = 34.0;
const TREE_INSET: f32 = 12.0;
const TREE_INDENT: f32 = 20.0;
const TREE_ICON: f32 = 16.0;
const FOLLOW_DEBOUNCE: Duration = Duration::from_millis(150);
/// Long enough to read a burst of keys once, short enough to feel live.
const MARKDOWN_DEBOUNCE: Duration = Duration::from_millis(30);
/// Oldest output is dropped beyond this size.
const MAX_LOG_BYTES: usize = 2 << 20;
const PACKAGE_CHECK_FILE_LIMIT: u64 = 4 << 20;
const MAX_LOG_LINES_SHOWN: usize = 2000;
const MAX_DIAGNOSTICS_SHOWN: usize = 200;
/// Beyond this many errors the later ones are echoes of the first, and marking
/// them would paint the whole document red.
const MAX_ERRORS_MARKED: usize = 5;
const DIALOG_WIDTH: f32 = 640.0;
const DIALOG_HEIGHT: f32 = 560.0;
const PICKER_WIDTH: f32 = 720.0;
const PICKER_HEIGHT: f32 = 440.0;
/// The entry in full, beside the citation list.
const CITATION_DETAIL_WIDTH: f32 = 300.0;
/// A pause in typing before the project is searched for the query.
const PICKER_SEARCH_DELAY: Duration = Duration::from_millis(120);

const TRAFFIC_LIGHT_SPACE: f32 = window_controls::LEFT_SPACE;

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
    /// The live session with others.
    #[cfg(feature = "collab")]
    Collab,
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
    Code,
    /// The right-click menu of a row in the file tree.
    Tree,
    /// The right-click menu of the editor.
    EditorContext,
}

/// How the editor and the PDF share the window.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum PaneLayout {
    Split,
    EditorOnly,
    PdfOnly,
    /// The PDF is a tab among the documents.
    PdfTab,
}

struct PresentState {
    /// Leaving the talk leaves full screen only when the talk entered it.
    entered_fullscreen: bool,
    /// The exit button shows while the mouse moves, and not over the slides otherwise.
    controls_shown: bool,
    hide_controls_task: Option<Task<()>>,
}

/// How long the exit button of a talk stays after the mouse stops.
const PRESENT_CONTROLS_TIME: Duration = Duration::from_secs(2);

#[derive(Clone, Copy, PartialEq, Eq)]
enum SettingsPage {
    Editor,
    Compiler,
    Tools,
    Shortcuts,
    Storage,
}

/// The name field for a new file or folder at the top of the file tree.
struct TreeInput {
    folder: bool,
    parent: PathBuf,
    view: Entity<EditorView>,
    /// Vim keeps working in the tree when the field closes.
    back_to_tree: bool,
    /// The entry whose name the field edits, in its own row, instead of a new one.
    renaming: Option<PathBuf>,
    _observer: Subscription,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Dialog {
    Settings(SettingsPage),
    Shortcuts,
    WordCount,
    #[cfg(feature = "collab")]
    JoinSession,
    GitSignIn,
    CommitDiff,
    ReportProblem,
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum MenuEntryKind {
    Action,
    Heading,
    Separator,
}

struct MenuEntry {
    kind: MenuEntryKind,
    label: String,
    shortcut: SharedString,
    handler: MenuHandler,
    /// Shown in the error color, for actions that remove something.
    danger: bool,
}

fn entry(
    label: impl Into<String>,
    shortcut: impl Into<SharedString>,
    handler: impl Fn(&mut Workspace, &mut Window, &mut Context<Workspace>) + 'static,
) -> MenuEntry {
    MenuEntry {
        kind: MenuEntryKind::Action,
        label: label.into(),
        shortcut: shortcut.into(),
        handler: Rc::new(handler),
        danger: false,
    }
}

fn menu_heading(label: &str) -> MenuEntry {
    MenuEntry { kind: MenuEntryKind::Heading, ..entry(label, "", |_, _, _| {}) }
}

fn menu_separator() -> MenuEntry {
    MenuEntry { kind: MenuEntryKind::Separator, ..entry("", "", |_, _, _| {}) }
}

struct PdfSnapshot {
    root: PathBuf,
    pdf: PathBuf,
    building: bool,
    unsaved: bool,
    has_errors: bool,
    last_good: bool,
    warnings: Vec<String>,
}

const REPORT_LIST_LIMIT: usize = 10;

fn shortened_list(items: &[String]) -> String {
    let mut lines: Vec<String> = items.iter().take(REPORT_LIST_LIMIT).cloned().collect();
    if items.len() > REPORT_LIST_LIMIT {
        lines.push(format!("and {} more", items.len() - REPORT_LIST_LIMIT));
    }
    lines.join("\n")
}

async fn ask(
    this: &WeakEntity<Workspace>,
    cx: &mut AsyncWindowContext,
    level: PromptLevel,
    message: &str,
    detail: &str,
    answers: &[&str],
) -> Option<usize> {
    let answer = this.update_in(cx, |_, window, cx| window.prompt(level, message, Some(detail), answers, cx)).ok()?;
    answer.await.ok()
}

/// The palette of the chosen theme, from the cache only, so that it never waits for Neovim.
fn theme_palette(name: Option<&str>) -> Option<theme::Palette> {
    let name = name.unwrap_or(theme::DEFAULT_THEME);
    theme::built_in(name).or_else(|| nvim_themes::cache_path().and_then(|path| nvim_themes::load_cache(&path).remove(name)))
}

/// Paper mode lays its own ink over the theme; without it the theme comes back.
fn apply_look(settings: &Settings) {
    if settings.paper_mode {
        theme::set_palette(if settings.paper_dark { theme::Palette::PAPER_INK_DARK } else { theme::Palette::PAPER_INK });
    } else if let Some(palette) = theme_palette(settings.theme.as_deref()) {
        theme::set_palette(palette);
    }
    theme::set_look(settings.look());
}

fn show_status(this: &WeakEntity<Workspace>, cx: &mut AsyncWindowContext, message: &str) {
    let message = message.to_string();
    this.update(cx, |this, cx| this.set_status(message, cx)).ok();
}

async fn fail_export(this: &WeakEntity<Workspace>, cx: &mut AsyncWindowContext, error: &str) {
    show_status(this, cx, "Export failed. Nothing was written.");
    ask(this, cx, PromptLevel::Critical, "The export failed", error, &["OK"]).await;
}

struct OpenDocument {
    view: Entity<EditorView>,
    _subscription: Subscription,
    _observer: Subscription,
}

/// The panel of an add-on that is turned on.
enum CatalogState {
    /// Not asked for yet in this session.
    Idle,
    Loading,
    Ready(Vec<CatalogEntry>),
    Failed(String),
}

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
    /// How many documents come before this tab.
    after: usize,
    _subscription: Subscription,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Tab {
    Document(usize),
    Image(usize),
    Pdf,
}

/// The tabs from left to right. `image_after` is sorted, as images only join at the end.
fn tab_order(documents: usize, image_after: &[usize]) -> Vec<Tab> {
    let mut tabs = Vec::with_capacity(documents + image_after.len());
    let mut images = image_after.iter().enumerate().peekable();
    for document in 0..=documents {
        while let Some((image, _)) = images.next_if(|(_, after)| **after <= document) {
            tabs.push(Tab::Image(image));
        }
        if document < documents {
            tabs.push(Tab::Document(document));
        }
    }
    tabs
}

/// The headings that hold a line, from the outermost in. `line` is zero-based.
fn heading_trail(items: &[OutlineItem], line: usize) -> Vec<usize> {
    let mut trail: Vec<usize> = Vec::new();
    for (index, item) in items.iter().enumerate() {
        if item.line > line {
            break;
        }
        while trail.last().is_some_and(|&last| items[last].depth >= item.depth) {
            trail.pop();
        }
        trail.push(index);
    }
    trail
}

/// Which tab a click in the box of a tab belongs to, as an offset to that tab: -1, 0
/// or 1. Each tab reaches into the box of its neighbors, so that the slanted edge
/// decides and not the box. `up` runs from 0 at the bottom to 1 at the top.
fn tab_hit(x: f32, left: f32, right: f32, up: f32, (left_forward, right_forward): (bool, bool)) -> isize {
    let lean = |forward: bool| TAB_SLANT * (up - 0.5) * if forward { 1. } else { -1. };
    if x < left + lean(left_forward) {
        -1
    } else if x > right + lean(right_forward) {
        1
    } else {
        0
    }
}

/// Whether the left and the right edge of a tab lean to the right going up. Two
/// neighbors share an edge, and edges lean toward the active tab, so that it is
/// the one wide tab at the bottom and the others interlock with it.
fn tab_edges(position: usize, count: usize, active: Option<usize>) -> (bool, bool) {
    let active = active.unwrap_or(count);
    (position <= active, position < active && position + 1 < count)
}

pub struct Workspace {
    root: Option<PathBuf>,
    entries: Vec<TreeEntry>,
    collapsed: HashSet<PathBuf>,
    documents: Vec<OpenDocument>,
    active: Option<usize>,
    outline: Vec<OutlineItem>,
    outline_version: (usize, u64),
    /// The active document and the number of documents when the active view last
    /// got the names of the others.
    symbols_version: (usize, usize, usize),
    package_hints: Vec<PackageHint>,
    /// What the project loads, for the suggestions of every document.
    loaded_packages: Arc<Loaded>,
    package_extra: Arc<Extra>,
    package_check_task: Option<Task<()>>,
    conflict_task: Option<Task<()>>,
    recovery_task: Option<Task<()>>,
    recovery_offer_task: Option<Task<()>>,
    /// Documents with a recovery copy and the version the copy holds.
    recovered: HashMap<PathBuf, u64>,
    show_file_panel: bool,
    left_panel: LeftPanel,
    git_panel: Entity<GitPanel>,
    _git_subscription: Subscription,
    installed_addons: Vec<Installed>,
    addon_panels: Vec<AddonEntry>,
    addon_dialog_focus: FocusHandle,
    addon_dialog_changed: bool,
    addon_dialog_open: bool,
    catalog: CatalogState,
    /// The id of the add-on being downloaded.
    installing: Option<String>,
    /// By add-on and group, the default group being the empty name.
    tracked_edits: HashMap<(String, String), TrackedEdits>,
    /// Add-on requests that wait for the running build to end.
    compile_waiters: Vec<(Entity<AddonPanel>, u64)>,
    /// Where the cursor rests, as last told to add-ons, and what waits to be told.
    addon_context_sent: addon_host::AddonContext,
    addon_context_pending: Option<addon_host::AddonContext>,
    addon_context_task: Option<Task<()>>,
    /// Documents whose change add-ons hear of when typing pauses.
    addon_changed: HashSet<PathBuf>,
    addon_changed_task: Option<Task<()>>,
    /// Focuses the active document at the next frame, for actions without a window.
    focus_requested: bool,
    /// Changes from add-ons that wait for the user.
    proposals: Vec<review::Review>,
    layout: PaneLayout,
    /// The PDF tab is the one on screen.
    pdf_tab_shown: bool,
    focus_mode: bool,
    home: Entity<HomeView>,
    /// The setup of the first start, shown in place of everything else.
    setup: Option<(Entity<SetupView>, Subscription)>,
    updater: Entity<Updater>,
    _updater_observer: Subscription,
    mover: Entity<Mover>,
    _mover_subscriptions: [Subscription; 2],
    /// Package updates and new years of MacTeX.
    texlive: Entity<TexLive>,
    _texlive_observer: Subscription,
    _texlive_events: Subscription,
    /// Opens the setup at the next frame, for actions without a window.
    setup_requested: bool,
    /// The draft export that waits for Pandoc to be installed.
    export_after_pandoc: Option<DraftFormat>,
    tour: Option<Tour>,
    /// A file or folder from Finder or the terminal that arrived during the setup.
    pending_open: Option<PathBuf>,
    /// Where the parts that the tour points at were drawn last.
    tour_anchors: Rc<RefCell<HashMap<TourAnchor, Bounds<Pixels>>>>,
    toolchain: Entity<Toolchain>,
    _toolchain_subscriptions: [Subscription; 2],
    /// The hub also shows by itself while no project is open.
    show_hub: bool,
    /// The logo turns into a house under the mouse, as it leads to the hub.
    logo_hovered: bool,
    _home_subscription: Subscription,
    presenting: Option<PresentState>,
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
    /// The last status messages, for a problem report.
    recent_messages: VecDeque<String>,
    report_field: Entity<EditorView>,
    report_with_details: bool,
    report_preview: bool,
    report_error: Option<SharedString>,
    /// The details as the dialog opened, so that drawing it reads nothing.
    report_snapshot: Option<problem_report::Details>,
    /// What the app wrote when it quit unexpectedly last time, until reported.
    crash_report: Option<String>,
    pdf: Entity<PdfView>,
    /// The preview of a Markdown file, which needs no build.
    markdown: Entity<MarkdownPreview>,
    /// Reads the edited Markdown again once typing pauses a moment.
    markdown_task: Option<Task<()>>,
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
    /// A change arrived during a build, so one more build follows it.
    build_queued: bool,
    engine: Option<Engine>,
    synctex: Option<SyncTex>,
    sync_on_click: bool,
    /// The line of the active document that the PDF last followed.
    follow_line: Option<usize>,
    follow_task: Option<Task<()>>,
    /// Shown instead of the editor. The tabs stay as they are.
    images: Vec<ImagePreview>,
    shown_image: Option<usize>,
    /// Exports run one at a time, from the first dialog to the last.
    export_running: bool,
    last_build_succeeded: bool,
    word_count: Option<Result<WordCount, String>>,
    word_count_task: Option<Task<()>>,
    last_pdf: Option<PathBuf>,
    last_main: Option<PathBuf>,
    /// The main file of the PDF on screen.
    pdf_main: Option<PathBuf>,
    settings: Settings,
    settings_path: Option<PathBuf>,
    /// The user's shortcuts, from `keymap.json`.
    keymap: Keymap,
    /// The user's vim mappings, from `vimrc`.
    vim_mappings: Mappings,
    vim_mapper: Mapper,
    /// Runs keys that waited for a longer mapping in vain.
    vim_map_timer: Option<Task<()>>,
    /// The command on the shortcuts page that waits for its new keys.
    recording_shortcut: Option<&'static str>,
    dialog: Option<Dialog>,
    dialog_focus: FocusHandle,
    /// What the app's folders take, measured each time the storage settings open.
    storage_usage: Option<storage::Usage>,
    storage_task: Option<Task<()>>,
    picker: Option<Picker>,
    /// Where `\cite{` last opened the picker by itself.
    citation_trigger: Option<(EntityId, usize)>,
    /// What the document cites and refers to, rebuilt off the main thread after changes.
    citations: Arc<citations::index::CitationIndex>,
    /// Whether the first index is built, since before that an empty one says nothing.
    citations_ready: bool,
    citation_cache: Arc<std::sync::Mutex<citations::index::Cache>>,
    /// The text of open documents by version, so that a rebuild copies only what changed.
    citation_texts: HashMap<PathBuf, (u64, Arc<str>)>,
    citations_task: Option<Task<()>>,
    /// Something changed during a rebuild, so another one follows.
    citations_stale: bool,
    /// The picker needs a window, so the next render opens it.
    open_citations_soon: bool,
    /// A definition to open, which needs the window of the next render.
    definition_soon: Option<(PathBuf, std::ops::Range<usize>, usize)>,
    picker_focus: FocusHandle,
    picker_scroll: ScrollHandle,
    picker_files_task: Option<Task<()>>,
    tree_focus: FocusHandle,
    tree_scroll: UniformListScrollHandle,
    tree_cursor: usize,
    show_tree: bool,
    /// The row last clicked, which decides where a new entry goes.
    tree_target: Option<PathBuf>,
    tree_input: Option<TreeInput>,
    /// The row the tree's menu belongs to.
    tree_menu_path: Option<PathBuf>,
    tree_error: Option<SharedString>,
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
    _markdown_subscription: Subscription,
    _watcher: Option<RecommendedWatcher>,
    _watch_task: Option<Task<()>>,
    refresh_task: Option<Task<()>>,
    refresh_again: bool,
    #[cfg(feature = "collab")]
    collab: collab_session::CollabUi,
    git: git_sync::GitUi,
}

impl Workspace {
    pub fn new(root: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let pdf = cx.new(PdfView::new);
        let markdown = cx.new(MarkdownPreview::new);
        // A crash of the last run is offered for a report once the window shows.
        cx.defer_in(window, |this, window, cx| {
            if this.crash_report.is_some() {
                this.open_report(window, cx);
            }
        });
        let markdown_subscription = cx.subscribe_in(&markdown, window, |this, _, event, window, cx| match event {
            MarkdownPreviewEvent::Clicked(offset) => {
                if let Some(view) = this.active_view() {
                    let offset = *offset;
                    view.update(cx, |view, cx| view.select_range(offset..offset, window, cx));
                }
            }
        });
        let pdf_subscription = cx.subscribe_in(&pdf, window, |this, _, event, window, cx| match event {
            PdfViewEvent::BackwardSearch { page, x, y } => this.backward_search(*page, *x, *y, window, cx),
            PdfViewEvent::Failed(message) => this.set_status(message.clone(), cx),
        });
        // Edits live in memory only, so closing must not lose them.
        let quit_subscription = cx.on_app_quit(|this, cx| {
            this.save_all(cx);
            this.forget_saved_recovery(cx);
            this.build.shut_down();
            for addon in &this.addon_panels {
                addon.panel.update(cx, |panel, _| panel.shut_down());
            }
            addons::remove_private_directory();
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
        let toolchain = cx.new(|_| Toolchain::default());
        let updater = cx.new(Updater::new);
        let _updater_observer = cx.observe(&updater, |_, _, cx| cx.notify());
        let texlive = cx.new(TexLive::new);
        let _texlive_observer = cx.observe(&texlive, |_, _, cx| cx.notify());
        let _texlive_events = cx.subscribe(&texlive, |this, _, event, cx| match event {
            TexLiveEvent::Installed(package) => {
                this.set_status(format!("{package} is installed. Compiling again."), cx);
                this.recompile(cx);
            }
            TexLiveEvent::InstallFailed(error) => this.set_status(error.clone(), cx),
        });
        let toolchain_subscriptions = [
            cx.observe(&toolchain, |_, _, cx| cx.notify()),
            cx.subscribe(&toolchain, |this, _, event, cx| match event {
                ToolchainEvent::Installed(tool) => this.tool_installed(*tool, cx),
                ToolchainEvent::InstallerChosen(use_homebrew) => {
                    this.settings.use_homebrew = *use_homebrew;
                    this.save_settings(cx);
                }
            }),
        ];
        let home = cx.new(|cx| HomeView::new(toolchain.clone(), cx));
        let home_subscription = cx.subscribe_in(&home, window, |this, _, event, window, cx| match event {
            HomeEvent::Open(path) => {
                let path = path.clone();
                if this.root.as_deref() == Some(path.as_path()) {
                    this.close_hub(window, cx);
                } else {
                    this.show_hub = false;
                    this.open_root(path, window, cx);
                }
            }
            HomeEvent::Close => this.close_hub(window, cx),
            HomeEvent::Trashed(path) => this.forget_trashed_root(path, cx),
            #[cfg(feature = "collab")]
            HomeEvent::JoinSession => this.open_join_dialog(window, cx),
            HomeEvent::CardsChanged(cards) => {
                this.settings.hub_cards = *cards;
                this.save_settings(cx);
            }
            HomeEvent::StartTour => this.start_tour(window, cx),
            HomeEvent::InstallCommand => this.install_shell_command(cx),
            HomeEvent::RunSetup => this.run_setup(window, cx),
            HomeEvent::ShowShortcuts => this.open_dialog(Dialog::Shortcuts, window, cx),
            HomeEvent::ReportProblem => this.open_report(window, cx),
        });
        let workspace_handle = cx.entity().downgrade();
        let git_panel = cx.new(|cx| GitPanel::new(workspace_handle, cx));
        let git_subscription = cx.subscribe_in(&git_panel, window, |this, _, event, window, cx| match event {
            GitPanelEvent::OpenFile(path) => this.open_file(path.clone(), window, cx),
            GitPanelEvent::PickBranch => this.open_picker(PickerKind::Branches, window, cx),
            GitPanelEvent::Changed(branch) => {
                this.git_branch = branch.clone().or_else(|| this.root.as_deref().and_then(git_branch));
                this.refresh_in_background(cx);
                this.refresh_diff_bases(cx);
            }
            GitPanelEvent::SignIn { host } => this.open_sign_in(host.clone(), window, cx),
            GitPanelEvent::Conflicts(files) => this.show_conflicts(files, window, cx),
            GitPanelEvent::ShowCommit { title, patch } => this.show_commit(title, patch, window, cx),
        });
        let settings_path = settings::default_path();
        let mut settings = settings_path.as_deref().map(Settings::load).unwrap_or_default();
        // Homebrew is only the way for people who have it. Anyone else installs
        // into the app's folder, unless they choose to get Homebrew in the setup.
        let use_homebrew = settings.use_homebrew && toolchain::homebrew().is_some();
        toolchain.update(cx, |toolchain, _| toolchain.use_homebrew = use_homebrew);
        // Nothing of the editor on disk means that it was just installed.
        let first_start = settings_path.as_deref().is_some_and(|path| !path.exists())
            && projects::registry_path().is_none_or(|path| !path.exists());
        let settings_migrated = !first_start && !settings.first_run_done;
        if settings_migrated {
            // People who used the editor before the setup existed skip it and the tour.
            settings.first_run_done = true;
            settings.tour_done = true;
        }
        apply_look(&settings);
        let mover = cx.new(|_| Mover::new(settings.move_declined));
        let _mover_subscriptions = [
            cx.observe(&mover, |_, _, cx| cx.notify()),
            cx.subscribe(&mover, |this, _, event, cx| match event {
                MoverEvent::Declined => {
                    this.settings.move_declined = true;
                    this.save_settings(cx);
                }
            }),
        ];
        #[cfg(feature = "collab")]
        let collab = {
            let name = if settings.collab_name.is_empty() { collab_session::default_name() } else { settings.collab_name.clone() };
            collab_session::CollabUi::new(&name, cx)
        };
        let mut workspace = Self {
            root: None,
            entries: Vec::new(),
            collapsed: HashSet::new(),
            documents: Vec::new(),
            active: None,
            outline: Vec::new(),
            outline_version: (usize::MAX, 0),
            symbols_version: (usize::MAX, 0, 0),
            package_hints: Vec::new(),
            loaded_packages: Arc::default(),
            package_extra: Arc::default(),
            package_check_task: None,
            conflict_task: None,
            recovery_task: None,
            recovery_offer_task: None,
            recovered: HashMap::new(),
            show_file_panel: true,
            left_panel: LeftPanel::Files,
            git_panel,
            _git_subscription: git_subscription,
            installed_addons: Vec::new(),
            addon_panels: Vec::new(),
            addon_dialog_focus: cx.focus_handle(),
            addon_dialog_changed: false,
            addon_dialog_open: false,
            catalog: CatalogState::Idle,
            installing: None,
            tracked_edits: HashMap::new(),
            compile_waiters: Vec::new(),
            addon_context_sent: addon_host::AddonContext::default(),
            addon_context_pending: None,
            addon_context_task: None,
            addon_changed: HashSet::new(),
            addon_changed_task: None,
            focus_requested: false,
            proposals: Vec::new(),
            layout: PaneLayout::Split,
            pdf_tab_shown: false,
            focus_mode: false,
            home,
            setup: None,
            updater,
            _updater_observer,
            mover,
            _mover_subscriptions,
            texlive,
            _texlive_observer,
            _texlive_events,
            setup_requested: false,
            export_after_pandoc: None,
            tour: None,
            pending_open: None,
            tour_anchors: Rc::default(),
            toolchain,
            _toolchain_subscriptions: toolchain_subscriptions,
            show_hub: false,
            logo_hovered: false,
            _home_subscription: home_subscription,
            presenting: None,
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
            recent_messages: VecDeque::new(),
            report_field: cx.new(|cx| {
                let mut view = EditorView::new("", None, cx);
                view.set_plain();
                view
            }),
            report_with_details: true,
            report_preview: false,
            report_error: None,
            report_snapshot: None,
            crash_report: problem_report::take_crash(),
            pdf,
            markdown,
            markdown_task: None,
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
            build_queued: false,
            engine: None,
            synctex: None,
            sync_on_click: true,
            follow_line: None,
            follow_task: None,
            images: Vec::new(),
            shown_image: None,
            export_running: false,
            last_build_succeeded: false,
            word_count: None,
            word_count_task: None,
            last_pdf: None,
            last_main: None,
            pdf_main: None,
            settings,
            settings_path,
            keymap: Keymap::default(),
            vim_mappings: Mappings::default(),
            vim_mapper: Mapper::default(),
            vim_map_timer: None,
            recording_shortcut: None,
            dialog: None,
            dialog_focus: cx.focus_handle(),
            storage_usage: None,
            storage_task: None,
            picker: None,
            citation_trigger: None,
            citations: Arc::default(),
            citations_ready: false,
            citation_cache: Arc::default(),
            citation_texts: HashMap::new(),
            citations_task: None,
            citations_stale: false,
            open_citations_soon: false,
            definition_soon: None,
            picker_focus: cx.focus_handle(),
            picker_scroll: ScrollHandle::new(),
            picker_files_task: None,
            tree_focus: cx.focus_handle(),
            tree_scroll: UniformListScrollHandle::new(),
            tree_cursor: 0,
            show_tree: true,
            tree_target: None,
            tree_input: None,
            tree_menu_path: None,
            tree_error: None,
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
            _markdown_subscription: markdown_subscription,
            _watcher: None,
            _watch_task: None,
            refresh_task: None,
            refresh_again: false,
            #[cfg(feature = "collab")]
            collab,
            git: git_sync::GitUi::default(),
        };
        workspace.reload_addons(cx);
        workspace.reload_keymap(cx);
        workspace.reload_vimrc(cx);
        if settings_migrated {
            workspace.save_settings(cx);
        }
        if first_start {
            workspace.run_setup(window, cx);
        }
        match root {
            Some(path) => workspace.open_path(path, window, cx),
            None => {
                let home = workspace.home.clone();
                let (vim_mode, cards) = (workspace.settings.vim_mode, workspace.settings.hub_cards);
                cx.defer_in(window, move |_, window, cx| {
                    home.update(cx, |home, cx| {
                        home.set_context(false, vim_mode, cards);
                        home.show(window, cx);
                    })
                });
            }
        }
        workspace
    }

    /// Adds the `editor` command for the terminal. The password prompt of
    /// macOS can take a while, so this runs off the main thread.
    fn install_shell_command(&mut self, cx: &mut Context<Self>) {
        let install = cx.background_spawn(async { crate::shell_command::install() });
        cx.spawn(async move |this, cx| {
            let message = match install.await {
                Ok(crate::shell_command::Outcome::Installed(_)) => "Added. In a new Terminal window, editor . opens the folder you are in.".to_string(),
                Ok(crate::shell_command::Outcome::AlreadyThere(link)) => format!("The editor command is already there, at {}.", link.display()),
                Ok(crate::shell_command::Outcome::NeedsAdmin) => "The editor command was not added.".to_string(),
                Err(error) => error,
            };
            this.update(cx, |this, cx| this.set_status(message, cx)).ok();
        })
        .detach();
    }

    /// Opens a folder as the project, or a file in its project. Finder, the
    /// `editor` command and the command line all come here.
    pub fn open_path(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let path = match path.canonicalize().map(crate::platform::plain_path) {
            Ok(path) => path,
            Err(_) => return self.set_status(format!("Cannot find {}.", path.display()), cx),
        };
        if self.setup.is_some() {
            self.pending_open = Some(path);
            return;
        }
        let (root, file) = if path.is_dir() {
            (path, None)
        } else {
            let home = crate::platform::home().unwrap_or_else(|| PathBuf::from("/"));
            (projects::project_root_for(&path, self.root.as_deref(), &home), Some(path))
        };
        if self.root.as_deref() != Some(root.as_path()) {
            self.open_root(root.clone(), window, cx);
            // Unsaved text that could not be saved keeps the old project open.
            if self.root.as_deref() != Some(root.as_path()) {
                return;
            }
        }
        self.show_hub = false;
        match file {
            Some(file) => self.open_file(file, window, cx),
            None => self.focus_main(window, cx),
        }
        cx.notify();
    }

    fn hub_shown(&self) -> bool {
        (self.show_hub || self.root.is_none()) && self.presenting.is_none()
    }

    fn open_hub(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.open_menu.is_some() {
            self.close_menu(window, cx);
        }
        self.show_hub = true;
        let (has_project, vim_mode, cards) = (self.root.is_some(), self.settings.vim_mode, self.settings.hub_cards);
        self.home.update(cx, |home, cx| {
            home.set_context(has_project, vim_mode, cards);
            home.show(window, cx);
        });
        cx.notify();
    }

    fn close_hub(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.root.is_none() {
            return;
        }
        self.show_hub = false;
        self.focus_main(window, cx);
        cx.notify();
    }

    fn save_project_as_template(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.root.clone() else {
            return;
        };
        // The copy reads the disk, so the edits have to be there.
        if !self.save_all(cx) {
            return;
        }
        self.open_hub(window, cx);
        self.home.update(cx, |home, cx| home.open_save_template(root, window, cx));
    }

    fn toggle_hub(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.hub_shown() { self.close_hub(window, cx) } else { self.open_hub(window, cx) }
    }

    /// The open project went to the trash, so nothing may write into it any more.
    fn forget_trashed_root(&mut self, path: &Path, cx: &mut Context<Self>) {
        if self.root.as_deref() != Some(path) {
            return;
        }
        self.cancel_refresh();
        self._watcher = None;
        self._watch_task = None;
        self.documents.clear();
        self.active = None;
        self.images.clear();
        self.shown_image = None;
        self.entries.clear();
        self.tree_input = None;
        self.root = None;
        self.git_branch = None;
        self.git_panel.update(cx, |panel, cx| panel.set_root(None, cx));
        self.addon_project_closed(cx);
        for entry in &self.addon_panels {
            entry.panel.update(cx, |panel, cx| panel.set_root(None, cx));
        }
        let (vim_mode, cards) = (self.settings.vim_mode, self.settings.hub_cards);
        self.home.update(cx, |home, _| home.set_context(false, vim_mode, cards));
        cx.notify();
    }

    fn open_root(&mut self, root: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        // The open documents go away here, so unsaved text must reach the disk first.
        if !self.save_all(cx) {
            return;
        }
        #[cfg(feature = "collab")]
        self.collab_root_changing(&root, cx);
        self.cancel_refresh();
        self.entries = file_tree::scan(&root, &self.collapsed);
        self.tree_target = None;
        self.tree_input = None;
        self.tree_error = None;
        self.documents.clear();
        self.active = None;
        self.images.clear();
        self.shown_image = None;
        self.start_watching(&root, cx);
        self.git_branch = git_branch(&root);
        // Before the add-ons restart for the new project, with the old one.
        if self.root.is_some() {
            self.addon_project_closed(cx);
        }
        self.root = Some(root.clone());
        self.citations = Arc::default();
        self.citations_ready = false;
        self.citation_cache = Arc::default();
        self.schedule_citations(cx);
        let (vim_mode, cards) = (self.settings.vim_mode, self.settings.hub_cards);
        self.home.update(cx, |home, cx| {
            home.touch(&root, cx);
            home.set_context(true, vim_mode, cards);
        });
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
        if !self.config.trigger_set && self.settings.compile_while_typing {
            self.config.trigger = Trigger::Type;
        }

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
            // A build of the project before must not finish into this one.
            if self.markdown_view(cx).is_some() {
                self.build.cancel();
                self.build_task = None;
                self.building = false;
            } else {
                self.recompile(cx);
            }
        }
        self.forget_saved_recovery(cx);
        let swept = root.clone();
        cx.background_spawn(async move { crate::atomic::sweep(&swept) }).detach();
        self.git_root_opened(cx);
        #[cfg(feature = "collab")]
        self.collab_root_opened(&root, cx);
        self.offer_recovery(root, window, cx);
        cx.notify();
    }

    fn start_watching(&mut self, root: &Path, cx: &mut Context<Self>) {
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel::<()>();
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
            // Sleeps until something changes, then takes what follows in the
            // same burst, such as a checkout, in one refresh.
            while receiver.recv().await.is_some() {
                cx.background_executor().timer(WATCH_SETTLE).await;
                while receiver.try_recv().is_ok() {}
                if this.update(cx, |this, cx| this.refresh_in_background(cx)).is_err() {
                    return;
                }
            }
        }));
    }

    /// For a change from outside. A big project takes a while to walk, which must
    /// not stop typing, and changes during a walk lead to one more walk afterward.
    fn refresh_in_background(&mut self, cx: &mut Context<Self>) {
        if self.refresh_task.is_some() {
            self.refresh_again = true;
            return;
        }
        let root = self.root.clone();
        let collapsed = self.collapsed.clone();
        let documents: Vec<_> = self
            .documents
            .iter()
            .filter_map(|document| {
                let view = document.view.read(cx);
                let path = view.path.clone().filter(|_| !view.state.is_dirty())?;
                Some((document.view.clone(), path, view.state.version()))
            })
            .collect();
        let paths: Vec<PathBuf> = documents.iter().map(|(_, path, _)| path.clone()).collect();
        self.refresh_task = Some(cx.spawn(async move |this, cx| {
            // Open files come first, since the walk can take much longer.
            let texts: Vec<std::io::Result<text_file::Loaded>> =
                cx.background_spawn(async move { paths.iter().map(|path| text_file::read(path)).collect() }).await;
            let mut gone = Vec::new();
            cx.update(|cx| {
                for ((view, path, version), loaded) in documents.iter().zip(&texts) {
                    match loaded {
                        Ok(loaded) => view.update(cx, |view, cx| view.reload_text(&loaded.text, loaded.encoding, *version, cx)),
                        // Another program deleted or moved the file. The text stays
                        // and counts as unsaved, so closing asks and a crash keeps it.
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                            view.update(cx, |view, cx| view.mark_file_gone(cx));
                            gone.push(path.file_name().unwrap_or_default().to_string_lossy().into_owned());
                        }
                        Err(_) => {}
                    }
                }
            });
            if !gone.is_empty() {
                this.update(cx, |this, cx| {
                    this.set_status(format!("{} was deleted outside the editor. The text is still here; save to keep it.", gone.join(", ")), cx);
                    this.schedule_recovery(cx);
                })
                .ok();
            }
            let entries = cx.background_spawn(async move { root.map(|root| file_tree::scan(&root, &collapsed)) }).await;
            this.update(cx, |this, cx| {
                this.refresh_task = None;
                if let Some(entries) = entries {
                    this.entries = entries;
                }
                this.git_panel.update(cx, |panel, cx| panel.refresh(cx));
                this.refresh_git(cx);
                this.schedule_citations(cx);
                cx.notify();
                if std::mem::take(&mut this.refresh_again) {
                    this.refresh_in_background(cx);
                }
            })
            .ok();
        }));
    }

    /// A walk that is still running is older than what comes next, so it must not
    /// overwrite it.
    fn cancel_refresh(&mut self) {
        self.refresh_task = None;
        self.refresh_again = false;
    }

    fn refresh_from_disk(&mut self, cx: &mut Context<Self>) {
        self.cancel_refresh();
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
        let loaded = match text_file::read(&path) {
            Ok(loaded) => loaded,
            Err(error) => {
                self.status = Some(format!("Cannot open {}: {error}", path.display()).into());
                cx.notify();
                return;
            }
        };
        if loaded.encoding != text_file::UTF_8 {
            self.status = Some(format!("{} is stored as {} and is saved that way.", path.file_name().unwrap_or_default().to_string_lossy(), loaded.encoding.name()).into());
        }
        self.add_document(path, &loaded.text, cx);
        if let Some(view) = self.documents.last().map(|document| document.view.clone()) {
            view.update(cx, |view, _| view.set_encoding(loaded.encoding));
        }
        self.activate(self.documents.len() - 1, window, cx);
        self.apply_marks(cx);
    }

    /// Opens a tab without switching to it.
    fn add_document(&mut self, path: PathBuf, text: &str, cx: &mut Context<Self>) {
        let settings = self.settings.clone();
        let (loaded, extra) = (self.loaded_packages.clone(), self.package_extra.clone());
        let view = cx.new(|cx| {
            let mut view = EditorView::new(text, Some(path), cx);
            view.set_vim_mode(settings.vim_mode, settings.smooth_cursor, cx);
            view.set_auto_suggestions(settings.auto_suggestions, cx);
            view.set_loaded_packages(loaded, extra);
            view
        });
        let subscription = cx.subscribe(&view, |this, view, event, cx| match event {
            EditorEvent::Dropped(item) => this.drop_into(&view, item.clone(), cx),
            EditorEvent::Edited => {
                #[cfg(feature = "collab")]
                this.collab_edited(&view, cx);
                this.schedule_recovery(cx);
                this.schedule_type_build(cx);
                this.schedule_markdown(cx);
                if view.read(cx).path.as_deref().is_some_and(citation_index::matters) {
                    this.schedule_citations(cx);
                }
                this.check_citation_trigger(&view, cx);
                this.addon_document_changed(&view, cx);
                cx.notify();
            }
            EditorEvent::EditedByOthers => {
                this.addon_document_changed(&view, cx);
                this.schedule_recovery(cx);
                this.schedule_type_build(cx);
                this.schedule_markdown(cx);
                cx.notify();
            }
            EditorEvent::CursorClicked { line } => {
                if this.sync_on_click {
                    this.forward_search(*line, false, cx);
                }
            }
            EditorEvent::GoToDefinition { path, range, line } => this.go_to_definition(path.clone(), range.clone(), *line, cx),
        });
        let observer = cx.observe(&view, |this, view, cx| {
            this.cursor_changed(&view, cx);
            #[cfg(feature = "collab")]
            this.collab_cursor(&view, cx);
        });
        self.documents.push(OpenDocument {
            view: view.clone(),
            _subscription: subscription,
            _observer: observer,
        });
        self.load_diff_base(&view, cx);
        #[cfg(feature = "collab")]
        if self.collab.active() {
            self.collab_document_opened(&view, cx);
        }
    }

    fn open_image(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(index) = self.images.iter().position(|image| image.view.read(cx).path() == path) {
            self.show_image(index, window, cx);
            return;
        }
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
        self.images.push(ImagePreview { view, after: self.documents.len(), _subscription: subscription });
        self.show_image(self.images.len() - 1, window, cx);
    }

    fn show_image(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.shown_image = Some(index);
        self.pdf_tab_shown = false;
        let focus = self.images[index].view.read(cx).focus_handle().clone();
        window.focus(&focus, cx);
        cx.notify();
    }

    fn image_preview(&self) -> Option<&ImagePreview> {
        self.shown_image.and_then(|index| self.images.get(index))
    }

    fn tabs(&self) -> Vec<Tab> {
        let after: Vec<usize> = self.images.iter().map(|image| image.after).collect();
        let mut tabs = tab_order(self.documents.len(), &after);
        if self.layout == PaneLayout::PdfTab {
            tabs.push(Tab::Pdf);
        }
        tabs
    }

    fn active_tab(&self) -> Option<Tab> {
        if self.pdf_tab_shown {
            return Some(Tab::Pdf);
        }
        self.shown_image.map(Tab::Image).or(self.active.map(Tab::Document))
    }

    fn activate_tab(&mut self, tab: Tab, window: &mut Window, cx: &mut Context<Self>) {
        match tab {
            Tab::Document(index) => self.activate(index, window, cx),
            Tab::Image(index) => self.show_image(index, window, cx),
            Tab::Pdf => {
                self.pdf_tab_shown = true;
                self.focus_main(window, cx);
                cx.notify();
            }
        }
    }

    fn close_tab(&mut self, tab: Tab, window: &mut Window, cx: &mut Context<Self>) {
        match tab {
            Tab::Document(index) => self.close_document(index, window, cx),
            Tab::Image(index) => self.close_image(index, window, cx),
            Tab::Pdf => self.set_layout(PaneLayout::Split, window, cx),
        }
    }

    fn go_to_tab(&mut self, position: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(tab) = self.tabs().get(position).copied() {
            self.activate_tab(tab, window, cx);
        }
    }

    /// A click on the close button of another tab must not take the keys away.
    fn focus_shown_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pdf_tab_shown {
            window.focus(&self.pdf.read(cx).focus_handle(), cx);
            return;
        }
        match self.image_preview().map(|image| image.view.read(cx).focus_handle().clone()) {
            Some(focus) => window.focus(&focus, cx),
            None => self.focus_active(window, cx),
        }
    }

    fn close_image(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let position = self.tabs().iter().position(|tab| *tab == Tab::Image(index)).unwrap_or(0);
        self.images.remove(index);
        match self.shown_image {
            Some(shown) if shown == index => {
                self.shown_image = None;
                let tabs = self.tabs();
                match tabs.get(position).or(tabs.last()).copied() {
                    Some(tab) => self.activate_tab(tab, window, cx),
                    None => self.focus_active(window, cx),
                }
            }
            Some(shown) => {
                if shown > index {
                    self.shown_image = Some(shown - 1);
                }
                self.focus_shown_tab(window, cx);
            }
            None => self.focus_active(window, cx),
        }
        cx.notify();
    }

    /// Keeps the images in their place when the document at `index` goes away.
    fn move_images_for_removed_document(&mut self, index: usize) {
        for image in &mut self.images {
            if image.after > index {
                image.after -= 1;
            }
        }
    }

    fn activate(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.shown_image = None;
        self.pdf_tab_shown = false;
        self.follow_line = None;
        self.active = Some(index);
        let view = self.documents[index].view.clone();
        window.focus(&view.read(cx).focus_handle(cx), cx);
        self.compile_if_document_changed(cx);
        self.refresh_markdown(cx);
        self.schedule_citations(cx);
        // The others see the switch, also before the cursor moves.
        #[cfg(feature = "collab")]
        self.collab_cursor(&view, cx);
        cx.notify();
    }

    fn close_document(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let view = self.documents[index].view.clone();
        match view.update(cx, |view, cx| view.save(cx)) {
            Ok(_) => self.remove_document(index, window, cx),
            Err(error) if is_save_conflict(&error) => self.ask_about_conflict(view, true, window, cx),
            Err(error) => self.set_status(save_failed(&error), cx),
        }
        self.forget_saved_recovery(cx);
    }

    /// Lets the author decide, since either version could be the one that matters.
    fn ask_about_conflict(&mut self, view: Entity<EditorView>, close: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.conflict_task.is_some() {
            return;
        }
        let title = view.read(cx).title();
        self.conflict_task = Some(cx.spawn_in(window, async move |this, cx| {
            let message = format!("{title} changed on disk");
            let detail = "Another program changed this file after you last opened or saved it.";
            let answers = ["Keep my version", "Load from disk", "Save a copy", "Cancel"];
            let answer = ask(&this, cx, PromptLevel::Warning, &message, detail, &answers).await;
            let result = match answer {
                Some(0) => view.update(cx, |view, cx| view.save_anyway(cx)).map(|()| true),
                Some(1) => view.update(cx, |view, cx| view.load_from_disk(cx)).map(|()| true),
                Some(2) => Self::save_conflict_copy(&this, &view, cx).await,
                _ => Ok(false),
            };
            this.update_in(cx, |this, window, cx| {
                this.conflict_task = None;
                match result {
                    Ok(true) => {
                        this.notify_addons_saved(&view, cx);
                        if close && let Some(index) = this.documents.iter().position(|document| document.view == view) {
                            this.remove_document(index, window, cx);
                        } else if this.config.trigger == Trigger::Save {
                            this.build_when_free(cx);
                        }
                    }
                    Ok(false) => {}
                    Err(error) => this.set_status(save_failed(&error), cx),
                }
                this.forget_saved_recovery(cx);
            })
            .ok();
        }));
    }

    /// My text goes to a new file, and the document shows what is on disk. Both stay.
    async fn save_conflict_copy(
        this: &WeakEntity<Self>,
        view: &Entity<EditorView>,
        cx: &mut AsyncWindowContext,
    ) -> std::io::Result<bool> {
        let Some(path) = view.read_with(cx, |view, _| view.path.clone()) else {
            return Ok(false);
        };
        let directory = path.parent().unwrap_or(Path::new(".")).to_path_buf();
        let stem = path.file_stem().unwrap_or_default().to_string_lossy();
        let name = match path.extension() {
            Some(extension) => format!("{stem} copy.{}", extension.to_string_lossy()),
            None => format!("{stem} copy"),
        };
        let Ok(answer) = this.update(cx, |_, cx| cx.prompt_for_new_path(&directory, Some(&name))) else {
            return Ok(false);
        };
        let Ok(Ok(Some(target))) = answer.await else {
            return Ok(false);
        };
        view.update(cx, |view, cx| view.save_copy(&target).and_then(|()| view.load_from_disk(cx)).map(|()| true))
    }

    /// Copies unsaved text a few seconds after the last pause, and does nothing
    /// while nobody types.
    fn schedule_recovery(&mut self, cx: &mut Context<Self>) {
        if self.recovery_task.is_some() {
            return;
        }
        let Some(directory) = recovery::directory() else {
            return;
        };
        self.recovery_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(RECOVERY_DELAY).await;
            let Ok(unsaved) = this.update(cx, |this, cx| this.take_unsaved(cx)) else {
                return;
            };
            cx.background_spawn(async move {
                for (path, buffer) in unsaved {
                    recovery::write(&directory, &path, &buffer.to_string()).ok();
                }
            })
            .await;
            this.update(cx, |this, cx| {
                this.recovery_task = None;
                this.forget_saved_recovery(cx);
                // Typing during the write needs another copy.
                if this.documents.iter().any(|document| this.needs_recovery_copy(&document.view, cx)) {
                    this.schedule_recovery(cx);
                }
            })
            .ok();
        }));
    }

    fn needs_recovery_copy(&self, view: &Entity<EditorView>, cx: &App) -> bool {
        let view = view.read(cx);
        view.state.is_dirty()
            && view.path.as_ref().is_some_and(|path| self.recovered.get(path) != Some(&view.state.version()))
    }

    /// The text to copy. Cloning a buffer is cheap, and the text is built elsewhere.
    fn take_unsaved(&mut self, cx: &App) -> Vec<(PathBuf, editor_core::Buffer)> {
        let mut unsaved = Vec::new();
        for document in &self.documents {
            if self.needs_recovery_copy(&document.view, cx) {
                let view = document.view.read(cx);
                let path = view.path.clone().expect("only documents with a path need a copy");
                self.recovered.insert(path.clone(), view.state.version());
                unsaved.push((path, view.state.buffer().clone()));
            }
        }
        unsaved
    }

    /// Removes copies of text that is safe on disk now.
    fn forget_saved_recovery(&mut self, cx: &App) {
        if self.recovered.is_empty() {
            return;
        }
        let Some(directory) = recovery::directory() else {
            return;
        };
        let documents = &self.documents;
        self.recovered.retain(|path, _| {
            let unsaved = documents.iter().map(|document| document.view.read(cx)).any(|view| {
                view.path.as_ref() == Some(path) && view.state.is_dirty()
            });
            if !unsaved {
                recovery::remove(&directory, path);
            }
            unsaved
        });
    }

    fn offer_recovery(&mut self, root: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let Some(directory) = recovery::directory() else {
            return;
        };
        self.recovery_offer_task = Some(cx.spawn_in(window, async move |this, cx| {
            let found = directory.clone();
            let snapshots = cx.background_spawn(async move { recovery::pending(&found, &root) }).await;
            if snapshots.is_empty() {
                return;
            }
            let names: Vec<String> = snapshots
                .iter()
                .map(|snapshot| snapshot.path.file_name().unwrap_or_default().to_string_lossy().into_owned())
                .collect();
            let detail = format!("The editor ended without saving {}.", names.join(", "));
            let answers = ["Recover", "Discard", "Decide later"];
            match ask(&this, cx, PromptLevel::Warning, "Recover unsaved changes?", &detail, &answers).await {
                Some(0) => {
                    this.update_in(cx, |this, window, cx| this.restore_snapshots(snapshots, window, cx)).ok();
                }
                Some(1) => {
                    let history = history::directory();
                    for snapshot in &snapshots {
                        // Discarded here, still one click away in Earlier versions.
                        if let Some(history) = &history {
                            history::keep(history, &snapshot.path, snapshot.text.as_bytes()).ok();
                        }
                        recovery::remove(&directory, &snapshot.path);
                    }
                }
                _ => {}
            }
        }));
    }

    /// The text comes back as unsaved edits, so that undo shows what is on disk.
    fn restore_snapshots(&mut self, snapshots: Vec<recovery::Snapshot>, window: &mut Window, cx: &mut Context<Self>) {
        for snapshot in snapshots {
            if !snapshot.path.exists() {
                let written = snapshot.path.parent().map_or(Ok(()), std::fs::create_dir_all);
                if let Err(error) = written.and_then(|()| crate::atomic::write(&snapshot.path, b"")) {
                    self.set_status(format!("Cannot recover {}: {error}", snapshot.path.display()), cx);
                    continue;
                }
            }
            self.open_file(snapshot.path.clone(), window, cx);
            let Some(index) = self.document_at(&snapshot.path, cx) else {
                continue;
            };
            let view = self.documents[index].view.clone();
            view.update(cx, |view, cx| {
                let everything = 0..view.state.buffer().len();
                view.edit_from_outside(everything, &snapshot.text, cx);
            });
            // Its copy goes away with the next save, like any other.
            self.recovered.insert(snapshot.path, 0);
        }
    }

    fn remove_document(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let closed_active = self.active == Some(index);
        self.documents.remove(index);
        self.move_images_for_removed_document(index);
        self.active = match self.active {
            Some(active) if active == index => self.documents.len().checked_sub(1).map(|last| index.min(last)),
            Some(active) if active > index => Some(active - 1),
            other => other,
        };
        // Another tab stays as it is, also when it is an image with this document below it.
        if !closed_active || self.shown_image.is_some() {
            self.focus_shown_tab(window, cx);
            cx.notify();
            return;
        }
        if let Some(active) = self.active {
            self.activate(active, window, cx);
        } else if let Some(last) = self.images.len().checked_sub(1) {
            self.show_image(last, window, cx);
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

    fn save_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(view) = self.active_view() else {
            return;
        };
        match view.update(cx, |view, cx| view.save(cx)) {
            Ok(saved) => {
                if saved {
                    self.notify_addons_saved(&view, cx);
                    self.reload_key_files(&view, cx);
                }
                if saved && self.config.trigger == Trigger::Save {
                    self.build_when_free(cx);
                }
            }
            Err(error) if is_save_conflict(&error) => self.ask_about_conflict(view, false, window, cx),
            Err(error) => self.set_status(save_failed(&error), cx),
        }
        self.forget_saved_recovery(cx);
    }

    // Shortcuts

    /// Binds the shortcuts of `keymap.json`, or the defaults without it.
    fn reload_keymap(&mut self, cx: &mut Context<Self>) {
        let (keymap, mut problems) = keymap::default_path().map(|path| Keymap::load(&path)).unwrap_or_default();
        problems.extend(crate::actions::bind_keys(&keymap, &self.addon_bindings(cx), cx));
        self.keymap = keymap;
        if let Some(problem) = problems.first() {
            self.set_status(format!("keymap.json: {problem}"), cx);
        }
        cx.notify();
    }

    /// Saves and binds the shortcuts changed on the shortcuts page.
    fn apply_keymap(&mut self, keymap: Keymap, cx: &mut Context<Self>) {
        let problems = crate::actions::bind_keys(&keymap, &self.addon_bindings(cx), cx);
        self.keymap = keymap;
        if let Some(path) = keymap::default_path()
            && let Err(error) = self.keymap.save(&path)
        {
            self.set_status(format!("Cannot save keymap.json: {error}"), cx);
        } else if let Some(problem) = problems.first() {
            self.set_status(format!("keymap.json: {problem}"), cx);
        }
        cx.notify();
    }

    fn reload_vimrc(&mut self, cx: &mut Context<Self>) {
        let text = settings::vimrc_path().and_then(|path| std::fs::read_to_string(path).ok()).unwrap_or_default();
        let (mappings, problems) = Mappings::parse(&text);
        self.vim_mappings = mappings;
        self.vim_mapper.reset();
        if let Some(problem) = problems.first() {
            self.set_status(format!("vimrc line {}: {}", problem.line, problem.message), cx);
        }
    }

    /// Takes up a saved `keymap.json` or `vimrc` at once.
    fn reload_key_files(&mut self, view: &Entity<EditorView>, cx: &mut Context<Self>) {
        let path = view.read(cx).path.clone();
        if path.is_some() && path == keymap::default_path() {
            self.reload_keymap(cx);
            self.set_status("Shortcuts reloaded", cx);
        } else if path.is_some() && path == settings::vimrc_path() {
            self.reload_vimrc(cx);
            if self.status.is_none() {
                self.set_status(format!("{} vim mappings loaded", self.vim_mappings.len()), cx);
            }
        }
    }

    /// The first shortcut of a command, for a menu.
    fn shortcut(&self, id: &str) -> SharedString {
        self.keymap.label(id, keymap::host_os()).into()
    }

    fn set_status(&mut self, message: impl Into<SharedString>, cx: &mut Context<Self>) {
        let message = message.into();
        if self.recent_messages.len() == problem_dialog::KEPT_MESSAGES {
            self.recent_messages.pop_front();
        }
        self.recent_messages.push_back(message.to_string());
        self.status = Some(message);
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
        if !addons::ENABLED {
            return;
        }
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
        // Add-ons that were turned off or removed: their program ends with
        // the panel, and nothing of them stays behind.
        for entry in old {
            self.end_proposals_of(&entry.panel, cx);
            self.compile_waiters.retain(|(panel, _)| panel != &entry.panel);
        }
        if matches!(self.left_panel, LeftPanel::Addon(_)) {
            let index = self.addon_panels.iter().position(|entry| Some(&entry.panel.read(cx).add_on.manifest.id) == shown.as_ref());
            self.left_panel = index.map_or(LeftPanel::Files, LeftPanel::Addon);
        }
        self.installed_addons = installed;
        // The keys of add-ons that were turned on come, and of others go.
        let problems = crate::actions::bind_keys(&self.keymap, &self.addon_bindings(cx), cx);
        if let Some(problem) = problems.first() {
            self.set_status(format!("keymap.json: {problem}"), cx);
        }
        cx.notify();
    }

    fn new_addon_entry(&self, add_on: Installed, cx: &mut Context<Self>) -> AddonEntry {
        let root = self.root.clone();
        let panel = cx.new(|_| AddonPanel::new(add_on, root));
        let subscription = cx.subscribe(&panel, |this, panel, event, cx| {
            match event {
                AddonPanelEvent::Request { id, request } => this.handle_addon_request(panel, *id, request.clone(), cx),
                AddonPanelEvent::DialogChanged => {
                    this.addon_dialog_changed = true;
                    cx.notify();
                }
                AddonPanelEvent::LookupFailed(reason) => this.set_status(reason.clone(), cx),
                AddonPanelEvent::Notify(text) => this.set_status(text.clone(), cx),
                AddonPanelEvent::Stopped => this.end_proposals_of(&panel, cx),
                AddonPanelEvent::InsertText(text) => {
                    if this.active_view().is_some() {
                        this.with_active(cx, |view, cx| view.insert_text(text, cx));
                    } else {
                        this.set_status("Open a document to insert into.", cx);
                    }
                }
            }
        });
        panel.update(cx, |panel, cx| panel.activate(cx));
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

    fn fetch_catalog(&mut self, cx: &mut Context<Self>) {
        self.catalog = CatalogState::Loading;
        cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async move { marketplace::fetch() }).await;
            this.update(cx, |this, cx| {
                this.catalog = match result {
                    Ok(entries) => CatalogState::Ready(entries),
                    Err(error) => CatalogState::Failed(error),
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn install_from_catalog(&mut self, entry: CatalogEntry, cx: &mut Context<Self>) {
        let Some(directory) = addons::default_directory() else {
            return self.set_status("The add-on folder is unknown, because HOME is not set.", cx);
        };
        if self.installing.is_some() {
            return;
        }
        let previous = self.installed_addons.iter().find(|add_on| add_on.manifest.id == entry.id).cloned();
        self.installing = Some(entry.id.clone());
        cx.spawn(async move |this, cx| {
            let name = entry.name.clone();
            let updating = previous.is_some();
            let result = cx.background_spawn(async move { marketplace::install(&entry, &directory, previous.as_ref()) }).await;
            this.update(cx, |this, cx| {
                this.installing = None;
                let message = match result {
                    Ok(true) => format!("Updated {name}."),
                    Ok(false) if updating => format!("Updated {name}. It is turned off, so check its permissions and turn it on."),
                    Ok(false) => format!("Installed {name}. Turn it on to use it."),
                    Err(error) => error,
                };
                this.set_status(message, cx);
                this.reload_addons(cx);
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn addon_file(&self, relative: &str) -> Result<PathBuf, String> {
        let root = self.root.as_deref().ok_or("No project is open.")?;
        addons::resolve_path(root, relative)
    }

    fn document_at(&self, path: &Path, cx: &App) -> Option<usize> {
        let wanted = path.canonicalize().map(crate::platform::plain_path).ok();
        self.documents.iter().position(|document| {
            let open = document.view.read(cx).path.as_deref();
            open == Some(path) || (wanted.is_some() && open.and_then(|open| open.canonicalize().map(crate::platform::plain_path).ok()) == wanted)
        })
    }

    fn relative_to_project(&self, path: &Path) -> String {
        let root = self.root.as_deref().and_then(|root| root.canonicalize().map(crate::platform::plain_path).ok());
        let real = path.canonicalize().map(crate::platform::plain_path).unwrap_or_else(|_| path.to_path_buf());
        root.and_then(|root| real.strip_prefix(root).ok().map(|inner| inner.display().to_string()))
            .unwrap_or_else(|| path.display().to_string())
    }

    fn diagnostic_infos(&self) -> Vec<DiagnosticInfo> {
        self.diagnostics
            .iter()
            .map(|diagnostic| {
                let explanation = build::explain(diagnostic);
                let hint = [explanation.as_ref().map(|explanation| explanation.hint.clone()), diagnostic.hint.clone()]
                    .into_iter()
                    .flatten()
                    .filter(|hint| !hint.is_empty())
                    .collect::<Vec<_>>()
                    .join("\n");
                DiagnosticInfo {
                    path: diagnostic.file.as_deref().map(|file| self.relative_to_project(file)),
                    line: diagnostic.line,
                    error: diagnostic.severity == Severity::Error,
                    message: diagnostic.message.clone(),
                    column: diagnostic.column,
                    severity: if diagnostic.severity == Severity::Error { "error" } else { "warning" }.to_string(),
                    hint: (!hint.is_empty()).then_some(hint),
                    context: diagnostic.context.clone(),
                    fix: explanation.as_ref().and_then(|explanation| explanation.fix.as_ref()).map(addon_reports::fix_info),
                    explanation: explanation.map(|explanation| explanation.title),
                }
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
            Request::Compile { clean } => {
                if clean {
                    self.recompile_from_scratch(cx);
                } else {
                    self.recompile(cx);
                }
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
            Request::EditFile { path, old, new, group } => self.addon_edit(addon_host::edit_group(&add_on, group), &path, &old, &new, cx).map(|()| Reply::Done),
            Request::CreateFile { path, text, group } => self.addon_create(addon_host::edit_group(&add_on, group), &path, &text, cx).map(|()| Reply::Done),
            Request::WriteFile { path, text } => self.addon_write(&path, &text, cx).map(|()| Reply::Done),
            Request::InsertCitation { key } => Ok(Reply::Text { text: self.insert_citation(&key, false, cx) }),
            Request::BeginEdits { group } => {
                self.tracked_edits.insert(addon_host::edit_group(&add_on, group), TrackedEdits::default());
                Ok(Reply::Done)
            }
            Request::Reveal { path, line, column } => self.addon_reveal(&path, line, column, cx),
            Request::ProjectInfo => self.addon_project_info(cx),
            Request::BuildLog { max_bytes } => Ok(self.addon_build_log(max_bytes)),
            Request::PdfInfo => self.addon_pdf_info(cx),
            Request::PdfText { page } => return self.answer_from_pdf(panel, id, page, None, cx),
            Request::PdfPage { page, width } => return self.answer_from_pdf(panel, id, page, Some(addon_host::pdf_page_width(width)), cx),
            Request::BibEntries => return self.answer_bib_entries(panel, id, cx),
            Request::Outline => return self.answer_outline(panel, id, cx),
            Request::InsertText { text } => self.addon_insert_text(&text, cx),
            Request::InsertAt { path, line, column, text } => self.addon_insert_at(&path, line, column, &text, cx),
            Request::CopyIntoProject { source, dest } => return self.answer_copy_into_project(panel, id, source, dest, cx),
            Request::ProposeEdits { path, title, edits, text } => match self.start_proposal(panel.clone(), id, &path, title, edits, text, cx) {
                Some(result) => result,
                // The reply waits for the user.
                None => return,
            },
            Request::CancelReview { path } => self.cancel_proposal(&path, cx),
            Request::DiscardEdits { group } => {
                self.discard_addon_edits(&addon_host::edit_group(&add_on, group), cx);
                Ok(Reply::Done)
            }
            Request::CancelBuild => {
                if !self.building {
                    return panel.update(cx, |panel, _| panel.reply(id, Err("No build runs.".to_string())));
                }
                self.build.cancel();
                Ok(Reply::Done)
            }
            Request::OpenDocuments => Ok(self.addon_open_documents(cx)),
            Request::SaveFile { path } => self.addon_save(&path, cx),
            Request::RenameFile { from, to } => self.addon_rename(&from, &to, cx),
            Request::DeleteFile { path } => return self.answer_delete(panel, id, &path, cx),
            Request::Selection => Ok(self.addon_selection(cx)),
            Request::SourceToPdf { path, line } => self.addon_source_to_pdf(&path, line),
            Request::PdfToSource { page, x, y } => self.addon_pdf_to_source(page, x, y),
            Request::ApplyFix { id: fix } => self.addon_apply_fix(&fix, cx),
            Request::GitStatus | Request::GitDiff { .. } | Request::GitLog { .. } => return self.answer_from_git(panel, id, request, cx),
            Request::PackageHints => return self.answer_package_hints(panel, id, cx),
            Request::References => return self.answer_references(panel, id, cx),
            Request::WordCount { path } => return self.answer_word_count(panel, id, path, cx),
            Request::PdfOutline => return self.answer_pdf_outline(panel, id, cx),
            Request::PdfSearch { query } => return self.answer_pdf_search(panel, id, query, cx),
            Request::PdfSelection => Ok(Reply::Text { text: self.pdf.read(cx).selected_text().unwrap_or_default() }),
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

    fn addon_edit(&mut self, group: (String, String), relative: &str, old: &str, new: &str, cx: &mut Context<Self>) -> Result<(), String> {
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
        self.tracked_edits.entry(group).or_default().originals.entry(path).or_insert(text);
        view.update(cx, |view, cx| view.edit_from_outside(range, new, cx));
        cx.notify();
        Ok(())
    }

    fn addon_create(&mut self, group: (String, String), relative: &str, text: &str, cx: &mut Context<Self>) -> Result<(), String> {
        let path = self.addon_file(relative)?;
        if path.exists() {
            return Err(format!("{relative} already exists."));
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| format!("Cannot create {relative}: {error}"))?;
        }
        std::fs::write(&path, "").map_err(|error| format!("Cannot create {relative}: {error}"))?;
        self.tracked_edits.entry(group).or_default().created.push(path.clone());
        self.add_document(path, "", cx);
        let index = self.documents.len() - 1;
        if self.active.is_none() {
            self.active = Some(index);
        }
        self.documents[index].view.update(cx, |view, cx| view.edit_from_outside(0..0, text, cx));
        self.refresh_from_disk(cx);
        Ok(())
    }

    fn addon_write(&mut self, relative: &str, text: &str, cx: &mut Context<Self>) -> Result<(), String> {
        let path = self.addon_file(relative)?;
        let document = self.document_at(&path, cx).map(|index| self.documents[index].view.clone());
        if document.as_ref().is_some_and(|view| view.read(cx).state.is_dirty()) {
            return Err(format!("{relative} has unsaved changes."));
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| format!("Cannot write {relative}: {error}"))?;
        }
        crate::atomic::write(&path, text.as_bytes()).map_err(|error| format!("Cannot write {relative}: {error}"))?;
        if let Some(view) = document {
            view.update(cx, |view, cx| view.reload_from_disk(cx));
        }
        self.refresh_from_disk(cx);
        Ok(())
    }

    fn notify_addons_saved(&self, view: &Entity<EditorView>, cx: &mut Context<Self>) {
        let Some(path) = view.read(cx).path.as_deref().map(|path| self.relative_to_project(path)) else {
            return;
        };
        for entry in &self.addon_panels {
            entry.panel.read(cx).notify_saved(&path);
        }
    }

    /// Puts back the text from before the first edit, as one step that undo can
    /// reverse. Edits by the user in those files since then go with it.
    fn discard_addon_edits(&mut self, group: &(String, String), cx: &mut Context<Self>) {
        let Some(tracked) = self.tracked_edits.remove(group) else {
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
                self.move_images_for_removed_document(index);
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
        let mut all_saved = true;
        for index in 0..self.documents.len() {
            let view = self.documents[index].view.clone();
            match view.update(cx, |view, cx| view.save(cx)) {
                Ok(saved) => {
                    if saved {
                        self.notify_addons_saved(&view, cx);
                    }
                }
                // The other documents still get saved. This one waits for a decision.
                Err(error) if is_save_conflict(&error) => {
                    let title = view.read(cx).title();
                    self.set_status(format!("{title} changed on disk and was not saved. Save it to decide."), cx);
                    all_saved = false;
                }
                Err(error) => {
                    self.set_status(save_failed(&error), cx);
                    all_saved = false;
                }
            }
        }
        self.forget_saved_recovery(cx);
        all_saved
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
        let compiler = match engine {
            Engine::Typst => Tool::Typst,
            Engine::Pandoc => Tool::Pandoc,
            Engine::Tectonic => Tool::Tectonic,
            Engine::PdfLatex | Engine::LuaLatex => Tool::MacTex,
        };
        if let Some(pending) = self.toolchain.read(cx).pending(compiler) {
            // `tool_installed` compiles once it is there.
            self.last_build_succeeded = false;
            return self.set_status(pending, cx);
        }
        self.engine = Some(engine);
        self.last_main = Some(main.clone());
        self.build_queued = false;
        // The runner recovers from broken files by itself, and the files make the next
        // run much faster, so only a request from the author deletes them.
        let clean = std::mem::take(&mut self.clean_next_build);
        self.building = true;
        self.build_log.clear();
        self.status = Some(format!("Compiling with {}…", engine.label()).into());
        let mut events = self.build.start(BuildRequest { root, main, engine, clean });
        self.addon_build_started(cx);
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
        if self.markdown_view(cx).is_some() {
            return;
        }
        let (Some(root), Some(last_main)) = (&self.root, &self.last_main) else {
            return;
        };
        let active = self.active_view().and_then(|view| view.read(cx).path.clone());
        let main = build::find_main(root, &self.config, active.as_deref());
        if main.is_some_and(|main| main != *last_main) {
            self.recompile(cx);
        }
    }

    /// The active document when it is Markdown that the preview shows as a page,
    /// rather than through a PDF build. A compiler chosen in the settings wins.
    fn markdown_view(&self, cx: &App) -> Option<Entity<EditorView>> {
        if self.config.engine.is_some() || self.active_language(cx) != Some(Language::Markdown) {
            return None;
        }
        self.active_view()
    }

    fn schedule_markdown(&mut self, cx: &mut Context<Self>) {
        if self.markdown_view(cx).is_none() || self.markdown_task.is_some() {
            return;
        }
        self.markdown_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(MARKDOWN_DEBOUNCE).await;
            this.update(cx, |this, cx| {
                this.markdown_task = None;
                this.refresh_markdown(cx);
            })
            .ok();
        }));
    }

    /// Hands the active Markdown text to the preview, if it changed and shows.
    fn refresh_markdown(&mut self, cx: &mut Context<Self>) {
        let Some(view) = self.markdown_view(cx) else {
            return;
        };
        if self.focus_mode || self.layout == PaneLayout::EditorOnly {
            return;
        }
        let (id, version) = (view.entity_id(), view.read(cx).state.version());
        if self.markdown.read(cx).shows(id, version) {
            return;
        }
        let view = view.read(cx);
        let text = view.state.buffer().to_string();
        let path = view.path.clone();
        self.markdown.update(cx, |preview, cx| preview.show(id, version, path.as_deref(), &text, cx));
    }

    fn schedule_type_build(&mut self, cx: &mut Context<Self>) {
        if self.config.trigger != Trigger::Type {
            return;
        }
        let delay = Duration::from_millis(self.config.debounce_ms);
        self.debounce_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(delay).await;
            this.update(cx, |this, cx| this.build_when_free(cx)).ok();
        }));
    }

    /// For builds nobody asked for by hand. Stopping a run for every pause in typing
    /// would mean that a long document never gets a new preview.
    fn build_when_free(&mut self, cx: &mut Context<Self>) {
        if self.markdown_view(cx).is_some() {
            return;
        }
        if self.building {
            self.build_queued = true;
        } else {
            self.recompile(cx);
        }
    }

    /// `latexmk` repeats the old failure while no file changed, also if a package
    /// has been installed since. Asking again by hand means starting over then.
    fn recompile_by_hand(&mut self, cx: &mut Context<Self>) {
        let unchanged = !self.documents.iter().any(|document| document.view.read(cx).state.is_dirty());
        if unchanged && !self.last_build_succeeded && !self.building {
            self.clean_next_build = true;
        }
        self.recompile(cx);
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
                self.build_queued = false;
                self.set_status("Build stopped.", cx);
                self.reply_to_compile_waiters(|| Err("The build was stopped.".to_string()), cx);
            }
            BuildEvent::Finished(outcome) => self.finish_build(outcome, cx),
        }
    }

    fn finish_build(&mut self, outcome: BuildOutcome, cx: &mut Context<Self>) {
        self.building = false;
        // New numbers for the labels, and letters for the entries.
        self.schedule_citations(cx);
        if self.dialog == Some(Dialog::Settings(SettingsPage::Storage)) {
            self.measure_storage(cx);
        }
        self.status = Some(
            format!(
                "{} in {:.1} s",
                if outcome.success { "Compiled" } else { "Failed" },
                outcome.duration.as_secs_f32(),
            )
            .into(),
        );
        self.last_build_succeeded = outcome.success;
        self.diagnostics = outcome.diagnostics;
        self.diagnostics.sort_by_key(|diagnostic| diagnostic.severity != Severity::Error);
        self.problems = problems::problems(&self.diagnostics);
        self.error_cursor = None;
        self.error_card_dismissed = false;
        self.apply_marks(cx);
        self.check_packages(cx);
        let (success, diagnostics) = (outcome.success, self.diagnostic_infos());
        self.reply_to_compile_waiters(|| Ok(Reply::Compiled { success, diagnostics: diagnostics.clone() }), cx);
        self.addon_build_finished(success, cx);
        self.addon_diagnostics_changed(cx);
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
        if std::mem::take(&mut self.build_queued) {
            self.recompile(cx);
        }
        cx.notify();
    }

    /// Looks for missing and unused packages in the whole project, off the main
    /// thread. A result is dropped when a document has changed in the meantime,
    /// because the next build brings a new one.
    fn check_packages(&mut self, cx: &mut Context<Self>) {
        let (Some(root), Some(main)) = (self.root.clone(), self.last_main.clone()) else {
            return;
        };
        if Language::from_path(&main) != Language::Latex {
            self.package_check_task = None;
            self.show_package_hints(Vec::new(), Loaded::default(), Extra::default(), cx);
            return;
        }
        // The suggestions need what the project loads even without the hints.
        let show_hints = self.settings.package_hints;
        let open: Vec<(PathBuf, String, u64)> = self
            .documents
            .iter()
            .filter_map(|document| {
                let view = document.view.read(cx);
                Some((view.path.clone()?, view.state.buffer().to_string(), view.state.version()))
            })
            .collect();
        self.package_check_task = Some(cx.spawn(async move |this, cx| {
            let versions: Vec<(PathBuf, u64)> = open.iter().map(|(path, _, version)| (path.clone(), *version)).collect();
            let (report, extra) = cx
                .background_spawn(async move {
                    let mut sources: Vec<(PathBuf, String)> = Vec::new();
                    let mut has_own_style_files = false;
                    let mut own_styles = Vec::new();
                    for entry in file_tree::scan(&root, &HashSet::new()) {
                        let extension = entry.path.extension().and_then(|extension| extension.to_str());
                        has_own_style_files |= !entry.is_dir && matches!(extension, Some("sty" | "cls"));
                        if !entry.is_dir && extension == Some("sty") {
                            own_styles.push(entry.path.clone());
                        }
                        if entry.is_dir || !matches!(extension, Some("tex" | "latex")) {
                            continue;
                        }
                        if let Some((_, text, _)) = open.iter().find(|(path, ..)| *path == entry.path) {
                            sources.push((entry.path, text.clone()));
                        } else if std::fs::metadata(&entry.path).is_ok_and(|metadata| metadata.len() <= PACKAGE_CHECK_FILE_LIMIT)
                            && let Ok(text) = text_file::read(&entry.path).map(|loaded| loaded.text)
                        {
                            sources.push((entry.path, text));
                        }
                    }
                    let report = package_check::hints(&main, &sources, has_own_style_files);
                    let (commands, environments) = package_commands::collect(&report.packages, &own_styles);
                    let used = report.used.clone();
                    (report, Extra { commands, environments, used })
                })
                .await;
            this.update(cx, |this, cx| {
                let unchanged = versions.iter().all(|(path, version)| {
                    this.documents.iter().all(|document| {
                        let view = document.view.read(cx);
                        view.path.as_ref() != Some(path) || view.state.version() == *version
                    })
                });
                if unchanged {
                    let hints = if show_hints { report.hints } else { Vec::new() };
                    this.show_package_hints(hints, report.loaded, extra, cx);
                }
            })
            .ok();
        }));
    }

    fn show_package_hints(&mut self, hints: Vec<PackageHint>, loaded: Loaded, extra: Extra, cx: &mut Context<Self>) {
        let (loaded, extra) = (Arc::new(loaded), Arc::new(extra));
        for document in &self.documents {
            document.view.update(cx, |view, cx| {
                view.set_loaded_packages(loaded.clone(), extra.clone());
                let marks = hints
                    .iter()
                    .filter(|hint| view.path.as_deref() == Some(hint.path.as_path()))
                    .map(|hint| Mark { range: hint.range.clone(), severity: MarkSeverity::Hint, message: hint.message.clone(), source: MarkSource::Packages })
                    .collect();
                view.set_own_marks(MarkSource::Packages, marks, cx);
            });
        }
        self.package_hints = hints;
        self.loaded_packages = loaded;
        self.package_extra = extra;
        cx.notify();
    }

    /// Makes the edit of `fix`, with undo. A new package goes to the main file, and
    /// a removal to the file that loads the package.
    fn apply_fix(&mut self, fix: &Fix, path: Option<&Path>, cx: &mut Context<Self>) {
        if let Fix::InstallTool(language) = fix {
            // LaTeX comes in two sizes, which the setup explains.
            if language == "LaTeX" {
                self.setup_requested = true;
                return cx.notify();
            }
            if let Some(tool) = Tool::for_language(language) {
                self.toolchain.update(cx, |toolchain, cx| toolchain.start(tool, cx));
            }
            self.dialog = Some(Dialog::Settings(SettingsPage::Tools));
            return cx.notify();
        }
        if let Fix::InstallLatexPackage(file) = fix {
            let file = file.clone();
            self.set_status(format!("Installing the package of {file}. macOS asks for your Mac password; Editor never sees it."), cx);
            self.texlive.update(cx, |texlive, cx| texlive.install_file(file, cx));
            return cx.notify();
        }
        if *fix == Fix::UpdateLatex {
            // The settings say why macOS asks for the password, then ask.
            self.texlive.update(cx, |texlive, cx| texlive.ask_update(cx));
            self.dialog = Some(Dialog::Settings(SettingsPage::Tools));
            return cx.notify();
        }
        let Some(path) = path.map(Path::to_path_buf).or_else(|| self.last_main.clone()) else {
            return;
        };
        let Some(view) = self.document_view(&path, cx) else {
            return;
        };
        let text = view.read(cx).state.buffer().to_string();
        let Some((range, new_text)) = package_check::edit_for(fix, &text) else {
            return self.set_status(format!("Cannot apply this: {}.", fix.label()), cx);
        };
        view.update(cx, |view, cx| view.edit_from_outside(range, &new_text, cx));
        // The hint is done, and the next build checks again.
        let mut hints = std::mem::take(&mut self.package_hints);
        hints.retain(|hint| hint.fix != *fix);
        let (loaded, extra) = ((*self.loaded_packages).clone(), (*self.package_extra).clone());
        self.show_package_hints(hints, loaded, extra, cx);
        let done = match fix {
            Fix::AddPackage(package) => format!("Added {package}"),
            Fix::RemovePackage(package) => format!("Removed {package}"),
            Fix::InstallTool(_) | Fix::InstallLatexPackage(_) | Fix::UpdateLatex => return,
        };
        self.set_status(done, cx);
    }

    /// The view of the file at `path`, opened in the background when needed.
    fn document_view(&mut self, path: &Path, cx: &mut Context<Self>) -> Option<Entity<EditorView>> {
        if !self.documents.iter().any(|document| document.view.read(cx).path.as_deref() == Some(path)) {
            let Ok(loaded) = text_file::read(path) else {
                self.set_status(format!("Cannot read {}.", path.display()), cx);
                return None;
            };
            self.add_document(path.to_path_buf(), &loaded.text, cx);
            if let Some(view) = self.documents.last().map(|document| document.view.clone()) {
                view.update(cx, |view, _| view.set_encoding(loaded.encoding));
            }
        }
        self.documents.iter().map(|document| document.view.clone()).find(|view| view.read(cx).path.as_deref() == Some(path))
    }

    /// Loads `listings` and `xcolor` and colors code blocks, in the preamble of
    /// the shown file or else of the main file.
    fn set_up_listings(&mut self, cx: &mut Context<Self>) {
        let Some(active) = self.active_view() else {
            return;
        };
        let text = active.read(cx).state.buffer().to_string();
        let view = if text.contains("\\documentclass") {
            Some(active)
        } else {
            self.last_main.clone().and_then(|main| self.document_view(&main, cx))
        };
        let Some(view) = view else {
            return;
        };
        let text = view.read(cx).state.buffer().to_string();
        if let Some((range, new_text)) = package_check::listings_setup(&text) {
            view.update(cx, |view, cx| view.edit_from_outside(range, &new_text, cx));
        }
    }

    /// The package hint on the line of the cursor.
    fn cursor_hint(&self, cx: &App) -> Option<&PackageHint> {
        let view = self.active_view()?;
        let view = view.read(cx);
        let message = view.hint_at_cursor()?;
        self.package_hints.iter().find(|hint| hint.message == message && view.path.as_ref() == Some(&hint.path))
    }

    /// Whether `fix` can help on this Mac: updating packages needs MacTeX.
    fn offers(&self, fix: &Fix, cx: &App) -> bool {
        !matches!(fix, Fix::UpdateLatex | Fix::InstallLatexPackage(_)) || self.texlive.read(cx).installed()
    }

    /// Applies the package fix on the line of the cursor.
    fn quick_fix(&mut self, cx: &mut Context<Self>) {
        let Some(view) = self.active_view() else {
            return;
        };
        let hint = self.cursor_hint(cx).cloned();
        let view = view.read(cx);
        let path = view.path.clone();
        let (line, _) = view.cursor_position();
        let fix = hint.map(|hint| (hint.fix, Some(hint.path))).or_else(|| {
            self.diagnostics
                .iter()
                .filter(|diagnostic| diagnostic.line == Some(line) && diagnostic.file == path)
                .find_map(|diagnostic| Some((build::explain(diagnostic)?.fix.filter(|fix| self.offers(fix, cx))?, None)))
        });
        match fix {
            Some((fix, path)) => self.apply_fix(&fix, path.as_deref(), cx),
            None => self.set_status("No package fix on this line.", cx),
        }
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
                // The editor's own marks follow edits and get replaced by the next check.
                let hints: Vec<Mark> = view.state.marks().iter().filter(|mark| mark.source != MarkSource::Compiler).cloned().collect();
                let compiler_marks: Vec<Mark> = self
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
                            source: MarkSource::Compiler,
                        })
                    })
                    .collect();
                // Hints come first, so that a compiler error on the same text draws on top.
                view.set_marks(hints.into_iter().chain(compiler_marks).collect(), cx);
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
        self.addon_context_changed(cx);
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
        if let Some(view) = self.markdown_view(cx) {
            let head = view.read(cx).state.selections().primary().head;
            self.markdown.update(cx, |preview, cx| preview.reveal(head, cx));
            return;
        }
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
        let look = (self.settings.look(), self.settings.paper_dark);
        update(&mut self.settings);
        if (self.settings.look(), self.settings.paper_dark) != look {
            apply_look(&self.settings);
            for document in &self.documents {
                document.view.update(cx, |view, cx| view.forget_layout(cx));
            }
        }
        if let Some(path) = &self.settings_path
            && let Err(error) = self.settings.save(path)
        {
            self.set_status(format!("Cannot save settings: {error}"), cx);
        }
        let settings = self.settings.clone();
        for document in &self.documents {
            document.view.update(cx, |view, cx| {
                view.set_vim_mode(settings.vim_mode, settings.smooth_cursor, cx);
                view.set_auto_suggestions(settings.auto_suggestions, cx);
            });
        }
        if !settings.vim_mode && self.tree_focus.is_focused(window) {
            self.focus_active(window, cx);
        }
        cx.notify();
    }

    fn open_dialog(&mut self, dialog: Dialog, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(dialog, Dialog::Settings(_)) {
            // Tools may have come or gone outside the app since the last look.
            self.toolchain.read(cx).forget_found();
        }
        if dialog == Dialog::Settings(SettingsPage::Storage) {
            self.storage_usage = None;
            self.measure_storage(cx);
        }
        self.open_menu = None;
        self.dialog = Some(dialog);
        window.focus(&self.dialog_focus, cx);
        cx.notify();
    }

    fn close_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.dialog = None;
        self.recording_shortcut = None;
        self.focus_active(window, cx);
        cx.notify();
    }

    fn reveal_in_tree(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(path) = self.active_view().and_then(|view| view.read(cx).path.clone()) {
            for ancestor in path.ancestors().skip(1) {
                self.collapsed.remove(ancestor);
            }
            self.refresh_from_disk(cx);
        }
        self.focus_tree(window, cx);
    }

    // Layout

    /// The PDF fills the place of the editor.
    fn pdf_is_main(&self) -> bool {
        !self.focus_mode && (self.layout == PaneLayout::PdfOnly || (self.layout == PaneLayout::PdfTab && self.pdf_tab_shown))
    }

    fn focus_main(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.presenting.is_some() || self.pdf_is_main() {
            window.focus(&self.pdf.read(cx).focus_handle(), cx);
        } else {
            self.focus_shown_tab(window, cx);
        }
    }

    fn set_layout(&mut self, layout: PaneLayout, window: &mut Window, cx: &mut Context<Self>) {
        self.layout = layout;
        self.pdf_tab_shown = layout == PaneLayout::PdfTab;
        self.focus_mode = false;
        self.focus_main(window, cx);
        self.refresh_markdown(cx);
        cx.notify();
    }

    /// Hides or shows the PDF next to the editor, as `Space p` always did.
    fn toggle_preview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let layout = if self.layout == PaneLayout::EditorOnly { PaneLayout::Split } else { PaneLayout::EditorOnly };
        self.set_layout(layout, window, cx);
    }

    fn toggle_focus_mode(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.focus_mode = !self.focus_mode;
        self.focus_main(window, cx);
        self.refresh_markdown(cx);
        cx.notify();
    }

    fn start_presentation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.presenting.is_some() {
            return;
        }
        if !self.pdf.read(cx).has_document() {
            self.set_status("No PDF to present yet. Compile first.", cx);
            return;
        }
        let entered_fullscreen = !window.is_fullscreen();
        if entered_fullscreen {
            window.toggle_fullscreen();
        }
        self.presenting = Some(PresentState { entered_fullscreen, controls_shown: false, hide_controls_task: None });
        self.pdf.update(cx, |pdf, cx| pdf.set_presenting(true, cx));
        self.focus_main(window, cx);
        cx.notify();
    }

    fn exit_presentation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(state) = self.presenting.take() else {
            return;
        };
        if state.entered_fullscreen && window.is_fullscreen() {
            window.toggle_fullscreen();
        }
        self.pdf.update(cx, |pdf, cx| pdf.set_presenting(false, cx));
        self.focus_main(window, cx);
        cx.notify();
    }

    /// A visible way out of focus and presentation mode, for those who do not use shortcuts.
    fn exit_button(
        &self,
        id: &'static str,
        label: &'static str,
        on_dark: bool,
        cx: &mut Context<Self>,
        handler: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> impl IntoElement {
        let palette = theme::palette();
        let (background, text): (gpui::Hsla, u32) =
            if on_dark { (gpui::rgba(0x2b2b2bdd).into(), 0xffffff) } else { (theme::color(palette.divider), palette.ui_text) };
        div()
            .id(id)
            .flex()
            .items_center()
            .gap(px(6.))
            .px(px(10.))
            .py(px(4.))
            .rounded(px(4.))
            .cursor_pointer()
            .bg(background)
            .text_color(theme::color(text))
            .hover(|style| style.bg(theme::color(palette.accent_soft)))
            .on_click(cx.listener(move |this, _, window, cx| handler(this, window, cx)))
            .child(Self::icon("close", 12., text))
            .child(label)
    }

    fn show_tab_bar(&self) -> bool {
        self.settings.show_tabs && !self.focus_mode
    }

    fn show_breadcrumbs(&self) -> bool {
        self.settings.show_breadcrumbs && !self.focus_mode
    }

    /// Where the bar above the editor body ends, for popups that open below it.
    fn editor_toolbar_bottom(&self, toolbar: f32) -> f32 {
        let tabs = if self.show_tab_bar() { TAB_BAR_HEIGHT } else { 0. };
        let breadcrumbs = if self.show_breadcrumbs() { BREADCRUMB_HEIGHT } else { 0. };
        TOP_BAR_HEIGHT + tabs + breadcrumbs + toolbar
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
        if self.setup.is_some() {
            return;
        }
        if self.recording_shortcut.is_some() {
            cx.stop_propagation();
            self.record_shortcut(&event.keystroke, cx);
            return;
        }
        if self.tour.is_some() {
            let modifiers = event.keystroke.modifiers;
            if !(modifiers.platform || modifiers.control || modifiers.alt) {
                match event.keystroke.key.as_str() {
                    "right" | "enter" => self.tour_step(true, window, cx),
                    "left" => self.tour_step(false, window, cx),
                    "escape" => self.end_tour(window, cx),
                    _ => return,
                }
                cx.stop_propagation();
            }
            return;
        }
        if self.hub_shown() && self.dialog.is_none() && self.open_menu.is_none() {
            if self.home.update(cx, |home, cx| home.handle_key(&event.keystroke, window, cx)) {
                cx.stop_propagation();
            }
            return;
        }
        if self.git_key(event, window, cx) {
            return;
        }
        // Cmd+Enter in the citation picker cites another key instead of compiling.
        let secondary = if cfg!(target_os = "macos") { event.keystroke.modifiers.platform } else { event.keystroke.modifiers.control };
        if secondary
            && event.keystroke.key == "enter"
            && self.picker.as_ref().is_some_and(|picker| picker.kind == PickerKind::Citations)
        {
            cx.stop_propagation();
            self.cite_another(cx);
            return;
        }
        if event.keystroke.modifiers.platform {
            return;
        }
        if self.picker.is_some() {
            cx.stop_propagation();
            self.picker_key(&event.keystroke, window, cx);
            return;
        }
        if let Some(panel) = self.dialog_panel(cx) {
            match event.keystroke.key.as_str() {
                "escape" => {
                    cx.stop_propagation();
                    self.close_addon_dialog(window, cx);
                }
                "enter" => {
                    cx.stop_propagation();
                    panel.update(cx, |panel, cx| panel.confirm_dialog(cx));
                }
                _ => {}
            }
            return;
        }
        if let Some(input) = &self.tree_input
            && input.view.read(cx).focus_handle(cx).is_focused(window)
        {
            match event.keystroke.key.as_str() {
                "escape" => {
                    cx.stop_propagation();
                    self.close_tree_input(window, cx);
                }
                "enter" => {
                    cx.stop_propagation();
                    self.confirm_tree_input(window, cx);
                }
                "tab" => cx.stop_propagation(),
                _ => {}
            }
            return;
        }
        #[cfg(feature = "collab")]
        if self.collab_key(event, window, cx) {
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
        if self.pdf.read(cx).is_focused(window) {
            return;
        }
        if self.image_preview().is_some_and(|image| image.view.read(cx).focus_handle().is_focused(window)) {
            if event.keystroke.modifiers.shift && matches!(event.keystroke.key.as_str(), "h" | "l") {
                cx.stop_propagation();
                self.step_tab(event.keystroke.key == "l", window, cx);
            }
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
        // On Windows the app's shortcuts are Ctrl keys, which on the Mac are
        // Cmd keys and never reach vim: a Ctrl key vim has no use for, such as
        // Ctrl+1 for the first tab or Ctrl+S, goes to the shortcuts.
        if cfg!(windows) && takes_keys && is_unknown_ctrl(key) {
            return;
        }
        // The input method composes without the user's mappings.
        let step = if view.read(cx).is_composing() {
            Step::Type
        } else {
            let mode = view.read(cx).vim_mode().unwrap_or_default();
            self.vim_mapper.feed(&self.vim_mappings, mode, key, Instant::now())
        };
        let keys = match step {
            Step::Run(keys) => keys,
            Step::Insert { erase, mut keys } => {
                keys.splice(0..0, std::iter::repeat_n(Key::Backspace, erase));
                keys
            }
            Step::Type if !takes_keys && key != Key::Esc => {
                // Other `Ctrl` keys go to the key bindings, as without vim.
                // `Tab` accepts here, and accepting records its own result.
                if !is_unknown_ctrl(key) && (key != Key::Tab || !view.read(cx).completion_open()) {
                    view.update(cx, |view, _| view.vim_record_insert(key));
                }
                return;
            }
            // `Esc` belongs to the input method while it composes.
            Step::Type if !takes_keys && view.read(cx).is_composing() => return,
            Step::Type => vec![key],
        };
        cx.stop_propagation();
        self.run_vim_keys(&view, keys, window, cx);
        if self.vim_mapper.is_waiting() {
            self.vim_map_timer = Some(cx.spawn_in(window, async move |this, cx| {
                cx.background_executor().timer(vim_map::TIMEOUT).await;
                this.update_in(cx, |this, window, cx| this.vim_mapping_timed_out(window, cx)).ok();
            }));
        }
        cx.notify();
    }

    fn run_vim_keys(&mut self, view: &Entity<EditorView>, keys: Vec<Key>, window: &mut Window, cx: &mut Context<Self>) {
        for key in keys {
            // Unknown `Ctrl` keys do nothing in vim, but a mapping may use them.
            if is_unknown_ctrl(key) {
                continue;
            }
            let wants_clipboard = matches!(key, Key::Char('p' | 'P' | '.'));
            let clipboard = wants_clipboard.then(|| cx.read_from_clipboard().and_then(|item| item.text())).flatten();
            let effects = view.update(cx, |view, cx| view.vim_key(key, clipboard, cx));
            for effect in effects {
                self.apply_effect(effect, window, cx);
            }
        }
    }

    /// Runs the keys that waited for a longer mapping, once none came.
    fn vim_mapping_timed_out(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let keys = self.vim_mapper.timeout(&self.vim_mappings, Instant::now());
        if let Some(view) = self.active_view().filter(|_| !keys.is_empty()) {
            self.run_vim_keys(&view, keys, window, cx);
            cx.notify();
        }
    }

    fn apply_effect(&mut self, effect: Effect, window: &mut Window, cx: &mut Context<Self>) {
        match effect {
            // The editor handles these two itself.
            Effect::Yank(_) | Effect::Scroll(_) => {}
            Effect::Save => self.save_active(window, cx),
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
            Effect::RevealInTree => self.reveal_in_tree(window, cx),
            Effect::TogglePreview => self.toggle_preview(window, cx),
            Effect::FindFiles => self.open_picker(PickerKind::Files, window, cx),
            Effect::FindText => self.open_picker(PickerKind::Text, window, cx),
            Effect::PickTheme => self.open_theme_picker(window, cx),
            Effect::OpenHub => self.open_hub(window, cx),
            Effect::LookupDoi => self.lookup_doi(cx),
            Effect::QuickFix => self.quick_fix(cx),
            Effect::ShowError => {
                if self.problems.is_empty() {
                    self.set_status("No errors.", cx);
                } else {
                    match self.layout {
                        PaneLayout::EditorOnly => self.layout = PaneLayout::Split,
                        PaneLayout::PdfTab => self.pdf_tab_shown = true,
                        PaneLayout::Split | PaneLayout::PdfOnly => {}
                    }
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
        let tabs = self.tabs();
        let count = tabs.len();
        let active = self.active_tab();
        if let Some(active) = tabs.iter().position(|tab| Some(*tab) == active) {
            let next = if forward { (active + 1) % count } else { (active + count - 1) % count };
            self.activate_tab(tabs[next], window, cx);
        }
    }

    // File menu

    /// Copies the open file beside itself, with unsaved changes, and opens the copy.
    fn make_copy(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let source = match (self.image_preview(), self.active_view()) {
            (Some(preview), _) => Some((preview.view.read(cx).path().to_path_buf(), None)),
            (None, Some(view)) => {
                let view = view.read(cx);
                view.path.clone().map(|path| (path, Some(view.state.buffer().to_string())))
            }
            (None, None) => None,
        };
        let Some((path, text)) = source else {
            self.set_status("Open a file to copy it.", cx);
            return;
        };
        cx.spawn_in(window, async move |this, cx| {
            let copied = cx.background_spawn(async move { file_tree::make_copy(&path, text.as_deref()) }).await;
            this.update_in(cx, |this, window, cx| match copied {
                Ok(copy) => {
                    this.reveal_new_entry(&copy, cx);
                    this.open_file(copy, window, cx);
                }
                Err(error) => this.set_status(format!("Cannot copy: {error}"), cx),
            })
            .ok();
        })
        .detach();
    }

    fn show_version_history(&mut self, cx: &mut Context<Self>) {
        self.show_file_panel = true;
        self.left_panel = LeftPanel::Git;
        self.git_panel.update(cx, |panel, cx| panel.show_history_section(cx));
        cx.notify();
    }

    fn show_word_count(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.root.clone() else {
            self.set_status("Open a folder first.", cx);
            return;
        };
        let active = self.active_view().and_then(|view| view.read(cx).path.clone());
        let Some(main) = build::find_main(&root, &self.config, active.as_deref()) else {
            self.set_status("This file type has no word count.", cx);
            return;
        };
        let open: Vec<(PathBuf, String)> = self
            .documents
            .iter()
            .filter_map(|document| {
                let view = document.view.read(cx);
                Some((view.path.clone()?, view.state.buffer().to_string()))
            })
            .collect();
        // LaTeX runs in the main file's folder, and Typst resolves `/` from the project.
        let base = if Language::from_path(&main) == Language::Typst {
            root
        } else {
            main.parent().unwrap_or(&root).to_path_buf()
        };
        self.word_count = None;
        self.open_dialog(Dialog::WordCount, window, cx);
        self.word_count_task = Some(cx.spawn(async move |this, cx| {
            let counted = cx
                .background_spawn(async move {
                    let open: HashMap<PathBuf, String> =
                        open.into_iter().filter_map(|(path, text)| Some((path.canonicalize().map(crate::platform::plain_path).ok()?, text))).collect();
                    let mut read = |path: &Path| {
                        path.canonicalize().map(crate::platform::plain_path)
                            .ok()
                            .and_then(|path| open.get(&path).cloned().or_else(|| text_file::read(&path).ok().map(|loaded| loaded.text)))
                    };
                    let text = read(&main).ok_or_else(|| format!("Cannot read {}", main.display()))?;
                    Ok(word_count::count_words(&main, &text, &base, &mut read))
                })
                .await;
            this.update(cx, |this, cx| {
                this.word_count = Some(counted);
                cx.notify();
            })
            .ok();
        }));
    }

    fn render_word_count(&self) -> Vec<AnyElement> {
        let note = |text: String| div().text_color(theme::color(theme::palette().ui_muted)).mt(px(12.)).child(text).into_any_element();
        let row = |label: String, value: usize, strong: bool| {
            div()
                .flex()
                .flex_none()
                .justify_between()
                .gap(px(24.))
                .py(px(2.))
                .when(strong, |element| element.font_weight(FontWeight::SEMIBOLD))
                .child(div().min_w_0().truncate().child(label))
                .child(div().flex_none().child(value.to_string()))
                .into_any_element()
        };
        let counted = match &self.word_count {
            None => return vec![div().child("Counting…").into_any_element()],
            Some(Err(error)) => return vec![div().child(error.clone()).into_any_element()],
            Some(Ok(counted)) => counted,
        };
        let mut rows = vec![
            row("Total words".to_string(), counted.total(), true),
            row("Text".to_string(), counted.body, false),
            row("Headings".to_string(), counted.headings, false),
            row("Captions".to_string(), counted.captions, false),
            row("Inline formulas".to_string(), counted.inline_math, false),
            row("Display formulas".to_string(), counted.display_math, false),
        ];
        if !counted.missing.is_empty() {
            rows.push(note(format!("Not found, so not counted: {}", counted.missing.join(", "))));
        }
        rows.push(note(
            "Counted from the source. Comments, math, code, labels, citations and the preamble do not count, nor does text that only macros or Typst functions generate."
                .to_string(),
        ));
        if !counted.sections.is_empty() {
            rows.push(div().mt(px(12.)).font_weight(FontWeight::SEMIBOLD).child("Words per section").into_any_element());
            rows.extend(counted.sections.iter().map(|section| row(section.title.clone(), section.words, false)));
        }
        rows
    }

    /// Runs one export, from its first dialog to its last, off the main thread's way.
    fn start_export(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        flow: impl AsyncFnOnce(WeakEntity<Self>, &mut AsyncWindowContext) + 'static,
    ) {
        if self.export_running {
            self.set_status("An export is running.", cx);
            return;
        }
        self.export_running = true;
        cx.spawn_in(window, async move |this, cx| {
            flow(this.clone(), cx).await;
            this.update(cx, |this, cx| {
                this.export_running = false;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn show_notice(&mut self, message: &str, detail: &str, window: &mut Window, cx: &mut Context<Self>) {
        // No answer to wait for: the dialog stays open without the receiver.
        drop(window.prompt(PromptLevel::Info, message, Some(detail), &["OK"], cx));
    }

    /// Everything saved, or the author agreed to save. `false` ends the export.
    async fn save_for_export(this: &WeakEntity<Self>, cx: &mut AsyncWindowContext) -> bool {
        let unsaved = this
            .update(cx, |this, cx| this.documents.iter().any(|document| document.view.read(cx).state.is_dirty()))
            .unwrap_or(false);
        if unsaved {
            let question = "Save your changes before the export?";
            let detail = "The export reads the files on disk.";
            let answer = ask(this, cx, PromptLevel::Warning, question, detail, &["Save and export", "Cancel"]).await;
            if answer != Some(0) {
                return false;
            }
        }
        this.update(cx, |this, cx| this.save_all(cx)).unwrap_or(false)
    }

    /// Asks where the export goes. A source file is never a valid place.
    async fn ask_target(this: &WeakEntity<Self>, cx: &mut AsyncWindowContext, directory: PathBuf, name: String) -> Option<PathBuf> {
        let sources = this
            .update(cx, |this, cx| {
                let open = this.documents.iter().filter_map(|document| document.view.read(cx).path.clone());
                open.chain(this.last_main.clone()).collect::<Vec<PathBuf>>()
            })
            .ok()?;
        let answer = this.update(cx, |_, cx| cx.prompt_for_new_path(&directory, Some(&name))).ok()?;
        let target = match answer.await {
            Ok(Ok(Some(target))) => target,
            Ok(Err(error)) => {
                fail_export(this, cx, &error.to_string()).await;
                return None;
            }
            _ => return None,
        };
        if export::is_source(&target, &sources) {
            fail_export(this, cx, &format!("{} is a source file of this project. Choose another name.", target.display())).await;
            return None;
        }
        Some(target)
    }

    fn download_source(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.root.clone() else {
            self.set_status("Open a folder first.", cx);
            return;
        };
        let downloads_packages = self.config.engine == Some(Engine::Tectonic);
        self.start_export(window, cx, async move |this, cx| {
            if !Self::save_for_export(&this, cx).await {
                return;
            }
            let name = root.file_name().map_or_else(|| "project".to_string(), |name| name.to_string_lossy().into_owned());
            let folder = root.parent().unwrap_or(&root).to_path_buf();
            let Some(target) = Self::ask_target(&this, cx, folder, format!("{name}.zip")).await else {
                return;
            };
            show_status(&this, cx, "Reading the project…");
            let (source, destination) = (root.clone(), target.clone());
            let listed = cx.background_spawn(async move { export::project_entries(&source, &destination) }).await;
            let mut entries = match listed {
                Ok(entries) => entries,
                Err(error) => return fail_export(&this, cx, &error).await,
            };
            let suspicious = export::suspicious_files(&entries);
            if !suspicious.is_empty() {
                let question = "These files may hold secrets";
                let detail = shortened_list(&suspicious);
                match ask(&this, cx, PromptLevel::Warning, question, &detail, &["Leave out", "Include", "Cancel"]).await {
                    Some(0) => entries.retain(|entry| !suspicious.contains(&entry.relative)),
                    Some(1) => {}
                    _ => return show_status(&this, cx, "Export cancelled."),
                }
            }
            show_status(&this, cx, "Packing and checking the archive…");
            let files = entries.len();
            let destination = target.clone();
            let packed = cx
                .background_spawn(async move {
                    export::zip_project(&entries, &destination)?;
                    Ok::<bool, String>(export::uses_online_packages(&entries))
                })
                .await;
            match packed {
                Ok(online) => {
                    show_status(&this, cx, &format!("Archive saved and checked, {files} entries."));
                    if online || downloads_packages {
                        let detail = "This project downloads packages when it compiles. They are not in the archive.";
                        ask(&this, cx, PromptLevel::Info, "Archive saved", detail, &["OK"]).await;
                    }
                }
                Err(error) => fail_export(&this, cx, &error).await,
            }
        });
    }

    /// The build's state as the check before a PDF export reads it.
    fn pdf_snapshot(&self, cx: &App) -> Option<PdfSnapshot> {
        Some(PdfSnapshot {
            root: self.root.clone()?,
            pdf: self.last_pdf.clone()?,
            building: self.building,
            unsaved: self.documents.iter().any(|document| document.view.read(cx).state.is_dirty()),
            has_errors: self.preview_state == PreviewState::Incomplete,
            last_good: self.preview_state == PreviewState::LastGood,
            warnings: self
                .diagnostics
                .iter()
                .filter(|diagnostic| diagnostic.severity == Severity::Warning)
                .map(|diagnostic| diagnostic.message.clone())
                .collect(),
        })
    }

    fn download_pdf(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.last_pdf.is_none() {
            self.show_notice("There is no PDF yet", "Recompile, then download the PDF.", window, cx);
            return;
        }
        self.start_export(window, cx, async move |this, cx| {
            let pdf = loop {
                let Ok(Some(snapshot)) = this.update(cx, |this, cx| this.pdf_snapshot(cx)) else {
                    return show_status(&this, cx, "The build left no PDF, so nothing was exported.");
                };
                let (root, pdf) = (snapshot.root.clone(), snapshot.pdf.clone());
                let (newest_source, pdf_modified) = cx
                    .background_spawn(async move {
                        let name = pdf.file_name().map_or_else(String::new, |name| name.to_string_lossy().into_owned());
                        (export::newest_source(&root, &name), std::fs::metadata(&pdf).and_then(|metadata| metadata.modified()).ok())
                    })
                    .await;
                let problems = export::pdf_problems(&PdfState {
                    building: snapshot.building,
                    unsaved: snapshot.unsaved,
                    has_errors: snapshot.has_errors,
                    last_good: snapshot.last_good,
                    newest_source,
                    pdf_modified,
                    warnings: &snapshot.warnings,
                });
                if problems.is_empty() {
                    break snapshot.pdf;
                }
                let buttons = ["Recompile first", "Export anyway", "Cancel"];
                match ask(&this, cx, PromptLevel::Warning, "This PDF may not be ready", &problems.join("\n"), &buttons).await {
                    Some(0) => {
                        if !Self::rebuild_for_export(&this, cx).await {
                            return show_status(&this, cx, "The build failed, so nothing was exported.");
                        }
                    }
                    Some(1) => break snapshot.pdf,
                    _ => return,
                }
            };
            let Ok(Some(root)) = this.read_with(cx, |this, _| this.root.clone()) else {
                return;
            };
            let name = pdf.file_name().map_or_else(|| "document.pdf".to_string(), |name| name.to_string_lossy().into_owned());
            let Some(target) = Self::ask_target(&this, cx, root, name).await else {
                return;
            };
            let destination = target.clone();
            match cx.background_spawn(async move { export::copy_pdf(&pdf, &destination) }).await {
                Ok(()) => show_status(&this, cx, &format!("PDF saved to {}", target.display())),
                Err(error) => fail_export(&this, cx, &error).await,
            }
        });
    }

    /// Saves and builds, and tells whether the build worked.
    async fn rebuild_for_export(this: &WeakEntity<Self>, cx: &mut AsyncWindowContext) -> bool {
        let started = this.update(cx, |this, cx| {
            this.last_build_succeeded = false;
            this.recompile(cx);
            this.building
        });
        if !started.unwrap_or(false) {
            return false;
        }
        loop {
            cx.background_executor().timer(Duration::from_millis(100)).await;
            match this.read_with(cx, |this, _| (this.building, this.last_build_succeeded)) {
                Ok((true, _)) => {}
                Ok((false, succeeded)) => return succeeded,
                Err(_) => return false,
            }
        }
    }

    fn download_draft(&mut self, format: DraftFormat, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.root.clone() else {
            self.set_status("Open a folder first.", cx);
            return;
        };
        let active = self.active_view().and_then(|view| view.read(cx).path.clone());
        let main = build::find_main(&root, &self.config, active.as_deref()).filter(|main| export::pandoc_reader(main).is_some());
        let Some(main) = main else {
            self.show_notice("This file has no draft export", "Drafts need a LaTeX, Typst or Markdown file.", window, cx);
            return;
        };
        let Some(pandoc) = build::find_tool("pandoc") else {
            // Installed on the first export only, since few people need it.
            let size = toolchain::size(Tool::Pandoc.megabytes().unwrap_or(0));
            let detail = format!("Word, HTML and Markdown drafts need Pandoc, a free converter ({size}). The export goes on by itself once it is installed. The PDF and source downloads work without it.");
            let answer = window.prompt(PromptLevel::Info, "Install Pandoc for this export?", Some(&detail), &["Install and export", "Cancel"], cx);
            cx.spawn_in(window, async move |this, cx| {
                if answer.await == Ok(0) {
                    this.update(cx, |this, cx| {
                        this.export_after_pandoc = Some(format);
                        this.toolchain.update(cx, |toolchain, cx| toolchain.start(Tool::Pandoc, cx));
                    })
                    .ok();
                }
            })
            .detach();
            return;
        };
        self.start_export(window, cx, async move |this, cx| {
            if !Self::save_for_export(&this, cx).await {
                return;
            }
            let stem = main.file_stem().map_or_else(|| "document".to_string(), |stem| stem.to_string_lossy().into_owned());
            let name = format!("{stem}-draft.{}", format.extension());
            let Some(target) = Self::ask_target(&this, cx, root, name).await else {
                return;
            };
            show_status(&this, cx, "Converting with Pandoc…");
            let destination = target.clone();
            let converted = cx.background_spawn(async move { export::run_pandoc(&pandoc, &main, format, &destination) }).await;
            let draft = match converted {
                Ok(draft) => draft,
                Err(error) => return fail_export(&this, cx, &error).await,
            };
            if !draft.report.is_empty() {
                let detail = shortened_list(&draft.report);
                let answer = ask(&this, cx, PromptLevel::Warning, "Not in this draft", &detail, &["Keep draft", "Discard"]).await;
                if answer != Some(0) {
                    return show_status(&this, cx, "Draft discarded.");
                }
            }
            match cx.background_spawn(async move { draft.keep(&target) }).await {
                Ok(()) => show_status(&this, cx, "Draft saved. Layout differs from the PDF."),
                Err(error) => fail_export(&this, cx, &error).await,
            }
        });
    }

    // File tree keys

    fn focus_tree(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.show_file_panel = true;
        self.show_tree = true;
        self.left_panel = LeftPanel::Files;
        let active = self.active_view().and_then(|view| view.read(cx).path.clone());
        if let Some(index) = active.and_then(|path| self.entries.iter().position(|entry| entry.path == path)) {
            self.tree_cursor = index;
        }
        self.tree_scroll.scroll_to_item(self.tree_cursor, ScrollStrategy::Nearest);
        self.tree_g_pending = false;
        self.tree_leader = None;
        window.focus(&self.tree_focus, cx);
        cx.notify();
    }

    fn move_tree_cursor(&mut self, index: usize, cx: &mut Context<Self>) {
        self.tree_cursor = index.min(self.entries.len().saturating_sub(1));
        self.tree_scroll.scroll_to_item(self.tree_cursor, ScrollStrategy::Nearest);
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
            Key::Char('a') => self.open_tree_input(false, window, cx),
            Key::Char('A') => self.open_tree_input(true, window, cx),
            Key::Char('u') => self.prompt_add_files(window, cx),
            Key::Char('r') => {
                if let Some(entry) = self.entries.get(self.tree_cursor) {
                    self.open_rename(entry.path.clone(), window, cx);
                }
            }
            Key::Char('D') => {
                if let Some(entry) = self.entries.get(self.tree_cursor) {
                    self.trash_tree_entry(entry.path.clone(), window, cx);
                }
            }
            Key::Char(' ') => self.tree_leader = Some(String::new()),
            Key::Tab => self.focus_active(window, cx),
            // `q` does nothing in the nvim-tree config either.
            _ => {}
        }
        cx.notify();
    }

    /// The folder for a new entry: where the tree's cursor is, or the last clicked row.
    fn creation_folder(&self, window: &Window) -> Option<PathBuf> {
        let root = self.root.as_ref()?;
        let selected = if self.tree_focus.is_focused(window) {
            self.entries.get(self.tree_cursor).map(|entry| entry.path.as_path())
        } else {
            self.tree_target.as_deref()
        };
        Some(file_tree::creation_folder(selected, root))
    }

    fn open_tree_input(&mut self, folder: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(parent) = self.creation_folder(window) else {
            return;
        };
        let back_to_tree = self.tree_focus.is_focused(window);
        self.show_files();
        let view = cx.new(|cx| {
            let mut view = EditorView::new("", None, cx);
            view.set_plain();
            view
        });
        let _observer = cx.observe(&view, |_, _, cx| cx.notify());
        window.focus(&view.read(cx).focus_handle(cx), cx);
        self.tree_input = Some(TreeInput { folder, parent, view, back_to_tree, renaming: None, _observer });
        self.tree_error = None;
        cx.notify();
    }

    /// Menu actions can start with the panel closed, or on another of its pages.
    fn show_files(&mut self) {
        self.show_file_panel = true;
        self.show_tree = true;
        self.left_panel = LeftPanel::Files;
    }

    fn close_tree_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(input) = self.tree_input.take() else {
            return;
        };
        self.tree_error = None;
        if input.back_to_tree {
            window.focus(&self.tree_focus, cx);
        } else {
            self.focus_active(window, cx);
        }
        cx.notify();
    }

    /// Creates the entry and keeps the field open with the reason when that fails.
    fn confirm_tree_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(input) = &self.tree_input else {
            return;
        };
        let folder = input.folder;
        let name = input.view.read(cx).state.buffer().to_string();
        if let Some(from) = input.renaming.clone() {
            match file_tree::rename_entry(&from, &name) {
                Ok(to) => {
                    self.close_tree_input(window, cx);
                    self.follow_rename(&from, &to, cx);
                }
                Err(error) => {
                    self.tree_error = Some(error.into());
                    cx.notify();
                }
            }
            return;
        }
        let created = file_tree::new_entry_path(&input.parent, &name).and_then(|path| {
            if folder { file_tree::create_folder(&path) } else { file_tree::create_file(&path) }.map(|()| path)
        });
        match created {
            Ok(path) => {
                self.close_tree_input(window, cx);
                self.reveal_new_entry(&path, cx);
                if !folder && !is_image(&path) {
                    self.open_file(path, window, cx);
                }
            }
            Err(error) => {
                self.tree_error = Some(error.into());
                cx.notify();
            }
        }
    }

    /// Opens the folders down to a new entry and selects it.
    fn reveal_new_entry(&mut self, path: &Path, cx: &mut Context<Self>) {
        for ancestor in path.ancestors() {
            self.collapsed.remove(ancestor);
        }
        self.refresh_from_disk(cx);
        if let Some(index) = self.entries.iter().position(|entry| entry.path == path) {
            self.tree_cursor = index;
            self.tree_scroll.scroll_to_item(index, ScrollStrategy::Nearest);
        }
        self.tree_target = Some(path.to_path_buf());
    }

    /// Edits the name of an entry in its own row, with the cursor before the extension.
    fn open_rename(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if self.root.as_deref() == Some(path.as_path()) {
            return;
        }
        let Some(name) = path.file_name().map(|name| name.to_string_lossy().into_owned()) else {
            return;
        };
        let back_to_tree = self.tree_focus.is_focused(window);
        self.show_files();
        let stem = if path.is_dir() { name.len() } else { name.rfind('.').filter(|dot| *dot > 0).unwrap_or(name.len()) };
        let view = cx.new(|cx| {
            let mut view = EditorView::new(&name, None, cx);
            view.set_plain();
            view.state.set_selections(vec![Selection::cursor(stem)]);
            view
        });
        let _observer = cx.observe(&view, |_, _, cx| cx.notify());
        window.focus(&view.read(cx).focus_handle(cx), cx);
        let parent = path.parent().map(Path::to_path_buf).unwrap_or_default();
        self.tree_target = Some(path.clone());
        self.tree_input = Some(TreeInput { folder: path.is_dir(), parent, view, back_to_tree, renaming: Some(path), _observer });
        self.tree_error = None;
        cx.notify();
    }

    /// Open tabs, folded folders and the main document move along with a rename.
    fn follow_rename(&mut self, from: &Path, to: &Path, cx: &mut Context<Self>) {
        for document in &self.documents {
            document.view.update(cx, |view, _| {
                if let Some(path) = view.path.as_deref().and_then(|path| file_tree::moved_path(path, from, to)) {
                    view.path = Some(path);
                }
            });
        }
        for image in &self.images {
            image.view.update(cx, |view, _| {
                if let Some(path) = file_tree::moved_path(view.path(), from, to) {
                    view.set_path(path);
                }
            });
        }
        self.collapsed = self.collapsed.iter().map(|path| file_tree::moved_path(path, from, to).unwrap_or_else(|| path.clone())).collect();
        if let Some(root) = self.root.clone()
            && let Some(main) = self.config.main.as_deref().and_then(|main| file_tree::moved_path(&root.join(main), from, to))
            && let Ok(relative) = main.strip_prefix(&root)
        {
            self.set_main_document(relative.to_path_buf(), cx);
        }
        self.reveal_new_entry(to, cx);
        let name = to.file_name().map_or_else(String::new, |name| name.to_string_lossy().into_owned());
        self.set_status(format!("Renamed to {name}"), cx);
    }

    /// Asks first, then moves the entry to the Trash, from where Finder can put it back.
    fn trash_tree_entry(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if self.root.as_deref() == Some(path.as_path()) {
            return;
        }
        let name = path.file_name().map_or_else(String::new, |name| name.to_string_lossy().into_owned());
        let unsaved = self.documents.iter().any(|document| {
            let view = document.view.read(cx);
            view.state.is_dirty() && view.path.as_deref().is_some_and(|open| open.starts_with(&path))
        });
        cx.spawn_in(window, async move |this, cx| {
            let message = format!("Move “{name}” to the {}?", crate::platform::TRASH);
            let detail = if unsaved {
                if cfg!(windows) { "Windows can put it back from the Recycle Bin. Unsaved changes stay in Earlier versions." } else { "Finder can put it back from the Trash. Unsaved changes stay in Earlier versions." }
            } else {
                if cfg!(windows) { "Windows can put it back from the Recycle Bin." } else { "Finder can put it back from the Trash." }
            };
            if ask(&this, cx, PromptLevel::Warning, &message, detail, &[crate::platform::MOVE_TO_TRASH, "Cancel"]).await != Some(0) {
                return;
            }
            let target = path.clone();
            let trashed = cx
                .background_spawn(async move {
                    // Also safe from emptying the Trash. Writing is text, so a big
                    // file such as a video has no place in the history.
                    if std::fs::metadata(&target).is_ok_and(|metadata| metadata.is_file() && metadata.len() <= text_file::HISTORY_LIMIT)
                        && let Some(directory) = history::directory()
                    {
                        history::keep_disk(&directory, &target).ok();
                    }
                    trash::delete(&target)
                })
                .await;
            this.update_in(cx, |this, window, cx| {
                if let Err(error) = trashed {
                    this.set_status(format!("Cannot move {name} to the {}: {error}", crate::platform::TRASH), cx);
                    return;
                }
                for document in &this.documents {
                    let view = document.view.read(cx);
                    if view.state.is_dirty() && view.path.as_deref().is_some_and(|open| open.starts_with(&path)) {
                        view.keep_current_text();
                    }
                }
                while let Some(index) =
                    this.documents.iter().rposition(|document| document.view.read(cx).path.as_deref().is_some_and(|open| open.starts_with(&path)))
                {
                    this.remove_document(index, window, cx);
                }
                while let Some(index) = this.images.iter().rposition(|image| image.view.read(cx).path().starts_with(&path)) {
                    this.close_image(index, window, cx);
                }
                this.collapsed.retain(|folder| !folder.starts_with(&path));
                this.refresh_from_disk(cx);
                this.tree_cursor = this.tree_cursor.min(this.entries.len().saturating_sub(1));
                this.set_status(format!("Moved {name} to the {}", crate::platform::TRASH), cx);
            })
            .ok();
        })
        .detach();
    }

    fn duplicate_tree_entry(&mut self, path: &Path, cx: &mut Context<Self>) {
        match file_tree::make_copy(path, None) {
            Ok(copy) => self.reveal_new_entry(&copy, cx),
            Err(error) => self.set_status(format!("Cannot duplicate: {error}"), cx),
        }
    }

    /// Writes the choice to `.editor.toml`, where every build finds it.
    fn set_main_document(&mut self, relative: PathBuf, cx: &mut Context<Self>) {
        let Some(root) = self.root.clone() else {
            return;
        };
        match build::write_main(&root, &relative) {
            Ok(()) => {
                self.set_status(format!("{} is the main document", relative.display()), cx);
                self.config.main = Some(relative);
                self.recompile(cx);
            }
            Err(error) => self.set_status(format!("Cannot set the main document: {error}"), cx),
        }
    }

    fn open_tree_menu(&mut self, path: PathBuf, position: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        self.tree_target = Some(path.clone());
        if let Some(index) = self.entries.iter().position(|entry| entry.path == path) {
            self.tree_cursor = index;
        }
        self.tree_menu_path = Some(path);
        self.open_menu = Some(Menu::Tree);
        self.menu_origin = position;
        window.focus(&self.menu_focus, cx);
        cx.notify();
    }

    fn tree_menu_entries(&self) -> Vec<MenuEntry> {
        let (Some(path), Some(root)) = (self.tree_menu_path.clone(), self.root.clone()) else {
            return Vec::new();
        };
        let is_root = path == root;
        let is_dir = is_root || path.is_dir();
        let vim = self.settings.vim_mode;
        let here = |label: &str, folder: bool, key: &'static str| {
            let target = path.clone();
            entry(label, key, move |this, window, cx| {
                this.tree_target = Some(target.clone());
                this.open_tree_input(folder, window, cx);
            })
        };
        let mut entries = vec![here("New file", false, if vim { "a" } else { "" }), here("New folder", true, if vim { "A" } else { "" })];
        if !is_root {
            entries.push(menu_separator());
            let target = path.clone();
            entries.push(entry("Rename", if vim { "r" } else { "" }, move |this, window, cx| this.open_rename(target.clone(), window, cx)));
            if !is_dir {
                let target = path.clone();
                entries.push(entry("Duplicate", "", move |this, _, cx| this.duplicate_tree_entry(&target, cx)));
                let target = path.clone();
                entries.push(entry("Earlier versions…", "", move |this, window, cx| this.open_history_picker(target.clone(), window, cx)));
            }
            let target = path.clone();
            entries.push(MenuEntry {
                danger: true,
                ..entry(crate::platform::MOVE_TO_TRASH, if vim { "D" } else { "" }, move |this, window, cx| this.trash_tree_entry(target.clone(), window, cx))
            });
        }
        entries.push(menu_separator());
        let target = path.clone();
        entries.push(entry(if cfg!(windows) { "Show in Explorer" } else { "Reveal in Finder" }, "", move |_, _, cx| cx.reveal_path(&target)));
        let target = path.clone();
        entries.push(entry("Copy path", "", move |this, _, cx| {
            let relative = this.relative_to_project(&target);
            cx.write_to_clipboard(ClipboardItem::new_string(relative.clone()));
            this.set_status(format!("Copied {relative}"), cx);
        }));
        let document = path.extension().is_some_and(|extension| extension == "tex" || extension == "typ");
        if document && let Ok(relative) = path.strip_prefix(&root).map(Path::to_path_buf) {
            let current = self.config.main.as_deref() == Some(relative.as_path());
            let label = if current { "✓ Main document" } else { "Set as main document" };
            entries.push(menu_separator());
            entries.push(entry(label, "", move |this, _, cx| this.set_main_document(relative.clone(), cx)));
        }
        entries
    }

    /// Copies files from elsewhere into the project. Files with a name that the
    /// folder already has stay out, so that nothing gets replaced.
    fn prompt_add_files(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(folder) = self.creation_folder(window) else {
            return;
        };
        self.show_files();
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Add".into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(sources))) = paths.await else {
                return;
            };
            let target = folder.clone();
            let report = cx.background_spawn(async move { file_tree::copy_into(&target, &sources) }).await;
            this.update(cx, |this, cx| {
                this.reveal_new_entry(&folder, cx);
                let mut problems = report.failed;
                if !report.existing.is_empty() {
                    problems.push(format!("Already there, not copied: {}", report.existing.join(", ")));
                }
                this.tree_error = (!problems.is_empty()).then(|| problems.join("\n").into());
                if report.copied > 0 {
                    let files = if report.copied == 1 { "file" } else { "files" };
                    this.set_status(format!("Added {} {files}", report.copied), cx);
                }
            })
            .ok();
        })
        .detach();
    }

    fn close_file_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.show_file_panel = false;
        if self.tree_focus.is_focused(window) {
            self.focus_active(window, cx);
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
        self.tree_scroll.scroll_to_item(self.tree_cursor, ScrollStrategy::Nearest);
    }

    // Picker

    fn open_picker(&mut self, kind: PickerKind, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.root.clone() else {
            self.set_status("Open a folder first.", cx);
            return;
        };
        let scan_root = root.clone();
        let language = self.active_language(cx);
        let offered = if kind == PickerKind::Citations { self.offered_citations(cx) } else { Vec::new() };
        let index = self.citations.clone();
        // The labels that `@` refers to in Typst, from the whole document.
        let labels = if kind == PickerKind::Citations && language == Some(Language::Typst) { self.typst_labels(cx) } else { Vec::new() };
        let cited = if kind == PickerKind::Citations { self.cite_slot(cx).map(|slot| slot.cited).unwrap_or_default() } else { Vec::new() };
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
            themes: Arc::new(nvim_themes::Themes::new()),
            original: None,
            citations: None,
            cited: Vec::new(),        });
        window.focus(&self.picker_focus, cx);
        self.picker_files_task = Some(cx.spawn(async move |this, cx| {
            let (files, entries, citations) = cx
                .background_spawn(async move {
                    if let Some(repository) = repository.filter(|_| kind == PickerKind::Branches) {
                        return (Vec::new(), picker::branch_items(repository.branches().unwrap_or_default()), None);
                    }
                    // Citations come from the index, without a walk of the project.
                    if kind == PickerKind::Citations {
                        return (Vec::new(), Vec::new(), Some(Arc::new(bib::CitationRows::new(index, offered, picker::label_items(labels)))));
                    }
                    let files = file_tree::scan(&scan_root, &HashSet::new())
                        .into_iter()
                        .filter(|entry| !entry.is_dir)
                        .map(|entry| entry.path)
                        .filter(|path| match kind {
                            PickerKind::Files | PickerKind::Text | PickerKind::Branches | PickerKind::Themes | PickerKind::Fonts(_) | PickerKind::Commands => true,
                            PickerKind::History => false,
                            PickerKind::Figures => language.is_some_and(|language| snippets::is_figure_file(language, path)),
                            PickerKind::Citations => false,
                        })
                        .collect::<Vec<_>>();
                    (files, Vec::new(), None)
                })
                .await;
            this.update(cx, |this, cx| {
                if let Some(picker) = &mut this.picker {
                    picker.files = Arc::new(files);
                    picker.entries = Arc::new(entries);
                    picker.citations = citations;
                    picker.cited = cited;
                }
                this.refresh_picker(cx);
            })
            .ok();
        }));
        cx.notify();
    }

    /// Every command by name, with its keys. An item's path holds the
    /// command's id, since commands have no file.
    fn open_command_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let os = keymap::host_os();
        let mut entries: Vec<picker::PickerItem> = COMMANDS
            .iter()
            .filter(|command| command.runs_anywhere())
            .map(|command| picker::PickerItem {
                path: PathBuf::from(command.id),
                label: format!("{}: {}", command.group, command.label),
                position: None,
                summary: None,
                detail: Some(self.keymap.keys(command).first().map(|keys| keymap::display(keys, os)).unwrap_or_default()).filter(|keys| !keys.is_empty()),
            })
            .collect();
        for entry in &self.addon_panels {
            let manifest = &entry.panel.read(cx).add_on.manifest;
            for command in &manifest.commands {
                let id = format!("{}{}.{}", keymap::ADDON_PREFIX, manifest.id, command.id);
                let keys = self.keymap.addon_keys(&id, &command.keys);
                entries.push(picker::PickerItem {
                    path: PathBuf::from(id),
                    label: format!("{}: {}", manifest.name, command.label),
                    position: None,
                    summary: None,
                    detail: keys.first().map(|keys| keymap::display(keys, os)),
                });
            }
        }
        self.picker = Some(Picker {
            kind: PickerKind::Commands,
            query: String::new(),
            items: Vec::new(),
            selected: 0,
            root: PathBuf::new(),
            files: Arc::new(Vec::new()),
            entries: Arc::new(entries),
            cancel: Arc::new(AtomicBool::new(false)),
            task: None,
            themes: Arc::default(),
            original: None,
            citations: None,
            cited: Vec::new(),        });
        window.focus(&self.picker_focus, cx);
        self.refresh_picker(cx);
    }

    fn run_palette_command(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((addon, command)) = id.strip_prefix(keymap::ADDON_PREFIX).and_then(|rest| rest.split_once('.')) {
            return self.run_addon_command(&RunAddonCommand { addon: addon.to_string(), command: command.to_string() }, cx);
        }
        let Some(command) = COMMANDS.iter().find(|command| command.id == id) else {
            return;
        };
        // Commands of the editor act on the document.
        if command.context.is_some() {
            self.focus_active(window, cx);
        }
        window.dispatch_action(command.action(), cx);
    }

    /// Lists the cached themes at once and asks Neovim for the current ones.
    fn open_theme_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let cache = nvim_themes::cache_path();
        let themes = cache.as_deref().map(nvim_themes::load_cache).unwrap_or_default();
        self.picker = Some(Picker {
            kind: PickerKind::Themes,
            query: String::new(),
            items: Vec::new(),
            selected: 0,
            root: PathBuf::new(),
            files: Arc::new(Vec::new()),
            entries: Arc::new(picker::theme_items(&themes)),
            cancel: Arc::new(AtomicBool::new(false)),
            task: None,
            themes: Arc::new(themes),
            original: Some(theme::palette()),
            citations: None,
            cited: Vec::new(),        });
        window.focus(&self.picker_focus, cx);
        self.refresh_picker(cx);
        self.picker_files_task = Some(cx.spawn(async move |this, cx| {
            let scanned = cx
                .background_spawn(async move {
                    let themes = nvim_themes::scan()?;
                    if let Some(cache) = &cache {
                        nvim_themes::save_cache(cache, &themes).ok();
                    }
                    Ok::<_, String>(themes)
                })
                .await;
            this.update(cx, |this, cx| match scanned {
                Ok(themes) => {
                    if let Some(picker) = this.picker.as_mut().filter(|picker| picker.kind == PickerKind::Themes) {
                        picker.entries = Arc::new(picker::theme_items(&themes));
                        picker.themes = Arc::new(themes);
                        this.refresh_picker(cx);
                    }
                }
                Err(message) => this.set_status(message, cx),
            })
            .ok();
        }));
        cx.notify();
    }

    /// Lists the earlier versions of `path`, newest first, with how much each
    /// differs from the text now.
    fn open_history_picker(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let Some(directory) = history::directory() else {
            return;
        };
        let current = match self.document_at(&path, cx) {
            Some(index) => Some(self.documents[index].view.read(cx).state.buffer().clone()),
            None => None,
        };
        self.picker = Some(Picker {
            kind: PickerKind::History,
            query: String::new(),
            items: Vec::new(),
            selected: 0,
            // The file whose versions these are.
            root: path.clone(),
            files: Arc::new(Vec::new()),
            entries: Arc::new(Vec::new()),
            cancel: Arc::new(AtomicBool::new(false)),
            task: None,
            themes: Arc::new(nvim_themes::Themes::new()),
            original: None,
            citations: None,
            cited: Vec::new(),        });
        window.focus(&self.picker_focus, cx);
        self.picker_files_task = Some(cx.spawn(async move |this, cx| {
            let entries = cx
                .background_spawn(async move {
                    let current = match current {
                        Some(buffer) => buffer.to_string(),
                        None => text_file::read(&path).map(|loaded| loaded.text).unwrap_or_default(),
                    };
                    history_items(&history::versions(&directory, &path), &current)
                })
                .await;
            this.update(cx, |this, cx| {
                if let Some(picker) = this.picker.as_mut().filter(|picker| picker.kind == PickerKind::History) {
                    picker.entries = Arc::new(entries);
                    this.refresh_picker(cx);
                }
            })
            .ok();
        }));
        cx.notify();
    }

    /// Puts an earlier version in place as one edit, which undo takes back.
    fn restore_version(&mut self, path: PathBuf, item: PickerItem, cx: &mut Context<Self>) {
        let loaded = match history::read(&item.path).and_then(text_file::decode) {
            Ok(loaded) => loaded,
            Err(error) => return self.set_status(format!("Cannot read that version: {error}"), cx),
        };
        let Some(view) = self.document_view(&path, cx) else {
            return;
        };
        if let Some(index) = self.document_at(&path, cx) {
            self.active = Some(index);
        }
        view.update(cx, |view, cx| {
            let everything = 0..view.state.buffer().len();
            view.edit_from_outside(everything, &loaded.text, cx);
        });
        self.set_status(format!("Restored the version from {}. Undo brings the text back, and saving keeps it.", item.label.to_lowercase()), cx);
    }

    fn open_font_picker(&mut self, slot: FontSlot, window: &mut Window, cx: &mut Context<Self>) {
        let names = cx.text_system().all_font_names();
        self.picker = Some(Picker {
            kind: PickerKind::Fonts(slot),
            query: String::new(),
            items: Vec::new(),
            selected: 0,
            root: PathBuf::new(),
            files: Arc::new(Vec::new()),
            entries: Arc::new(picker::font_items(names)),
            cancel: Arc::new(AtomicBool::new(false)),
            task: None,
            themes: Arc::new(nvim_themes::Themes::new()),
            original: None,
            citations: None,
            cited: Vec::new(),        });
        window.focus(&self.picker_focus, cx);
        self.refresh_picker(cx);
        cx.notify();
    }

    /// The settings with the font in the slot set, None for the default of the mode.
    fn with_font(&self, slot: FontSlot, label: &str) -> Settings {
        let font = (label != picker::DEFAULT_FONT).then(|| label.to_string());
        let mut settings = self.settings.clone();
        match slot {
            FontSlot::Editor => settings.editor_font = font,
            FontSlot::Interface => settings.ui_font = font,
        }
        settings
    }

    /// Lays out the documents again in the type that is set now.
    fn relayout(&self, cx: &mut Context<Self>) {
        for document in &self.documents {
            document.view.update(cx, |view, cx| view.forget_layout(cx));
        }
        cx.refresh_windows();
    }

    /// Shows the selected font, so that moving through the list previews it.
    fn preview_font(&mut self, cx: &mut Context<Self>) {
        let Some((PickerKind::Fonts(slot), Some(item))) =
            self.picker.as_ref().map(|picker| (picker.kind, picker.items.get(picker.selected).cloned()))
        else {
            return;
        };
        let look = self.with_font(slot, &item.label).look();
        if look != theme::look() {
            theme::set_look(look);
            self.relayout(cx);
        }
    }

    /// Shows the selected theme, so that moving through the list previews it.
    fn preview_theme(&mut self, cx: &mut Context<Self>) {
        let Some(picker) = self.picker.as_ref().filter(|picker| picker.kind == PickerKind::Themes) else {
            return;
        };
        let Some(palette) = picker
            .items
            .get(picker.selected)
            .and_then(|item| theme::built_in(&item.label).or_else(|| picker.themes.get(&item.label).copied()))
        else {
            return;
        };
        if palette != theme::palette() {
            theme::set_palette(palette);
            cx.refresh_windows();
        }
    }

    fn close_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(picker) = self.picker.take() {
            picker.cancel.store(true, Ordering::Relaxed);
            if let Some(original) = picker.original {
                theme::set_palette(original);
                cx.refresh_windows();
            }
            if matches!(picker.kind, PickerKind::Fonts(_)) && theme::look() != self.settings.look() {
                theme::set_look(self.settings.look());
                self.relayout(cx);
            }
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
                picker.items = picker.citations.as_ref().map(|rows| rows.matches(&picker.query, &picker.cited)).unwrap_or_default();
                picker.selected = 0;
            }
            PickerKind::History | PickerKind::Commands => {
                picker.items = picker::match_in_order(&picker.entries, &picker.query);
                picker.selected = 0;
            }
            PickerKind::Branches => {
                picker.items = picker::match_branches(&picker.entries, &picker.query);
                picker.selected = 0;
            }
            PickerKind::Themes => {
                picker.items = picker::match_in_order(&picker.entries, &picker.query);
                // Without a query the list starts at the theme in use.
                let current = self.settings.theme.as_deref().unwrap_or(theme::DEFAULT_THEME);
                picker.selected = if picker.query.trim().is_empty() {
                    picker.items.iter().position(|item| item.label == current).unwrap_or(0)
                } else {
                    0
                };
                self.picker_scroll.scroll_to_item(picker.selected);
                self.preview_theme(cx);
            }
            PickerKind::Fonts(slot) => {
                picker.items = picker::match_in_order(&picker.entries, &picker.query);
                // Without a query the list starts at the font in use.
                let current = match slot {
                    FontSlot::Editor => self.settings.editor_font.as_deref(),
                    FontSlot::Interface => self.settings.ui_font.as_deref(),
                }
                .unwrap_or(picker::DEFAULT_FONT);
                picker.selected = if picker.query.trim().is_empty() {
                    picker.items.iter().position(|item| item.label == current).unwrap_or(0)
                } else {
                    0
                };
                self.picker_scroll.scroll_to_item(picker.selected);
                self.preview_font(cx);
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
        self.preview_theme(cx);
        self.preview_font(cx);
        cx.notify();
    }

    fn accept_picker(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some((kind, item, target)) =
            self.picker.as_ref().and_then(|picker| Some((picker.kind, picker.items.get(index).cloned()?, picker.root.clone())))
        else {
            return;
        };
        if let Some(picker) = &mut self.picker {
            // The previewed theme stays.
            picker.original = None;
        }
        self.close_picker(window, cx);
        match kind {
            PickerKind::Files | PickerKind::Text => {}
            PickerKind::Commands => return self.run_palette_command(&item.path.to_string_lossy(), window, cx),
            PickerKind::Themes => {
                let name = (item.label != theme::DEFAULT_THEME).then_some(item.label);
                // A theme is a choice against paper, which would cover it.
                return self.update_settings(
                    |settings| {
                        settings.theme = name;
                        settings.paper_mode = false;
                    },
                    window,
                    cx,
                );
            }
            PickerKind::Fonts(slot) => {
                let chosen = self.with_font(slot, &item.label);
                return self.update_settings(|settings| *settings = chosen, window, cx);
            }
            PickerKind::Figures => return self.insert_figure(&item.path, cx),
            PickerKind::History => return self.restore_version(target, item, cx),
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
                for entry in &self.addon_panels {
                    entry.panel.read(cx).notify_citation_picked(&item.label);
                }
                self.insert_citation(&item.label, false, cx);
                return;
            }
        }
        self.open_file(item.path, window, cx);
        if let (Some((line, column)), Some(view)) = (item.position, self.active_view()) {
            view.update(cx, |view, cx| view.go_to_line(line, column, window, cx));
        }
    }

    /// Writes a citation at the cursor, where the document takes one, and else
    /// puts the key on the clipboard. Says which.
    /// With `more`, another key follows, so the list stays open after a comma.
    fn insert_citation(&mut self, key: &str, more: bool, cx: &mut Context<Self>) -> String {
        if let Some(slot) = self.cite_slot(cx) {
            let mut text = editor_core::snippet::literal(key);
            if slot.before_key || more {
                text.push_str(", ");
            }
            if slot.close && !more {
                text.push('}');
            }
            self.insert_snippet(&text, false, cx);
            // The next thing to type is the sentence, not another key.
            let at_brace = self.active_view().and_then(|view| Self::around_cursor(view.read(cx))).is_some_and(|(_, _, after)| after.starts_with('}'));
            if !more && !slot.close && !slot.before_key && at_brace {
                self.with_active(cx, |view, cx| view.step_forward(cx));
            }
            return format!("Cited {key} at the cursor.");
        }
        // The `@` or `cite(<` that opened the picker is already there.
        if self.active_language(cx) == Some(Language::Typst)
            && let Some((_, before, after)) = self.active_view().and_then(|view| Self::around_cursor(view.read(cx)))
            && snippets::opens_reference(&before)
        {
            let mut text = editor_core::snippet::literal(key);
            if snippets::opens_typst_cite(&before) && !after.starts_with('>') {
                text.push('>');
            }
            if more {
                text.push_str(if snippets::opens_typst_cite(&before) { ") #cite(<" } else { " @" });
            }
            self.insert_snippet(&text, false, cx);
            return format!("Cited {key} at the cursor.");
        }
        // The `[@` that opened the picker is already there.
        if self.active_language(cx) == Some(Language::Markdown)
            && let Some((_, before, after)) = self.active_view().and_then(|view| Self::around_cursor(view.read(cx)))
            && snippets::opens_markdown_cite(&before)
        {
            let mut text = editor_core::snippet::literal(key);
            if more {
                text.push_str("; @");
            } else if !after.starts_with(']') {
                text.push(']');
            }
            self.insert_snippet(&text, false, cx);
            return format!("Cited {key} at the cursor.");
        }
        match self.active_language(cx) {
            Some(language @ (Language::Latex | Language::Typst | Language::Markdown)) => {
                self.insert_snippet(&snippets::citation(language, key), false, cx);
                format!("Cited {key} at the cursor.")
            }
            _ => {
                cx.write_to_clipboard(ClipboardItem::new_string(key.to_string()));
                format!("{key} is on the clipboard.")
            }
        }
    }

    fn dialog_panel(&self, cx: &App) -> Option<Entity<AddonPanel>> {
        self.addon_panels.iter().map(|entry| &entry.panel).find(|panel| panel.read(cx).has_dialog()).cloned()
    }

    fn lookup_doi(&mut self, cx: &mut Context<Self>) {
        if self.dialog_panel(cx).is_some() {
            return;
        }
        let Some(panel) = self.addon_panels.iter().map(|entry| entry.panel.clone()).find(|panel| panel.read(cx).looks_up_dois()) else {
            return self.set_status("Turn on an add-on that looks up DOIs, such as Zotero, in the Add-ons panel.", cx);
        };
        if self.root.is_none() {
            return self.set_status("Open a project to look up a DOI.", cx);
        }
        let selection = self.active_view().and_then(|view| view.read(cx).state.selected_text());
        let clipboard = cx.read_from_clipboard().and_then(|item| item.text());
        let suggestion = [selection, clipboard].into_iter().flatten().find_map(|text| doi_suggestion(&text)).unwrap_or_default();
        if let Err(reason) = panel.update(cx, |panel, cx| panel.lookup_doi(suggestion, cx)) {
            self.set_status(reason, cx);
        }
    }

    fn close_addon_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(panel) = self.dialog_panel(cx) {
            panel.update(cx, |panel, cx| panel.close_dialog(cx));
            self.addon_dialog_changed = false;
            self.addon_dialog_open = false;
            self.focus_active(window, cx);
            cx.notify();
        }
    }

    /// The dialog is drawn here, and only here is the window at hand to move the focus.
    fn settle_addon_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let panel = self.dialog_panel(cx);
        let open = panel.is_some();
        if std::mem::take(&mut self.addon_dialog_changed) || open != self.addon_dialog_open {
            match panel.map(|panel| panel.read(cx).dialog_input(cx)) {
                Some(Some(input)) => window.focus(&input, cx),
                Some(None) => window.focus(&self.addon_dialog_focus, cx),
                None if self.addon_dialog_open => self.focus_active(window, cx),
                None => {}
            }
        }
        self.addon_dialog_open = open;
    }

    fn render_addon_dialog(&mut self, panel: Entity<AddonPanel>, cx: &mut Context<Self>) -> AnyElement {
        let content = panel.update(cx, |panel, cx| panel.render_dialog(cx));
        div()
            .id("addon-dialog-backdrop")
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .flex()
            .justify_center()
            .items_start()
            .pt(px(TOP_BAR_HEIGHT + 40.))
            .bg(black().opacity(0.4))
            .occlude()
            .track_focus(&self.addon_dialog_focus)
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| this.close_addon_dialog(window, cx)))
            .child(
                div()
                    .id("addon-dialog")
                    .occlude()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .w(px(PICKER_WIDTH))
                    .max_h(gpui::relative(0.8))
                    .h_auto()
                    .p(px(12.))
                    .rounded(px(8.))
                    .overflow_hidden()
                    .bg(theme::color(theme::palette().bg))
                    .border_1()
                    .border_color(theme::color(theme::palette().divider))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .children(content),
            )
            .into_any_element()
    }

    /// The labels of all open documents, once each.
    fn document_labels(&self, cx: &App) -> Vec<String> {
        let mut labels: Vec<String> = self
            .documents
            .iter()
            .flat_map(|document| document.view.read(cx).symbols().to_vec())
            .filter(|symbol| symbol.kind == SymbolKind::Label)
            .map(|symbol| symbol.name)
            .collect();
        labels.sort();
        labels.dedup();
        labels
    }

    fn offered_citations(&self, cx: &App) -> Vec<(String, String)> {
        self.addon_panels
            .iter()
            .flat_map(|entry| entry.panel.read(cx).citations())
            .map(|entry| (entry.key.clone(), entry.detail.clone()))
            .collect()
    }

    /// The text before and after a single cursor, on its line.
    fn around_cursor(view: &EditorView) -> Option<(usize, String, String)> {
        let state = &view.state;
        let primary = state.selections().primary();
        if state.selections().len() != 1 || !primary.is_empty() {
            return None;
        }
        let head = primary.head;
        let line = state.buffer().line_text(state.buffer().offset_to_line(head));
        let column = head - state.buffer().line_start(state.buffer().offset_to_line(head));
        let (before, after) = line.split_at(column);
        Some((head, before.to_string(), after.to_string()))
    }

    /// Where a key goes when the cursor is in the braces of a LaTeX citation.
    fn cite_slot(&self, cx: &App) -> Option<snippets::CiteSlot> {
        if self.active_language(cx) != Some(Language::Latex) {
            return None;
        }
        let view = self.active_view()?;
        let (_, before, after) = Self::around_cursor(view.read(cx))?;
        snippets::cite_slot(&before, &after)
    }

    /// Opens the entry or label a Cmd+click named, with its name selected.
    fn go_to_definition(&mut self, path: PathBuf, range: std::ops::Range<usize>, line: usize, cx: &mut Context<Self>) {
        self.definition_soon = Some((path, range, line));
        cx.notify();
    }

    /// Cites the chosen entry and keeps the list open for the next key.
    fn cite_another(&mut self, cx: &mut Context<Self>) {
        let Some(item) = self.picker.as_ref().and_then(|picker| picker.items.get(picker.selected).cloned()) else {
            return;
        };
        for entry in &self.addon_panels {
            entry.panel.read(cx).notify_citation_picked(&item.label);
        }
        self.insert_citation(&item.label, true, cx);
        if let Some(picker) = &mut self.picker {
            picker.cited.push(item.label);
            picker.query.clear();
        }
        self.refresh_picker(cx);
    }

    /// The labels of the whole document, which Typst's `@` refers to.
    fn typst_labels(&self, cx: &App) -> Vec<String> {
        let mut labels = self.document_labels(cx);
        labels.extend(self.citations.labels.keys().cloned());
        labels.sort();
        labels.dedup();
        labels
    }

    /// Opens the picker when `\cite{`, or `@` in Typst, was typed and there is
    /// something to pick. Taking the key back with Backspace does not open it
    /// again at that place.
    fn check_citation_trigger(&mut self, view: &Entity<EditorView>, cx: &mut Context<Self>) {
        let language = self.active_language(cx);
        if self.picker.is_some() || self.active_view().as_ref() != Some(view) || !matches!(language, Some(Language::Latex | Language::Typst | Language::Markdown)) {
            return;
        }
        let Some((head, before, _)) = Self::around_cursor(view.read(cx)) else {
            return;
        };
        let id = view.entity_id();
        if self.citation_trigger.is_some_and(|(view, offset)| view != id || head < offset) {
            self.citation_trigger = None;
        }
        let after = Self::around_cursor(view.read(cx)).map(|(_, _, after)| after).unwrap_or_default();
        let opens = match language {
            Some(Language::Typst) => snippets::opens_reference(&before),
            Some(Language::Markdown) => snippets::opens_markdown_cite(&before),
            _ => snippets::cite_slot(&before, &after).is_some(),
        };
        if !opens || self.citation_trigger == Some((id, head)) {
            return;
        }
        self.citation_trigger = Some((id, head));
        if self.root.is_none() {
            return;
        }
        // Before the first index is built, an empty one says nothing, so it opens anyway.
        let available = !self.citations_ready
            || !self.citations.entries.is_empty()
            || !self.offered_citations(cx).is_empty()
            || (language == Some(Language::Typst) && !self.typst_labels(cx).is_empty());
        if available {
            self.open_citations_soon = true;
            cx.notify();
        }
    }

    fn insert_figure(&mut self, path: &Path, cx: &mut Context<Self>) {
        let Some(language) = self.active_language(cx) else {
            return;
        };
        let active = self.active_view().and_then(|view| view.read(cx).path.clone());
        // Typst and Markdown look next to the current file, LaTeX next to the main file.
        let main = match language {
            Language::Typst | Language::Markdown => active,
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
        if self.shown_image.is_some() {
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
        if self.pdf_is_main() {
            window.focus(&self.pdf.read(cx).focus_handle(), cx);
        } else if let Some(view) = self.active_view() {
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

    /// Gives the active view what the other open documents define, and the
    /// labels of the whole document from the citation index. Those change
    /// rarely, so this runs on a switch or a new index and not on every key.
    fn sync_project_symbols(&mut self, cx: &mut Context<Self>) {
        let Some(active) = self.active else {
            return;
        };
        let key = (active, self.documents.len(), Arc::as_ptr(&self.citations) as usize);
        if key == self.symbols_version {
            return;
        }
        self.symbols_version = key;
        let mut symbols: Vec<Symbol> = self
            .documents
            .iter()
            .enumerate()
            .filter(|(index, _)| *index != active)
            .flat_map(|(_, document)| document.view.read(cx).symbols().to_vec())
            .collect();
        symbols.extend(self.citations.labels.keys().map(|name| Symbol { kind: SymbolKind::Label, name: name.clone() }));
        let details = Arc::new(citation_index::label_details(&self.citations));
        if let Some(document) = self.documents.get(active) {
            document.view.update(cx, |view, _| {
                view.set_project_symbols(Arc::new(symbols));
                view.set_label_details(details);
            });
        }
    }

    fn on_root_mouse_move(&mut self, event: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(state) = &mut self.presenting {
            if !state.controls_shown {
                state.controls_shown = true;
                cx.notify();
            }
            state.hide_controls_task = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(PRESENT_CONTROLS_TIME).await;
                this.update(cx, |this, cx| {
                    if let Some(state) = &mut this.presenting {
                        state.controls_shown = false;
                        cx.notify();
                    }
                })
                .ok();
            }));
        }
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

    fn menu_entries(&self, menu: Menu, cx: &App) -> Vec<MenuEntry> {
        let mut entries = self.base_menu_entries(menu, cx);
        // Only a file in an older encoding needs a way out of it.
        if menu == Menu::File
            && let Some(view) = self.active_view()
            && view.read(cx).encoding() != text_file::UTF_8
        {
            let save = entries.len() - 1;
            entries.insert(save, entry("Convert to UTF-8", "", |this, _, cx| this.with_active(cx, |view, cx| view.convert_to_utf8(cx))));
        }
        #[cfg(feature = "collab")]
        self.add_collab_entries(menu, &mut entries);
        entries
    }

    fn base_menu_entries(&self, menu: Menu, cx: &App) -> Vec<MenuEntry> {
        let check = |on: bool, label: &str| format!("{} {label}", if on { "✓" } else { "  " });
        match menu {
            Menu::File => vec![
                entry("New file…", self.shortcut("new_file"), |this, window, cx| this.open_tree_input(false, window, cx)),
                entry("New folder…", "", |this, window, cx| this.open_tree_input(true, window, cx)),
                entry("Add files…", "", |this, window, cx| this.prompt_add_files(window, cx)),
                entry("Make a copy", "", |this, window, cx| this.make_copy(window, cx)),
                menu_separator(),
                entry("Earlier versions of this file…", "", |this, window, cx| {
                    if let Some(path) = this.active_view().and_then(|view| view.read(cx).path.clone()) {
                        this.open_history_picker(path, window, cx);
                    }
                }),
                entry("Version history", "", |this, _, cx| this.show_version_history(cx)),
                entry("Commit and sync…", self.shortcut("git_panel"), |this, window, cx| this.open_git_panel(window, cx)),
                entry("Word count…", "", |this, window, cx| this.show_word_count(window, cx)),
                menu_separator(),
                menu_heading("Download"),
                entry("Source (.zip)…", "", |this, window, cx| this.download_source(window, cx)),
                entry("PDF…", "", |this, window, cx| this.download_pdf(window, cx)),
                entry("Word draft (.docx)…", "", |this, window, cx| this.download_draft(DraftFormat::Word, window, cx)),
                entry("HTML draft…", "", |this, window, cx| this.download_draft(DraftFormat::Html, window, cx)),
                entry("Markdown draft…", "", |this, window, cx| this.download_draft(DraftFormat::Markdown, window, cx)),
                menu_separator(),
                entry("Project hub…", self.shortcut("project_hub"), |this, window, cx| this.open_hub(window, cx)),
                entry("Open folder…", self.shortcut("open_folder"), |this, window, cx| this.prompt_open_folder(window, cx)),
                entry("Save project as template…", "", |this, window, cx| this.save_project_as_template(window, cx)),
                entry("Save", self.shortcut("save"), |this, window, cx| this.save_active(window, cx)),
                entry("Recompile", self.shortcut("recompile"), |this, _, cx| this.recompile_by_hand(cx)),
            ],
            Menu::Edit => vec![
                entry("Undo", self.shortcut("undo"), |this, _, cx| this.with_active(cx, |view, cx| view.undo_edit(cx))),
                entry("Redo", self.shortcut("redo"), |this, _, cx| this.with_active(cx, |view, cx| view.redo_edit(cx))),
            ],
            Menu::Insert => vec![
                entry("Citation from a DOI", "", |this, _, cx| this.lookup_doi(cx)),
                entry("Inline math", "", |this, _, cx| {
                    this.with_active(cx, |view, cx| view.format(FormatKind::Math, cx))
                }),
                entry("Link", "", |this, _, cx| {
                    this.with_active(cx, |view, cx| view.format(FormatKind::Link, cx))
                }),
            ],
            Menu::View | Menu::Layout => {
                let layout = self.layout;
                let layout_entry = |target: PaneLayout, label: &str, shortcut: SharedString| {
                    entry(check(layout == target, label), shortcut, move |this, window, cx| this.set_layout(target, window, cx))
                };
                vec![
                    menu_heading("Layout"),
                    layout_entry(PaneLayout::Split, "Split view", self.shortcut("layout_split")),
                    layout_entry(PaneLayout::EditorOnly, "Editor only", self.shortcut("layout_editor")),
                    layout_entry(PaneLayout::PdfOnly, "PDF only", self.shortcut("layout_pdf")),
                    layout_entry(PaneLayout::PdfTab, "Open PDF in separate tab", self.shortcut("layout_pdf_tab")),
                    menu_separator(),
                    entry(check(self.focus_mode, "Focus mode"), self.shortcut("focus_mode"), |this, window, cx| this.toggle_focus_mode(window, cx)),
                    entry("  Presentation mode", "", |this, window, cx| this.start_presentation(window, cx)),
                    menu_separator(),
                    entry(check(self.settings.show_breadcrumbs, "Show breadcrumbs"), "", |this, window, cx| {
                        this.update_settings(|settings| settings.show_breadcrumbs = !settings.show_breadcrumbs, window, cx);
                    }),
                    entry(check(self.settings.show_tabs, "Show editor tabs"), "", |this, window, cx| {
                        this.update_settings(|settings| settings.show_tabs = !settings.show_tabs, window, cx);
                    }),
                    menu_separator(),
                    entry("  Toggle file panel", "", |this, _, cx| {
                        this.show_file_panel = !this.show_file_panel;
                        cx.notify();
                    }),
                    entry("  Toggle logs", "", |this, _, cx| {
                        this.toggle_logs();
                        cx.notify();
                    }),
                ]
            }
            Menu::Format => vec![
                entry("Bold", self.shortcut("bold"), |this, _, cx| {
                    this.with_active(cx, |view, cx| view.format(FormatKind::Bold, cx))
                }),
                entry("Italic", self.shortcut("italic"), |this, _, cx| {
                    this.with_active(cx, |view, cx| view.format(FormatKind::Italic, cx))
                }),
            ],
            Menu::Tree => self.tree_menu_entries(),
            Menu::EditorContext => self.editor_menu_entries(cx),
            Menu::Symbols | Menu::Table | Menu::Code => Vec::new(),
            Menu::Help => {
                let mut entries = vec![
                    entry("Editor tour", "", |this, window, cx| this.start_tour(window, cx)),
                    entry("Keyboard shortcuts", "", |this, window, cx| {
                        this.open_dialog(Dialog::Shortcuts, window, cx);
                    }),
                    entry("Writing tools…", "", |this, window, cx| this.open_dialog(Dialog::Settings(SettingsPage::Tools), window, cx)),
                ];
                // The Windows installer puts the command on the PATH itself.
                if !cfg!(windows) {
                    entries.push(entry("Add editor command for Terminal…", "", |this, _, cx| this.install_shell_command(cx)));
                }
                entries.push(entry("Report a problem…", "", |this, window, cx| this.open_report(window, cx)));
                entries
            }
            Menu::Compile => {
                let trigger = self.config.trigger;
                let engine = self.config.engine;
                let debounce = self.config.debounce_ms;
                vec![
                    entry("Recompile", self.shortcut("recompile"), |this, _, cx| this.recompile_by_hand(cx)),
                    entry("Recompile from scratch", "", |this, _, cx| this.recompile_from_scratch(cx)),
                    entry("Stop build", "", |this, _, _| this.build.cancel()),
                    entry(check(trigger == Trigger::Save, "Compile on save"), "", |this, _, cx| this.set_trigger(Trigger::Save, cx)),
                    entry(
                        check(trigger == Trigger::Type, &format!("Compile on type, autosaves after {debounce} ms")),
                        "",
                        |this, _, cx| this.set_trigger(Trigger::Type, cx),
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
            .hover(|style| style.bg(theme::color(theme::palette().divider)))
            .on_click(cx.listener(move |this, _, window, cx| handler(this, window, cx)))
            .child(Self::icon(name, 16., theme::palette().ui_text))
    }

    /// A button inside the floating toolbar, with a spot as round as the toolbar.
    fn tool_button(
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
            .rounded(px(TOOL_RADIUS))
            .cursor_pointer()
            .hover(|style| style.bg(theme::color(theme::palette().divider).opacity(0.55)))
            .on_click(cx.listener(move |this, _, window, cx| handler(this, window, cx)))
            .child(Self::icon(name, 16., theme::palette().ui_text))
    }

    fn menu_button(&self, menu: Menu, label: &'static str, cx: &mut Context<Self>) -> impl IntoElement {
        let open = self.open_menu == Some(menu);
        window_controls::clickable(div().id(label))
            .px(px(9.))
            .h(px(28.))
            .flex()
            .items_center()
            .rounded(px(4.))
            .cursor_pointer()
            .when(open, |style| style.bg(theme::color(theme::palette().panel)))
            .hover(|style| style.bg(theme::color(theme::palette().panel)))
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                this.toggle_menu(menu, event.position(), TOP_BAR_HEIGHT, window, cx);
            }))
            .child(label)
            .when(open, |element| element.child(self.render_menu(menu, cx)))
    }

    /// The tree's menu is a sheet of the side panel: a raised tone without a
    /// frame, and a pill under the row in reach.
    fn render_menu(&self, menu: Menu, cx: &mut Context<Self>) -> impl IntoElement {
        let items = self.menu_entries(menu, cx);
        let palette = theme::palette();
        let paper = matches!(menu, Menu::Tree | Menu::EditorContext);
        deferred(
            anchored()
                .anchor(Anchor::TopLeft)
                .position(self.menu_origin)
                .child(
                    div()
                        .id("menu")
                        .occlude()
                        .min_w(px(220.))
                        // A menu can open inside an accent button, whose text would carry over.
                        .text_color(theme::color(palette.ui_text))
                        .font_weight(FontWeight::NORMAL)
                        .when(paper, |style| style.p(px(6.)).bg(theme::color(theme::raised(&palette))).shadow_md())
                        .when(!paper, |style| {
                            style
                                .py(px(4.))
                                .rounded(px(4.))
                                .bg(theme::color(palette.panel))
                                .border_1()
                                .border_color(theme::color(palette.divider))
                                .shadow_lg()
                        })
                        .children(items.into_iter().enumerate().map(|(index, item)| {
                            match item.kind {
                                MenuEntryKind::Separator => {
                                    return div()
                                        .my(px(4.))
                                        .when(paper, |line| line.mx(px(12.)))
                                        .h(px(1.))
                                        .bg(theme::color(palette.divider))
                                        .into_any_element();
                                }
                                MenuEntryKind::Heading => {
                                    return div()
                                        .px(px(12.))
                                        .py(px(3.))
                                        .text_color(theme::color(theme::palette().ui_muted))
                                        .child(item.label.clone())
                                        .into_any_element();
                                }
                                MenuEntryKind::Action => {}
                            }
                            let handler = item.handler.clone();
                            div()
                                .id(index)
                                .flex()
                                .justify_between()
                                .gap(px(24.))
                                .px(px(12.))
                                .py(px(5.))
                                .cursor_pointer()
                                .when(item.danger, |style| style.text_color(theme::color(palette.error)))
                                .when(paper, |style| style.h(px(32.)).items_center().py_0().rounded_full().hover(|style| style.bg(theme::color(palette.panel))))
                                .when(!paper, |style| style.hover(|style| style.bg(theme::color(palette.accent_soft))))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.close_menu(window, cx);
                                    handler(this, window, cx);
                                    cx.notify();
                                }))
                                .child(item.label.clone())
                                .child(
                                    div()
                                        .text_color(theme::color(theme::palette().ui_muted))
                                        .child(item.shortcut.clone()),
                                )
                                .into_any_element()
                        })),
                ),
        )
    }

    fn render_top_bar(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let title = self
            .root
            .as_deref()
            .and_then(|root| root.file_name())
            .map_or_else(|| "No project".to_string(), |name| name.to_string_lossy().into_owned());
        window_controls::drag_area(div().id("top-bar"))
            .relative()
            .flex()
            .flex_none()
            .items_center()
            .justify_between()
            .h(px(TOP_BAR_HEIGHT))
            .pl(px(TRAFFIC_LIGHT_SPACE))
            .pr(px(if cfg!(windows) { 0. } else { 10. }))
            .border_b_1()
            .border_color(theme::color(theme::palette().panel))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(2.))
                    .child(
                        window_controls::clickable(div().id("hub-logo"))
                            .relative()
                            .children(self.tour_anchor(TourAnchor::Home))
                            .flex()
                            .flex_none()
                            .items_center()
                            .justify_center()
                            .size(px(28.))
                            .mr(px(4.))
                            .rounded(px(4.))
                            .cursor_pointer()
                            .when(self.logo_hovered, |style| style.bg(theme::color(theme::palette().divider)))
                            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                                this.logo_hovered = *hovered;
                                cx.notify();
                            }))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.logo_hovered = false;
                                this.open_hub(window, cx);
                            }))
                            .child(if self.logo_hovered {
                                Self::icon("home", 18., theme::palette().ui_text)
                            } else {
                                Self::icon("pencil", 18., theme::palette().ui_text)
                            }),
                    )
                    .child(div().relative().child(self.menu_button(Menu::File, "File", cx)).children(self.tour_anchor(TourAnchor::FileMenu)))
                    .child(self.menu_button(Menu::Edit, "Edit", cx))
                    .child(self.menu_button(Menu::Insert, "Insert", cx))
                    .child(self.menu_button(Menu::View, "View", cx))
                    .child(self.menu_button(Menu::Format, "Format", cx))
                    .child(div().relative().child(self.menu_button(Menu::Help, "Help", cx)).children(self.tour_anchor(TourAnchor::HelpMenu))),
            )
            .child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .flex()
                    .justify_center()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(
                        window_controls::clickable(div().id("project-title"))
                            .px(px(8.))
                            .py(px(2.))
                            .rounded(px(4.))
                            .cursor_pointer()
                            .hover(|style| style.bg(theme::color(theme::palette().divider)))
                            .on_click(cx.listener(|this, _, window, cx| this.open_hub(window, cx)))
                            .child(title),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(self.menu_button(Menu::Layout, "Layout", cx))
                    .children(window_controls::buttons(window, TOP_BAR_HEIGHT)),
            )
    }

    /// A round place on the rail. The open panel sits on a raised disc, the rest
    /// only shade on hover.
    fn rail_spot(id: impl Into<gpui::ElementId>, on: bool) -> gpui::Stateful<gpui::Div> {
        let palette = theme::palette();
        div()
            .id(id)
            .flex()
            .items_center()
            .justify_center()
            .size(px(32.))
            .rounded_full()
            .cursor_pointer()
            .when(on, |style| style.bg(theme::color(theme::raised(&palette))))
            .when(!on, |style| style.hover(|style| style.bg(theme::color(palette.divider).opacity(0.35))))
    }

    fn render_rail(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = theme::palette();
        let rail_button = |id: &'static str, icon: &'static str, panel: LeftPanel, cx: &mut Context<Self>| -> gpui::Stateful<gpui::Div> {
            let on = self.show_file_panel && self.left_panel == panel;
            Self::rail_spot(id, on)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.show_file_panel = !(this.show_file_panel && this.left_panel == panel);
                    this.left_panel = panel;
                    cx.notify();
                }))
                .child(Self::icon(icon, 18., palette.ui_text))
        };
        #[cfg(feature = "collab")]
        let collab_rail = Some(rail_button("rail-collab", "people", LeftPanel::Collab, cx));
        #[cfg(not(feature = "collab"))]
        let collab_rail: Option<gpui::Stateful<gpui::Div>> = None;
        div()
            .flex()
            .flex_none()
            .flex_col()
            .justify_between()
            .items_center()
            .w(px(RAIL_WIDTH))
            .h_full()
            .py(px(10.))
            .bg(theme::color(palette.panel))
            .border_r_1()
            .border_color(theme::color(palette.divider))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(8.))
                    .child(rail_button("rail-files", "file", LeftPanel::Files, cx))
                    .child(rail_button("rail-git", "git-branch", LeftPanel::Git, cx))
                    .when(addons::ENABLED, |rail| rail.child(rail_button("rail-addons", "addon", LeftPanel::Addons, cx)))
                    .children(collab_rail)
                    .children(self.addon_panels.iter().enumerate().map(|(index, entry)| {
                        let panel = LeftPanel::Addon(index);
                        let on = self.show_file_panel && self.left_panel == panel;
                        let icon = entry.panel.read(cx).add_on.icon();
                        let path: SharedString = icon.map_or_else(|| "icons/puzzle.svg".to_string(), |icon| icon.display().to_string()).into();
                        Self::rail_spot(("rail-addon", index), on)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.show_file_panel = !(this.show_file_panel && this.left_panel == panel);
                                this.left_panel = panel;
                                cx.notify();
                            }))
                            .child(svg().path(path).size(px(18.)).flex_none().text_color(theme::color(palette.ui_text)))
                    })),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(8.))
                    .child(div().w(px(16.)).h(px(1.)).mb(px(4.)).bg(theme::color(palette.divider)))
                    .child(
                        div()
                            .relative()
                            .child(
                                Self::rail_spot("rail-logs", false)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.toggle_logs();
                                        cx.notify();
                                    }))
                                    .child(Self::icon("log", 18., palette.ui_text)),
                            )
                            .children(self.tour_anchor(TourAnchor::Logs)),
                    )
                    .child(
                        Self::rail_spot("rail-help", false)
                            .on_click(cx.listener(|this, _, window, cx| this.open_dialog(Dialog::Shortcuts, window, cx)))
                            .child(Self::icon("help", 18., palette.ui_text)),
                    )
                    .child(
                        Self::rail_spot("rail-settings", false)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_dialog(Dialog::Settings(SettingsPage::Editor), window, cx);
                            }))
                            .child(Self::icon("settings", 18., palette.ui_text)),
                    ),
            )
    }

    /// The Material icon of a row, in two tones of ink on paper.
    fn tree_icon(&self, entry: &TreeEntry, collapsed: bool, ink: u32, row_bg: u32) -> impl IntoElement + use<> {
        let path = if entry.is_dir {
            material_icons::folder(&entry.name, !collapsed)
        } else {
            material_icons::file(&entry.name)
        };
        let path = if self.settings.paper_mode { material_icons::inked(&path, ink, row_bg) } else { path };
        img(path).size(px(TREE_ICON)).flex_none()
    }

    /// Only the rows in view exist, because a project can hold many thousand files.
    fn render_file_tree(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("file-tree")
            .flex_1()
            .min_h_0()
            .track_focus(&self.tree_focus)
            .px(px(8.))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    if let Some(root) = this.root.clone() {
                        this.open_tree_menu(root, event.position, window, cx);
                    }
                }),
            )
            .child(
                uniform_list(
                    "file-tree-rows",
                    self.entries.len(),
                    cx.processor(|this, range: Range<usize>, window, cx| this.render_tree_rows(range, window, cx)),
                )
                .size_full()
                .track_scroll(&self.tree_scroll),
            )
    }

    /// The open file sits on a raised pill and its folder on a pill of ink, so the
    /// eye finds both at once. Children hang on a fine line from their folder.
    fn render_tree_rows(&self, range: Range<usize>, window: &Window, cx: &mut Context<Self>) -> Vec<impl IntoElement + use<>> {
        let palette = theme::palette();
        let tree_cursor = self.tree_focus.is_focused(window).then_some(self.tree_cursor);
        // A shown image tab covers the document, so its file is the one marked.
        let active_path = match self.image_preview() {
            Some(image) => Some(image.view.read(cx).path().to_path_buf()),
            None => self.active_view().and_then(|view| view.read(cx).path.clone()),
        };
        let active_folder = active_path.as_deref().and_then(Path::parent).map(Path::to_path_buf);
        let shade = theme::color(palette.divider).opacity(0.35);
        let renaming = self.tree_input.as_ref().and_then(|input| input.renaming.clone().map(|path| (path, input.view.clone())));
        range
            .map(|index| {
                let entry = &self.entries[index];
                let path = entry.path.clone();
                let is_dir = entry.is_dir;
                let collapsed = self.collapsed.contains(&entry.path);
                let selected = active_path.as_deref() == Some(entry.path.as_path());
                let inked = is_dir && !collapsed && active_folder.as_deref() == Some(entry.path.as_path());
                let git_mark = self.tree_git_mark(&entry.path, is_dir);
                let indent = TREE_INSET + entry.depth as f32 * TREE_INDENT;
                let field = renaming.as_ref().filter(|(path, _)| *path == entry.path).map(|(_, view)| view.clone());
                let menu_path = entry.path.clone();
                let row_bg = if inked {
                    palette.accent
                } else if selected {
                    theme::selected_row(&palette)
                } else {
                    palette.panel
                };
                let text = if inked { palette.on_accent } else { palette.ui_text };
                div()
                    .id(index)
                    .relative()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .w_full()
                    .h(px(TREE_ROW))
                    .pl(px(indent))
                    .pr(px(12.))
                    .cursor_pointer()
                    .when(!selected && !inked, |style| {
                        style.hover(|style| style.bg(shade))
                    })
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                            cx.stop_propagation();
                            this.open_tree_menu(menu_path.clone(), event.position, window, cx);
                        }),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if this.tree_input.as_ref().is_some_and(|input| input.renaming.as_deref() == Some(path.as_path())) {
                            return;
                        }
                        this.tree_target = Some(path.clone());
                        if is_dir {
                            if !this.collapsed.remove(&path) {
                                this.collapsed.insert(path.clone());
                            }
                            this.refresh_from_disk(cx);
                        } else {
                            this.open_file(path.clone(), window, cx);
                        }
                    }))
                    // Rows touch, so the lines of neighbours join, and each runs below
                    // the middle of its folder's icon.
                    .children((0..entry.depth).map(|depth| {
                        div()
                            .absolute()
                            .top_0()
                            .bottom_0()
                            .left(px(TREE_INSET + depth as f32 * TREE_INDENT + TREE_ICON / 2.))
                            .w(px(1.))
                            .bg(theme::color(palette.divider))
                    }))
                    .when(selected || inked, |element| {
                        element.child(
                            div()
                                .absolute()
                                .top(px(2.))
                                .bottom(px(2.))
                                .left(px(indent - 10.))
                                .right_0()
                                .rounded_full()
                                .bg(theme::color(row_bg)),
                        )
                    })
                    // The row that Enter opens while the tree has the keys, ringed in
                    // the color of its text so it shows on the open file too.
                    .when(tree_cursor == Some(index), |element| {
                        element.child(
                            div()
                                .absolute()
                                .top(px(2.))
                                .bottom(px(2.))
                                .left(px(indent - 10.))
                                .right_0()
                                .rounded_full()
                                .border(px(1.5))
                                .border_color(theme::color(text)),
                        )
                    })
                    .child(
                        div()
                            .relative()
                            .flex_none()
                            .child(self.tree_icon(entry, collapsed, text, row_bg))
                            .children(git_mark.map(|(_, color)| {
                                div().absolute().top(px(-1.)).right(px(-2.)).size(px(6.)).rounded_full().bg(theme::color(color))
                            })),
                    )
                    .child(match field {
                        // The name turns into the field, so the row keeps its place.
                        Some(view) => div()
                            .relative()
                            .flex_1()
                            .min_w_0()
                            .h(px(26.))
                            .px(px(8.))
                            .ml(px(-8.))
                            .rounded_full()
                            .overflow_hidden()
                            .bg(theme::color(palette.bg))
                            .child(view)
                            .into_any_element(),
                        None => div().relative().flex_1().min_w_0().truncate().text_color(theme::color(text)).child(entry.name.clone()).into_any_element(),
                    })
                    .children(git_mark.map(|(letter, _)| {
                        div()
                            .relative()
                            .flex_none()
                            .font_family(theme::code_font())
                            .text_size(px(11.))
                            .text_color(theme::color(if inked { palette.on_accent } else { palette.ui_muted }))
                            .child(letter.to_string())
                    }))
                    .when(is_dir, |element| {
                        element.child(
                            div()
                                .relative()
                                .flex_none()
                                .child(Self::icon(if collapsed { "plus" } else { "minus" }, 12., if inked { palette.on_accent } else { palette.ui_muted })),
                        )
                    })
            })
            .collect()
    }

    fn render_outline(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = theme::palette();
        let cursor_line = match self.active_tab() {
            Some(Tab::Document(index)) => Some(self.documents[index].view.read(cx).cursor_position().0 - 1),
            _ => None,
        };
        let trail = cursor_line.map_or_else(Vec::new, |line| heading_trail(&self.outline, line));
        let here = trail.last().copied();
        let rows = outline::visible(&self.outline).map(|(index, item)| {
            let line = item.line;
            let label = outline::marker_element(item, theme::color(palette.ui_muted));
            let row = div()
                .id(index)
                .relative()
                .w_full()
                .flex()
                .cursor_pointer()
                .hover(|style| style.bg(theme::color(palette.divider).opacity(0.35)))
                .on_click(cx.listener(move |this, _, window, cx| {
                    if let Some(view) = this.active_view() {
                        view.update(cx, |view, cx| view.go_to_line(line, 0, window, cx));
                    }
                }));
            if item.depth == 0 {
                // The pill sits on a line across the panel and folds the chapter.
                let in_trail = trail.first() == Some(&index);
                let pill = div()
                    .id(("outline-toggle", index))
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .w(px(OUTLINE_PILL))
                    .h(px(20.))
                    .rounded_full()
                    .bg(theme::color(palette.panel))
                    .text_size(px(11.))
                    .font_weight(FontWeight::MEDIUM)
                    .map(|pill| {
                        if item.collapsed {
                            pill.border_1().border_color(theme::color(palette.accent)).text_color(theme::color(palette.ui_text))
                        } else {
                            let ink = theme::color(palette.accent).opacity(if in_trail { 1. } else { 0.6 });
                            pill.bg(ink).text_color(theme::color(palette.on_accent))
                        }
                    })
                    .when(item.has_children, |pill| {
                        pill.on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.outline[index].collapsed ^= true;
                            cx.notify();
                        }))
                    })
                    .child(outline::marker_element(item, theme::color(palette.on_accent)));
                return row
                    .h(px(44.))
                    .items_center()
                    .pl(px(OUTLINE_SPINE - OUTLINE_PILL / 2.))
                    .pr(px(12.))
                    .child(div().absolute().left_0().right_0().top(px(22.)).h(px(1.)).bg(theme::color(palette.divider)))
                    .child(pill)
                    .child(
                        div()
                            .relative()
                            .min_w_0()
                            .ml(px(6.))
                            .px(px(6.))
                            .bg(theme::color(palette.panel))
                            .font_weight(FontWeight::MEDIUM)
                            .truncate()
                            .child(item.title.clone()),
                    )
                    .into_any_element();
            }
            // Sections hang on the line as dots, without indent: the number
            // and the dot tell the depth, and the title keeps the width.
            let dot_top = 6. + OUTLINE_LINE / 2.;
            let dot = if here == Some(index) {
                div()
                    .absolute()
                    .left(px(OUTLINE_SPINE - 4.5))
                    .top(px(dot_top - 4.5))
                    .size(px(9.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .border_1()
                    .border_color(theme::color(palette.ui_text))
                    .bg(theme::color(palette.panel))
                    .child(div().size(px(3.)).rounded_full().bg(theme::color(palette.ui_text)))
            } else {
                div()
                    .absolute()
                    .left(px(OUTLINE_SPINE - 2.5))
                    .top(px(dot_top - 2.5))
                    .size(px(5.))
                    .rounded_full()
                    .map(|dot| {
                        if item.depth == 1 {
                            dot.bg(theme::color(palette.ui_text))
                        } else {
                            dot.border_1().border_color(theme::color(palette.ui_text)).bg(theme::color(palette.panel))
                        }
                    })
            };
            row.items_start()
                .py(px(6.))
                .pl(px(OUTLINE_SPINE + 12.))
                .pr(px(12.))
                .line_height(px(OUTLINE_LINE))
                .child(dot)
                .child(
                    div()
                        .flex_none()
                        .w(px(34.))
                        .font_family(theme::code_font())
                        .text_size(px(11.))
                        .text_color(theme::color(palette.ui_muted))
                        .child(label),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .line_clamp(2)
                        .text_ellipsis()
                        .when(here == Some(index), |title| title.font_weight(FontWeight::MEDIUM))
                        .child(item.title.clone()),
                )
                .into_any_element()
        });
        div().id("outline").flex_1().min_h_0().overflow_y_scroll().child(
            div()
                .relative()
                .w_full()
                .pb(px(12.))
                .child(div().absolute().top_0().bottom_0().left(px(OUTLINE_SPINE - 0.5)).w(px(1.)).bg(theme::color(palette.divider)))
                .children(rows),
        )
    }

    /// A quiet title in sentence case with its count, as on a page of notes. A
    /// click folds the section.
    fn panel_header(
        &self,
        id: &'static str,
        title: &'static str,
        count: Option<usize>,
        open: bool,
        cx: &mut Context<Self>,
        toggle: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> gpui::Stateful<gpui::Div> {
        let palette = theme::palette();
        div()
            .id(id)
            .flex()
            .flex_none()
            .items_center()
            .gap(px(6.))
            .h(px(44.))
            .pl(px(20.))
            .pr(px(12.))
            .font_weight(FontWeight::MEDIUM)
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, window, cx| {
                toggle(this, window, cx);
                cx.notify();
            }))
            .child(div().flex_none().child(title))
            .children(count.map(|count| div().flex_none().font_weight(FontWeight::NORMAL).text_color(theme::color(palette.ui_muted)).child(format!("({count})"))))
            .child(Self::icon(if open { "chevron-down" } else { "chevron-right" }, 10., palette.ui_muted))
            .child(div().flex_1())
    }

    /// A button in a panel header, where a click must not also fold the panel.
    fn header_button(
        &self,
        id: &'static str,
        name: &'static str,
        cx: &mut Context<Self>,
        handler: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> impl IntoElement {
        div()
            .id(id)
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(px(24.))
            .cursor_pointer()
            .opacity(0.6)
            .hover(|style| style.opacity(1.))
            .on_click(cx.listener(move |this, _, window, cx| {
                cx.stop_propagation();
                handler(this, window, cx);
            }))
            .child(Self::icon(name, 15., theme::palette().ui_text))
    }

    fn render_tree_input(&self, input: &TreeInput, cx: &mut Context<Self>) -> impl IntoElement {
        let kind = if input.folder { "New folder" } else { "New file" };
        let label = match input.parent.strip_prefix(self.root.as_deref().unwrap_or(Path::new(""))) {
            Ok(relative) if !relative.as_os_str().is_empty() => format!("{kind} in {}", relative.display()),
            _ => kind.to_string(),
        };
        div()
            .flex()
            .flex_none()
            .flex_col()
            .gap(px(4.))
            .px(px(12.))
            .pb(px(8.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .pl(px(8.))
                    .text_color(theme::color(theme::palette().ui_muted))
                    .child(div().flex_1().min_w_0().truncate().child(label))
                    .child(self.header_button("tree-input-cancel", "close", cx, |this, window, cx| {
                        this.close_tree_input(window, cx);
                    })),
            )
            .child(
                div()
                    .h(px(28.))
                    .px(px(10.))
                    .rounded_full()
                    .overflow_hidden()
                    .bg(theme::color(theme::raised(&theme::palette())))
                    .child(input.view.clone()),
            )
    }

    fn render_file_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let show_outline = self.show_outline;
        let show_tree = self.show_tree;
        let has_project = self.root.is_some();
        div()
            .relative()
            .children(self.tour_anchor(TourAnchor::FileTree))
            .flex()
            .flex_none()
            .flex_col()
            .w(self.file_panel_width)
            .h_full()
            .pb(px(8.))
            .bg(theme::color(theme::palette().panel))
            .when(self.open_menu == Some(Menu::Tree), |panel| panel.child(self.render_menu(Menu::Tree, cx)))
            .child(
                self.panel_header("tree-header", "Files", None, show_tree, cx, |this, window, cx| {
                    this.show_tree = !this.show_tree;
                    if !this.show_tree && this.tree_focus.is_focused(window) {
                        this.focus_active(window, cx);
                    }
                })
                .overflow_hidden()
                .when(has_project, |header| {
                    header
                        .child(self.header_button("tree-new-file", "file-plus", cx, |this, window, cx| {
                            this.open_tree_input(false, window, cx);
                        }))
                        .child(self.header_button("tree-new-folder", "folder-plus", cx, |this, window, cx| {
                            this.open_tree_input(true, window, cx);
                        }))
                        .child(self.header_button("tree-add-files", "upload", cx, |this, window, cx| {
                            this.prompt_add_files(window, cx);
                        }))
                })
                .child(self.header_button("tree-close", "close", cx, |this, window, cx| {
                    this.close_file_panel(window, cx);
                })),
            )
            .when(show_tree, |panel| {
                panel
                    .when_some(self.tree_input.as_ref().filter(|input| input.renaming.is_none()), |panel, input| {
                        panel.child(self.render_tree_input(input, cx))
                    })
                    .when_some(self.tree_error.clone(), |panel, error| {
                        panel.child(
                            div()
                                .id("tree-error")
                                .flex_none()
                                .px(px(20.))
                                .pb(px(8.))
                                .cursor_pointer()
                                .text_color(theme::color(theme::palette().error))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.tree_error = None;
                                    cx.notify();
                                }))
                                .child(error),
                        )
                    })
                    .child(self.render_file_tree(cx))
                    .child(self.divider_horizontal(cx))
            })
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_none()
                    .when(show_outline && show_tree, |style| style.h(self.outline_height))
                    .when(show_outline && !show_tree, |style| style.flex_1().min_h_0())
                    .child(self.panel_header(
                        "outline-header",
                        "Outline",
                        Some(self.outline.len()).filter(|count| *count > 0),
                        show_outline,
                        cx,
                        |this, _, _| this.show_outline = !this.show_outline,
                    ))
                    .when(show_outline, |element| element.child(self.render_outline(cx))),
            )
    }

    /// A hairline between the sections, with room around it to grab.
    fn divider_horizontal(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("outline-divider")
            .flex()
            .flex_none()
            .items_center()
            .h(px(13.))
            .px(px(20.))
            .cursor(CursorStyle::ResizeUpDown)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.dragging = Some(Divider::Outline);
                    cx.notify();
                }),
            )
            .child(div().w_full().h(px(1.)).bg(theme::color(theme::palette().divider)))
    }

    fn divider_vertical(&self, id: &'static str, divider: Divider, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id(id)
            .flex_none()
            .w(px(DIVIDER_WIDTH))
            .h_full()
            // The side panel ends in a hairline; the grip stays as wide.
            .map(|style| {
                if divider == Divider::FilePanel {
                    style.child(div().w(px(1.)).h_full().bg(theme::color(theme::palette().divider)))
                } else {
                    style.bg(theme::color(theme::palette().divider))
                }
            })
            .cursor(CursorStyle::ResizeLeftRight)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    this.dragging = Some(divider);
                    cx.notify();
                }),
            )
    }

    /// The background of a tab. It reaches half a slant past the tab on each side,
    /// where the neighbor leaves room for it, so that tabs interlock without overlap.
    /// The active tab has the color of the toolbar below, so that the two join,
    /// and a thin line on its top edge.
    fn tab_shape(active: bool, (left_forward, right_forward): (bool, bool), box_bounds: Rc<Cell<Bounds<Pixels>>>) -> impl IntoElement {
        canvas(
            |_, _, _| {},
            move |bounds: Bounds<Pixels>, _, window, _| {
                box_bounds.set(bounds);
                let half = px(TAB_SLANT / 2.);
                let edge = |x: Pixels, forward: bool, top: bool| if forward == top { x + half } else { x - half };
                let left = bounds.left() + px(TAB_GAP / 2.);
                let right = bounds.right() - px(TAB_GAP / 2.);
                let top_left = point(edge(left, left_forward, true), bounds.top());
                let top_right = point(edge(right, right_forward, true), bounds.top());
                let corners = [
                    point(edge(left, left_forward, false), bounds.bottom()),
                    top_left,
                    top_right,
                    point(edge(right, right_forward, false), bounds.bottom()),
                ];
                if let Some(path) = theme::polygon(corners) {
                    window.paint_path(path, theme::color(if active { theme::palette().tab_active } else { theme::palette().tab_inactive }));
                }
                if active {
                    let line = Bounds::new(top_left, size(top_right.x - top_left.x, px(1.)));
                    window.paint_quad(fill(line, theme::color(theme::palette().focus)));
                }
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full()
    }

    fn render_tabs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let tabs = self.tabs();
        let count = tabs.len();
        let current = self.active_tab();
        let current_position = tabs.iter().position(|tab| Some(*tab) == current);
        div()
            .flex()
            .flex_none()
            .items_end()
            .h(px(TAB_BAR_HEIGHT))
            .bg(theme::color(theme::palette().bg))
            .pl(px(TAB_SLANT / 2. + 4.))
            .children(tabs.into_iter().enumerate().map(|(position, tab)| {
                let active = current_position == Some(position);
                let edges = tab_edges(position, count, current_position);
                let box_bounds = Rc::new(Cell::new(Bounds::default()));
                let shape_bounds = box_bounds.clone();
                let (title, dirty, icon): (SharedString, bool, &'static str) = match tab {
                    Tab::Document(index) => {
                        let view = self.documents[index].view.read(cx);
                        (view.title().into(), view.state.is_dirty(), "file")
                    }
                    Tab::Image(index) => {
                        let view = self.images[index].view.read(cx);
                        let name = view.path().file_name().map_or_else(String::new, |name| name.to_string_lossy().into_owned());
                        (name.into(), false, "image")
                    }
                    Tab::Pdf => {
                        let name = self.last_pdf.as_deref().and_then(Path::file_name);
                        (name.map_or_else(|| "PDF".to_string(), |name| name.to_string_lossy().into_owned()).into(), false, "book")
                    }
                };
                div()
                    .id(("tab", position))
                    .relative()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .h(px(TAB_HEIGHT))
                    .px(px(TAB_SLANT / 2. + 8.))
                    .cursor_pointer()
                    .when(!active, |style| style.text_color(theme::color(theme::palette().ui_muted)))
                    .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                        let bounds = box_bounds.get();
                        let click = event.position();
                        let up = f32::from(bounds.bottom() - click.y) / f32::from(bounds.size.height);
                        let offset = tab_hit(f32::from(click.x), f32::from(bounds.left()), f32::from(bounds.right()), up, edges);
                        if let Some(target) = position.checked_add_signed(offset) {
                            this.go_to_tab(target, window, cx);
                        }
                    }))
                    .child(Self::tab_shape(active, edges, shape_bounds))
                    .child(Self::icon(icon, 14., if active { theme::palette().ui_text } else { theme::palette().ui_muted }))
                    .child(title)
                    .when(dirty, |element| {
                        element.child(div().size(px(6.)).rounded_full().bg(theme::color(theme::palette().ui_text)))
                    })
                    .child(
                        div()
                            .id(("tab-close", position))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.close_tab(tab, window, cx);
                            }))
                            .child(Self::icon("close", 12., theme::palette().ui_muted)),
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
            .rounded(px(TOOL_RADIUS))
            .cursor_pointer()
            .hover(|style| style.bg(theme::color(theme::palette().divider).opacity(0.55)))
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
            .rounded(px(TOOL_RADIUS))
            .cursor_pointer()
            .when(open, |style| style.bg(theme::color(theme::palette().divider).opacity(0.55)))
            .hover(|style| style.bg(theme::color(theme::palette().divider).opacity(0.55)))
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                let bottom = this.editor_toolbar_bottom(TOOLBAR_ROW_HEIGHT);
                this.toggle_menu(menu, event.position(), bottom, window, cx);
            }))
            .child(Self::icon(icon, 16., theme::palette().ui_text))
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
                    .bg(theme::color(theme::palette().panel))
                    .border_1()
                    .border_color(theme::color(theme::palette().divider))
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
                .when(index == self.symbol_group, |style| style.bg(theme::color(theme::palette().accent_soft)))
                .when(index != self.symbol_group, |style| style.hover(|style| style.bg(theme::color(theme::palette().divider))))
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
                .font_family(theme::code_font())
                .hover(|style| style.bg(theme::color(theme::palette().divider)))
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
                        .font_family(theme::code_font())
                        .text_size(px(theme::code_font_size()))
                        .text_color(theme::color(theme::palette().ui_muted))
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
                    .border_color(theme::color(theme::palette().divider))
                    .when(selected, |style| style.bg(theme::color(theme::palette().accent_soft)))
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
                .child(div().pt(px(4.)).text_color(theme::color(theme::palette().ui_muted)).child(format!("{hover_rows} × {hover_columns}"))),
        )
    }

    /// The languages a code block can be highlighted in.
    fn render_code_languages(&self, language: Language, cx: &mut Context<Self>) -> AnyElement {
        let rows = snippets::CODE_LANGUAGES.iter().enumerate().map(|(index, code)| {
            div()
                .id(("code-language", index))
                .px(px(8.))
                .py(px(3.))
                .rounded(px(4.))
                .cursor_pointer()
                .hover(|style| style.bg(theme::color(theme::palette().divider)))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.close_menu(window, cx);
                    // The preamble first, so that the cursor moves with it.
                    if language == Language::Latex {
                        this.set_up_listings(cx);
                    }
                    this.insert_snippet(&snippets::code_block(language, code), false, cx);
                }))
                .child(code.label)
        });
        self.render_popup("code-languages", div().flex().flex_col().min_w(px(140.)).children(rows))
    }

    fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let separator = || div().w(px(1.)).h(px(14.)).mx(px(6.)).bg(theme::color(theme::palette().divider));
        let palette = theme::palette();
        let language = self.active_language(cx);
        let formats = matches!(language, Some(Language::Latex | Language::Typst | Language::Markdown));
        // Markdown takes LaTeX math between dollars, as Pandoc reads it.
        let markup = formats;
        // A capsule floating on the editor's paper rather than a bar across it;
        // it is a little see-through, so the grain of paper mode shows.
        let capsule = div()
            .relative()
            .children(self.tour_anchor(TourAnchor::Toolbar))
            .flex()
            .flex_none()
            .items_center()
            .h(px(TOOLBAR_HEIGHT))
            .px(px(6.))
            .rounded(px(TOOLBAR_RADIUS))
            .bg(theme::color(theme::raised(&palette)).opacity(0.72))
            .border_1()
            .border_color(theme::color(palette.divider).opacity(0.7))
            .shadow_sm()
            .child(self.tool_button("undo", "undo", cx, |this, _, cx| {
                this.with_active(cx, |view, cx| view.undo_edit(cx));
            }))
            .child(self.tool_button("redo", "redo", cx, |this, _, cx| {
                this.with_active(cx, |view, cx| view.redo_edit(cx));
            }))
            .when(formats, |element| {
                element
                    .child(separator())
                    .child(self.format_button(
                        "bold",
                        FormatKind::Bold,
                        cx,
                        // Its own font: the bold of iA Writer Quattro claims a regular weight.
                        div().font_family(theme::CODE_FONT).font_weight(FontWeight::BOLD).child("B"),
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
                    Self::icon("math", 16., theme::palette().ui_text),
                ))
            })
            .when(formats, |element| {
                element.child(self.format_button(
                    "link",
                    FormatKind::Link,
                    cx,
                    Self::icon("link", 16., theme::palette().ui_text),
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
                    .child(self.popup_button("code", "code", Menu::Code, cx, move |this, cx| {
                        this.render_code_languages(language, cx)
                    }))
                    .child(self.tool_button("figure", "image", cx, |this, window, cx| {
                        this.open_picker(PickerKind::Figures, window, cx);
                    }))
                    .child(self.tool_button("citation", "quote", cx, |this, window, cx| {
                        this.open_picker(PickerKind::Citations, window, cx);
                    }))
            });
        div()
            .flex()
            .flex_none()
            .justify_center()
            .items_center()
            .h(px(TOOLBAR_ROW_HEIGHT))
            .px(px(12.))
            .bg(theme::color(palette.bg))
            .child(capsule)
    }

    /// The path of the shown tab and, in a document, the headings around the cursor.
    fn render_breadcrumbs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let (path, line) = match self.active_tab() {
            Some(Tab::Document(index)) => {
                let view = self.documents[index].view.read(cx);
                (view.path.clone(), Some(view.cursor_position().0 - 1))
            }
            Some(Tab::Image(index)) => (Some(self.images[index].view.read(cx).path().to_path_buf()), None),
            Some(Tab::Pdf) => (self.last_pdf.clone(), None),
            None => (None, None),
        };
        let relative = path.map(|path| match &self.root {
            Some(root) if path.starts_with(root) => relative_path(root, &path),
            _ => path.display().to_string(),
        });
        let trail = line.map_or_else(Vec::new, |line| heading_trail(&self.outline, line));
        let separator = || div().text_color(theme::color(theme::palette().ui_muted)).child("›");
        let segments: Vec<String> = relative.map_or_else(Vec::new, |relative| relative.split('/').map(str::to_string).collect());
        let last = segments.len().saturating_sub(1);
        div()
            .id("breadcrumbs")
            .flex()
            .flex_none()
            .items_center()
            .gap(px(6.))
            .h(px(BREADCRUMB_HEIGHT))
            .px(px(12.))
            .overflow_hidden()
            .whitespace_nowrap()
            .bg(theme::color(theme::palette().panel))
            .text_color(theme::color(theme::palette().ui_muted))
            .children(segments.into_iter().enumerate().map(|(position, segment)| {
                div()
                    .id(("crumb-path", position))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .when(position > 0, |element| element.child(separator()))
                    .child(
                        div()
                            .when(position == last, |element| {
                                element
                                    .text_color(theme::color(theme::palette().ui_text))
                                    .cursor_pointer()
                                    .hover(|style| style.underline())
                            })
                            .child(segment),
                    )
                    .when(position == last, |element| {
                        element.on_click(cx.listener(|this, _, window, cx| {
                            this.reveal_in_tree(window, cx);
                            if !this.settings.vim_mode {
                                this.focus_active(window, cx);
                            }
                        }))
                    })
            }))
            .children(trail.into_iter().map(|index| {
                let item = &self.outline[index];
                let line = item.line;
                div()
                    .id(("crumb-heading", index))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .min_w_0()
                    .child(separator())
                    .child(
                        div()
                            .truncate()
                            .cursor_pointer()
                            .hover(|style| style.text_color(theme::color(theme::palette().ui_text)))
                            .child(item.title.clone()),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if let Some(view) = this.active_view() {
                            view.update(cx, |view, cx| view.go_to_line(line, 0, window, cx));
                        }
                    }))
            }))
    }

    fn render_editor_pane(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let active = self.active_view();
        let has_document = active.is_some();
        let pdf_tab = self.pdf_tab_shown && !self.focus_mode;
        let toolbar = if self.focus_mode {
            None
        } else if pdf_tab {
            Some(self.render_preview_toolbar(cx).into_any_element())
        } else {
            Some(self.render_toolbar(cx).into_any_element())
        };
        let body: AnyElement = if pdf_tab {
            self.render_preview_body(cx)
        } else {
            match (self.image_preview(), active) {
            (Some(preview), _) => preview.view.clone().into_any_element(),
            (None, Some(_)) if let Some(index) = self.active_proposal(cx) => self.render_proposal(index, cx),
            (None, Some(view)) => view.into_any_element(),
            (None, None) => div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_color(theme::color(theme::palette().ui_muted))
                .child(if self.root.is_some() {
                    "Select a file in the file tree."
                } else {
                    "Open a folder to start."
                })
                .into_any_element(),
            }
        };
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .h_full()
            .when(self.show_tab_bar(), |element| element.child(self.render_tabs(cx)))
            .when(self.show_breadcrumbs(), |element| element.child(self.render_breadcrumbs(cx)))
            .children(toolbar)
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .when(has_document && !pdf_tab, |element| {
                        element.on_mouse_down(MouseButton::Right, cx.listener(|this, event: &MouseDownEvent, window, cx| this.open_editor_menu(event.position, window, cx)))
                    })
                    .child(body)
                    .children(self.tour_anchor(TourAnchor::Editor))
                    .when(self.open_menu == Some(Menu::EditorContext), |element| element.child(self.render_menu(Menu::EditorContext, cx))),
            )
            .children(self.render_conflict_strip(cx))
            .when_some(self.cursor_hint(cx).cloned(), |element, hint| element.child(self.render_hint_strip(&hint, cx)))
            .when(self.cursor_hint(cx).is_none(), |element| element.children(self.render_citation_strip(cx)))
            .when(self.error_count() > 0, |element| element.child(self.render_error_strip(cx)))
    }

    /// Says what the quiet underline on the cursor's line means.
    fn render_hint_strip(&self, hint: &PackageHint, cx: &mut Context<Self>) -> impl IntoElement {
        let (fix, path) = (hint.fix.clone(), hint.path.clone());
        div()
            .id("hint-strip")
            .flex()
            .flex_none()
            .items_center()
            .gap(px(8.))
            .h(px(28.))
            .px(px(10.))
            .bg(theme::color(theme::palette().divider))
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| this.apply_fix(&fix, Some(&path), cx)))
            .child(div().flex_1().min_w_0().truncate().child(hint.message.clone()))
            .child(div().flex_none().font_weight(FontWeight::SEMIBOLD).child(hint.fix.label()))
    }

    /// What the citation mark at the cursor means, below the editor.
    fn render_citation_strip(&self, cx: &App) -> Option<impl IntoElement> {
        let view = self.active_view()?;
        let view = view.read(cx);
        let mark = view.mark_at_cursor(MarkSource::Citations)?;
        let palette = theme::palette();
        let color = match mark.severity {
            MarkSeverity::Error => palette.error,
            MarkSeverity::Warning => palette.warning,
            MarkSeverity::Hint => palette.ui_muted,
        };
        Some(
            div()
                .id("citation-strip")
                .flex()
                .flex_none()
                .items_center()
                .gap(px(8.))
                .h(px(28.))
                .px(px(10.))
                .bg(theme::color(palette.divider))
                .child(div().flex_none().w(px(3.)).h(px(14.)).rounded(px(1.5)).bg(theme::color(color)))
                .child(div().flex_1().min_w_0().truncate().child(mark.message.clone())),
        )
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
            .bg(theme::color(theme::palette().error_panel))
            .border_t_1()
            .border_color(theme::color(theme::palette().error))
            .cursor_pointer()
            .on_click(cx.listener(|this, _, window, cx| this.go_to_diagnostic(0, window, cx)))
            .child(div().flex_1().min_w_0().truncate().child(label))
            .child(div().flex_none().text_color(theme::color(theme::palette().ui_muted)).child("Go to line"))
    }

    fn render_preview_toolbar(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.markdown_view(cx).is_some() {
            return self.render_markdown_toolbar(cx);
        }
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
            .bg(theme::color(theme::palette().panel))
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
                            .relative()
                            .children(self.tour_anchor(TourAnchor::Recompile))
                            .rounded(px(12.))
                            .bg(theme::color(theme::palette().accent))
                            .text_color(theme::color(theme::palette().on_accent))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(
                                div()
                                    .id("recompile")
                                    .px(px(14.))
                                    .cursor_pointer()
                                    .on_click(cx.listener(|this, _, _, cx| this.recompile_by_hand(cx)))
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
                                    .border_color(theme::color(theme::palette().on_accent).opacity(0.3))
                                    .cursor_pointer()
                                    .on_click(cx.listener(|this, event: &ClickEvent, window, cx| {
                                        let bottom = if this.pdf_tab_shown {
                                            this.editor_toolbar_bottom(PREVIEW_TOOLBAR_HEIGHT)
                                        } else {
                                            TOP_BAR_HEIGHT + PREVIEW_TOOLBAR_HEIGHT
                                        };
                                        this.toggle_menu(Menu::Compile, event.position(), bottom, window, cx);
                                    }))
                                    .child(Self::icon("chevron-down", 12., theme::palette().on_accent))
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
                                .bg(theme::color(theme::palette().error))
                                .text_color(theme::color(theme::palette().on_error))
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(errors.to_string()),
                        )
                    })
                    .when(warnings > 0, |element| {
                        element.child(
                            div()
                                .px(px(5.))
                                .rounded(px(8.))
                                .bg(theme::color(theme::palette().warning))
                                .text_color(theme::color(theme::palette().on_warning))
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
                    .text_color(theme::color(theme::palette().ui_muted))
                    .child(self.status.clone().or_else(|| self.toolchain.read(cx).summary().map(Into::into)).unwrap_or_default()),
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
                            .when(self.settings.auto_scroll_pdf, |element| element.bg(theme::color(theme::palette().accent_soft)))
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
            .into_any_element()
    }

    /// The preview of Markdown has no build: its bar shows the status and
    /// whether the page follows the cursor.
    fn render_markdown_toolbar(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(8.))
            .h(px(PREVIEW_TOOLBAR_HEIGHT))
            .px(px(8.))
            .bg(theme::color(theme::palette().panel))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(theme::color(theme::palette().ui_muted))
                    .child(self.status.clone().unwrap_or_else(|| "Markdown preview. Double-click a block to go to its source.".into())),
            )
            .child(
                div()
                    .rounded(px(4.))
                    .when(self.settings.auto_scroll_pdf, |element| element.bg(theme::color(theme::palette().accent_soft)))
                    .child(self.icon_button("auto-scroll-markdown", "position", cx, |this, window, cx| {
                        this.toggle_auto_scroll(window, cx);
                    })),
            )
            .into_any_element()
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
            .bg(theme::color(if primary { theme::palette().accent } else { theme::palette().divider }))
            .when(primary, |style| style.text_color(theme::color(theme::palette().on_accent)))
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
                element.child(div().text_color(theme::color(theme::palette().ui_muted)).child(location))
            })
            .when(!problem.excerpt.is_empty(), |element| {
                element.child(
                    div()
                        .p(px(8.))
                        .rounded(px(4.))
                        .bg(theme::color(theme::palette().bg))
                        .font_family(theme::code_font())
                        .text_size(px(theme::code_font_size()))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .children(problem.excerpt.iter().flat_map(|line| {
                            let number = format!("{:>5}  ", line.number);
                            let text = div()
                                .when(!line.target, |style| style.text_color(theme::color(theme::palette().ui_muted)))
                                .when(line.target, |style| style.font_weight(FontWeight::SEMIBOLD))
                                .child(format!("{number}{}", line.text));
                            // Non-breaking spaces, because plain ones collapse in a text run.
                            let marker = problem.marker.filter(|_| line.target).map(|(before, width)| {
                                div()
                                    .text_color(theme::color(theme::palette().error))
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
                        .text_color(theme::color(theme::palette().ui_muted))
                        .child(format!("Compiler message: {message}")),
                )
            })
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(6.))
                    .when_some(problem.fix.clone().filter(|fix| self.offers(fix, cx)), |element, fix| {
                        element.child(self.panel_button("apply-fix", fix.label(), true, cx, move |this, _, cx| this.apply_fix(&fix, None, cx)))
                    })
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
            .bg(theme::color(theme::palette().error_panel))
            .border_b_2()
            .border_color(theme::color(theme::palette().error))
            .child(div().font_weight(FontWeight::SEMIBOLD).child(self.preview_message()))
            .when_some(self.problems.first(), |element, problem| {
                element.child(self.render_first_error(problem, cx))
            })
            .when(errors > 1, |element| {
                element.child(div().text_color(theme::color(theme::palette().ui_muted)).child(format!(
                    "{} more. Fix this one first and save, because later errors often come from it.",
                    errors - 1
                )))
            })
            .child(
                div()
                    .id("report-build-problem")
                    .flex_none()
                    .cursor_pointer()
                    .text_size(px(12.))
                    .text_color(theme::color(theme::palette().ui_muted))
                    .hover(|style| style.text_color(theme::color(theme::palette().ui_text)))
                    .on_click(cx.listener(|this, _, window, cx| this.open_report(window, cx)))
                    .child("Looks like a fault of the editor? Report it"),
            )
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
            .bg(theme::color(theme::palette().error_panel))
            .cursor_pointer()
            .on_click(cx.listener(|this, _, _, cx| {
                this.error_card_dismissed = false;
                cx.notify();
            }))
            .child(div().truncate().child(self.preview_message()))
    }

    fn render_problem_row(&self, index: usize, problem: &Problem, cx: &mut Context<Self>) -> impl IntoElement {
        let color = match self.diagnostics[index].severity {
            Severity::Error => theme::palette().error,
            Severity::Warning => theme::palette().warning,
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
            .bg(theme::color(theme::palette().panel))
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, window, cx| this.go_to_diagnostic(index, window, cx)))
            .child(problem.title.clone())
            .child(
                div()
                    .text_color(theme::color(theme::palette().ui_muted))
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
            .bg(theme::color(theme::palette().bg))
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
                        .text_color(theme::color(theme::palette().ui_muted))
                        .child(format!("{} more are not shown.", self.diagnostics.len() - MAX_DIAGNOSTICS_SHOWN)),
                )
            })
            .when(!self.package_hints.is_empty(), |element| {
                element
                    .child(div().mt(px(10.)).mb(px(4.)).font_weight(FontWeight::SEMIBOLD).child("Packages"))
                    .children(self.package_hints.iter().enumerate().map(|(index, hint)| {
                        let (fix, path) = (hint.fix.clone(), hint.path.clone());
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .justify_between()
                            .gap(px(6.))
                            .mb(px(4.))
                            .child(hint.message.clone())
                            .child(
                                div()
                                    .id(("package-fix", index))
                                    .flex_none()
                                    .px(px(10.))
                                    .py(px(4.))
                                    .rounded(px(4.))
                                    .cursor_pointer()
                                    .bg(theme::color(theme::palette().divider))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .on_click(cx.listener(move |this, _, _, cx| this.apply_fix(&fix, Some(&path), cx)))
                                    .child(hint.fix.label()),
                            )
                    }))
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
                        .font_family(theme::code_font())
                        .text_size(px(11.))
                        .text_color(theme::color(theme::palette().ui_muted))
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
            None if !self.vim_mapper.pending().is_empty() => key_names(self.vim_mapper.pending()),
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
            .bg(theme::color(if on { theme::palette().accent } else { theme::palette().divider }))
            .when(enabled, |style| {
                style
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, window, cx| handler(this, window, cx)))
            })
            .when(!enabled, |style| style.opacity(0.4))
            .child(div().size(px(16.)).rounded_full().bg(theme::color(if on { theme::palette().on_accent } else { theme::palette().ui_text })))
    }

    /// All choices stay visible, so one click is enough to change them.
    fn segmented(&self, id: &'static str, options: Vec<(&'static str, bool, MenuHandler)>, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_none()
            .p(px(2.))
            .gap(px(2.))
            .rounded(px(6.))
            .bg(theme::color(theme::palette().bg))
            .border_1()
            .border_color(theme::color(theme::palette().divider))
            .children(options.into_iter().enumerate().map(|(index, (label, selected, handler))| {
                div()
                    .id((id, index))
                    .px(px(10.))
                    .py(px(3.))
                    .rounded(px(4.))
                    .cursor_pointer()
                    // The accent, so that the choice shows in any theme.
                    .when(selected, |style| {
                        style
                            .bg(theme::color(theme::palette().accent))
                            .text_color(theme::color(theme::palette().on_accent))
                            .font_weight(FontWeight::SEMIBOLD)
                    })
                    .when(!selected, |style| style.hover(|style| style.bg(theme::color(theme::palette().divider))))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        handler(this, window, cx);
                        cx.notify();
                    }))
                    .child(label)
            }))
    }

    /// Names the font in use for a slot and opens the font picker.
    fn font_button(&self, id: &'static str, slot: FontSlot, cx: &mut Context<Self>) -> impl IntoElement {
        let look = theme::look();
        let name: SharedString = match slot {
            FontSlot::Editor => look.code_font,
            FontSlot::Interface => look.ui_font.unwrap_or_else(|| "System".into()),
        };
        div()
            .id(id)
            .flex_none()
            .px(px(8.))
            .py(px(3.))
            .rounded(px(4.))
            .cursor_pointer()
            .bg(theme::color(theme::palette().divider))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.close_dialog(window, cx);
                this.open_font_picker(slot, window, cx);
            }))
            .font_family(name.clone())
            .child(name)
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
                    .child(div().text_color(theme::color(theme::palette().ui_muted)).child(description.into())),
            )
            .child(control)
    }

    fn render_editor_settings(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let vim_on = self.settings.vim_mode;
        let paper_on = self.settings.paper_mode;
        let paper_tone = |dark: bool| -> MenuHandler {
            Rc::new(move |this, window, cx| {
                this.update_settings(
                    |settings| {
                        settings.paper_dark = dark;
                        settings.paper_mode = true;
                    },
                    window,
                    cx,
                )
            })
        };
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
                "Theme",
                "Any colorscheme installed in Neovim. Moving through the list previews it.",
                div()
                    .id("theme-setting")
                    .flex_none()
                    .px(px(8.))
                    .py(px(3.))
                    .rounded(px(4.))
                    .cursor_pointer()
                    .bg(theme::color(theme::palette().divider))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.close_dialog(window, cx);
                        this.open_theme_picker(window, cx);
                    }))
                    .child(self.settings.theme.clone().unwrap_or_else(|| theme::DEFAULT_THEME.to_string())),
            )
            .into_any_element(),
            Self::setting_row(
                "Paper mode",
                "Paper texture, ink-like text and a calm, monochrome editor. Covers the theme while on.",
                self.toggle_switch("paper-mode-setting", self.settings.paper_mode, true, cx, |this, window, cx| {
                    this.update_settings(|settings| settings.paper_mode = !settings.paper_mode, window, cx);
                }),
            )
            .into_any_element(),
            Self::setting_row(
                "Paper",
                if paper_on { "Parchment with graphite ink, or dark paper with pale ink." } else { "Needs paper mode." },
                self.segmented(
                    "paper-tone-setting",
                    vec![
                        ("Light", !self.settings.paper_dark, paper_tone(false)),
                        ("Dark", self.settings.paper_dark, paper_tone(true)),
                    ],
                    cx,
                ),
            )
            .into_any_element(),
            Self::setting_row(
                "Ruled lines",
                if paper_on { "Draws a line under every row, like ruled paper." } else { "Needs paper mode." },
                self.toggle_switch("ruled-lines-setting", paper_on && self.settings.ruled_lines, paper_on, cx, |this, window, cx| {
                    this.update_settings(|settings| settings.ruled_lines = !settings.ruled_lines, window, cx);
                }),
            )
            .into_any_element(),
            Self::setting_row(
                "Editor font",
                "Any installed font. Moving through the list previews it.",
                self.font_button("editor-font-setting", FontSlot::Editor, cx),
            )
            .into_any_element(),
            Self::setting_row(
                "Interface font",
                "The font of menus, panels and dialogs.",
                self.font_button("ui-font-setting", FontSlot::Interface, cx),
            )
            .into_any_element(),
            Self::setting_row(
                "Font size",
                "The size of the text in the editor.",
                self.segmented(
                    "font-size-setting",
                    [12u32, 13, 14, 16]
                        .into_iter()
                        .map(|size| {
                            let label: &'static str = match size {
                                12 => "12",
                                13 => "13",
                                14 => "14",
                                _ => "16",
                            };
                            let handler: MenuHandler =
                                Rc::new(move |this, window, cx| this.update_settings(|settings| settings.editor_font_size = Some(size), window, cx));
                            (label, theme::look().code_size.round() as u32 == size, handler)
                        })
                        .collect(),
                    cx,
                ),
            )
            .into_any_element(),
            Self::setting_row(
                "Breadcrumbs",
                "Shows the path of the file and the headings around the cursor above the editor.",
                self.toggle_switch("breadcrumbs-setting", self.settings.show_breadcrumbs, true, cx, |this, window, cx| {
                    this.update_settings(|settings| settings.show_breadcrumbs = !settings.show_breadcrumbs, window, cx);
                }),
            )
            .into_any_element(),
            Self::setting_row(
                "Sync with git in the background",
                "Fetches every few minutes and brings in new commits when nothing local is in the way. Pushing always waits for you.",
                self.toggle_switch("git-sync-setting", self.settings.git_background_sync, true, cx, |this, window, cx| {
                    this.update_settings(|settings| settings.git_background_sync = !settings.git_background_sync, window, cx);
                }),
            )
            .into_any_element(),
            Self::setting_row(
                "Editor tabs",
                "Shows the open files as tabs. Without them, the file tree and the picker switch files.",
                self.toggle_switch("tabs-setting", self.settings.show_tabs, true, cx, |this, window, cx| {
                    this.update_settings(|settings| settings.show_tabs = !settings.show_tabs, window, cx);
                }),
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
                "Auto suggestions",
                "Shows commands and project names while typing a command, environment or reference. Tab accepts.",
                self.toggle_switch("auto-suggestions", self.settings.auto_suggestions, true, cx, |this, window, cx| {
                    this.update_settings(|settings| settings.auto_suggestions = !settings.auto_suggestions, window, cx);
                }),
            )
            .into_any_element(),
            Self::setting_row(
                "Package hints",
                "Underlines LaTeX packages that are missing or seem unused. Fix them from the log panel or with the shortcut.",
                self.toggle_switch("package-hints", self.settings.package_hints, true, cx, |this, window, cx| {
                    this.update_settings(|settings| settings.package_hints = !settings.package_hints, window, cx);
                    this.check_packages(cx);
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

    fn catalog_button(&self, id: impl Into<gpui::ElementId>, label: String, entry: CatalogEntry, cx: &mut Context<Self>) -> impl IntoElement {
        let busy = self.installing.as_deref() == Some(entry.id.as_str());
        div()
            .id(id)
            .flex_none()
            .px(px(8.))
            .py(px(3.))
            .rounded(px(4.))
            .cursor_pointer()
            .font_weight(FontWeight::SEMIBOLD)
            .bg(theme::color(theme::palette().accent_soft))
            .on_click(cx.listener(move |this, _, _, cx| this.install_from_catalog(entry.clone(), cx)))
            .child(if busy { "Downloading…".to_string() } else { label })
    }

    fn render_addons_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let catalog: &[CatalogEntry] = if let CatalogState::Ready(entries) = &self.catalog { entries } else { &[] };
        let rows: Vec<AnyElement> = self
            .installed_addons
            .iter()
            .enumerate()
            .map(|(index, add_on)| {
                let manifest = &add_on.manifest;
                let (for_toggle, for_remove) = (add_on.clone(), add_on.clone());
                let enabled = add_on.enabled;
                let update = catalog.iter().find(|entry| entry.id == manifest.id && marketplace::is_newer(&entry.version, &manifest.version)).cloned();
                div()
                    .flex()
                    .flex_col()
                    .flex_none()
                    .gap(px(4.))
                    .py(px(10.))
                    .border_t_1()
                    .border_color(theme::color(theme::palette().panel))
                    .child(div().font_weight(FontWeight::SEMIBOLD).child(format!("{} {}", manifest.name, manifest.version)))
                    .child(div().text_color(theme::color(theme::palette().ui_muted)).child(format!("By {}", manifest.author)))
                    .when(!manifest.description.is_empty(), |element| {
                        element.child(div().text_color(theme::color(theme::palette().ui_muted)).child(manifest.description.clone()))
                    })
                    .child(div().text_color(theme::color(theme::palette().ui_muted)).child(marketplace::permission_text(&manifest.permissions)))
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
                                    .bg(theme::color(if enabled { theme::palette().accent_soft } else { theme::palette().divider }))
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
                                    .bg(theme::color(theme::palette().divider))
                                    .on_click(cx.listener(move |this, _, _, cx| this.remove_addon(&for_remove, cx)))
                                    .child("Remove"),
                            )
                            .children(update.map(|entry| {
                                let label = format!("Update to {}", entry.version);
                                self.catalog_button(("addon-update", index), label, entry, cx)
                            })),
                    )
                    .into_any_element()
            })
            .collect();
        let available: Vec<AnyElement> = catalog
            .iter()
            .filter(|entry| !self.installed_addons.iter().any(|add_on| add_on.manifest.id == entry.id))
            .enumerate()
            .map(|(index, entry)| {
                div()
                    .flex()
                    .flex_col()
                    .flex_none()
                    .gap(px(4.))
                    .py(px(10.))
                    .border_t_1()
                    .border_color(theme::color(theme::palette().panel))
                    .child(div().font_weight(FontWeight::SEMIBOLD).child(format!("{} {}", entry.name, entry.version)))
                    .child(div().text_color(theme::color(theme::palette().ui_muted)).child(format!("By {}", entry.author)))
                    .child(div().text_color(theme::color(theme::palette().ui_muted)).child(entry.description.clone()))
                    .child(div().text_color(theme::color(theme::palette().ui_muted)).child(entry.permission_text()))
                    .child(div().flex().pt(px(4.)).child(self.catalog_button(("addon-install", index), "Install".to_string(), entry.clone(), cx)))
                    .into_any_element()
            })
            .collect();
        let catalog_note = match &self.catalog {
            CatalogState::Idle | CatalogState::Loading => Some("Looking for add-ons…".to_string()),
            CatalogState::Failed(error) => Some(format!("Cannot reach the catalog. {error}")),
            CatalogState::Ready(_) if available.is_empty() => Some("Everything in the catalog is installed.".to_string()),
            CatalogState::Ready(_) => None,
        };
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
                    .text_color(theme::color(theme::palette().ui_muted))
                    .child("Add-ons run with your user rights and have no sandbox, so turn on only those you trust. Permissions limit their requests to the editor, not the program itself."),
            )
            .child(
                div().flex().flex_none().child(self.panel_button("addon-install", "Install from folder", false, cx, |this, _, cx| {
                    this.prompt_install_addon(cx);
                })),
            )
            .child(div().flex_none().font_weight(FontWeight::SEMIBOLD).child("Installed"))
            .when(rows.is_empty(), |element| element.child(div().flex_none().child("No add-ons are installed.")))
            .children(rows)
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_between()
                    .pt(px(8.))
                    .child(div().font_weight(FontWeight::SEMIBOLD).child("Available"))
                    .child(self.panel_button("addon-refresh", "Refresh", false, cx, |this, _, cx| this.fetch_catalog(cx))),
            )
            .child(
                div()
                    .flex_none()
                    .text_color(theme::color(theme::palette().ui_muted))
                    .child("Downloads come from the repository sudo-Oliver/editor-addons. A checksum protects the download but does not say who wrote it, so installing means trusting that repository."),
            )
            .children(catalog_note.map(|note| div().flex_none().child(note)))
            .children(available)
    }

    fn render_compiler_settings(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let trigger = self.config.trigger;
        let engine = self.config.engine;
        let set_trigger = |trigger: Trigger| -> MenuHandler {
            Rc::new(move |this, _, cx| this.set_trigger(trigger, cx))
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
                if self.config.trigger_set {
                    "This project's .editor.toml chooses, so a change here lasts until the app closes."
                } else {
                    "While typing saves your files by itself. Applies to all projects."
                },
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

    /// Gives the command on the shortcuts page the keys just pressed. `Esc`
    /// cancels, so it is no key for a command from here.
    fn record_shortcut(&mut self, keystroke: &Keystroke, cx: &mut Context<Self>) {
        let Some(id) = self.recording_shortcut else {
            return;
        };
        if keystroke.key.is_empty() || matches!(keystroke.key.as_str(), "shift" | "control" | "alt" | "platform" | "function") {
            return;
        }
        self.recording_shortcut = None;
        if keystroke.key == "escape" && !keystroke.modifiers.modified() {
            return cx.notify();
        }
        let Some(command) = COMMANDS.iter().find(|command| command.id == id) else {
            return;
        };
        let mut keymap = self.keymap.clone();
        keymap.set(command, vec![keymap::record(keystroke, keymap::host_os())]);
        self.apply_keymap(keymap, cx);
    }

    /// Opens one of the user's own files as a tab, with `template` when it is new.
    fn open_config_file(&mut self, path: Option<PathBuf>, template: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = path else {
            return self.set_status("No folder for settings: HOME is not set.", cx);
        };
        if !path.exists() {
            let written = path.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|()| crate::atomic::write(&path, template.as_bytes()));
            if let Err(error) = written {
                return self.set_status(format!("Cannot create {}: {error}", path.display()), cx);
            }
        }
        self.close_dialog(window, cx);
        self.show_hub = false;
        self.open_file(path, window, cx);
    }

    fn settings_button(&self, id: &'static str, label: &'static str, cx: &mut Context<Self>, handler: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static) -> impl IntoElement {
        div()
            .id(id)
            .flex_none()
            .px(px(8.))
            .py(px(3.))
            .rounded(px(4.))
            .cursor_pointer()
            .bg(theme::color(theme::palette().divider))
            .on_click(cx.listener(move |this, _, window, cx| handler(this, window, cx)))
            .child(label)
    }

    fn render_shortcut_settings(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let os = keymap::host_os();
        let palette = theme::palette();
        let mut elements = vec![
            Self::setting_row(
                "Vim mappings",
                format!(
                    "{} from vimrc, written like a .vimrc: inoremap jk <Esc>, nnoremap <leader>w :w<CR>. The leader is Space unless the file sets mapleader.",
                    match self.vim_mappings.len() {
                        1 => "1 mapping".to_string(),
                        count => format!("{count} mappings"),
                    }
                ),
                self.settings_button("open-vimrc", "Edit vimrc", cx, |this, window, cx| {
                    this.open_config_file(settings::vimrc_path(), VIMRC_TEMPLATE, window, cx);
                }),
            )
            .into_any_element(),
            Self::setting_row(
                "Keymap file",
                "keymap.json keeps the shortcuts changed below and can be edited by hand. Saving it applies it.",
                self.settings_button("open-keymap", "Edit keymap.json", cx, |this, window, cx| {
                    let template = this.keymap.to_json();
                    this.open_config_file(keymap::default_path(), &template, window, cx);
                }),
            )
            .into_any_element(),
            div()
                .flex_none()
                .pt(px(6.))
                .text_color(theme::color(palette.ui_muted))
                .child("Click a shortcut and press the new keys. Esc cancels.")
                .into_any_element(),
        ];
        let mut groups: Vec<&str> = Vec::new();
        for command in COMMANDS {
            if !groups.contains(&command.group) {
                groups.push(command.group);
            }
        }
        for group in groups {
            elements.push(div().flex_none().pt(px(14.)).pb(px(4.)).font_weight(FontWeight::SEMIBOLD).child(group).into_any_element());
            for command in COMMANDS.iter().filter(|command| command.group == group && command.available()) {
                let id = command.id;
                let recording = self.recording_shortcut == Some(id);
                let keys = self.keymap.keys(command);
                let shown: SharedString = if recording {
                    "Press keys…".into()
                } else if keys.is_empty() {
                    "None".into()
                } else {
                    keys.iter().map(|keys| keymap::display(keys, os)).collect::<Vec<_>>().join(", ").into()
                };
                let conflicts: Vec<&str> = keys
                    .iter()
                    .flat_map(|keys| self.keymap.conflicts(command, keys, os))
                    .map(|other| other.label)
                    .collect();
                let changed = self.keymap.is_changed(command);
                elements.push(
                    div()
                        .flex()
                        .flex_col()
                        .flex_none()
                        .py(px(3.))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(12.))
                                .child(div().flex_1().min_w_0().child(command.label))
                                .when(changed, |row| {
                                    row.child(
                                        div()
                                            .id(("shortcut-reset", COMMANDS.iter().position(|other| other.id == id).unwrap_or(0)))
                                            .flex_none()
                                            .cursor_pointer()
                                            .text_color(theme::color(palette.ui_muted))
                                            .hover(|style| style.text_color(theme::color(palette.ui_text)))
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                if let Some(command) = COMMANDS.iter().find(|command| command.id == id) {
                                                    let mut keymap = this.keymap.clone();
                                                    keymap.reset(command);
                                                    this.apply_keymap(keymap, cx);
                                                }
                                            }))
                                            .child("Reset"),
                                    )
                                })
                                .child(
                                    div()
                                        .id(("shortcut-keys", COMMANDS.iter().position(|other| other.id == id).unwrap_or(0)))
                                        .flex_none()
                                        .min_w(px(110.))
                                        .px(px(8.))
                                        .py(px(2.))
                                        .rounded(px(4.))
                                        .cursor_pointer()
                                        .font_family(theme::code_font())
                                        .text_size(px(theme::code_font_size()))
                                        .text_color(theme::color(if keys.is_empty() && !recording { palette.ui_muted } else { palette.function }))
                                        .bg(theme::color(if recording { palette.accent_soft } else { palette.divider }))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.recording_shortcut = if this.recording_shortcut == Some(id) { None } else { Some(id) };
                                            cx.notify();
                                        }))
                                        .child(shown),
                                ),
                        )
                        .when(!conflicts.is_empty(), |row| {
                            row.child(
                                div()
                                    .text_color(theme::color(palette.error))
                                    .child(format!("Also bound to: {}", conflicts.join(", "))),
                            )
                        })
                        .into_any_element(),
                );
            }
        }
        elements
    }

    /// Where the build of the open project writes, as `recompile` and the runner decide it.
    fn build_directory(&self, cx: &App) -> Option<PathBuf> {
        let root = self.root.clone()?;
        let active = self.active_view().and_then(|view| view.read(cx).path.clone());
        let main = self.last_main.clone().or_else(|| find_main(&root, &self.config, active.as_deref()));
        let engine = self.config.engine.or_else(|| main.as_deref().and_then(Engine::for_path));
        let folder = match main {
            Some(main) if engine != Some(Engine::Typst) => main.parent().map_or_else(|| root.clone(), Path::to_path_buf),
            _ => root,
        };
        Some(folder.join(build::OUTPUT_DIRECTORY))
    }

    /// Measures in the background, since walking gigabytes of tools takes a while.
    fn measure_storage(&mut self, cx: &mut Context<Self>) {
        let build = self.build_directory(cx);
        self.storage_task = Some(cx.spawn(async move |this, cx| {
            let usage = cx.background_spawn(async move { storage::measure(build.as_deref()) }).await;
            this.update(cx, |this, cx| {
                this.storage_usage = Some(usage);
                cx.notify();
            })
            .ok();
        }));
    }

    /// Clears away from the UI thread, then measures again.
    fn clear_storage(&mut self, part: &'static str, clear: impl FnOnce() -> std::io::Result<()> + Send + 'static, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let cleared = cx.background_spawn(async move { clear() }).await;
            this.update(cx, |this, cx| {
                match cleared {
                    Ok(()) => this.set_status(format!("{part} cleared."), cx),
                    Err(error) => this.set_status(format!("Cannot clear {}: {error}", part.to_lowercase()), cx),
                }
                this.measure_storage(cx);
            })
            .ok();
        })
        .detach();
    }

    /// Nothing brings the versions back, so it asks first.
    fn clear_history(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(directory) = history::directory() else {
            return;
        };
        cx.spawn_in(window, async move |this, cx| {
            let detail = "Earlier versions of every file go away, also of deleted files. The files themselves stay as they are.";
            if ask(&this, cx, PromptLevel::Warning, "Clear the file history?", detail, &["Clear history", "Cancel"]).await != Some(0) {
                return;
            }
            this.update(cx, |this, cx| this.clear_storage("File history", move || history::clear(&directory), cx)).ok();
        })
        .detach();
    }

    /// The colors of a Neovim theme in use stay, since the next start reads them from the cache.
    fn clear_caches(&mut self, cx: &mut Context<Self>) {
        let theme = self.settings.theme.clone().filter(|name| theme::built_in(name).is_none());
        self.clear_storage(
            "Caches",
            move || {
                let kept = theme.zip(nvim_themes::cache_path()).and_then(|(name, path)| {
                    let palette = nvim_themes::load_cache(&path).remove(&name)?;
                    Some((path, nvim_themes::Themes::from([(name, palette)])))
                });
                storage::clear_caches()?;
                kept.map_or(Ok(()), |(path, themes)| nvim_themes::save_cache(&path, &themes))
            },
            cx,
        );
    }

    fn render_storage_settings(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let palette = theme::palette();
        let usage = self.storage_usage.as_ref();
        // The sizes stand in one column at the right, with "…" while measuring.
        let row = |button: Option<AnyElement>, bytes: Option<u64>| {
            div().flex().flex_none().items_center().gap(px(12.)).children(button).child(
                div()
                    .flex()
                    .flex_none()
                    .justify_end()
                    .w(px(64.))
                    .text_color(theme::color(palette.ui_muted))
                    .child(bytes.map_or_else(|| "…".to_string(), storage::human)),
            )
        };
        let size = |part: fn(&storage::Usage) -> u64| usage.map(part);
        let home = crate::platform::home();
        let shown = |path: &Path| match home.as_deref().and_then(|home| path.strip_prefix(home).ok()) {
            Some(rest) => format!("~/{}", rest.display()),
            None => path.display().to_string(),
        };
        let installing = self.toolchain.read(cx).busy();
        let limit = history::TOTAL_LIMIT / (1024 * 1024);
        let mut elements = vec![
            div()
                .flex_none()
                .pb(px(4.))
                .text_color(theme::color(palette.ui_muted))
                .child(match usage {
                    Some(usage) => format!("The app keeps {} outside your projects.", storage::human(usage.total)),
                    None => "Measuring what the app keeps outside your projects…".to_string(),
                })
                .into_any_element(),
            Self::setting_row(
                "File history",
                format!("Earlier versions of your files. Compressed and at most {limit} MB in total, so older versions thin out over time."),
                row(
                    Some(self.settings_button("clear-history", "Clear history", cx, |this, window, cx| this.clear_history(window, cx)).into_any_element()),
                    size(|usage| usage.history),
                ),
            )
            .into_any_element(),
            Self::setting_row(
                "Crash recovery copies",
                "Unsaved text, so that a crash loses at most a few seconds. Gone once saved.",
                row(None, size(|usage| usage.recovery)),
            )
            .into_any_element(),
            Self::setting_row("Installed tools", "Tectonic, Typst and Pandoc. Remove them under Writing tools.", row(None, size(|usage| usage.tools)))
                .into_any_element(),
            Self::setting_row("Templates", "Your own templates. Remove them on the Home screen under Templates.", row(None, size(|usage| usage.templates)))
                .into_any_element(),
            Self::setting_row(
                "Downloads",
                if installing {
                    "Installs that did not finish, so that a second try continues. In use while an install runs."
                } else {
                    "Installs that did not finish, so that a second try continues. Gone after two weeks."
                },
                row(
                    (!installing).then(|| {
                        self.settings_button("clear-downloads", "Clear", cx, |this, _, cx| {
                            this.clear_storage("Downloads", || storage::downloads_dir().map_or(Ok(()), |downloads| storage::clear(&downloads)), cx);
                        })
                        .into_any_element()
                    }),
                    size(|usage| usage.downloads),
                ),
            )
            .into_any_element(),
            Self::setting_row(
                "Caches",
                "Lists of themes and LaTeX commands, made again when needed.",
                row(Some(self.settings_button("clear-caches", "Clear", cx, |this, _, cx| this.clear_caches(cx)).into_any_element()), size(|usage| usage.caches)),
            )
            .into_any_element(),
        ];
        if addons::ENABLED {
            elements.push(Self::setting_row("Add-ons", "Remove them in the Add-ons panel.", row(None, size(|usage| usage.addons))).into_any_element());
        }
        if let Some(root) = &self.root {
            let name = root.file_name().map_or_else(|| shown(root), |name| name.to_string_lossy().into_owned());
            elements.push(
                Self::setting_row(
                    "Build output",
                    format!("The .build folder of {name}, which makes the next build fast. Recompiling from scratch starts it over."),
                    row(
                        Some(self.settings_button("storage-recompile", "Recompile from scratch", cx, |this, _, cx| this.recompile_from_scratch(cx)).into_any_element()),
                        usage.and_then(|usage| usage.build),
                    ),
                )
                .into_any_element(),
            );
        }
        for (id, title, folder) in [("show-data", "App folder", storage::data_dir()), ("show-cache", "Cache folder", storage::cache_dir())] {
            let Some(folder) = folder else {
                continue;
            };
            elements.push(
                Self::setting_row(
                    title,
                    shown(&folder),
                    self.settings_button(id, if cfg!(windows) { "Show in Explorer" } else { "Show in Finder" }, cx, move |_, _, cx| cx.reveal_path(&folder)),
                )
                .into_any_element(),
            );
        }
        elements
    }

    fn render_tool_settings(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut elements = vec![
            div()
                .flex_none()
                .pb(px(4.))
                .text_color(theme::color(theme::palette().ui_muted))
                .child("A compiler turns what you write into a PDF. Install the ones for the languages you use.")
                .into_any_element(),
        ];
        elements.extend(toolchain::installer_choice(&self.toolchain, cx));
        elements.extend(toolchain::tool_rows(&self.toolchain, Tool::ALL, true, cx));
        elements.extend(texlive::section(&self.texlive, &self.toolchain, cx));
        elements.push(div().flex_none().pt(px(4.)).text_size(px(11.)).text_color(theme::color(theme::palette().ui_muted)).child(toolchain::promise(&self.toolchain, cx)).into_any_element());
        elements
    }

    fn run_setup(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let again = self.settings.first_run_done;
        let setup = cx.new(|cx| SetupView::new(self.toolchain.clone(), again, cx));
        let subscription = cx.subscribe_in(&setup, window, |this, _, event, window, cx| match event {
            SetupEvent::Finished { example, guided } => this.finish_setup(*example, *guided, window, cx),
            SetupEvent::InstallCommand => this.install_shell_command(cx),
            SetupEvent::ReportProblem => this.open_report(window, cx),
        });
        let focus = setup.read(cx).focus_handle(cx);
        window.focus(&focus, cx);
        self.setup = Some((setup, subscription));
        cx.notify();
    }

    fn finish_setup(&mut self, example: Option<TemplateLanguage>, guided: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.setup = None;
        self.settings.first_run_done = true;
        if guided {
            // Nobody new has to know that saving makes the PDF.
            self.settings.compile_while_typing = true;
            if self.root.is_some() && !self.config.trigger_set {
                self.config.trigger = Trigger::Type;
            }
        } else {
            self.settings.tour_done = true;
        }
        self.save_settings(cx);
        if let Some(path) = self.pending_open.take() {
            self.open_path(path, window, cx);
            if example.is_some() && !self.settings.tour_done && self.root.is_some() {
                self.start_tour(window, cx);
            }
            return cx.notify();
        }
        match example {
            Some(language) => match self.home.update(cx, |home, cx| home.create_welcome(language, cx)) {
                Ok(dir) => {
                    self.show_hub = false;
                    self.open_root(dir, window, cx);
                    if !self.settings.tour_done {
                        self.start_tour(window, cx);
                    }
                }
                Err(error) => {
                    self.open_hub(window, cx);
                    self.set_status(error, cx);
                }
            },
            None => self.open_hub(window, cx),
        }
        cx.notify();
    }

    /// Without a project, the tour opens the welcome example to have something to show.
    fn start_tour(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.root.is_none() {
            let language = if !toolchain::language_ready(Tool::Tectonic) && toolchain::language_ready(Tool::Typst) {
                TemplateLanguage::Typst
            } else {
                TemplateLanguage::Latex
            };
            match self.home.update(cx, |home, cx| home.create_welcome(language, cx)) {
                Ok(dir) => self.open_root(dir, window, cx),
                Err(error) => return self.set_status(error, cx),
            }
        }
        if self.open_menu.is_some() {
            self.close_menu(window, cx);
        }
        self.dialog = None;
        self.picker = None;
        self.show_hub = false;
        self.tour = Some(Tour::default());
        self.prepare_tour_step(window, cx);
    }

    fn end_tour(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.tour = None;
        self.tour_anchors.borrow_mut().clear();
        if !self.settings.tour_done {
            self.settings.tour_done = true;
            self.save_settings(cx);
        }
        self.focus_active(window, cx);
        cx.notify();
    }

    fn tour_step(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tour) = &mut self.tour else {
            return;
        };
        if forward {
            if !tour.next() {
                return self.end_tour(window, cx);
            }
        } else {
            tour.back();
        }
        self.prepare_tour_step(window, cx);
    }

    /// Shows the part that the step points at, in case the user hid it.
    fn prepare_tour_step(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(tour) = &self.tour else {
            return;
        };
        match tour.current().anchor {
            TourAnchor::FileTree => {
                self.show_file_panel = true;
                self.left_panel = LeftPanel::Files;
                self.show_tree = true;
            }
            TourAnchor::Editor | TourAnchor::Toolbar | TourAnchor::Recompile | TourAnchor::Pdf => {
                self.focus_mode = false;
                self.pdf_tab_shown = false;
                self.layout = PaneLayout::Split;
            }
            _ => {}
        }
        cx.notify();
    }

    /// An invisible layer that notes where its parent was drawn, for the tour.
    fn tour_anchor(&self, anchor: TourAnchor) -> Option<impl IntoElement> {
        self.tour.as_ref()?;
        let anchors = self.tour_anchors.clone();
        Some(
            canvas(
                move |bounds, window, _| {
                    let previous = anchors.borrow_mut().insert(anchor, bounds);
                    if previous != Some(bounds) {
                        window.refresh();
                    }
                },
                |_, _, _, _| {},
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full(),
        )
    }

    fn render_tour(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        const BUBBLE: (f32, f32) = (330., 180.);
        const PADDING: f32 = 6.;
        let viewport = window.viewport_size();
        let (width, height) = (viewport.width.to_f64() as f32, viewport.height.to_f64() as f32);
        let anchor = self.tour.as_ref()?.current().anchor;
        let found = self.tour_anchors.borrow().get(&anchor).copied();
        let target = match found {
            Some(bounds) => tour::Rect {
                x: bounds.origin.x.to_f64() as f32,
                y: bounds.origin.y.to_f64() as f32,
                width: bounds.size.width.to_f64() as f32,
                height: bounds.size.height.to_f64() as f32,
            }
            .grow(PADDING)
            .clamp(width, height),
            // The part is not on screen, so the explanation stands alone in the middle.
            None => tour::Rect { x: width / 2., y: height / 2., width: 0., height: 0. },
        };
        let tour = self.tour.as_mut()?;
        let (light, moving) = tour.light(target);
        if moving {
            window.request_animation_frame();
        }
        let step = tour.current();
        let (number, last) = (tour.step + 1, tour.is_last());
        let (bubble_x, bubble_y) = tour::place_bubble(light, BUBBLE, (width, height), 14.);
        let palette = theme::palette();
        let shade = |x: f32, y: f32, w: f32, h: f32| div().absolute().left(px(x)).top(px(y)).w(px(w.max(0.))).h(px(h.max(0.))).bg(black().opacity(0.55));
        let button = |id: &'static str, label: &'static str, primary: bool, cx: &mut Context<Self>, forward: Option<bool>| {
            div()
                .id(id)
                .px(px(12.))
                .py(px(6.))
                .rounded(px(theme::RADIUS_MD))
                .cursor_pointer()
                .font_weight(FontWeight::SEMIBOLD)
                .when(primary, |style| style.bg(theme::color(palette.accent)).text_color(theme::color(palette.on_accent)))
                .when(!primary, |style| style.text_color(theme::color(palette.ui_muted)).hover(|style| style.bg(theme::color(palette.divider))))
                .on_click(cx.listener(move |this, _, window, cx| match forward {
                    Some(forward) => this.tour_step(forward, window, cx),
                    None => this.end_tour(window, cx),
                }))
                .child(label)
        };
        Some(
            div()
                .id("tour")
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .occlude()
                .child(shade(0., 0., width, light.y))
                .child(shade(0., light.bottom(), width, height - light.bottom()))
                .child(shade(0., light.y, light.x, light.height))
                .child(shade(light.right(), light.y, width - light.right(), light.height))
                .child(
                    div()
                        .absolute()
                        .left(px(light.x))
                        .top(px(light.y))
                        .w(px(light.width))
                        .h(px(light.height))
                        .rounded(px(theme::RADIUS_MD))
                        .border_2()
                        .border_color(theme::color(palette.accent)),
                )
                .child(
                    div()
                        .absolute()
                        .left(px(bubble_x))
                        .top(px(bubble_y))
                        .w(px(BUBBLE.0))
                        .flex()
                        .flex_col()
                        .gap(px(8.))
                        .p(px(16.))
                        .rounded(px(theme::RADIUS_LG))
                        .bg(theme::color(palette.panel))
                        .border_1()
                        .border_color(theme::color(palette.divider))
                        .shadow_lg()
                        .child(
                            div()
                                .text_size(px(11.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(theme::color(palette.accent))
                                .child(format!("STEP {number} OF {}", tour::STEPS.len())),
                        )
                        .child(div().text_size(px(16.)).font_weight(FontWeight::SEMIBOLD).child(step.title))
                        .child(div().line_height(px(19.)).text_color(theme::color(palette.ui_muted)).child(step.body))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(4.))
                                .pt(px(4.))
                                .child(button("tour-skip", "Skip tour", false, cx, None))
                                .child(div().flex_1())
                                .when(number > 1, |element| element.child(button("tour-back", "Back", false, cx, Some(false))))
                                .child(button("tour-next", if last { "Done" } else { "Next" }, true, cx, Some(true))),
                        ),
                )
                .into_any_element(),
        )
    }

    /// Keeps the choice for all projects, unless this one's `.editor.toml` makes it.
    fn set_trigger(&mut self, trigger: Trigger, cx: &mut Context<Self>) {
        self.config.trigger = trigger;
        if !self.config.trigger_set {
            self.settings.compile_while_typing = trigger == Trigger::Type;
            self.save_settings(cx);
        }
        cx.notify();
    }

    fn save_settings(&mut self, cx: &mut Context<Self>) {
        if let Some(path) = &self.settings_path
            && let Err(error) = self.settings.save(path)
        {
            self.set_status(format!("Cannot save settings: {error}"), cx);
        }
    }

    /// A build that failed for want of this tool runs again.
    fn tool_installed(&mut self, tool: Tool, cx: &mut Context<Self>) {
        self.set_status(format!("{} is ready.", tool.language()), cx);
        if tool == Tool::MacTex {
            // A new year changes what there is to update, and what can go.
            self.texlive.update(cx, |texlive, cx| texlive.look(cx));
        }
        if self.root.is_some() && !self.last_build_succeeded {
            self.recompile(cx);
        }
    }

    fn render_shortcuts(&self) -> Vec<AnyElement> {
        let mut elements: Vec<AnyElement> = shortcuts::sections(self.settings.vim_mode, &self.keymap)
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
                                    .font_family(theme::code_font())
                                    .text_size(px(theme::code_font_size()))
                                    .text_color(theme::color(theme::palette().function))
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
                    .text_color(theme::color(theme::palette().ui_muted))
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
            .when(page == current, |style| style.bg(theme::color(theme::palette().accent_soft)).font_weight(FontWeight::SEMIBOLD))
            .when(page != current, |style| style.hover(|style| style.bg(theme::color(theme::palette().divider))))
            .on_click(cx.listener(move |this, _, _, cx| {
                if page == SettingsPage::Storage && this.dialog != Some(Dialog::Settings(page)) {
                    this.storage_usage = None;
                    this.measure_storage(cx);
                }
                this.dialog = Some(Dialog::Settings(page));
                this.recording_shortcut = None;
                cx.notify();
            }))
            .child(label)
    }

    fn render_dialog(&self, dialog: Dialog, cx: &mut Context<Self>) -> impl IntoElement {
        let title: SharedString = match dialog {
            Dialog::Settings(_) => "Settings".into(),
            Dialog::Shortcuts => "Keyboard shortcuts".into(),
            Dialog::WordCount => "Word count".into(),
            #[cfg(feature = "collab")]
            Dialog::JoinSession => "Join a live session".into(),
            Dialog::GitSignIn => "Sign in".into(),
            Dialog::CommitDiff => self.commit_title(),
            Dialog::ReportProblem => "Report a problem".into(),
        };
        let content = match dialog {
            Dialog::Settings(SettingsPage::Editor) => self.render_editor_settings(cx),
            Dialog::Settings(SettingsPage::Compiler) => self.render_compiler_settings(cx),
            Dialog::Settings(SettingsPage::Tools) => self.render_tool_settings(cx),
            Dialog::Settings(SettingsPage::Shortcuts) => self.render_shortcut_settings(cx),
            Dialog::Settings(SettingsPage::Storage) => self.render_storage_settings(cx),
            Dialog::Shortcuts => self.render_shortcuts(),
            Dialog::WordCount => self.render_word_count(),
            #[cfg(feature = "collab")]
            Dialog::JoinSession => self.render_join_dialog(cx),
            Dialog::GitSignIn => self.render_sign_in(cx),
            Dialog::CommitDiff => self.render_commit_view(),
            Dialog::ReportProblem => self.render_report_dialog(cx),
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
                    .bg(theme::color(theme::palette().panel))
                    .border_1()
                    .border_color(theme::color(theme::palette().divider))
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
                            .border_color(theme::color(theme::palette().divider))
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
                                    _ => None,
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
                                            .border_color(theme::color(theme::palette().divider))
                                            .child(self.settings_tab(SettingsPage::Editor, page, "Editor", cx))
                                            .child(self.settings_tab(SettingsPage::Compiler, page, "Compiler", cx))
                                            .child(self.settings_tab(SettingsPage::Tools, page, "Writing tools", cx))
                                            .child(self.settings_tab(SettingsPage::Shortcuts, page, "Shortcuts", cx))
                                            .child(self.settings_tab(SettingsPage::Storage, page, "Storage", cx)),
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
            PickerKind::Themes => "Select theme",
            PickerKind::History => "Earlier versions",
            PickerKind::Fonts(FontSlot::Editor) => "Editor font",
            PickerKind::Fonts(FontSlot::Interface) => "Interface font",
            PickerKind::Commands => "Run a command",
        };
        let empty = match (picker.kind, picker.query.is_empty()) {
            (PickerKind::Text, true) => "Type to search the project.",
            (PickerKind::Figures, true) => "No images in this project.",
            (PickerKind::Citations, true) => "No .bib entries in this project.",
            (PickerKind::Branches, true) => "Type a name to create a branch.",
            (PickerKind::History, true) => "No earlier versions yet. Each save keeps the one before.",
            _ => "No results.",
        };
        let detail = self.citation_detail(picker);
        let width = PICKER_WIDTH + if detail.is_some() { CITATION_DETAIL_WIDTH } else { 0. };
        // How many entries the list leaves out, and how to cite more than one.
        let citation_hint = picker.citations.as_ref().filter(|_| picker.kind == PickerKind::Citations).map(|rows| {
            let more = keymap::display("secondary-enter", keymap::host_os());
            let shown = picker.items.len();
            let total = rows.len().saturating_sub(picker.cited.len());
            if picker.query.trim().is_empty() && shown < total { format!("{shown} of {total}. {more} cites another") } else { format!("{more} cites another") }
        });
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
                    .w(px(width))
                    .h(px(PICKER_HEIGHT))
                    .max_h(gpui::relative(0.8))
                    .rounded(px(6.))
                    .bg(theme::color(theme::palette().panel))
                    .border_1()
                    .border_color(theme::color(theme::palette().divider))
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
                            .border_color(theme::color(theme::palette().divider))
                            .child(div().flex_none().text_color(theme::color(theme::palette().ui_muted)).child(title))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .font_family(theme::code_font())
                                    .flex_1()
                                    .min_w_0()
                                    .child(picker.query.clone())
                                    .child(div().w(px(1.5)).h(px(15.)).bg(theme::color(theme::palette().focus))),
                            )
                            .when_some(citation_hint, |element, hint| {
                                element.child(div().flex_none().text_size(px(11.)).text_color(theme::color(theme::palette().ui_muted)).child(hint))
                            }),
                    )
                    .child(
                        div().flex().flex_1().min_h_0().child(
                        div()
                            .id("picker-results")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .track_scroll(&self.picker_scroll)
                            .when(picker.items.is_empty(), |element| {
                                element.child(div().p(px(12.)).text_color(theme::color(theme::palette().ui_muted)).child(empty))
                            })
                            .children(picker.items.iter().enumerate().map(|(index, item)| {
                                let location = match item.position {
                                    Some((line, _)) => format!("{}:{}", item.label, line + 1),
                                    None => item.label.clone(),
                                };
                                let row = div()
                                    .id(("picker-item", index))
                                    .px(px(12.))
                                    .cursor_pointer()
                                    .when(index == picker.selected, |style| style.bg(theme::color(theme::palette().accent_soft)))
                                    .when(index != picker.selected, |style| style.hover(|style| style.bg(theme::color(theme::palette().divider))))
                                    .on_click(cx.listener(move |this, _, window, cx| this.accept_picker(index, window, cx)));
                                let muted = theme::color(theme::palette().ui_muted);
                                // A citation: who and when above the key, then title and venue.
                                if let Some(summary) = item.summary.clone() {
                                    return row
                                        .flex()
                                        .flex_col()
                                        .justify_center()
                                        .h(px(44.))
                                        .child(
                                            div()
                                                .flex()
                                                .items_center()
                                                .gap(px(12.))
                                                .child(div().flex_1().min_w_0().truncate().font_weight(FontWeight::SEMIBOLD).child(summary))
                                                .child(
                                                    div()
                                                        .flex_none()
                                                        .max_w(px(220.))
                                                        .truncate()
                                                        .font_family(theme::code_font())
                                                        .text_size(px(theme::code_font_size()))
                                                        .text_color(muted)
                                                        .child(location),
                                                ),
                                        )
                                        .when_some(item.detail.clone(), |element, detail| {
                                            element.child(div().truncate().text_color(muted).child(detail))
                                        })
                                        .into_any_element();
                                }
                                row.flex()
                                    .items_center()
                                    .gap(px(12.))
                                    .h(px(26.))
                                    .child(div().flex_none().max_w(px(320.)).truncate().child(location))
                                    .when_some(item.detail.clone(), |element, detail| {
                                        element.child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .truncate()
                                                .font_family(theme::code_font())
                                                .text_size(px(theme::code_font_size()))
                                                .text_color(muted)
                                                .child(detail),
                                        )
                                    })
                                    .into_any_element()
                            })),
                        )
                        .children(detail),
                    ),
            )
    }

    /// Beside the citation list: the chosen entry in full, as the
    /// bibliography will show it, and where it is written.
    fn citation_detail(&self, picker: &Picker) -> Option<AnyElement> {
        let rows = picker.citations.as_ref().filter(|_| picker.kind == PickerKind::Citations)?;
        let palette = theme::palette();
        let muted = theme::color(palette.ui_muted);
        let panel = div()
            .flex()
            .flex_col()
            .flex_none()
            .w(px(CITATION_DETAIL_WIDTH))
            .gap(px(8.))
            .p(px(14.))
            .border_l_1()
            .border_color(theme::color(palette.divider))
            .overflow_hidden();
        let Some(item) = picker.items.get(picker.selected) else {
            return Some(panel.into_any_element());
        };
        let Some(entry) = rows.entry(&item.label) else {
            // Offered by an add-on, or a Typst label: only what the row says.
            return Some(panel.child(div().font_family(theme::code_font()).child(item.label.clone())).when_some(item.detail.clone(), |panel, detail| panel.child(div().text_color(muted).child(detail))).into_any_element());
        };
        let small = |text: String| div().text_size(px(11.)).text_color(muted).child(text);
        let mut venue = entry.venue.clone();
        if !entry.year.is_empty() {
            if !venue.is_empty() {
                venue.push_str(", ");
            }
            venue.push_str(&entry.year);
            venue.extend(entry.letter);
        }
        let place = {
            let relative = self.root.as_deref().and_then(|root| entry.path.strip_prefix(root).ok()).unwrap_or(&entry.path).to_string_lossy().into_owned();
            if entry.kind == "bibitem" { relative } else { format!("{relative}:{}", entry.line + 1) }
        };
        Some(
            panel
                .when(!entry.full_authors.is_empty(), |panel| panel.child(div().text_color(muted).child(entry.full_authors.clone())))
                .when(!entry.title.is_empty(), |panel| panel.child(div().font_weight(FontWeight::SEMIBOLD).child(entry.title.clone())))
                .when(!venue.is_empty(), |panel| panel.child(div().child(venue)))
                .when(!entry.doi.is_empty(), |panel| panel.child(small(format!("doi: {}", entry.doi))))
                .when(entry.doi.is_empty() && !entry.url.is_empty(), |panel| panel.child(small(entry.url.clone())))
                .when(!entry.abstract_text.is_empty(), |panel| panel.child(div().flex_1().min_h_0().overflow_hidden().text_size(px(12.)).text_color(muted).child(entry.abstract_text.clone())))
                .child(
                    div()
                        .flex()
                        .gap(px(8.))
                        .mt_auto()
                        .font_family(theme::code_font())
                        .text_size(px(theme::code_font_size() - 1.))
                        .text_color(muted)
                        .child(div().flex_none().child(format!("@{}", entry.kind)))
                        .child(div().flex_1().min_w_0().truncate().child(place)),
                )
                .into_any_element(),
        )
    }

    /// The logs or the PDF with the errors of the last build above it.
    fn render_preview_body(&self, cx: &mut Context<Self>) -> AnyElement {
        let has_errors = self.error_count() > 0;
        if self.markdown_view(cx).is_some() {
            self.markdown.clone().into_any_element()
        } else if self.show_logs {
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
        }
    }

    /// The column beside the editor, or the whole width in `PdfOnly`.
    fn render_preview(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let body = self.render_preview_body(cx);
        let alone = self.layout == PaneLayout::PdfOnly;
        div()
            .relative()
            .children(self.tour_anchor(TourAnchor::Pdf))
            .flex()
            .flex_col()
            .when(alone, |element| element.flex_1().min_w_0())
            .when(!alone, |element| element.flex_none().w(self.preview_width))
            .h_full()
            .bg(theme::color(theme::palette().panel))
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

/// What a new `vimrc` holds, so that the format explains itself.
const VIMRC_TEMPLATE: &str = "\" Vim mappings for the editor, written like a .vimrc and applied on save.
\" Supported: map, nmap, vmap, xmap, imap and their noremap forms
\" (noremap, nnoremap, vnoremap, xnoremap, inoremap), and let mapleader.
\" The leader is Space unless set here, like the built-in Space commands.
\"
\" inoremap jk <Esc>
\" nnoremap H ^
\" nnoremap L $
\" nnoremap <leader>q :q<CR>
";

/// Keys that wait for a mapping, for the status bar.
fn key_names(keys: &[Key]) -> String {
    keys.iter()
        .map(|key| match key {
            Key::Char(' ') => "Space".to_string(),
            Key::Char(ch) => ch.to_string(),
            Key::Ctrl(ch) => format!("Ctrl+{ch}"),
            Key::Esc => "Esc".to_string(),
            Key::Enter => "Enter".to_string(),
            Key::Tab => "Tab".to_string(),
            Key::Backspace => "Backspace".to_string(),
            Key::Delete => "Delete".to_string(),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// A `Ctrl` key that vim has no command for and that only a mapping can use.
fn is_unknown_ctrl(key: Key) -> bool {
    matches!(key, Key::Ctrl(ch) if !matches!(ch, 'd' | 'u' | 'r'))
}

/// Turns a keystroke into a vim key. Characters come from `key_char`, which
/// respects the keyboard layout: on a German layout, `/` is Shift+7 and `{` is Alt+8.
fn translate_key(keystroke: &Keystroke, map_navigation: bool) -> Option<Key> {
    let name = keystroke.key.as_str();
    // AltGr is Ctrl+Alt on Windows: with it, `\`, `{` or `@` on a German
    // layout come as Ctrl keys, but with the character they type.
    if cfg!(windows)
        && keystroke.modifiers.control
        && keystroke.modifiers.alt
        && let Some(text) = keystroke.key_char.as_deref()
    {
        let mut chars = text.chars();
        if let (Some(ch), None) = (chars.next(), chars.next())
            && !ch.is_control()
        {
            return Some(Key::Char(ch));
        }
    }
    if keystroke.modifiers.control {
        let mut chars = name.chars();
        return match (chars.next(), chars.next()) {
            (Some(ch), None) if ch.is_ascii_graphic() => Some(Key::Ctrl(ch)),
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
        self.sync_project_symbols(cx);
        self.settle_addon_dialog(window, cx);
        if std::mem::take(&mut self.setup_requested) && self.setup.is_none() {
            self.run_setup(window, cx);
        }
        if std::mem::take(&mut self.focus_requested) {
            self.focus_active(window, cx);
        }
        // Another document may have become active.
        self.addon_context_changed(cx);
        if let Some(format) = self.export_after_pandoc.filter(|_| !self.toolchain.read(cx).installing(Tool::Pandoc)) {
            self.export_after_pandoc = None;
            match self.toolchain.read(cx).errors.get(&Tool::Pandoc).cloned() {
                Some(error) => self.set_status(error, cx),
                None => self.download_draft(format, window, cx),
            }
        }
        #[cfg(feature = "collab")]
        self.collab_frame(window, cx);
        let addon_dialog = self.dialog_panel(cx).map(|panel| self.render_addon_dialog(panel, cx));
        self.tree_cursor = self.tree_cursor.min(self.entries.len().saturating_sub(1));
        let dragging = self.dragging;
        let presenting = self.presenting.is_some();
        let setup = self.setup.as_ref().map(|(setup, _)| setup.clone());
        let hub = self.hub_shown() || setup.is_some();
        let focus = self.focus_mode && !presenting && !hub;
        let tour = if hub || presenting { None } else { self.render_tour(window, cx) };
        let show_preview = !focus && matches!(self.layout, PaneLayout::Split | PaneLayout::PdfOnly);
        let show_editor = focus || self.layout != PaneLayout::PdfOnly;
        let show_file_panel = self.show_file_panel;
        // Git runs only while its panel shows.
        let git_visible = show_file_panel && self.left_panel == LeftPanel::Git;
        self.git_panel.update(cx, |panel, cx| panel.set_visible(git_visible, cx));
        if git_visible {
            self.trust_git_root(window, cx);
        }
        if show_file_panel && self.left_panel == LeftPanel::Addons && matches!(self.catalog, CatalogState::Idle) {
            self.fetch_catalog(cx);
        }
        if std::mem::take(&mut self.open_citations_soon) {
            cx.defer_in(window, |this, window, cx| this.open_picker(PickerKind::Citations, window, cx));
        }
        if let Some((path, range, line)) = self.definition_soon.take() {
            cx.defer_in(window, move |this, window, cx| {
                this.open_file(path, window, cx);
                if let Some(view) = this.active_view() {
                    view.update(cx, |view, cx| view.select_definition(range, line, window, cx));
                }
            });
        }
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
            .bg(theme::color(theme::palette().bg))
            .text_color(theme::color(theme::palette().ui_text))
            .text_size(px(theme::UI_FONT_SIZE))
            .when_some(theme::look().ui_font, |root, font| root.font_family(font))
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
            .on_action(cx.listener(|this, _: &Save, window, cx| this.save_active(window, cx)))
            .on_action(cx.listener(|this, _: &NewFile, window, cx| {
                if this.dialog.is_none() {
                    this.open_tree_input(false, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &LookupDoi, _, cx| this.lookup_doi(cx)))
            .on_action(cx.listener(|this, _: &QuickFix, _, cx| this.quick_fix(cx)))
            .on_action(cx.listener(|this, action: &RunAddonCommand, _, cx| this.run_addon_command(action, cx)))
            .on_action(cx.listener(|this, _: &InsertCitation, window, cx| {
                if matches!(this.active_language(cx), Some(Language::Latex | Language::Typst)) && this.dialog_panel(cx).is_none() {
                    this.open_picker(PickerKind::Citations, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &Recompile, _, cx| this.recompile_by_hand(cx)))
            .on_action(cx.listener(|this, _: &GoToTab1, window, cx| this.go_to_tab(0, window, cx)))
            .on_action(cx.listener(|this, _: &GoToTab2, window, cx| this.go_to_tab(1, window, cx)))
            .on_action(cx.listener(|this, _: &GoToTab3, window, cx| this.go_to_tab(2, window, cx)))
            .on_action(cx.listener(|this, _: &GoToTab4, window, cx| this.go_to_tab(3, window, cx)))
            .on_action(cx.listener(|this, _: &GoToTab5, window, cx| this.go_to_tab(4, window, cx)))
            .on_action(cx.listener(|this, _: &GoToTab6, window, cx| this.go_to_tab(5, window, cx)))
            .on_action(cx.listener(|this, _: &GoToTab7, window, cx| this.go_to_tab(6, window, cx)))
            .on_action(cx.listener(|this, _: &GoToTab8, window, cx| this.go_to_tab(7, window, cx)))
            .on_action(cx.listener(|this, _: &GoToTab9, window, cx| this.go_to_tab(8, window, cx)))
            .on_action(cx.listener(|this, _: &CloseTab, window, cx| {
                if this.pdf_tab_shown {
                    this.set_layout(PaneLayout::Split, window, cx);
                } else if let Some(index) = this.shown_image {
                    this.close_image(index, window, cx);
                } else if let Some(index) = this.active {
                    this.close_document(index, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &ToggleFileTree, _, cx| {
                this.show_file_panel = !this.show_file_panel;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &TogglePreview, window, cx| this.toggle_preview(window, cx)))
            .on_action(cx.listener(|this, _: &LayoutSplit, window, cx| this.set_layout(PaneLayout::Split, window, cx)))
            .on_action(cx.listener(|this, _: &LayoutEditorOnly, window, cx| this.set_layout(PaneLayout::EditorOnly, window, cx)))
            .on_action(cx.listener(|this, _: &LayoutPdfOnly, window, cx| this.set_layout(PaneLayout::PdfOnly, window, cx)))
            .on_action(cx.listener(|this, _: &LayoutPdfTab, window, cx| this.set_layout(PaneLayout::PdfTab, window, cx)))
            .on_action(cx.listener(|this, _: &ToggleFocusMode, window, cx| this.toggle_focus_mode(window, cx)))
            .on_action(cx.listener(|this, _: &ToggleHub, window, cx| this.toggle_hub(window, cx)))
            .on_action(cx.listener(|this, _: &CommandPalette, window, cx| this.open_command_palette(window, cx)))
            .on_action(cx.listener(|this, _: &OpenGitPanel, window, cx| this.open_git_panel(window, cx)))
            .on_action(cx.listener(|this, _: &StartPresentation, window, cx| this.start_presentation(window, cx)))
            .on_action(cx.listener(|this, _: &ExitPresentation, window, cx| this.exit_presentation(window, cx)))
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
            .on_action(cx.listener(|this, _: &MoveToApplications, _, cx| this.mover.update(cx, |mover, cx| mover.start(cx))))
            // Before the setup too: moving first saves a second setup in the copy.
            .when(!presenting, |element| element.children(app_location::banner(&self.mover, cx)))
            .when(!presenting && setup.is_none(), |element| element.children(updater::banner(&self.updater, cx)))
            .when(hub, |element| {
                let page = match setup {
                    Some(setup) => setup.into_any_element(),
                    None => self.home.clone().into_any_element(),
                };
                element.child(div().flex().flex_1().min_h_0().child(page))
            })
            .on_action(cx.listener(|this, _: &StartTour, window, cx| this.start_tour(window, cx)))
            .when(!presenting && !focus && !hub, |element| element.child(self.render_top_bar(window, cx)))
            // In focus mode Windows still needs a way to move and close the window,
            // as the Mac keeps its traffic lights.
            .when(cfg!(windows) && focus && !presenting, |element| {
                element.child(
                    window_controls::drag_area(div().id("focus-bar"))
                        .absolute()
                        .top_0()
                        .left_0()
                        .right_0()
                        .h(px(32.))
                        .flex()
                        .justify_end()
                        .children(window_controls::buttons(window, 32.)),
                )
            })
            .when(presenting, |element| {
                let controls = self.presenting.as_ref().is_some_and(|state| state.controls_shown);
                element.child(
                    div()
                        .relative()
                        .flex_1()
                        .min_h_0()
                        .bg(gpui::black())
                        .child(self.pdf.clone())
                        .when(controls, |element| {
                            element.child(div().absolute().top(px(12.)).right(px(12.)).child(self.exit_button(
                                "exit-presentation",
                                "Exit presentation",
                                true,
                                cx,
                                |this, window, cx| this.exit_presentation(window, cx),
                            )))
                        }),
                )
            })
            .when(focus, |element| {
                element
                    // The band keeps the window buttons off the text and holds the way out.
                    .child(
                        div()
                            .flex()
                            .flex_none()
                            .items_center()
                            .justify_end()
                            .h(px(TOP_BAR_HEIGHT))
                            .pr(px(10.))
                            .child(self.exit_button("exit-focus", "Exit focus mode", false, cx, |this, window, cx| {
                                this.toggle_focus_mode(window, cx);
                            })),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_1()
                            .min_h_0()
                            .justify_center()
                            .child(div().flex().w_full().max_w(px(FOCUS_WIDTH)).h_full().child(self.render_editor_pane(cx))),
                    )
            })
            .when(!presenting && !focus && !hub, |element| element.child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(self.render_rail(cx))
                    .when(show_file_panel, |element| {
                        let panel = match self.left_panel {
                            LeftPanel::Files => self.render_file_panel(cx).into_any_element(),
                            LeftPanel::Git => div()
                                .flex()
                                .flex_col()
                                .flex_none()
                                .w(self.file_panel_width)
                                .h_full()
                                .children(self.git_note().map(|note| {
                                    div().flex_none().px(px(10.)).py(px(6.)).text_color(theme::color(theme::palette().warning)).child(note)
                                }))
                                .child(div().flex_1().min_h_0().child(self.git_panel.clone()))
                                .into_any_element(),
                            LeftPanel::Addons => self.render_addons_panel(cx).into_any_element(),
                            #[cfg(feature = "collab")]
                            LeftPanel::Collab => self.render_collab_panel(cx).into_any_element(),
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
                    .when(show_editor, |element| element.child(self.render_editor_pane(cx)))
                    .when(show_editor && show_preview, |element| {
                        element.child(self.divider_vertical("preview-divider", Divider::Preview, cx))
                    })
                    .when(show_preview, |element| element.child(self.render_preview(cx))),
            ))
            // Vim needs its mode and command line in focus mode too.
            .when(!presenting && !hub && (!focus || self.settings.vim_mode), |element| element.children(self.render_status_bar(cx)))
            .when_some(self.dialog, |element, dialog| element.child(self.render_dialog(dialog, cx)))
            .when_some(self.picker.as_ref(), |element, picker| element.child(self.render_picker(picker, cx)))
            .when_some(addon_dialog, |element, dialog| element.child(dialog))
            .children(tour)
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
            // Last, over the text too, so that type looks printed into the page.
            // A canvas takes no clicks.
            .when(theme::look().paper && self.presenting.is_none(), |element| {
                element.child(
                    canvas(|_, _, _| {}, |bounds, _, window, _| crate::paper::paint(bounds, window))
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full(),
                )
            })
    }
}

/// One picker row per version: when, and how much it differs from now.
fn history_items(versions: &[history::Version], current: &str) -> Vec<PickerItem> {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |since| since.as_secs());
    versions
        .iter()
        .map(|version| {
            let text = history::read(&version.file).ok().and_then(|bytes| text_file::decode(bytes).ok()).map(|loaded| loaded.text);
            let changes = match text.as_deref().map(|text| editor_core::line_diff::line_diff(current, text)) {
                Some(Some(hunks)) if hunks.is_empty() => "Same as now".to_string(),
                Some(Some(hunks)) => {
                    let lines: usize = hunks.iter().map(|hunk| hunk.lines.len().max(1)).sum();
                    format!("{lines} line{} differ from now", if lines == 1 { "" } else { "s" })
                }
                _ => "Differs from now".to_string(),
            };
            PickerItem {
                path: version.file.clone(),
                label: projects::ago(now, version.time),
                position: None,
                summary: None,
                detail: Some(changes),
            }
        })
        .collect()
}

/// Why a save failed, in words that say what to do. The file on disk is
/// always whole, since saves replace it in one step.
fn save_failed(error: &std::io::Error) -> String {
    use std::io::ErrorKind;
    let advice = match error.kind() {
        ErrorKind::StorageFull | ErrorKind::QuotaExceeded => "The disk is full. Free some space and save again",
        ErrorKind::PermissionDenied | ErrorKind::ReadOnlyFilesystem => "This file or folder cannot be changed. Save a copy somewhere else",
        ErrorKind::NotFound => "Its folder is gone. Save a copy somewhere else",
        _ => return format!("Cannot save: {error}. Your text is still here and kept safe."),
    };
    format!("Cannot save: {advice}. Your text is still here and kept safe.")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn altgr_types_its_character_in_vim() {
        fn keystroke(source: &'static str, typed: Option<&str>) -> Keystroke {
            let mut keystroke = Keystroke::parse(source).unwrap();
            keystroke.key_char = typed.map(ToString::to_string);
            keystroke
        }
        // AltGr+ß and AltGr+7 on a German layout.
        assert!(matches!(translate_key(&keystroke("ctrl-alt-ß", Some("\\")), true), Some(Key::Char('\\'))));
        assert!(matches!(translate_key(&keystroke("ctrl-alt-7", Some("{")), true), Some(Key::Char('{'))));
        // A real Ctrl key stays one.
        assert!(matches!(translate_key(&keystroke("ctrl-d", None), true), Some(Key::Ctrl('d'))));
    }

    #[test]
    fn the_slanted_edge_decides_which_tab_a_click_hits() {
        // An edge that leans to the right going up, as on the left of an active tab.
        let edges = (true, false);
        assert_eq!(tab_hit(103., 100., 200., 1., edges), -1);
        assert_eq!(tab_hit(103., 100., 200., 0., edges), 0);
        assert_eq!(tab_hit(197., 100., 200., 1., edges), 1);
        assert_eq!(tab_hit(197., 100., 200., 0., edges), 0);
        assert_eq!(tab_hit(150., 100., 200., 0.5, edges), 0);
    }

    #[test]
    fn the_trail_holds_the_headings_around_a_line() {
        let item = |depth: usize, line: usize, title: &str| OutlineItem {
            level: depth,
            title: title.to_string(),
            line,
            depth,
            has_children: false,
            collapsed: false,
            number: None,
            kind: editor_core::HeadingKind::Section,
        };
        let items = [item(0, 0, "Intro"), item(1, 5, "Motivation"), item(0, 10, "Method"), item(1, 12, "Data"), item(2, 14, "Sources")];
        assert_eq!(heading_trail(&items, 3), [0]);
        assert_eq!(heading_trail(&items, 7), [0, 1]);
        assert_eq!(heading_trail(&items, 11), [2]);
        assert_eq!(heading_trail(&items, 20), [2, 3, 4]);
        assert!(heading_trail(&items[1..], 2).is_empty());
    }

    #[test]
    fn images_keep_their_place_among_documents() {
        use Tab::*;
        assert_eq!(tab_order(2, &[]), [Document(0), Document(1)]);
        assert_eq!(tab_order(2, &[1, 2]), [Document(0), Image(0), Document(1), Image(1)]);
        assert_eq!(tab_order(0, &[0, 0]), [Image(0), Image(1)]);
    }

    #[test]
    fn neighbors_share_an_edge_that_leans_toward_the_active_tab() {
        for active in [None, Some(0), Some(1), Some(3)] {
            for position in 0..3 {
                assert_eq!(tab_edges(position, 4, active).1, tab_edges(position + 1, 4, active).0);
            }
        }
        assert_eq!(tab_edges(1, 3, Some(1)), (true, false));
        // The outer edges of the bar always face inward.
        assert!(tab_edges(0, 3, None).0);
        assert!(!tab_edges(2, 3, None).1);
    }
}
