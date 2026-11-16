//! The project hub: every project the user opened, the templates new ones start
//! from, and the dialogs to make both.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::SystemTime,
};

use gpui::{
    AnyElement, Context, Entity, EventEmitter, FocusHandle, Focusable, FontWeight, Keystroke, ObjectFit, PathPromptOptions, PromptLevel,
    RenderImage, SharedString, Subscription, Task, Window, div, img, prelude::*, px, svg,
};
use pdf::{PageRequest, PdfEvent, PdfWorker};

use crate::{
    editor_view::EditorView,
    picker::fuzzy_score,
    projects::{self, Registry, Template, TemplateLanguage, TemplateSource},
    theme,
    toolchain::{self, Tool, Toolchain},
};

#[cfg(target_os = "macos")]
const TRAFFIC_LIGHT_SPACE: f32 = 78.0;
#[cfg(not(target_os = "macos"))]
const TRAFFIC_LIGHT_SPACE: f32 = 8.0;
const BAND_HEIGHT: f32 = 40.0;
const CONTENT_WIDTH: f32 = 1100.0;
const SIDEBAR_WIDTH: f32 = 220.0;
const CONTENT_PADDING: f32 = 32.0;
const ROW_HEIGHT: f32 = 40.0;
const FIELD_HEIGHT: f32 = 28.0;
const CARD_WIDTH: f32 = 248.0;
const CARD_GAP: f32 = 16.0;
const THUMBNAIL_HEIGHT: f32 = 150.0;
/// Rendered wider than shown, so that the page stays sharp on Retina screens.
const THUMBNAIL_PIXELS: u32 = 2 * CARD_WIDTH as u32;

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
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum HubPage {
    Projects,
    Templates,
    Tools,
    Help,
}

/// The first page of a project's PDF, as of the PDF's last change.
enum Thumbnail {
    Loading,
    Ready(Arc<RenderImage>, SystemTime),
    None,
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
        /// Text from the clipboard that becomes the main file, in place of a template.
        pasted: Option<(TemplateLanguage, String)>,
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
    thumbnails: HashMap<PathBuf, Thumbnail>,
    thumbnail_task: Option<Task<()>>,
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
            thumbnails: HashMap::new(),
            thumbnail_task: None,
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
        view.find_languages();
        view
    }

    /// Records an opened project, so that it comes first next time.
    pub fn touch(&mut self, path: &Path, cx: &mut Context<Self>) {
        self.registry.touch(path, projects::now());
        self.languages.insert(path.to_path_buf(), projects::detect_language(path));
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
        self.load_thumbnails(cx);
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

    fn find_languages(&mut self) {
        for entry in &self.registry.projects {
            if !self.languages.contains_key(&entry.path) && entry.path.is_dir() {
                self.languages.insert(entry.path.clone(), projects::detect_language(&entry.path));
            }
        }
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
            "escape" => {
                if self.card_menu.take().is_some() {
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
            cx.notify();
        }
        true
    }

    fn switch_page(&mut self, page: HubPage, window: &mut Window, cx: &mut Context<Self>) {
        self.page = page;
        self.selected = 0;
        self.rename = None;
        self.card_menu = None;
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
        self.dialog = Some(HubDialog::NewProject { name, location: self.registry.location(), template, pasted: None, error: None, busy: false });
        cx.notify();
    }

    /// The template named `name`, such as "Empty LaTeX", for the cards on top.
    fn template_named(&self, name: &str) -> usize {
        self.templates.iter().position(|template| template.name == name).unwrap_or(0)
    }

    /// A new project from what Claude or anyone else wrote, taken from the clipboard.
    fn paste_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = cx.read_from_clipboard().and_then(|item| item.text()).unwrap_or_default();
        if text.trim().is_empty() {
            self.message = Some("The clipboard is empty. Copy the LaTeX or Typst from Claude first, then click Paste from Claude again.".to_string());
            return cx.notify();
        }
        let language = projects::guess_language(&text);
        let name = plain_field("", cx);
        self.dialog_observers = vec![cx.observe(&name, |_, _, cx| cx.notify())];
        window.focus(&name.read(cx).focus_handle(cx), cx);
        self.dialog = Some(HubDialog::NewProject {
            name,
            location: self.registry.location(),
            template: 0,
            pasted: Some((language, text)),
            error: None,
            busy: false,
        });
        cx.notify();
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
            Some(HubDialog::NewProject { name, location, template, pasted, error, busy }) => {
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
                let pasted = pasted.clone();
                cx.spawn_in(window, async move |this, cx| {
                    let target = dir.clone();
                    let created = cx
                        .background_spawn(async move {
                            match pasted {
                                Some((language, text)) => projects::create_from_text(&target, language, &text),
                                None => projects::create_project(&template, &target),
                            }
                        })
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
                format!("Removing from the list keeps the folder {}. The trash keeps it until you empty the trash.", projects::tilde(&path))
            } else {
                "The folder is gone already, so only the entry remains.".to_string()
            };
            let answers: &[&str] = if exists { &["Remove from list", "Move to Trash", "Cancel"] } else { &["Remove from list", "Cancel"] };
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
                        this.message = Some(format!("Cannot move the folder to the trash: {error}"));
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
            let message = format!("Move the template {name} to the trash?");
            let detail = "Projects made from it keep their files.";
            let Ok(answer) = this.update_in(cx, |_, window, cx| {
                window.prompt(PromptLevel::Warning, &message, Some(detail), &["Move to Trash", "Cancel"], cx)
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

    // Thumbnails

    /// Renders the first page of each project's PDF, one project after the other,
    /// on a PDFium thread of its own. A PDF that did not change is not redone.
    fn load_thumbnails(&mut self, cx: &mut Context<Self>) {
        let wanted: Vec<(PathBuf, PathBuf, SystemTime)> = self
            .registry
            .projects
            .iter()
            .filter_map(|entry| {
                let (pdf, modified) = projects::newest_pdf(&entry.path)?;
                let current = matches!(self.thumbnails.get(&entry.path), Some(Thumbnail::Ready(_, seen)) if *seen == modified);
                (!current).then(|| (entry.path.clone(), pdf, modified))
            })
            .collect();
        for entry in &self.registry.projects {
            if projects::newest_pdf(&entry.path).is_none() {
                self.thumbnails.insert(entry.path.clone(), Thumbnail::None);
            }
        }
        if wanted.is_empty() {
            return;
        }
        for (project, _, _) in &wanted {
            self.thumbnails.entry(project.clone()).or_insert(Thumbnail::Loading);
        }
        let recolor = None;
        self.thumbnail_task = Some(cx.spawn(async move |this, cx| {
            let library = crate::pdf_view::library_directory();
            let Ok((worker, mut events)) = cx.background_spawn(async move { PdfWorker::spawn(&library) }).await else {
                return;
            };
            for (generation, (project, pdf, modified)) in wanted.into_iter().enumerate() {
                let generation = generation as u64;
                let Ok(bytes) = std::fs::read(&pdf) else {
                    continue;
                };
                worker.load(generation, bytes);
                let request = PageRequest { page: 0, width_px: THUMBNAIL_PIXELS, recolor };
                worker.render(generation, vec![request]);
                let image = loop {
                    match events.recv().await {
                        Some(PdfEvent::Rendered { generation: done, request, height_px, bgra }) if done == generation => {
                            break image::RgbaImage::from_raw(request.width_px, height_px, bgra)
                                .map(|buffer| Arc::new(RenderImage::new([image::Frame::new(buffer)])));
                        }
                        Some(PdfEvent::Failed { generation: done, .. }) if done == generation => break None,
                        Some(_) => {}
                        None => return,
                    }
                };
                let result = this.update(cx, |this, cx| {
                    let thumbnail = match image {
                        Some(image) => Thumbnail::Ready(image, modified),
                        None => Thumbnail::None,
                    };
                    this.thumbnails.insert(project, thumbnail);
                    cx.notify();
                });
                if result.is_err() {
                    return;
                }
            }
        }));
    }

    // Rendering

    fn render_band(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_none()
            .items_center()
            .justify_end()
            .h(px(BAND_HEIGHT))
            .pl(px(TRAFFIC_LIGHT_SPACE))
            .pr(px(10.))
            .bg(theme::color(theme::palette().panel))
            .when(self.has_project, |element| {
                element.child(button("hub-back", "Back to project", false, cx, |_, _, cx| cx.emit(HomeEvent::Close)))
            })
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
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_center()
                            .size(px(32.))
                            .rounded(px(theme::RADIUS_MD))
                            .bg(theme::color(palette.accent))
                            .child(svg().path("icons/logo.svg").size(px(20.)).text_color(theme::color(palette.on_accent))),
                    )
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
                    .child(self.action_card("hub-start-paste", "clipboard", "Paste from Claude", "Copied LaTeX or Typst becomes a new project.", cx, |this, window, cx| {
                        this.paste_project(window, cx)
                    }))
                    .child(self.action_card("hub-start-open", "folder", "Open folder", "A project you already have on this Mac.", cx, |this, _, cx| {
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

    fn render_project_cards(&self, visible: &[usize], cx: &mut Context<Self>) -> AnyElement {
        let palette = theme::palette();
        let now = projects::now();
        let cards = visible.iter().enumerate().map(|(position, &index)| {
            let entry = &self.registry.projects[index];
            let path = entry.path.clone();
            let exists = path.is_dir();
            let selected = position == self.selected;
            let language = self.languages.get(&path).copied().filter(|_| exists);
            let renaming = self.rename.as_ref().filter(|rename| rename.target == RenameTarget::Project(path.clone()));
            let title: AnyElement = match renaming {
                Some(rename) => field_box(&rename.field).into_any_element(),
                None => div().truncate().font_weight(FontWeight::SEMIBOLD).child(entry.title.clone()).into_any_element(),
            };
            let preview: AnyElement = match self.thumbnails.get(&path) {
                Some(Thumbnail::Ready(image, _)) => img(image.clone()).size_full().object_fit(ObjectFit::Cover).into_any_element(),
                _ => {
                    let icon = match language {
                        Some(TemplateLanguage::Typst) => "file-typ",
                        Some(TemplateLanguage::Latex) => "file-tex",
                        _ => "folder",
                    };
                    div()
                        .flex()
                        .items_center()
                        .justify_center()
                        .size_full()
                        .child(svg().path(format!("icons/{icon}.svg")).size(px(40.)).text_color(theme::color(palette.ui_muted)))
                        .into_any_element()
                }
            };
            let menu_open = self.card_menu.as_ref() == Some(&path);
            let (open_path, menu_path) = (path.clone(), path.clone());
            let menu = menu_open.then(|| self.render_card_menu(index, &path, &entry.title, exists, cx));
            div()
                .id(("hub-card", index))
                .relative()
                .flex()
                .flex_col()
                .flex_none()
                .w(px(CARD_WIDTH))
                .rounded(px(theme::RADIUS_LG))
                .cursor_pointer()
                .bg(theme::color(palette.panel))
                .border_2()
                .border_color(theme::color(if selected { palette.accent } else { palette.divider }))
                .when(!selected, |style| style.hover(|style| style.border_color(theme::color(palette.ui_muted))))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if this.rename.is_none() {
                        this.open_project(open_path.clone(), cx);
                    }
                }))
                .child(
                    div()
                        .h(px(THUMBNAIL_HEIGHT))
                        .w_full()
                        .overflow_hidden()
                        .rounded_t(px(theme::RADIUS_LG - 2.))
                        .bg(theme::color(if self.thumbnails.contains_key(&path) && matches!(self.thumbnails.get(&path), Some(Thumbnail::Ready(..))) {
                            0xffffffff
                        } else {
                            palette.bg
                        }))
                        .when(!exists, |style| style.opacity(0.4))
                        .child(preview),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .p(px(12.))
                        .border_t_1()
                        .border_color(theme::color(palette.divider))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.))
                                .child(div().flex_1().min_w_0().child(title))
                                .child(
                                    div()
                                        .id(("hub-card-menu", index))
                                        .flex_none()
                                        .p(px(2.))
                                        .rounded(px(theme::RADIUS_SM))
                                        .hover(|style| style.bg(theme::color(palette.divider)))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.card_menu = if this.card_menu.as_ref() == Some(&menu_path) { None } else { Some(menu_path.clone()) };
                                            cx.notify();
                                        }))
                                        .child(svg().path("icons/dots.svg").size(px(16.)).text_color(theme::color(palette.ui_muted))),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.))
                                .text_color(theme::color(palette.ui_muted))
                                .children(language.map(badge))
                                .child(if exists { projects::ago(now, entry.last_opened) } else { "Folder not found".to_string() }),
                        ),
                )
                .children(menu)
        });
        div().flex().flex_wrap().gap(px(CARD_GAP)).children(cards).into_any_element()
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
                    .child(menu_item(("hub-card-reveal", index), "Show in Finder", cx, move |this, _, cx| {
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
            let exists = path.is_dir();
            let selected = position == self.selected;
            let renaming = self.rename.as_ref().filter(|rename| rename.target == RenameTarget::Project(path.clone()));
            let title: AnyElement = match renaming {
                Some(rename) => field_box(&rename.field).into_any_element(),
                None => div().truncate().font_weight(FontWeight::MEDIUM).child(entry.title.clone()).into_any_element(),
            };
            let language = self.languages.get(&path).copied();
            let (open_path, rename_path, template_path, remove_path) = (path.clone(), path.clone(), path.clone(), path.clone());
            let current_title = entry.title.clone();
            div()
                .id(("hub-project", index))
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
                .child("A compiler turns what you write into a PDF. You need the one for the language you write in. Overleaf has these on its servers, here they live on your Mac.")
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
                .children(toolchain::tool_rows(&self.toolchain, &Tool::ALL, cx))
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
                .child(self.action_card("hub-help-command", "folder", "Terminal command", "Type editor . in Terminal to open a folder here.", cx, |_, _, cx| {
                    cx.emit(HomeEvent::InstallCommand)
                }))
                .into_any_element(),
        ]
    }

    fn render_dialog(&self, dialog: &HubDialog, cx: &mut Context<Self>) -> impl IntoElement {
        let (title, body, error, busy, confirm): (&str, AnyElement, Option<&String>, bool, &str) = match dialog {
            HubDialog::NewProject { name, location, template, pasted, error, busy } => {
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
                    .when(pasted.is_none(), |element| {
                        element
                            .child(label("Template"))
                            .child(div().id("hub-dialog-templates").flex().flex_col().max_h(px(260.)).overflow_y_scroll().children(templates))
                    })
                    .when_some(pasted.as_ref(), |element, (language, text)| {
                        let lines = text.lines().count();
                        let preview: String = text.lines().take(8).collect::<Vec<_>>().join("\n");
                        element
                            .child(label("From the clipboard"))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(8.))
                                    .text_color(theme::color(theme::palette().ui_muted))
                                    .child(badge(*language))
                                    .child(format!("{lines} lines become {}", if *language == TemplateLanguage::Typst { "main.typ" } else { "main.tex" })),
                            )
                            .child(
                                div()
                                    .max_h(px(150.))
                                    .overflow_hidden()
                                    .p(px(8.))
                                    .rounded(px(theme::RADIUS_SM))
                                    .bg(theme::color(theme::palette().bg))
                                    .font_family(theme::code_font())
                                    .text_size(px(theme::code_font_size()))
                                    .text_color(theme::color(theme::palette().ui_muted))
                                    .child(preview),
                            )
                    })
                    .into_any_element();
                let title = if pasted.is_some() { "New project from the clipboard" } else { "New project" };
                (title, body, error.as_ref(), *busy, "Create")
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
            .child(self.render_band(cx))
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
