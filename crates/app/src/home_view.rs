//! The project hub: every project the user opened, the templates new ones start
//! from, and the dialogs to make both.

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use gpui::{
    Anchor, AnyElement, Context, Entity, EventEmitter, FocusHandle, Focusable, FontWeight, Keystroke, ObjectFit, PathPromptOptions,
    PromptLevel, RenderImage, ScrollEvent, SharedString, Subscription, Task, Window, anchored, canvas, deferred, div, img, prelude::*, px, svg,
};
use editor_core::{Language, word_count};
use pdf::{OutlineEntry, PageRequest, PdfEvent, PdfWorker};

use crate::outline::{self, OutlineItem};

use crate::{
    editor_view::EditorView,
    picker::fuzzy_score,
    projects::{self, Registry, Template, TemplateLanguage, TemplateSource},
    theme,
    toolchain::{self, Tool, Toolchain},
};

const TRAFFIC_LIGHT_SPACE: f32 = crate::window_controls::LEFT_SPACE;
const BAND_HEIGHT: f32 = 40.0;
const CONTENT_WIDTH: f32 = 1100.0;
const SIDEBAR_WIDTH: f32 = 220.0;
const CONTENT_PADDING: f32 = 32.0;
const ROW_HEIGHT: f32 = 40.0;
const FIELD_HEIGHT: f32 = 28.0;
const CARD_WIDTH: f32 = 232.0;
const CARD_HEIGHT: f32 = 300.0;
const CARD_GAP: f32 = 20.0;
/// How far the sheets behind a folder stick out to the right and below.
const SHEET_OFFSET: f32 = 10.0;
/// Resting this long on a folder opens its preview; passing over it does not.
const PREVIEW_DELAY: Duration = Duration::from_millis(400);
/// The pointer may take this long from the folder to its preview.
const PREVIEW_GRACE: Duration = Duration::from_millis(200);
const PREVIEW_PAGE_WIDTH: f32 = 300.0;
const PREVIEW_PAGE_HEIGHT: f32 = PREVIEW_PAGE_WIDTH * 1.414;
const PREVIEW_SHEET_WIDTH: f32 = 320.0;
/// The whole preview: padding, page, gap and sheet.
const PREVIEW_WIDTH: f32 = 2. * 20. + PREVIEW_PAGE_WIDTH + 24. + PREVIEW_SHEET_WIDTH;
/// Rendered pages kept on either side of the one shown, so that paging back is instant.
const PREVIEW_KEPT_PAGES: usize = 3;
/// How far ahead in the bookmarks a heading of the sources is looked for, as a
/// bookmark can be missing, such as for a starred section.
const BOOKMARK_LOOKAHEAD: usize = 8;
/// Scrolled pixels that turn one page.
const PAGE_TURN_SCROLL: f32 = 60.0;

pub enum HomeEvent {
    Open(PathBuf),
    /// Back to the open project.
    Close,
    /// The folder went to the trash, so a workspace showing it has to let go.
    Trashed(PathBuf),
    #[cfg(feature = "collab")]
    JoinSession,
    /// Projects show as cards, or as rows when false.
    CardsChanged(bool),
    StartTour,
    /// Add the `editor` command for the terminal.
    InstallCommand,
    RunSetup,
    ShowShortcuts,
    ReportProblem,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum HubPage {
    Projects,
    Templates,
    Tools,
    Help,
}

/// A closer look at a project beside its folder: pages to leaf through and what
/// tells versions of a document apart, all without opening the project.
struct Preview {
    project: PathBuf,
    /// False during the short wait, so that passing over folders shows nothing.
    shown: bool,
    over_card: bool,
    over_preview: bool,
    /// Opened with the keyboard, so it stays until the keyboard closes it.
    pinned: bool,
    sheet: Option<DataSheet>,
    /// Lives as long as the preview, so that turning a page renders only that page.
    worker: Option<PdfWorker>,
    page: usize,
    page_count: Option<usize>,
    pages: HashMap<usize, Arc<RenderImage>>,
    /// The contents, from the sources or else from the PDF's bookmarks.
    contents: Vec<OutlineItem>,
    /// The page of each entry of `contents`, where the PDF's bookmarks tell it.
    contents_pages: Vec<Option<usize>>,
    bookmarks: Vec<OutlineEntry>,
    /// Scroll left over from the last page turn, so that a trackpad turns one page at a time.
    scroll: f32,
    _task: Task<()>,
    _closing: Option<Task<()>>,
}

impl Preview {
    /// Finds the page of each heading in the PDF's bookmarks, which name the same
    /// headings in the same order. Without headings from the sources, the
    /// bookmarks are the contents.
    fn link_pages(&mut self) {
        let sources = self.sheet.as_ref().map_or(&[][..], |sheet| &sheet.contents);
        if sources.is_empty() {
            self.contents = self
                .bookmarks
                .iter()
                .map(|bookmark| OutlineItem {
                    level: bookmark.level,
                    title: bookmark.title.clone(),
                    line: 0,
                    depth: bookmark.level,
                    has_children: false,
                    collapsed: false,
                    number: None,
                    kind: editor_core::HeadingKind::Section,
                })
                .collect();
            self.contents_pages = self.bookmarks.iter().map(|bookmark| Some(bookmark.page)).collect();
            return;
        }
        self.contents = sources.to_vec();
        let marks: Vec<String> = self.bookmarks.iter().map(|bookmark| plain(&bookmark.title)).collect();
        let mut next = 0;
        self.contents_pages = sources
            .iter()
            .map(|item| {
                let title = plain(&item.title);
                let end = (next + BOOKMARK_LOOKAHEAD).min(marks.len());
                let found = (next..end).find(|&index| !title.is_empty() && (marks[index] == title || marks[index].ends_with(&title)))?;
                next = found + 1;
                Some(self.bookmarks[found].page)
            })
            .collect();
    }
}

/// Letters and digits only, in lower case, so that a bookmark matches its heading
/// whatever spacing, markup or number it has.
fn plain(title: &str) -> String {
    title.chars().filter(|ch| ch.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

/// What the preview tells about a project, read on a background thread.
#[derive(Clone, Default)]
struct DataSheet {
    engine: Option<&'static str>,
    words: Option<usize>,
    /// The outline of the whole document as the editor shows it, included files too.
    contents: Vec<OutlineItem>,
    /// The newest change to the main file or a file it includes.
    edited: Option<SystemTime>,
    /// The newest PDF and when it was built.
    pdf: Option<(PathBuf, SystemTime)>,
    git: Option<git::HeadSummary>,
}

#[derive(Clone, PartialEq, Eq)]
enum RenameTarget {
    Project(PathBuf),
    Template(PathBuf),
}

struct Rename {
    target: RenameTarget,
    field: Entity<EditorView>,
    _observer: Subscription,
}

enum HubDialog {
    NewProject {
        name: Entity<EditorView>,
        location: PathBuf,
        template: usize,
        error: Option<String>,
        busy: bool,
    },
    SaveTemplate {
        from: PathBuf,
        language: TemplateLanguage,
        name: Entity<EditorView>,
        description: Entity<EditorView>,
        error: Option<String>,
        busy: bool,
    },
}

pub struct HomeView {
    registry: Registry,
    registry_path: Option<PathBuf>,
    library: Option<PathBuf>,
    templates: Vec<Template>,
    /// Found once per project, as it reads the folder.
    languages: HashMap<PathBuf, TemplateLanguage>,
    page: HubPage,
    cards: bool,
    /// Cards per row at the last render, for moving up and down with the keys.
    columns: usize,
    /// The project whose "…" menu is open.
    card_menu: Option<PathBuf>,
    /// Projects whose folder is gone, found off the main thread on each visit.
    missing: HashSet<PathBuf>,
    folders_task: Option<Task<()>>,
    preview: Option<Preview>,
    /// Data sheets read during this visit of the hub.
    sheets: HashMap<PathBuf, DataSheet>,
    /// For rendering pages as sharp as the screen shows them.
    scale: f32,
    toolchain: Entity<Toolchain>,
    _toolchain_observer: Subscription,
    search: Entity<EditorView>,
    selected: usize,
    rename: Option<Rename>,
    dialog: Option<HubDialog>,
    /// Kept apart from the dialog, so that its fields keep their observers.
    dialog_observers: Vec<Subscription>,
    has_project: bool,
    vim_mode: bool,
    message: Option<String>,
    focus: FocusHandle,
    _search_observer: Subscription,
}

impl EventEmitter<HomeEvent> for HomeView {}

impl HomeView {
    pub fn new(toolchain: Entity<Toolchain>, cx: &mut Context<Self>) -> Self {
        let registry_path = projects::registry_path();
        let registry = registry_path.as_deref().map(Registry::load).unwrap_or_default();
        let library = projects::library_path();
        let templates = projects::load_templates(library.as_deref());
        let search = plain_field("", cx);
        let _search_observer = cx.observe(&search, |this: &mut Self, _, cx| {
            this.selected = 0;
            cx.notify();
        });
        let _toolchain_observer = cx.observe(&toolchain, |_, _, cx| cx.notify());
        let mut view = Self {
            registry,
            registry_path,
            library,
            templates,
            languages: HashMap::new(),
            page: HubPage::Projects,
            cards: true,
            columns: 1,
            card_menu: None,
            missing: HashSet::new(),
            folders_task: None,
            preview: None,
            sheets: HashMap::new(),
            scale: 2.,
            toolchain,
            _toolchain_observer,
            search,
            selected: 0,
            rename: None,
            dialog: None,
            dialog_observers: Vec::new(),
            has_project: false,
            vim_mode: false,
            message: None,
            focus: cx.focus_handle(),
            _search_observer,
        };
        view.check_folders(cx);
        view
    }

    /// Records an opened project, so that it comes first next time.
    pub fn touch(&mut self, path: &Path, cx: &mut Context<Self>) {
        self.registry.touch(path, projects::now());
        self.languages.insert(path.to_path_buf(), projects::detect_language(path));
        self.missing.remove(path);
        self.save_registry();
        cx.notify();
    }

    /// Makes the welcome example in the usual place, under a name that is free.
    pub fn create_welcome(&mut self, language: TemplateLanguage, cx: &mut Context<Self>) -> Result<PathBuf, String> {
        let location = self.registry.location();
        let template = projects::welcome_template(language);
        let (dir, title) = (1..100)
            .map(|number| if number == 1 { "Welcome".to_string() } else { format!("Welcome {number}") })
            .find_map(|title| projects::new_project_dir(&location, &title).ok().map(|dir| (dir, title)))
            .ok_or_else(|| "There is no free name for the welcome project.".to_string())?;
        projects::create_project(&template, &dir).map_err(|error| format!("Cannot create the welcome project: {error}"))?;
        self.registry.touch(&dir, projects::now());
        self.registry.rename(&dir, &title);
        self.save_registry();
        cx.notify();
        Ok(dir)
    }

    pub fn set_context(&mut self, has_project: bool, vim_mode: bool, cards: bool) {
        self.has_project = has_project;
        self.vim_mode = vim_mode;
        self.cards = cards;
    }

    /// The hub opens with the cursor in the search, so typing filters at once.
    pub fn focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.message = None;
        self.reload_templates();
        let focus = self.search.read(cx).focus_handle(cx);
        window.focus(&focus, cx);
        cx.notify();
    }

    /// Each visit starts on the projects with an empty search.
    pub fn show(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.page = HubPage::Projects;
        self.selected = 0;
        self.rename = None;
        self.dialog = None;
        self.card_menu = None;
        self.preview = None;
        self.sheets.clear();
        self.check_folders(cx);
        self.search.update(cx, |field, cx| {
            let end = field.state.buffer().len();
            field.edit_from_outside(0..end, "", cx);
        });
        self.focus(window, cx);
    }

    pub fn open_save_template(&mut self, from: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let name = plain_field(&from.file_name().map_or_else(String::new, |name| name.to_string_lossy().into_owned()), cx);
        let description = plain_field("", cx);
        self.dialog_observers = vec![cx.observe(&name, |_, _, cx| cx.notify()), cx.observe(&description, |_, _, cx| cx.notify())];
        window.focus(&name.read(cx).focus_handle(cx), cx);
        let language = projects::detect_language(&from);
        self.dialog = Some(HubDialog::SaveTemplate { from, language, name, description, error: None, busy: false });
        cx.notify();
    }

    /// Finds out which folders still exist and the language of each, on a
    /// background thread, as a project on a slow disk would stall the hub.
    fn check_folders(&mut self, cx: &mut Context<Self>) {
        let paths: Vec<(PathBuf, bool)> =
            self.registry.projects.iter().map(|entry| (entry.path.clone(), self.languages.contains_key(&entry.path))).collect();
        self.folders_task = Some(cx.spawn(async move |this, cx| {
            let found = cx
                .background_spawn(async move {
                    paths
                        .into_iter()
                        .map(|(path, known)| {
                            let exists = path.is_dir();
                            let language = (exists && !known).then(|| projects::detect_language(&path));
                            (path, exists, language)
                        })
                        .collect::<Vec<_>>()
                })
                .await;
            this.update(cx, |this, cx| {
                this.missing.clear();
                for (path, exists, language) in found {
                    if let Some(language) = language {
                        this.languages.insert(path.clone(), language);
                    }
                    if !exists {
                        this.missing.insert(path);
                    }
                }
                if this.preview.as_ref().is_some_and(|preview| this.missing.contains(&preview.project)) {
                    this.preview = None;
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn reload_templates(&mut self) {
        self.templates = projects::load_templates(self.library.as_deref());
    }

    fn save_registry(&mut self) {
        if let Some(path) = &self.registry_path
            && let Err(error) = self.registry.save(path)
        {
            self.message = Some(format!("Cannot save the project list: {error}"));
        }
    }

    fn query(&self, cx: &Context<Self>) -> String {
        field_text(&self.search, cx)
    }

    fn visible_projects(&self, cx: &Context<Self>) -> Vec<usize> {
        self.registry.filter(&self.query(cx))
    }

    fn visible_templates(&self, cx: &Context<Self>) -> Vec<usize> {
        let query = self.query(cx);
        if query.trim().is_empty() {
            return (0..self.templates.len()).collect();
        }
        let mut scored: Vec<(i64, usize)> = self
            .templates
            .iter()
            .enumerate()
            .filter_map(|(index, template)| {
                let text = format!("{} {}", template.name, template.language.label());
                fuzzy_score(&query, &text).map(|score| (score, index))
            })
            .collect();
        scored.sort_by(|left, right| right.0.cmp(&left.0).then(left.1.cmp(&right.1)));
        scored.into_iter().map(|(_, index)| index).collect()
    }

    fn row_count(&self, cx: &Context<Self>) -> usize {
        match self.page {
            HubPage::Projects => self.visible_projects(cx).len(),
            HubPage::Templates => self.visible_templates(cx).len(),
            HubPage::Tools | HubPage::Help => 0,
        }
    }

    /// Cards move by one sideways and by a row up and down.
    fn grid_shown(&self) -> bool {
        self.page == HubPage::Templates || (self.page == HubPage::Projects && self.cards)
    }

    /// Takes the keys of the hub. Returns false for keys that should go on to the
    /// focused field.
    pub fn handle_key(&mut self, keystroke: &Keystroke, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let modifiers = keystroke.modifiers;
        if modifiers.platform || modifiers.control || modifiers.alt {
            return false;
        }
        let key = keystroke.key.as_str();
        if self.dialog.is_some() {
            match key {
                "escape" => self.close_dialog(window, cx),
                "enter" => self.confirm_dialog(window, cx),
                "tab" => self.next_dialog_field(window, cx),
                _ => return false,
            }
            return true;
        }
        if self.rename.is_some() {
            match key {
                "escape" => self.cancel_rename(window, cx),
                "enter" => self.confirm_rename(window, cx),
                _ => return false,
            }
            return true;
        }
        let search_focused = self.search.read(cx).focus_handle(cx).is_focused(window);
        let (grid, row) = (self.grid_shown(), self.columns.max(1) as isize);
        let step = match key {
            "down" => if grid { row } else { 1 },
            "up" => if grid { -row } else { -1 },
            "right" if grid && !search_focused => 1,
            "left" if grid && !search_focused => -1,
            "j" if self.vim_mode && !search_focused => if grid { row } else { 1 },
            "k" if self.vim_mode && !search_focused => if grid { -row } else { -1 },
            "l" if self.vim_mode && grid && !search_focused => 1,
            "h" if self.vim_mode && grid && !search_focused => -1,
            "enter" => {
                self.activate_selected(window, cx);
                return true;
            }
            // A search does not start with a space, so the key is free while it is empty.
            "space" if self.page == HubPage::Projects && (!search_focused || self.query(cx).is_empty()) => {
                self.toggle_preview(cx);
                return true;
            }
            "pagedown" | "pageup" if self.preview.is_some() => {
                self.turn_page(key == "pagedown", cx);
                return true;
            }
            "escape" => {
                if self.preview.take().is_some() || self.card_menu.take().is_some() {
                    cx.notify();
                } else if self.has_project {
                    cx.emit(HomeEvent::Close);
                } else if search_focused && !self.query(cx).is_empty() {
                    self.search.update(cx, |field, cx| {
                        let end = field.state.buffer().len();
                        field.edit_from_outside(0..end, "", cx);
                    });
                }
                return true;
            }
            "tab" => {
                self.switch_page(if self.page == HubPage::Projects { HubPage::Templates } else { HubPage::Projects }, window, cx);
                return true;
            }
            _ => return false,
        };
        let count = self.row_count(cx);
        if count > 0 {
            self.selected = (self.selected as isize + step).clamp(0, count as isize - 1) as usize;
            // An open preview follows the selection, so that versions can be paged through.
            if self.preview.is_some()
                && self.page == HubPage::Projects
                && let Some(&index) = self.visible_projects(cx).get(self.selected)
            {
                self.show_preview(self.registry.projects[index].path.clone(), false, cx);
            }
            cx.notify();
        }
        true
    }

    fn switch_page(&mut self, page: HubPage, window: &mut Window, cx: &mut Context<Self>) {
        self.page = page;
        self.selected = 0;
        self.rename = None;
        self.card_menu = None;
        self.preview = None;
        self.focus(window, cx);
    }

    fn activate_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.page {
            HubPage::Tools | HubPage::Help => {}
            HubPage::Projects => {
                if let Some(&index) = self.visible_projects(cx).get(self.selected) {
                    let path = self.registry.projects[index].path.clone();
                    self.open_project(path, cx);
                }
            }
            HubPage::Templates => {
                if let Some(&index) = self.visible_templates(cx).get(self.selected) {
                    self.open_new_project(index, window, cx);
                }
            }
        }
    }

    fn open_project(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.preview = None;
        if path.is_dir() {
            cx.emit(HomeEvent::Open(path));
        } else {
            self.message = Some(format!("{} is gone. Remove it from the list, or move it back.", projects::tilde(&path)));
            cx.notify();
        }
    }

    fn prompt_open_folder(&mut self, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions { files: false, directories: true, multiple: false, prompt: Some("Open".into()) });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            if let Some(path) = paths.into_iter().next() {
                this.update(cx, |_, cx| cx.emit(HomeEvent::Open(path))).ok();
            }
        })
        .detach();
    }

    fn prompt_import_template(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Use as template".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            if let Some(path) = paths.into_iter().next() {
                this.update_in(cx, |this, window, cx| this.open_save_template(path, window, cx)).ok();
            }
        })
        .detach();
    }

    // Dialogs

    fn open_new_project(&mut self, template: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.reload_templates();
        let name = plain_field("", cx);
        self.dialog_observers = vec![cx.observe(&name, |_, _, cx| cx.notify())];
        window.focus(&name.read(cx).focus_handle(cx), cx);
        let template = template.min(self.templates.len().saturating_sub(1));
        self.dialog = Some(HubDialog::NewProject { name, location: self.registry.location(), template, error: None, busy: false });
        cx.notify();
    }

    /// The template named `name`, such as "Empty LaTeX", for the cards on top.
    fn template_named(&self, name: &str) -> usize {
        self.templates.iter().position(|template| template.name == name).unwrap_or(0)
    }

    fn close_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.dialog = None;
        self.dialog_observers.clear();
        self.focus(window, cx);
    }

    fn next_dialog_field(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(HubDialog::SaveTemplate { name, description, .. }) = &self.dialog {
            let next = if name.read(cx).focus_handle(cx).is_focused(window) { description } else { name };
            let focus = next.read(cx).focus_handle(cx);
            window.focus(&focus, cx);
        }
    }

    fn browse_location(&mut self, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions { files: false, directories: true, multiple: false, prompt: Some("Choose".into()) });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            this.update(cx, |this, cx| {
                if let Some(HubDialog::NewProject { location, error, .. }) = &mut this.dialog {
                    *location = path;
                    *error = None;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn confirm_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match &mut self.dialog {
            Some(HubDialog::NewProject { name, location, template, error, busy }) => {
                if *busy {
                    return;
                }
                let title = field_text(name, cx).trim().to_string();
                let dir = match projects::new_project_dir(location, &title) {
                    Ok(dir) => dir,
                    Err(message) => {
                        *error = Some(message);
                        cx.notify();
                        return;
                    }
                };
                let Some(template) = self.templates.get(*template).cloned() else {
                    return;
                };
                *busy = true;
                let location = location.clone();
                cx.spawn_in(window, async move |this, cx| {
                    let target = dir.clone();
                    let created = cx
                        .background_spawn(async move { projects::create_project(&template, &target) })
                        .await;
                    this.update_in(cx, |this, window, cx| match created {
                        Ok(()) => {
                            this.registry.default_location = Some(location);
                            this.registry.touch(&dir, projects::now());
                            this.registry.rename(&dir, &title);
                            this.save_registry();
                            this.close_dialog(window, cx);
                            cx.emit(HomeEvent::Open(dir));
                        }
                        Err(failure) => {
                            if let Some(HubDialog::NewProject { error, busy, .. }) = &mut this.dialog {
                                *error = Some(format!("Cannot create the project: {failure}"));
                                *busy = false;
                            }
                            cx.notify();
                        }
                    })
                    .ok();
                })
                .detach();
            }
            Some(HubDialog::SaveTemplate { from, name, description, error, busy, .. }) => {
                if *busy {
                    return;
                }
                let Some(library) = self.library.clone() else {
                    *error = Some("There is no home folder for the library.".to_string());
                    cx.notify();
                    return;
                };
                let (title, text) = (field_text(name, cx), field_text(description, cx));
                if title.trim().is_empty() {
                    *error = Some("The template needs a name.".to_string());
                    cx.notify();
                    return;
                }
                *busy = true;
                let from = from.clone();
                cx.spawn_in(window, async move |this, cx| {
                    let saved = cx.background_spawn(async move { projects::save_template(&from, &library, &title, &text) }).await;
                    this.update_in(cx, |this, window, cx| match saved {
                        Ok(_) => {
                            this.close_dialog(window, cx);
                            this.page = HubPage::Templates;
                            this.message = Some("Saved to your templates.".to_string());
                            cx.notify();
                        }
                        Err(failure) => {
                            if let Some(HubDialog::SaveTemplate { error, busy, .. }) = &mut this.dialog {
                                *error = Some(format!("Cannot save the template: {failure}"));
                                *busy = false;
                            }
                            cx.notify();
                        }
                    })
                    .ok();
                })
                .detach();
            }
            None => {}
        }
    }

    // Rename

    fn start_rename(&mut self, target: RenameTarget, current: &str, window: &mut Window, cx: &mut Context<Self>) {
        let field = plain_field(current, cx);
        let _observer = cx.observe(&field, |_, _, cx| cx.notify());
        window.focus(&field.read(cx).focus_handle(cx), cx);
        self.rename = Some(Rename { target, field, _observer });
        cx.notify();
    }

    fn cancel_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.rename = None;
        self.focus(window, cx);
    }

    fn confirm_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(rename) = self.rename.take() else {
            return;
        };
        let title = field_text(&rename.field, cx);
        match rename.target {
            RenameTarget::Project(path) => {
                self.registry.rename(&path, &title);
                self.save_registry();
            }
            RenameTarget::Template(dir) => {
                if let Err(error) = projects::rename_template(&dir, &title) {
                    self.message = Some(format!("Cannot rename the template: {error}"));
                }
            }
        }
        self.focus(window, cx);
    }

    // Removal

    fn remove_project(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let exists = path.is_dir();
        let title = self.registry.projects.iter().find(|entry| entry.path == path).map_or_else(String::new, |entry| entry.title.clone());
        cx.spawn_in(window, async move |this, cx| {
            let message = format!("Remove {title}?");
            let detail = if exists {
                if cfg!(windows) { format!("Removing from the list keeps the folder {}. The Recycle Bin keeps it until you empty it.", projects::tilde(&path)) } else { format!("Removing from the list keeps the folder {}. The trash keeps it until you empty the trash.", projects::tilde(&path)) }
            } else {
                "The folder is gone already, so only the entry remains.".to_string()
            };
            let answers: &[&str] = if exists { &["Remove from list", crate::platform::MOVE_TO_TRASH, "Cancel"] } else { &["Remove from list", "Cancel"] };
            let Ok(answer) = this.update_in(cx, |_, window, cx| window.prompt(PromptLevel::Warning, &message, Some(&detail), answers, cx)) else {
                return;
            };
            let trash = match answer.await {
                Ok(0) => false,
                Ok(1) if exists => true,
                _ => return,
            };
            if trash {
                let target = path.clone();
                let trashed = cx.background_spawn(async move { trash::delete(&target) }).await;
                if let Err(error) = trashed {
                    this.update(cx, |this, cx| {
                        this.message = Some(format!("Cannot move the folder to the {}: {error}", crate::platform::TRASH));
                        cx.notify();
                    })
                    .ok();
                    return;
                }
            }
            this.update(cx, |this, cx| {
                this.registry.remove(&path);
                this.languages.remove(&path);
                this.save_registry();
                this.selected = 0;
                if trash {
                    cx.emit(HomeEvent::Trashed(path));
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn delete_template(&mut self, dir: PathBuf, name: String, window: &mut Window, cx: &mut Context<Self>) {
        cx.spawn_in(window, async move |this, cx| {
            let message = format!("Move the template {name} to the {}?", crate::platform::TRASH);
            let detail = "Projects made from it keep their files.";
            let Ok(answer) = this.update_in(cx, |_, window, cx| {
                window.prompt(PromptLevel::Warning, &message, Some(detail), &[crate::platform::MOVE_TO_TRASH, "Cancel"], cx)
            }) else {
                return;
            };
            if answer.await != Ok(0) {
                return;
            }
            let trashed = cx.background_spawn(async move { trash::delete(&dir) }).await;
            this.update(cx, |this, cx| {
                if let Err(error) = trashed {
                    this.message = Some(format!("Cannot move the template to the trash: {error}"));
                }
                this.reload_templates();
                this.selected = 0;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    // Previews

    /// Opens the preview of `project`, after a short wait when the pointer brought
    /// it. The data sheet comes first, then the PDF opens and stays open for paging.
    fn show_preview(&mut self, project: PathBuf, wait: bool, cx: &mut Context<Self>) {
        if let Some(preview) = self.preview.as_mut().filter(|preview| preview.project == project) {
            preview.over_card |= wait;
            return;
        }
        let cached = self.sheets.get(&project).cloned();
        let task = cx.spawn({
            let project = project.clone();
            async move |this, cx| {
                if wait {
                    cx.background_executor().timer(PREVIEW_DELAY).await;
                }
                let shown = this.update(cx, |this, cx| {
                    if let Some(preview) = this.preview.as_mut() {
                        preview.shown = true;
                    }
                    cx.notify();
                });
                if shown.is_err() {
                    return;
                }
                let sheet = match cached {
                    Some(sheet) => sheet,
                    None => {
                        let project = project.clone();
                        cx.background_spawn(async move { data_sheet(&project) }).await
                    }
                };
                let pdf = sheet.pdf.clone().map(|(pdf, _)| pdf);
                let updated = this.update(cx, |this, cx| {
                    this.sheets.insert(project.clone(), sheet.clone());
                    if let Some(preview) = this.preview.as_mut() {
                        preview.sheet = Some(sheet);
                        preview.link_pages();
                    }
                    cx.notify();
                });
                let (Ok(()), Some(pdf)) = (updated, pdf) else {
                    return;
                };
                let Ok((worker, mut events)) = cx.background_spawn(async move { PdfWorker::spawn(&crate::pdf_view::library_directory()) }).await
                else {
                    return;
                };
                worker.open(0, pdf);
                worker.outline(0);
                let started = this.update(cx, |this, _| {
                    if let Some(preview) = this.preview.as_mut() {
                        preview.worker = Some(worker);
                    }
                    this.request_pages();
                });
                if started.is_err() {
                    return;
                }
                while let Some(event) = events.recv().await {
                    let result = this.update(cx, |this, cx| {
                        let Some(preview) = this.preview.as_mut() else {
                            return;
                        };
                        match event {
                            PdfEvent::Loaded { pages, .. } => preview.page_count = Some(pages.len()),
                            PdfEvent::Rendered { request, height_px, bgra, .. } => {
                                if let Some(buffer) = image::RgbaImage::from_raw(request.width_px, height_px, bgra) {
                                    preview.pages.insert(request.page, Arc::new(RenderImage::new([image::Frame::new(buffer)])));
                                }
                            }
                            PdfEvent::Outline { entries, .. } => {
                                preview.bookmarks = entries;
                                preview.link_pages();
                            }
                            PdfEvent::PageContent { .. } | PdfEvent::Failed { .. } => {}
                        }
                        cx.notify();
                    });
                    if result.is_err() {
                        return;
                    }
                }
            }
        });
        self.preview = Some(Preview {
            project,
            shown: false,
            over_card: wait,
            over_preview: false,
            pinned: !wait,
            sheet: None,
            worker: None,
            page: 0,
            page_count: None,
            pages: HashMap::new(),
            contents: Vec::new(),
            contents_pages: Vec::new(),
            bookmarks: Vec::new(),
            scroll: 0.,
            _task: task,
            _closing: None,
        });
    }

    /// Renders the page shown and the next one, unless they are there already, and
    /// lets go of pages far from it.
    fn request_pages(&mut self) {
        let width = (PREVIEW_PAGE_WIDTH * self.scale).round() as u32;
        let Some(preview) = self.preview.as_mut() else {
            return;
        };
        let current = preview.page;
        preview.pages.retain(|page, _| page.abs_diff(current) <= PREVIEW_KEPT_PAGES);
        let last = preview.page_count.map_or(current + 1, |count| count.saturating_sub(1));
        let wanted: Vec<PageRequest> = [current, current + 1]
            .into_iter()
            .filter(|page| *page <= last && !preview.pages.contains_key(page))
            .map(|page| PageRequest { page, width_px: width, recolor: None })
            .collect();
        if let (Some(worker), false) = (&preview.worker, wanted.is_empty()) {
            worker.render(0, wanted);
        }
    }

    fn go_to_page(&mut self, page: usize, cx: &mut Context<Self>) {
        let Some(preview) = self.preview.as_mut() else {
            return;
        };
        let last = preview.page_count.unwrap_or(1).saturating_sub(1);
        let page = page.min(last);
        if page != preview.page {
            preview.page = page;
            self.request_pages();
            cx.notify();
        }
    }

    fn turn_page(&mut self, forward: bool, cx: &mut Context<Self>) {
        if let Some(page) = self.preview.as_ref().map(|preview| preview.page) {
            self.go_to_page(if forward { page + 1 } else { page.saturating_sub(1) }, cx);
        }
    }

    /// The pointer left the folder or the preview. The preview stays a moment, so
    /// that the pointer can cross the gap between the two.
    fn leave_preview(&mut self, cx: &mut Context<Self>) {
        let Some(preview) = self.preview.as_mut() else {
            return;
        };
        if preview.over_card || preview.over_preview || preview.pinned {
            return;
        }
        preview._closing = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(PREVIEW_GRACE).await;
            this.update(cx, |this, cx| {
                if this.preview.as_ref().is_some_and(|preview| !preview.over_card && !preview.over_preview) {
                    this.preview = None;
                    cx.notify();
                }
            })
            .ok();
        }));
    }

    fn hover_card(&mut self, project: &Path, hovered: bool, cx: &mut Context<Self>) {
        if hovered {
            // A folder that is gone has nothing to show.
            if self.card_menu.is_none() && self.rename.is_none() && !self.missing.contains(project) {
                self.show_preview(project.to_path_buf(), true, cx);
            }
        } else if let Some(preview) = self.preview.as_mut().filter(|preview| preview.project == project) {
            preview.over_card = false;
            self.leave_preview(cx);
        }
    }

    /// Space shows the selected project as a resting pointer would, and hides it again.
    fn toggle_preview(&mut self, cx: &mut Context<Self>) {
        let Some(&index) = self.visible_projects(cx).get(self.selected) else {
            return;
        };
        let project = self.registry.projects[index].path.clone();
        match self.preview.as_mut() {
            // Open for the selected project, maybe by the pointer: Space closes it.
            Some(preview) if preview.project == project && preview.shown => self.preview = None,
            // The pointer's preview, or one still waiting: the keyboard takes it over.
            Some(preview) if preview.project == project => {
                preview.pinned = true;
                preview.shown = true;
            }
            _ => {
                self.preview = None;
                self.show_preview(project, false, cx);
            }
        }
        cx.notify();
    }

    // Rendering

    fn render_band(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::window_controls::drag_area(div().id("hub-band"))
            .flex()
            .flex_none()
            .items_center()
            .justify_end()
            .gap(px(8.))
            .h(px(BAND_HEIGHT))
            .pl(px(TRAFFIC_LIGHT_SPACE))
            .pr(px(if cfg!(windows) { 0. } else { 10. }))
            .bg(theme::color(theme::palette().panel))
            .when(self.has_project, |element| {
                element.child(crate::window_controls::clickable(div().child(button("hub-back", "Back to project", false, cx, |_, _, cx| cx.emit(HomeEvent::Close)))))
            })
            .children(crate::window_controls::buttons(window, BAND_HEIGHT))
    }

    fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = theme::palette();
        let item = |id: &'static str, icon: &'static str, text: &'static str, target: HubPage, cx: &mut Context<Self>| {
            let active = self.page == target;
            div()
                .id(id)
                .flex()
                .items_center()
                .gap(px(10.))
                .h(px(34.))
                .px(px(10.))
                .rounded(px(theme::RADIUS_MD))
                .cursor_pointer()
                .when(active, |style| style.bg(theme::color(palette.accent_soft)).font_weight(FontWeight::SEMIBOLD))
                .when(!active, |style| style.text_color(theme::color(palette.ui_muted)).hover(|style| style.bg(theme::color(palette.divider)).text_color(theme::color(palette.ui_text))))
                .on_click(cx.listener(move |this, _, window, cx| this.switch_page(target, window, cx)))
                .child(svg().path(format!("icons/{icon}.svg")).size(px(16.)).flex_none().text_color(theme::color(if active { palette.accent } else { palette.ui_muted })))
                .child(text)
        };
        #[cfg(feature = "collab")]
        let join = Some(
            div()
                .id("hub-join")
                .flex()
                .items_center()
                .gap(px(10.))
                .h(px(34.))
                .px(px(10.))
                .rounded(px(theme::RADIUS_MD))
                .cursor_pointer()
                .text_color(theme::color(palette.ui_muted))
                .hover(|style| style.bg(theme::color(palette.divider)).text_color(theme::color(palette.ui_text)))
                .on_click(cx.listener(|_, _, _, cx| cx.emit(HomeEvent::JoinSession)))
                .child(svg().path("icons/people.svg").size(px(16.)).flex_none().text_color(theme::color(palette.ui_muted)))
                .child("Join live session…"),
        );
        #[cfg(not(feature = "collab"))]
        let join: Option<gpui::Stateful<gpui::Div>> = None;
        div()
            .flex()
            .flex_col()
            .flex_none()
            .w(px(SIDEBAR_WIDTH))
            .h_full()
            .px(px(12.))
            .pb(px(12.))
            .gap(px(2.))
            .bg(theme::color(palette.panel))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .px(px(10.))
                    .pt(px(8.))
                    .pb(px(20.))
                    .child(img(crate::assets::LOGO).size(px(32.)).flex_none())
                    .child(div().text_size(px(16.)).font_weight(FontWeight::SEMIBOLD).child("Editor")),
            )
            .child(item("hub-nav-projects", "folder", "Projects", HubPage::Projects, cx))
            .child(item("hub-nav-templates", "book", "Templates", HubPage::Templates, cx))
            .child(item("hub-nav-tools", "wrench", "Writing tools", HubPage::Tools, cx))
            .child(item("hub-nav-help", "help", "Help", HubPage::Help, cx))
            .child(div().flex_1())
            .children(join)
    }

    fn render_search(&self) -> impl IntoElement {
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(8.))
            .w(px(260.))
            .h(px(32.))
            .px(px(10.))
            .rounded(px(theme::RADIUS_MD))
            .bg(theme::color(theme::palette().panel))
            .border_1()
            .border_color(theme::color(theme::palette().divider))
            .child(div().flex_none().text_color(theme::color(theme::palette().ui_muted)).child("Search"))
            .child(div().flex_1().min_w_0().h(px(FIELD_HEIGHT)).overflow_hidden().child(self.search.clone()))
    }

    fn render_header(&self, title: &'static str, controls: Vec<AnyElement>) -> impl IntoElement {
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(10.))
            .child(div().flex_1().text_size(px(theme::TITLE_FONT_SIZE)).font_weight(FontWeight::SEMIBOLD).child(title))
            .children(controls)
    }

    fn render_view_switch(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = theme::palette();
        let option = |id: &'static str, icon: &'static str, cards: bool, cx: &mut Context<Self>| {
            let active = self.cards == cards;
            div()
                .id(id)
                .flex()
                .items_center()
                .justify_center()
                .size(px(28.))
                .rounded(px(theme::RADIUS_SM))
                .cursor_pointer()
                .when(active, |style| style.bg(theme::color(palette.accent_soft)))
                .when(!active, |style| style.hover(|style| style.bg(theme::color(palette.divider))))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.cards = cards;
                    this.selected = 0;
                    cx.emit(HomeEvent::CardsChanged(cards));
                    cx.notify();
                }))
                .child(svg().path(format!("icons/{icon}.svg")).size(px(16.)).text_color(theme::color(if active { palette.ui_text } else { palette.ui_muted })))
        };
        div()
            .flex()
            .flex_none()
            .p(px(2.))
            .gap(px(2.))
            .rounded(px(theme::RADIUS_MD))
            .border_1()
            .border_color(theme::color(palette.divider))
            .child(option("hub-view-cards", "grid", true, cx))
            .child(option("hub-view-list", "list", false, cx))
    }

    /// A large choice with an icon, a title and one line on what it does.
    fn action_card(
        &self,
        id: &'static str,
        icon: &'static str,
        title: &'static str,
        text: &'static str,
        cx: &mut Context<Self>,
        handler: impl Fn(&mut HomeView, &mut Window, &mut Context<HomeView>) + 'static,
    ) -> impl IntoElement {
        let palette = theme::palette();
        div()
            .id(id)
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .gap(px(10.))
            .p(px(16.))
            .rounded(px(theme::RADIUS_LG))
            .cursor_pointer()
            .bg(theme::color(palette.panel))
            .border_1()
            .border_color(theme::color(palette.divider))
            .hover(|style| style.border_color(theme::color(palette.accent)))
            .on_click(cx.listener(move |this, _, window, cx| handler(this, window, cx)))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(36.))
                    .rounded(px(theme::RADIUS_MD))
                    .bg(theme::color(palette.accent_soft))
                    .child(svg().path(format!("icons/{icon}.svg")).size(px(20.)).text_color(theme::color(palette.ui_text))),
            )
            .child(div().font_weight(FontWeight::SEMIBOLD).child(title))
            .child(div().text_color(theme::color(palette.ui_muted)).child(text))
    }

    fn render_start(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let (latex, typst) = (self.template_named("Empty LaTeX"), self.template_named("Empty Typst"));
        div()
            .flex()
            .flex_col()
            .flex_none()
            .gap(px(10.))
            .child(section_title("Start something new"))
            .child(
                div()
                    .flex()
                    .gap(px(12.))
                    .child(self.action_card("hub-start-latex", "file-tex", "Blank LaTeX", "An empty article, like a new Overleaf project.", cx, move |this, window, cx| {
                        this.open_new_project(latex, window, cx)
                    }))
                    .child(self.action_card("hub-start-typst", "file-typ", "Blank Typst", "A short document in the newer Typst language.", cx, move |this, window, cx| {
                        this.open_new_project(typst, window, cx)
                    }))
                    .child(self.action_card("hub-start-open", "folder", "Open folder", if cfg!(windows) { "A project you already have on this PC." } else { "A project you already have on this Mac." }, cx, |this, _, cx| {
                        this.prompt_open_folder(cx)
                    })),
            )
    }

    fn render_projects_page(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let search = self.render_search().into_any_element();
        let switch = self.render_view_switch(cx).into_any_element();
        let mut elements = vec![
            self.render_header("Projects", vec![search, switch]).into_any_element(),
            self.render_start(cx).into_any_element(),
        ];
        if let Some(message) = self.message.clone() {
            elements.push(div().text_color(theme::color(theme::palette().ui_muted)).child(message).into_any_element());
        }
        let visible = self.visible_projects(cx);
        if self.registry.projects.is_empty() {
            elements.push(empty_state("folder", "Your projects will appear here", "Start with one of the choices above, or open a folder you already have.").into_any_element());
            return elements;
        }
        elements.push(section_title(format!("Recent · {}", visible.len())).into_any_element());
        if visible.is_empty() {
            elements.push(empty_note("No project matches the search.").into_any_element());
        } else if self.cards {
            elements.push(self.render_project_cards(&visible, cx));
        } else {
            elements.push(self.render_project_rows(&visible, cx));
        }
        elements
    }

    /// Each project as a closed folder with a paper clip and the sheets of the
    /// build behind it. Resting on one opens its preview.
    fn render_project_cards(&self, visible: &[usize], cx: &mut Context<Self>) -> AnyElement {
        let palette = theme::palette();
        let paper = theme::look().paper;
        let sheet_color = if theme::is_light(palette.bg) { 0xfbfaf6 } else { palette.divider };
        let now = projects::now();
        let cards = visible.iter().enumerate().map(|(position, &index)| {
            let entry = &self.registry.projects[index];
            let path = entry.path.clone();
            let exists = !self.missing.contains(&path);
            let selected = position == self.selected;
            let language = self.languages.get(&path).copied().filter(|_| exists);
            let group: SharedString = format!("hub-folder-{index}").into();
            let renaming = self.rename.as_ref().filter(|rename| rename.target == RenameTarget::Project(path.clone()));
            let title: AnyElement = match renaming {
                Some(rename) => field_box(&rename.field).into_any_element(),
                None => div()
                    .text_size(px(15.))
                    .line_height(px(19.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .line_clamp(3)
                    .child(entry.title.to_uppercase())
                    .into_any_element(),
            };
            let icon = match language {
                Some(TemplateLanguage::Typst) => "file-typ",
                Some(TemplateLanguage::Latex) => "file-tex",
                _ => "folder",
            };
            let kind = language.map_or("Project", |language| language.label()).to_uppercase();
            let footer = if exists { format!("Opened {}", projects::ago(now, entry.last_opened)) } else { "Folder not found".to_string() };
            let menu_open = self.card_menu.as_ref() == Some(&path);
            let (open_path, menu_path, hover_path) = (path.clone(), path.clone(), path.clone());
            let menu = menu_open.then(|| self.render_card_menu(index, &path, &entry.title, exists, cx));
            let preview = self
                .preview
                .as_ref()
                .filter(|preview| preview.shown && preview.project == path)
                .map(|preview| self.render_preview(preview, entry, position % self.columns.max(1), cx));
            let sheet = |inset: f32| {
                div()
                    .absolute()
                    .top(px(inset))
                    .left(px(inset))
                    .right(px(SHEET_OFFSET - inset))
                    .bottom(px(SHEET_OFFSET - inset))
                    .rounded(px(theme::RADIUS_SM))
                    .bg(theme::color(sheet_color))
                    .border_1()
                    .border_color(theme::color(palette.divider))
            };
            let folder = div()
                .absolute()
                .top_0()
                .left_0()
                .right(px(SHEET_OFFSET))
                .bottom(px(SHEET_OFFSET))
                .flex()
                .flex_col()
                .pt(px(64.))
                .pb(px(16.))
                .pl(px(22.))
                .pr(px(18.))
                .overflow_hidden()
                .rounded(px(theme::RADIUS_MD))
                .bg(theme::color(palette.panel))
                .border_color(theme::color(if selected { palette.accent } else { palette.divider }))
                .when(selected, |style| style.border_2())
                .when(!selected, |style| style.border_1().group_hover(group.clone(), |style| style.border_color(theme::color(palette.ui_muted))))
                .shadow_sm()
                .group_hover(group.clone(), |style| style.top(px(-2.)).shadow_md())
                // The fold of the folder along its spine.
                .child(div().absolute().top_0().bottom_0().left(px(7.)).w(px(1.)).bg(theme::color(palette.divider)))
                .child(svg().path(format!("icons/{icon}.svg")).size(px(16.)).mb(px(10.)).text_color(theme::color(palette.ui_text)))
                .child(title)
                .child(
                    div()
                        .mt(px(4.))
                        .truncate()
                        .font_family(theme::MONO_FONT)
                        .text_size(px(11.))
                        .text_color(theme::color(palette.ui_muted))
                        .child(projects::tilde(&path)),
                )
                .child(div().flex_1())
                .child(
                    div()
                        .font_family(theme::MONO_FONT)
                        .text_size(px(10.))
                        .font_weight(FontWeight::BOLD)
                        .text_color(theme::color(if exists { palette.ui_muted } else { palette.error }))
                        .child(format!("{kind} - PROJECT FILE"))
                        .child(div().font_weight(FontWeight::NORMAL).child(footer.to_uppercase())),
                )
                .when(paper, |element| {
                    element.child(
                        canvas(|_, _, _| {}, |bounds, _, window, _| crate::paper::paint(bounds, window)).absolute().top_0().left_0().size_full(),
                    )
                });
            div()
                .id(("hub-card", index))
                .group(group.clone())
                .relative()
                .flex_none()
                .w(px(CARD_WIDTH))
                .h(px(CARD_HEIGHT))
                .cursor_pointer()
                .when(!exists, |style| style.opacity(0.4))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if this.rename.is_none() {
                        this.open_project(open_path.clone(), cx);
                    }
                }))
                .on_hover(cx.listener(move |this, hovered: &bool, _, cx| this.hover_card(&hover_path, *hovered, cx)))
                .child(sheet(SHEET_OFFSET))
                .child(sheet(SHEET_OFFSET / 2.))
                .child(folder)
                .child(
                    svg()
                        .path("icons/paperclip.svg")
                        .absolute()
                        .top(px(-10.))
                        .left(px(28.))
                        .w(px(14.))
                        .h(px(42.))
                        .text_color(theme::color(palette.ui_muted)),
                )
                .child(
                    svg()
                        .path("icons/binder-clip.svg")
                        .absolute()
                        .top(px(118.))
                        .left(px(CARD_WIDTH - SHEET_OFFSET - 33.))
                        .w(px(44.))
                        .h(px(32.))
                        .text_color(theme::color(palette.ui_muted)),
                )
                .child(
                    div()
                        .id(("hub-card-menu", index))
                        .absolute()
                        .top(px(12.))
                        .right(px(SHEET_OFFSET + 8.))
                        .p(px(2.))
                        .rounded(px(theme::RADIUS_SM))
                        .hover(|style| style.bg(theme::color(palette.divider)))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.preview = None;
                            this.card_menu = if this.card_menu.as_ref() == Some(&menu_path) { None } else { Some(menu_path.clone()) };
                            cx.notify();
                        }))
                        .child(svg().path("icons/dots.svg").size(px(16.)).text_color(theme::color(palette.ui_muted))),
                )
                .children(menu)
                .children(preview)
        });
        div().flex().flex_wrap().gap(px(CARD_GAP)).pt(px(6.)).children(cards).into_any_element()
    }

    /// The preview beside a folder: the pages to leaf through, the facts that tell
    /// one version from another and the contents to jump to. It opens on the side
    /// with room, the right for the left half of a row.
    fn render_preview(&self, preview: &Preview, entry: &projects::ProjectEntry, column: usize, cx: &mut Context<Self>) -> AnyElement {
        let palette = theme::palette();
        let now = projects::now();
        // The side with more room, so that the preview covers as few folders as it can.
        let step = CARD_WIDTH + CARD_GAP;
        let right_room = (self.columns.max(1) - 1 - column.min(self.columns.max(1) - 1)) as f32 * step;
        let left_room = column as f32 * step + SIDEBAR_WIDTH + CONTENT_PADDING;
        let left = right_room < PREVIEW_WIDTH && left_room > right_room;
        let language = self.languages.get(&entry.path).copied().filter(|language| *language != TemplateLanguage::Other);
        let has_pdf = preview.sheet.as_ref().is_some_and(|sheet| sheet.pdf.is_some());
        let page: AnyElement = match preview.pages.get(&preview.page) {
            Some(image) => img(image.clone()).size_full().object_fit(ObjectFit::Contain).into_any_element(),
            None => div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .font_family(theme::MONO_FONT)
                .text_color(theme::color(palette.ui_muted))
                .child(match &preview.sheet {
                    Some(sheet) if sheet.pdf.is_none() => "NOT BUILT YET",
                    _ => "",
                })
                .into_any_element(),
        };
        let count = preview.page_count;
        let pager = div()
            .flex()
            .items_center()
            .justify_center()
            .gap(px(14.))
            .h(px(28.))
            .font_family(theme::MONO_FONT)
            .text_color(theme::color(palette.ui_muted))
            .when(has_pdf, |element| {
                element
                    .child(page_button("hub-preview-back", "chevron-left", preview.page > 0, cx, |this, cx| this.turn_page(false, cx)))
                    .child(div().min_w(px(80.)).flex().justify_center().child(match count {
                        Some(count) => format!("{} / {count}", preview.page + 1),
                        None => String::new(),
                    }))
                    .child(page_button(
                        "hub-preview-forward",
                        "chevron-right",
                        count.is_some_and(|count| preview.page + 1 < count),
                        cx,
                        |this, cx| this.turn_page(true, cx),
                    ))
            });
        let pages = div()
            .flex()
            .flex_col()
            .flex_none()
            .gap(px(8.))
            .child(
                div()
                    .id("hub-preview-page")
                    .w(px(PREVIEW_PAGE_WIDTH))
                    .h(px(PREVIEW_PAGE_HEIGHT))
                    .bg(theme::color(if has_pdf { 0xffffff } else { palette.bg }))
                    .border_1()
                    .border_color(theme::color(palette.divider))
                    .when(!has_pdf, |style| style.border_dashed())
                    .on_scroll(cx.listener(|this, event: &ScrollEvent, _, cx| {
                        let delta = event.delta.pixel_delta(px(20.)).y.to_f64() as f32;
                        let Some(preview) = this.preview.as_mut() else {
                            return;
                        };
                        preview.scroll += delta;
                        if preview.scroll.abs() >= PAGE_TURN_SCROLL {
                            let forward = preview.scroll < 0.;
                            preview.scroll = 0.;
                            this.turn_page(forward, cx);
                        }
                    }))
                    .child(page),
            )
            .child(pager);
        let line = |key: &'static str, value: String| {
            div()
                .flex()
                .gap(px(10.))
                .child(div().flex_none().w(px(64.)).text_color(theme::color(palette.ui_muted)).child(key))
                .child(div().flex_1().min_w_0().child(value))
        };
        let mut facts = div().flex().flex_col().gap(px(3.)).font_family(theme::MONO_FONT).text_size(px(12.)).line_height(px(17.));
        if let Some(sheet) = &preview.sheet {
            let kind = match (language, sheet.engine) {
                (Some(language), Some(engine)) if language.label() != engine => format!("{} · {engine}", language.label()),
                (Some(language), _) => language.label().to_string(),
                (None, Some(engine)) => engine.to_string(),
                (None, None) => String::new(),
            };
            let seconds = |time: &SystemTime| time.duration_since(UNIX_EPOCH).map_or(0, |elapsed| elapsed.as_secs());
            facts = facts
                .when(!kind.is_empty(), |element| element.child(line("TYPE", kind)))
                .children(count.map(|count| line("PAGES", count.to_string())))
                .children(sheet.words.map(|words| line("WORDS", group_digits(words))))
                .children(sheet.edited.as_ref().map(|edited| line("EDITED", projects::ago(now, seconds(edited)))));
            if let Some((_, built)) = &sheet.pdf {
                let behind = sheet.edited.is_some_and(|edited| edited > *built);
                facts = facts.child(line("BUILT", format!("{}{}", projects::ago(now, seconds(built)), if behind { ", before the last edit" } else { "" })));
            }
            if let Some(head) = &sheet.git {
                let version = match &head.branch {
                    Some(branch) => format!("{branch} · {}", head.subject),
                    None => head.subject.clone(),
                };
                facts = facts.child(line("VERSION", version));
            }
        }
        let contents = (!preview.contents.is_empty()).then(|| self.render_contents(preview, cx));
        let card = div()
            .id("hub-preview")
            .flex()
            .gap(px(24.))
            .p(px(20.))
            .rounded(px(theme::RADIUS_LG))
            .bg(theme::color(palette.panel))
            .border_1()
            .border_color(theme::color(palette.divider))
            .shadow_lg()
            .occlude()
            .cursor_default()
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if let Some(preview) = this.preview.as_mut() {
                    preview.over_preview = *hovered;
                }
                if !*hovered {
                    this.leave_preview(cx);
                }
            }))
            // A click on the preview must not reach the folder below.
            .on_click(|_, _, cx| cx.stop_propagation())
            .child(pages)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(16.))
                    .w(px(PREVIEW_SHEET_WIDTH))
                    .h(px(PREVIEW_PAGE_HEIGHT + 36.))
                    .text_color(theme::color(palette.ui_text))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .child(div().text_size(px(17.)).line_height(px(22.)).font_weight(FontWeight::SEMIBOLD).line_clamp(2).child(entry.title.clone()))
                            .child(
                                div()
                                    .truncate()
                                    .font_family(theme::MONO_FONT)
                                    .text_size(px(11.))
                                    .text_color(theme::color(palette.ui_muted))
                                    .child(projects::tilde(&entry.path)),
                            ),
                    )
                    .child(facts)
                    .map(|column| match contents {
                        Some(contents) => column.child(contents),
                        None => column.child(div().flex_1()),
                    })
                    .child(button("hub-preview-open", "Open project", true, cx, {
                        let path = entry.path.clone();
                        move |this, _, cx| this.open_project(path.clone(), cx)
                    })),
            );
        // Padding and not margin keeps the gap, as the window's edge counts it.
        let placed = div().when(left, |style| style.pr(px(12.))).when(!left, |style| style.pl(px(CARD_WIDTH + 12.))).child(card);
        deferred(anchored().anchor(if left { Anchor::TopRight } else { Anchor::TopLeft }).snap_to_window_with_margin(px(8.)).child(placed))
            .with_priority(1)
            .into_any_element()
    }

    /// The contents of the previewed document, drawn like the outline beside the
    /// editor: chapters as numbered pills on a line, sections as dots. An entry
    /// whose page the PDF tells turns the preview to it.
    fn render_contents(&self, preview: &Preview, cx: &mut Context<Self>) -> AnyElement {
        let palette = theme::palette();
        let pages = &preview.contents_pages;
        // The entry of the page shown is the last one that starts on it or before.
        let here = pages.iter().rposition(|page| page.is_some_and(|page| page <= preview.page));
        let chapter = here.and_then(|here| preview.contents[..=here].iter().rposition(|item| item.depth == 0));
        let rows = preview.contents.iter().enumerate().map(|(index, item)| {
            let page = pages.get(index).copied().flatten();
            let label = outline::marker_element(item, theme::color(palette.ui_muted));
            let row = div()
                .id(("hub-preview-chapter", index))
                .relative()
                .w_full()
                .flex()
                .when_some(page, |row, page| {
                    row.cursor_pointer().hover(|style| style.bg(theme::color(palette.divider).opacity(0.35))).on_click(cx.listener(
                        move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.go_to_page(page, cx);
                        },
                    ))
                });
            let page_number = div()
                .flex_none()
                .pl(px(8.))
                .font_family(theme::MONO_FONT)
                .text_size(px(11.))
                .text_color(theme::color(palette.ui_muted))
                .children(page.map(|page| (page + 1).to_string()));
            if item.depth == 0 {
                let ink = theme::color(palette.accent).opacity(if chapter == Some(index) { 1. } else { 0.6 });
                return row
                    .h(px(40.))
                    .items_center()
                    .pl(px(outline::SPINE - outline::PILL / 2.))
                    .child(div().absolute().left_0().right_0().top(px(20.)).h(px(1.)).bg(theme::color(palette.divider)))
                    .child(
                        div()
                            .flex()
                            .flex_none()
                            .items_center()
                            .justify_center()
                            .w(px(outline::PILL))
                            .h(px(20.))
                            .rounded_full()
                            .bg(ink)
                            .text_size(px(11.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme::color(palette.on_accent))
                            .child(outline::marker_element(item, theme::color(palette.on_accent))),
                    )
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
                    .child(div().flex_1())
                    .child(page_number.relative().bg(theme::color(palette.panel)))
                    .into_any_element();
            }
            let dot_top = 5. + outline::LINE / 2.;
            let dot = if here == Some(index) {
                div()
                    .absolute()
                    .left(px(outline::SPINE - 4.5))
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
                    .left(px(outline::SPINE - 2.5))
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
                .py(px(5.))
                .pl(px(outline::SPINE + 12.))
                .line_height(px(outline::LINE))
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
                .child(page_number)
                .into_any_element()
        });
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .gap(px(6.))
            .child(
                div()
                    .font_family(theme::MONO_FONT)
                    .text_size(px(10.))
                    .text_color(theme::color(palette.ui_muted))
                    .child(format!("CONTENTS · {}", preview.contents.len())),
            )
            .child(
                div().id("hub-preview-contents").flex_1().min_h_0().overflow_y_scroll().child(
                    div()
                        .relative()
                        .w_full()
                        .child(div().absolute().top_0().bottom_0().left(px(outline::SPINE - 0.5)).w(px(1.)).bg(theme::color(palette.divider)))
                        .children(rows),
                ),
            )
            .into_any_element()
    }

    fn render_card_menu(&self, index: usize, path: &Path, title: &str, exists: bool, cx: &mut Context<Self>) -> AnyElement {
        let palette = theme::palette();
        let (rename_path, template_path, reveal_path, remove_path) = (path.to_path_buf(), path.to_path_buf(), path.to_path_buf(), path.to_path_buf());
        let current_title = title.to_string();
        div()
            .id(("hub-card-menu-list", index))
            .absolute()
            .right(px(8.))
            .bottom(px(44.))
            .flex()
            .flex_col()
            .min_w(px(180.))
            .p(px(4.))
            .rounded(px(theme::RADIUS_MD))
            .bg(theme::color(palette.panel))
            .border_1()
            .border_color(theme::color(palette.divider))
            .shadow_lg()
            .occlude()
            .when(exists, |element| {
                element
                    .child(menu_item(("hub-card-rename", index), "Rename", cx, move |this, window, cx| {
                        this.card_menu = None;
                        this.start_rename(RenameTarget::Project(rename_path.clone()), &current_title, window, cx);
                    }))
                    .child(menu_item(("hub-card-template", index), "Save as template…", cx, move |this, window, cx| {
                        this.card_menu = None;
                        this.open_save_template(template_path.clone(), window, cx);
                    }))
                    .child(menu_item(("hub-card-reveal", index), if cfg!(windows) { "Show in Explorer" } else { "Show in Finder" }, cx, move |this, _, cx| {
                        this.card_menu = None;
                        cx.reveal_path(&reveal_path);
                    }))
            })
            .child(menu_item(("hub-card-remove", index), "Remove…", cx, move |this, window, cx| {
                this.card_menu = None;
                this.remove_project(remove_path.clone(), window, cx);
            }))
            .into_any_element()
    }

    fn render_project_rows(&self, visible: &[usize], cx: &mut Context<Self>) -> AnyElement {
        let now = projects::now();
        let header = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(12.))
            .h(px(28.))
            .px(px(10.))
            .text_color(theme::color(theme::palette().ui_muted))
            .child(div().w(px(240.)).child("Title"))
            .child(div().w(px(64.)).child("Type"))
            .child(div().flex_1().child("Location"))
            .child(div().w(px(110.)).child("Last opened"))
            .child(div().w(px(250.)));
        let rows = visible.iter().enumerate().map(|(position, &index)| {
            let entry = &self.registry.projects[index];
            let path = entry.path.clone();
            let exists = !self.missing.contains(&path);
            let selected = position == self.selected;
            let renaming = self.rename.as_ref().filter(|rename| rename.target == RenameTarget::Project(path.clone()));
            let title: AnyElement = match renaming {
                Some(rename) => field_box(&rename.field).into_any_element(),
                None => div().truncate().font_weight(FontWeight::MEDIUM).child(entry.title.clone()).into_any_element(),
            };
            let language = self.languages.get(&path).copied();
            let (open_path, rename_path, template_path, remove_path, hover_path) = (path.clone(), path.clone(), path.clone(), path.clone(), path.clone());
            let current_title = entry.title.clone();
            // Beside the title column, so that the row stays readable.
            let preview = self
                .preview
                .as_ref()
                .filter(|preview| preview.shown && preview.project == path)
                .map(|preview| self.render_preview(preview, entry, 0, cx));
            div()
                .id(("hub-project", index))
                .relative()
                .on_hover(cx.listener(move |this, hovered: &bool, _, cx| this.hover_card(&hover_path, *hovered, cx)))
                .flex()
                .flex_none()
                .items_center()
                .gap(px(12.))
                .h(px(ROW_HEIGHT))
                .px(px(10.))
                .rounded(px(theme::RADIUS_MD))
                .cursor_pointer()
                .when(selected, |style| style.bg(theme::color(theme::palette().accent_soft)))
                .when(!selected, |style| style.hover(|style| style.bg(theme::color(theme::palette().panel))))
                .when(!exists, |style| style.text_color(theme::color(theme::palette().ui_muted)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if this.rename.is_none() {
                        this.open_project(open_path.clone(), cx);
                    }
                }))
                .child(div().w(px(240.)).min_w_0().child(title))
                .child(div().w(px(64.)).children(language.filter(|_| exists).map(badge)))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(theme::color(if exists { theme::palette().ui_muted } else { theme::palette().error }))
                        .child(if exists { projects::tilde(&path) } else { format!("Folder not found: {}", projects::tilde(&path)) }),
                )
                .child(div().w(px(110.)).text_color(theme::color(theme::palette().ui_muted)).child(projects::ago(now, entry.last_opened)))
                .child(
                    div()
                        .flex()
                        .w(px(250.))
                        .justify_end()
                        .gap(px(2.))
                        .when(exists, |element| {
                            element
                                .child(link(("hub-rename", index), "Rename", cx, move |this, window, cx| {
                                    this.start_rename(RenameTarget::Project(rename_path.clone()), &current_title, window, cx);
                                }))
                                .child(link(("hub-save-template", index), "Save as template…", cx, move |this, window, cx| {
                                    this.open_save_template(template_path.clone(), window, cx);
                                }))
                        })
                        .child(link(("hub-remove", index), "Remove…", cx, move |this, window, cx| {
                            this.remove_project(remove_path.clone(), window, cx);
                        })),
                )
                .children(preview)
        });
        div().flex().flex_col().child(header).children(rows).into_any_element()
    }

    fn render_templates_page(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let palette = theme::palette();
        let search = self.render_search().into_any_element();
        let import = button("hub-import", "Import folder as template…", false, cx, |this, window, cx| this.prompt_import_template(window, cx)).into_any_element();
        let mut elements = vec![
            self.render_header("Templates", vec![search, import]).into_any_element(),
            div()
                .text_color(theme::color(palette.ui_muted))
                .child("A template is a project to start from. Save any project as one from its \u{2026} menu.")
                .into_any_element(),
        ];
        if let Some(message) = self.message.clone() {
            elements.push(div().text_color(theme::color(palette.ui_muted)).child(message).into_any_element());
        }
        let visible = self.visible_templates(cx);
        if visible.is_empty() {
            elements.push(empty_note("No template matches the search.").into_any_element());
            return elements;
        }
        let cards = visible.into_iter().enumerate().map(|(position, index)| {
            let template = &self.templates[index];
            let selected = position == self.selected;
            let folder = match &template.source {
                TemplateSource::Folder(dir) => Some(dir.clone()),
                TemplateSource::Builtin(_) => None,
            };
            let renaming = folder
                .as_ref()
                .and_then(|dir| self.rename.as_ref().filter(|rename| rename.target == RenameTarget::Template(dir.clone())));
            let name: AnyElement = match renaming {
                Some(rename) => field_box(&rename.field).into_any_element(),
                None => div().truncate().font_weight(FontWeight::SEMIBOLD).child(template.name.clone()).into_any_element(),
            };
            let description = if template.description.is_empty() { "No description." } else { template.description.as_str() };
            let icon = match template.language {
                TemplateLanguage::Latex => "file-tex",
                TemplateLanguage::Typst => "file-typ",
                TemplateLanguage::Other => "folder",
            };
            let current_name = template.name.clone();
            div()
                .id(("hub-template", index))
                .flex()
                .flex_col()
                .flex_none()
                .gap(px(8.))
                .w(px(CARD_WIDTH))
                .p(px(14.))
                .rounded(px(theme::RADIUS_LG))
                .cursor_pointer()
                .bg(theme::color(palette.panel))
                .border_2()
                .border_color(theme::color(if selected { palette.accent } else { palette.divider }))
                .when(!selected, |style| style.hover(|style| style.border_color(theme::color(palette.ui_muted))))
                .on_click(cx.listener(move |this, _, window, cx| {
                    if this.rename.is_none() {
                        this.open_new_project(index, window, cx);
                    }
                }))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(svg().path(format!("icons/{icon}.svg")).size(px(24.)).text_color(theme::color(palette.accent)))
                        .child(badge(template.language)),
                )
                .child(name)
                .child(div().h(px(36.)).overflow_hidden().text_color(theme::color(palette.ui_muted)).child(description.to_string()))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(2.))
                        .child(div().flex_1().text_color(theme::color(palette.accent)).font_weight(FontWeight::SEMIBOLD).child("Use"))
                        .when_none(&folder, |element| element.child(div().text_color(theme::color(palette.ui_muted)).child("Built-in")))
                        .when_some(folder.clone(), |element, dir| {
                            let (rename_dir, reveal_dir, delete_dir) = (dir.clone(), dir.clone(), dir);
                            let delete_name = current_name.clone();
                            element
                                .child(link(("hub-template-rename", index), "Rename", cx, move |this, window, cx| {
                                    this.start_rename(RenameTarget::Template(rename_dir.clone()), &current_name, window, cx);
                                }))
                                .child(link(("hub-template-reveal", index), "Show", cx, move |_, _, cx| cx.reveal_path(&reveal_dir)))
                                .child(link(("hub-template-delete", index), "Delete…", cx, move |this, window, cx| {
                                    this.delete_template(delete_dir.clone(), delete_name.clone(), window, cx);
                                }))
                        }),
                )
        });
        elements.push(div().flex().flex_wrap().gap(px(CARD_GAP)).children(cards).into_any_element());
        elements
    }

    fn render_tools_page(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let palette = theme::palette();
        let setup = button("hub-run-setup", "Run setup again", false, cx, |_, _, cx| cx.emit(HomeEvent::RunSetup)).into_any_element();
        vec![
            self.render_header("Writing tools", vec![setup]).into_any_element(),
            div()
                .text_color(theme::color(palette.ui_muted))
                .child(if cfg!(windows) { "A compiler turns what you write into a PDF. You need the one for the language you write in. Overleaf has these on its servers, here they live on your PC." } else { "A compiler turns what you write into a PDF. You need the one for the language you write in. Overleaf has these on its servers, here they live on your Mac." })
                .into_any_element(),
            div().children(toolchain::installer_choice(&self.toolchain, cx)).into_any_element(),
            div()
                .flex()
                .flex_col()
                .px(px(16.))
                .py(px(4.))
                .rounded(px(theme::RADIUS_LG))
                .bg(theme::color(palette.panel))
                .border_1()
                .border_color(theme::color(palette.divider))
                .children(toolchain::tool_rows(&self.toolchain, Tool::ALL, true, cx))
                .into_any_element(),
            div().text_size(px(11.)).text_color(theme::color(palette.ui_muted)).child(toolchain::promise(&self.toolchain, cx)).into_any_element(),
        ]
    }

    fn render_help_page(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        vec![
            self.render_header("Help", Vec::new()).into_any_element(),
            div()
                .flex()
                .gap(px(12.))
                .child(self.action_card("hub-help-tour", "play", "Take the editor tour", "A short walk through the editor, one part at a time.", cx, |_, _, cx| {
                    cx.emit(HomeEvent::StartTour)
                }))
                .child(self.action_card("hub-help-keys", "vim", "Keyboard shortcuts", "Every key the editor knows.", cx, |_, _, cx| cx.emit(HomeEvent::ShowShortcuts)))
                .child(self.action_card("hub-help-tools", "wrench", "Set up again", "Choose LaTeX, Typst or both and install what is missing.", cx, |_, _, cx| {
                    cx.emit(HomeEvent::RunSetup)
                }))
                // The Windows installer puts the command on the PATH itself.
                .when(!cfg!(windows), |row| {
                    row.child(self.action_card("hub-help-command", "folder", "Terminal command", "Type editor . in Terminal to open a folder here.", cx, |_, _, cx| {
                        cx.emit(HomeEvent::InstallCommand)
                    }))
                })
                .child(self.action_card("hub-help-report", "log", "Report a problem", "Tell what went wrong. The editor adds the details.", cx, |_, _, cx| {
                    cx.emit(HomeEvent::ReportProblem)
                }))
                .into_any_element(),
        ]
    }

    fn render_dialog(&self, dialog: &HubDialog, cx: &mut Context<Self>) -> impl IntoElement {
        let (title, body, error, busy, confirm): (&str, AnyElement, Option<&String>, bool, &str) = match dialog {
            HubDialog::NewProject { name, location, template, error, busy } => {
                let entries: Vec<(TemplateLanguage, String, String)> =
                    self.templates.iter().map(|entry| (entry.language, entry.name.clone(), entry.description.clone())).collect();
                let templates: Vec<AnyElement> = entries.into_iter().enumerate().map(|(index, (language, name, description))| {
                    let chosen = index == *template;
                    div()
                        .id(("hub-dialog-template", index))
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .h(px(34.))
                        .px(px(8.))
                        .rounded(px(4.))
                        .cursor_pointer()
                        .when(chosen, |style| style.bg(theme::color(theme::palette().accent_soft)))
                        .when(!chosen, |style| style.hover(|style| style.bg(theme::color(theme::palette().bg))))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(HubDialog::NewProject { template, .. }) = &mut this.dialog {
                                *template = index;
                                cx.notify();
                            }
                        }))
                        .child(div().w(px(56.)).flex_none().child(badge(language)))
                        .child(div().flex_none().font_weight(FontWeight::MEDIUM).child(name))
                        .child(div().flex_1().min_w_0().truncate().text_color(theme::color(theme::palette().ui_muted)).child(description))
                        .into_any_element()
                }).collect();
                let body = div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(label("Name"))
                    .child(field_box(name))
                    .child(label("Location"))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(div().flex_1().min_w_0().truncate().child(projects::tilde(&location.join(field_text(name, cx).trim()))))
                            .child(button("hub-browse", "Browse…", false, cx, |this, _, cx| this.browse_location(cx))),
                    )
                    .child(label("Template"))
                    .child(div().id("hub-dialog-templates").flex().flex_col().max_h(px(260.)).overflow_y_scroll().children(templates))
                    .into_any_element();
                ("New project", body, error.as_ref(), *busy, "Create")
            }
            HubDialog::SaveTemplate { from, language, name, description, error, busy } => {
                let body = div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .text_color(theme::color(theme::palette().ui_muted))
                            .child(badge(*language))
                            .child(div().flex_1().min_w_0().truncate().child(format!("From {}", projects::tilde(from)))),
                    )
                    .child(label("Name"))
                    .child(field_box(name))
                    .child(label("Description"))
                    .child(field_box(description))
                    .child(
                        div()
                            .text_color(theme::color(theme::palette().ui_muted))
                            .child("The files are copied, without the build output and git history."),
                    )
                    .into_any_element();
                ("Save as template", body, error.as_ref(), *busy, "Save")
            }
        };
        div()
            .id("hub-dialog-backdrop")
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(gpui::rgba(0x00000066))
            .occlude()
            .child(
                div()
                    .id("hub-dialog")
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .w(px(560.))
                    .p(px(20.))
                    .rounded(px(theme::RADIUS_LG))
                    .bg(theme::color(theme::palette().panel))
                    .border_1()
                    .border_color(theme::color(theme::palette().divider))
                    .shadow_lg()
                    .child(div().text_size(px(17.)).font_weight(FontWeight::SEMIBOLD).child(title))
                    .child(body)
                    .when_some(error.cloned(), |element, error| {
                        element.child(div().text_color(theme::color(theme::palette().error)).child(error))
                    })
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap(px(8.))
                            .child(button("hub-dialog-cancel", "Cancel", false, cx, |this, window, cx| this.close_dialog(window, cx)))
                            .child(button("hub-dialog-confirm", if busy { "Working…" } else { confirm }, true, cx, |this, window, cx| {
                                this.confirm_dialog(window, cx);
                            })),
                    ),
            )
    }
}

impl Render for HomeView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.scale = window.scale_factor();
        let available = (window.viewport_size().width.to_f64() as f32 - SIDEBAR_WIDTH - 2. * CONTENT_PADDING).min(CONTENT_WIDTH);
        self.columns = (((available + CARD_GAP) / (CARD_WIDTH + CARD_GAP)).floor() as usize).max(1);
        let content = match self.page {
            HubPage::Projects => self.render_projects_page(cx),
            HubPage::Templates => self.render_templates_page(cx),
            HubPage::Tools => self.render_tools_page(cx),
            HubPage::Help => self.render_help_page(cx),
        };
        let dialog = self.dialog.as_ref().map(|dialog| self.render_dialog(dialog, cx).into_any_element());
        // A click on the empty hub must not leave the keys nowhere.
        if window.focused(cx).is_none() {
            window.focus(&self.focus, cx);
        }
        div()
            .id("hub")
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .track_focus(&self.focus)
            .bg(theme::color(theme::palette().bg))
            .text_color(theme::color(theme::palette().ui_text))
            .text_size(px(theme::UI_FONT_SIZE))
            .when_some(theme::look().ui_font, |root, font| root.font_family(font))
            .on_click(cx.listener(|this, _, _, cx| {
                if this.card_menu.take().is_some() {
                    cx.notify();
                }
            }))
            .child(self.render_band(window, cx))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(self.render_sidebar(cx))
                    .child(
                        div().id("hub-content").flex().flex_1().min_w_0().justify_center().overflow_y_scroll().child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(24.))
                                .w_full()
                                .max_w(px(CONTENT_WIDTH + 2. * CONTENT_PADDING))
                                .px(px(CONTENT_PADDING))
                                .pt(px(24.))
                                .pb(px(40.))
                                .children(content),
                        ),
                    ),
            )
            .children(dialog)
    }
}

fn plain_field(text: &str, cx: &mut Context<HomeView>) -> Entity<EditorView> {
    cx.new(|cx| {
        let mut view = EditorView::new(text, None, cx);
        view.set_plain();
        view
    })
}

fn field_text(field: &Entity<EditorView>, cx: &Context<HomeView>) -> String {
    field.read(cx).state.buffer().to_string().replace('\n', " ")
}

fn field_box(field: &Entity<EditorView>) -> impl IntoElement {
    div()
        .h(px(FIELD_HEIGHT))
        .px(px(6.))
        .rounded(px(4.))
        .overflow_hidden()
        .bg(theme::color(theme::palette().bg))
        .border_1()
        .border_color(theme::color(theme::palette().divider))
        .child(field.clone())
}

fn label(text: &'static str) -> impl IntoElement {
    div().pt(px(4.)).text_color(theme::color(theme::palette().ui_muted)).child(text)
}

fn section_title(text: impl Into<SharedString>) -> impl IntoElement {
    div().text_size(px(11.)).font_weight(FontWeight::SEMIBOLD).text_color(theme::color(theme::palette().ui_muted)).child(text.into().to_uppercase())
}

/// What an empty page shows, with room to breathe so that it reads as a start
/// rather than as a fault.
fn empty_state(icon: &'static str, title: &'static str, text: &'static str) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .items_center()
        .gap(px(8.))
        .py(px(48.))
        .rounded(px(theme::RADIUS_LG))
        .border_1()
        .border_dashed()
        .border_color(theme::color(theme::palette().divider))
        .child(svg().path(format!("icons/{icon}.svg")).size(px(40.)).text_color(theme::color(theme::palette().ui_muted)))
        .child(div().text_size(px(15.)).font_weight(FontWeight::SEMIBOLD).child(title))
        .child(div().text_color(theme::color(theme::palette().ui_muted)).child(text))
}

fn menu_item(
    id: impl Into<gpui::ElementId>,
    text: &'static str,
    cx: &mut Context<HomeView>,
    handler: impl Fn(&mut HomeView, &mut Window, &mut Context<HomeView>) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .px(px(10.))
        .py(px(6.))
        .rounded(px(theme::RADIUS_SM))
        .cursor_pointer()
        .hover(|style| style.bg(theme::color(theme::palette().accent_soft)))
        .on_click(cx.listener(move |this, _, window, cx| {
            cx.stop_propagation();
            handler(this, window, cx);
        }))
        .child(text)
}

fn empty_note(text: &'static str) -> impl IntoElement {
    div().py(px(40.)).flex().justify_center().text_color(theme::color(theme::palette().ui_muted)).child(text)
}

/// The kind of a template or project, in the color its source has in the editor.
fn badge(language: TemplateLanguage) -> impl IntoElement {
    let palette = theme::palette();
    let color = match language {
        TemplateLanguage::Latex => palette.function,
        TemplateLanguage::Typst => palette.keyword,
        TemplateLanguage::Other => palette.ui_muted,
    };
    div()
        .flex_none()
        .px(px(6.))
        .rounded(px(3.))
        .border_1()
        .border_color(theme::color(color))
        .text_color(theme::color(color))
        .text_size(px(11.))
        .child(language.label())
}

fn button(
    id: &'static str,
    text: &'static str,
    primary: bool,
    cx: &mut Context<HomeView>,
    handler: impl Fn(&mut HomeView, &mut Window, &mut Context<HomeView>) + 'static,
) -> impl IntoElement {
    let palette = theme::palette();
    div()
        .id(id)
        .flex_none()
        .px(px(12.))
        .py(px(6.))
        .rounded(px(theme::RADIUS_MD))
        .cursor_pointer()
        .font_weight(FontWeight::MEDIUM)
        .when(primary, |style| style.bg(theme::color(palette.accent)).text_color(theme::color(palette.on_accent)))
        .when(!primary, |style| style.bg(theme::color(palette.divider)).hover(|style| style.bg(theme::color(palette.accent_soft))))
        .on_click(cx.listener(move |this, _, window, cx| {
            cx.stop_propagation();
            handler(this, window, cx);
        }))
        .child(text)
}

/// A quiet action inside a row, which must not open the row.
fn link(
    id: impl Into<gpui::ElementId>,
    text: impl Into<SharedString>,
    cx: &mut Context<HomeView>,
    handler: impl Fn(&mut HomeView, &mut Window, &mut Context<HomeView>) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .flex_none()
        .px(px(6.))
        .py(px(2.))
        .rounded(px(3.))
        .cursor_pointer()
        .text_color(theme::color(theme::palette().ui_muted))
        .hover(|style| style.bg(theme::color(theme::palette().divider)).text_color(theme::color(theme::palette().ui_text)))
        .on_click(cx.listener(move |this, _, window, cx| {
            cx.stop_propagation();
            handler(this, window, cx);
        }))
        .child(text.into())
}

/// Reads what the preview tells about a project. Blocks on the disk, and on git
/// for a project in a repository.
fn data_sheet(project: &Path) -> DataSheet {
    let config = build::Config::load(project).unwrap_or_default();
    let main = config
        .main
        .clone()
        .or_else(|| ["main.typ", "main.tex"].into_iter().map(PathBuf::from).find(|name| project.join(name).is_file()))
        .map(|main| project.join(main));
    let engine = config.engine.or_else(|| main.as_deref().and_then(build::Engine::for_path)).map(build::Engine::label);
    let mut edited: Option<SystemTime> = None;
    let counted = main.as_ref().and_then(|main| {
        let mut read = |path: &Path| {
            let modified = std::fs::metadata(path).and_then(|metadata| metadata.modified()).ok();
            edited = edited.max(modified);
            std::fs::read_to_string(path).ok()
        };
        let text = read(main)?;
        // LaTeX runs in the main file's folder, and Typst resolves `/` from the project.
        let base = if Language::from_path(main) == Language::Typst { project } else { main.parent().unwrap_or(project) };
        Some(word_count::count_words(main, &text, base, &mut read))
    });
    let words = counted.as_ref().map(word_count::WordCount::total);
    let language = main.as_deref().map_or(Language::Latex, Language::from_path);
    let contents = counted.map_or_else(Vec::new, |counted| outline::outline(&counted.outline, language));
    // Only a project in a repository starts git.
    let git = project
        .ancestors()
        .any(|folder| folder.join(".git").exists())
        .then(crate::git_panel::program)
        .flatten()
        .and_then(|program| program.discover(project).ok().flatten())
        .and_then(|repository| repository.head_summary());
    DataSheet { engine, words, contents, edited, pdf: projects::newest_pdf(project), git }
}

/// 84210 as 84,210.
fn group_digits(value: usize) -> String {
    let digits = value.to_string();
    let mut grouped = String::new();
    for (position, digit) in digits.chars().enumerate() {
        if position > 0 && (digits.len() - position).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    grouped
}

fn page_button(
    id: &'static str,
    icon: &'static str,
    enabled: bool,
    cx: &mut Context<HomeView>,
    handler: impl Fn(&mut HomeView, &mut Context<HomeView>) + 'static,
) -> impl IntoElement {
    let palette = theme::palette();
    div()
        .id(id)
        .p(px(4.))
        .rounded(px(theme::RADIUS_SM))
        .when(enabled, |style| {
            style.cursor_pointer().hover(|style| style.bg(theme::color(palette.divider))).on_click(cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                handler(this, cx);
            }))
        })
        .when(!enabled, |style| style.opacity(0.3))
        .child(svg().path(format!("icons/{icon}.svg")).size(px(16.)).text_color(theme::color(palette.ui_text)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn heading(title: &str, depth: usize) -> OutlineItem {
        OutlineItem {
            level: depth,
            title: title.to_string(),
            line: 0,
            depth,
            has_children: false,
            collapsed: false,
            number: None,
            kind: editor_core::HeadingKind::Section,
        }
    }

    fn preview(contents: Vec<OutlineItem>, bookmarks: &[(&str, usize)]) -> Preview {
        Preview {
            project: PathBuf::new(),
            shown: true,
            over_card: false,
            over_preview: false,
            pinned: false,
            sheet: Some(DataSheet { contents, ..DataSheet::default() }),
            worker: None,
            page: 0,
            page_count: None,
            pages: HashMap::new(),
            contents: Vec::new(),
            contents_pages: Vec::new(),
            bookmarks: bookmarks.iter().map(|(title, page)| OutlineEntry { title: title.to_string(), level: 0, page: *page }).collect(),
            scroll: 0.,
            _task: Task::ready(()),
            _closing: None,
        }
    }

    #[test]
    fn headings_find_their_pages_in_the_bookmarks() {
        let contents = vec![heading("Introduction", 0), heading("Notes", 1), heading("The \\LaTeX{} way", 1), heading("End", 0)];
        let mut preview = preview(contents, &[("1 Introduction", 0), ("1.1 The LaTeX way", 2), ("2 End", 5)]);
        preview.link_pages();
        assert_eq!(preview.contents_pages, [Some(0), None, Some(2), Some(5)]);
    }

    #[test]
    fn without_sources_the_bookmarks_are_the_contents() {
        let mut preview = preview(Vec::new(), &[("Start", 0), ("More", 3)]);
        preview.link_pages();
        assert_eq!(preview.contents.iter().map(|item| item.title.as_str()).collect::<Vec<_>>(), ["Start", "More"]);
        assert_eq!(preview.contents_pages, [Some(0), Some(3)]);
    }

    #[test]
    fn digits_come_in_groups_of_three() {
        assert_eq!(group_digits(7), "7");
        assert_eq!(group_digits(1000), "1,000");
        assert_eq!(group_digits(84210), "84,210");
        assert_eq!(group_digits(1234567), "1,234,567");
    }
}
