//! The project hub: every project the user opened, the templates new ones start
//! from, and the dialogs to make both.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use gpui::{
    AnyElement, Context, Entity, EventEmitter, FocusHandle, Focusable, FontWeight, Keystroke, PathPromptOptions, PromptLevel,
    SharedString, Subscription, Window, div, prelude::*, px, svg,
};

use crate::{
    editor_view::EditorView,
    picker::fuzzy_score,
    projects::{self, Registry, Template, TemplateLanguage, TemplateSource},
    theme,
};

#[cfg(target_os = "macos")]
const TRAFFIC_LIGHT_SPACE: f32 = 78.0;
#[cfg(not(target_os = "macos"))]
const TRAFFIC_LIGHT_SPACE: f32 = 8.0;
const BAND_HEIGHT: f32 = 40.0;
const CONTENT_WIDTH: f32 = 1040.0;
const ROW_HEIGHT: f32 = 40.0;
const FIELD_HEIGHT: f32 = 28.0;

pub enum HomeEvent {
    Open(PathBuf),
    /// Back to the open project.
    Close,
    /// The folder went to the trash, so a workspace showing it has to let go.
    Trashed(PathBuf),
    #[cfg(feature = "collab")]
    JoinSession,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum HubTab {
    Projects,
    Templates,
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
    tab: HubTab,
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
    pub fn new(cx: &mut Context<Self>) -> Self {
        let registry_path = projects::registry_path();
        let registry = registry_path.as_deref().map(Registry::load).unwrap_or_default();
        let library = projects::library_path();
        let templates = projects::load_templates(library.as_deref());
        let search = plain_field("", cx);
        let _search_observer = cx.observe(&search, |this: &mut Self, _, cx| {
            this.selected = 0;
            cx.notify();
        });
        let mut view = Self {
            registry,
            registry_path,
            library,
            templates,
            languages: HashMap::new(),
            tab: HubTab::Projects,
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

    pub fn set_context(&mut self, has_project: bool, vim_mode: bool) {
        self.has_project = has_project;
        self.vim_mode = vim_mode;
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
        self.tab = HubTab::Projects;
        self.selected = 0;
        self.rename = None;
        self.dialog = None;
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
        match self.tab {
            HubTab::Projects => self.visible_projects(cx).len(),
            HubTab::Templates => self.visible_templates(cx).len(),
        }
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
        let step = match key {
            "down" => 1,
            "up" => -1,
            "j" if self.vim_mode && !search_focused => 1,
            "k" if self.vim_mode && !search_focused => -1,
            "enter" => {
                self.activate_selected(window, cx);
                return true;
            }
            "escape" => {
                if self.has_project {
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
                self.switch_tab(if self.tab == HubTab::Projects { HubTab::Templates } else { HubTab::Projects }, window, cx);
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

    fn switch_tab(&mut self, tab: HubTab, window: &mut Window, cx: &mut Context<Self>) {
        self.tab = tab;
        self.selected = 0;
        self.rename = None;
        self.focus(window, cx);
    }

    fn activate_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.tab {
            HubTab::Projects => {
                if let Some(&index) = self.visible_projects(cx).get(self.selected) {
                    let path = self.registry.projects[index].path.clone();
                    self.open_project(path, cx);
                }
            }
            HubTab::Templates => {
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
        self.dialog = Some(HubDialog::NewProject { name, location: self.registry.location(), template, error: None, busy: false });
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
                    let created = cx.background_spawn(async move { projects::create_project(&template, &target) }).await;
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
                            this.tab = HubTab::Templates;
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

    // Rendering

    fn render_band(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_none()
            .items_center()
            .justify_between()
            .h(px(BAND_HEIGHT))
            .pl(px(TRAFFIC_LIGHT_SPACE))
            .pr(px(10.))
            .border_b_1()
            .border_color(theme::color(theme::palette().panel))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(svg().path("icons/notepad.svg").size(px(18.)).flex_none().text_color(theme::color(theme::palette().accent)))
                    .child(div().font_weight(FontWeight::SEMIBOLD).child("Project hub")),
            )
            .when(self.has_project, |element| {
                element.child(button("hub-back", "✕  Back to project", false, cx, |_, _, cx| cx.emit(HomeEvent::Close)))
            })
    }

    fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let tab = |id: &'static str, label: &'static str, target: HubTab, cx: &mut Context<Self>| {
            let active = self.tab == target;
            div()
                .id(id)
                .px(px(12.))
                .py(px(4.))
                .rounded(px(4.))
                .cursor_pointer()
                .when(active, |style| style.bg(theme::color(theme::palette().accent_soft)).text_color(theme::color(theme::palette().ui_text)))
                .when(!active, |style| style.text_color(theme::color(theme::palette().ui_muted)).hover(|style| style.bg(theme::color(theme::palette().panel))))
                .on_click(cx.listener(move |this, _, window, cx| this.switch_tab(target, window, cx)))
                .child(label)
        };
        #[cfg(feature = "collab")]
        let join = Some(button("hub-join", "Join live session…", false, cx, |_, _, cx| cx.emit(HomeEvent::JoinSession)).into_any_element());
        #[cfg(not(feature = "collab"))]
        let join: Option<gpui::AnyElement> = None;
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(8.))
            .child(tab("hub-tab-projects", "Projects", HubTab::Projects, cx))
            .child(tab("hub-tab-templates", "Templates", HubTab::Templates, cx))
            .child(div().flex_1())
            .children(join)
            .child(button("hub-import", "Import folder as template…", false, cx, |this, window, cx| this.prompt_import_template(window, cx)))
            .child(button("hub-open", "Open folder…", false, cx, |this, _, cx| this.prompt_open_folder(cx)))
            .child(button("hub-new", "New project", true, cx, |this, window, cx| this.open_new_project(0, window, cx)))
    }

    fn render_search(&self) -> impl IntoElement {
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(8.))
            .h(px(32.))
            .px(px(8.))
            .rounded(px(4.))
            .bg(theme::color(theme::palette().panel))
            .border_1()
            .border_color(theme::color(theme::palette().divider))
            .child(div().flex_none().text_color(theme::color(theme::palette().ui_muted)).child("Search"))
            .child(div().flex_1().min_w_0().h(px(FIELD_HEIGHT)).overflow_hidden().child(self.search.clone()))
    }

    fn render_projects(&self, cx: &mut Context<Self>) -> AnyElement {
        let visible = self.visible_projects(cx);
        if visible.is_empty() {
            let text = if self.registry.projects.is_empty() {
                "No projects yet. Make a new one, or open a folder you already have."
            } else {
                "No project matches the search."
            };
            return empty_note(text).into_any_element();
        }
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
        let rows = visible.into_iter().enumerate().map(|(position, index)| {
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
                .rounded(px(4.))
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

    fn render_templates(&self, cx: &mut Context<Self>) -> AnyElement {
        let visible = self.visible_templates(cx);
        if visible.is_empty() {
            return empty_note("No template matches the search.").into_any_element();
        }
        let rows = visible.into_iter().enumerate().map(|(position, index)| {
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
                None => div().truncate().font_weight(FontWeight::MEDIUM).child(template.name.clone()).into_any_element(),
            };
            let description = if template.description.is_empty() { "No description." } else { template.description.as_str() };
            let current_name = template.name.clone();
            div()
                .id(("hub-template", index))
                .flex()
                .flex_none()
                .items_center()
                .gap(px(12.))
                .h(px(ROW_HEIGHT))
                .px(px(10.))
                .rounded(px(4.))
                .cursor_pointer()
                .when(selected, |style| style.bg(theme::color(theme::palette().accent_soft)))
                .when(!selected, |style| style.hover(|style| style.bg(theme::color(theme::palette().panel))))
                .on_click(cx.listener(move |this, _, window, cx| {
                    if this.rename.is_none() {
                        this.open_new_project(index, window, cx);
                    }
                }))
                .child(div().w(px(240.)).min_w_0().child(name))
                .child(div().w(px(64.)).child(badge(template.language)))
                .child(div().flex_1().min_w_0().truncate().text_color(theme::color(theme::palette().ui_muted)).child(description.to_string()))
                .child(
                    div()
                        .flex()
                        .w(px(300.))
                        .justify_end()
                        .gap(px(2.))
                        .child(link(("hub-use", index), "New project from it", cx, move |this, window, cx| {
                            this.open_new_project(index, window, cx);
                        }))
                        .when_none(&folder, |element| {
                            element.child(div().px(px(6.)).text_color(theme::color(theme::palette().ui_muted)).child("Built-in"))
                        })
                        .when_some(folder.clone(), |element, dir| {
                            let (rename_dir, reveal_dir, delete_dir) = (dir.clone(), dir.clone(), dir);
                            let delete_name = current_name.clone();
                            element
                                .child(link(("hub-template-rename", index), "Rename", cx, move |this, window, cx| {
                                    this.start_rename(RenameTarget::Template(rename_dir.clone()), &current_name, window, cx);
                                }))
                                .child(link(("hub-template-reveal", index), "Show in Finder", cx, move |_, _, cx| cx.reveal_path(&reveal_dir)))
                                .child(link(("hub-template-delete", index), "Delete…", cx, move |this, window, cx| {
                                    this.delete_template(delete_dir.clone(), delete_name.clone(), window, cx);
                                }))
                        }),
                )
        });
        div().flex().flex_col().children(rows).into_any_element()
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
                    .p(px(16.))
                    .rounded(px(6.))
                    .bg(theme::color(theme::palette().panel))
                    .border_1()
                    .border_color(theme::color(theme::palette().divider))
                    .shadow_lg()
                    .child(div().font_weight(FontWeight::SEMIBOLD).child(title))
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
        let list = match self.tab {
            HubTab::Projects => self.render_projects(cx),
            HubTab::Templates => self.render_templates(cx),
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
            .child(self.render_band(cx))
            .child(
                div().flex().flex_1().min_h_0().justify_center().child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(12.))
                        .w_full()
                        .max_w(px(CONTENT_WIDTH))
                        .h_full()
                        .px(px(24.))
                        .pt(px(20.))
                        .child(self.render_toolbar(cx))
                        .child(self.render_search())
                        .when_some(self.message.clone(), |element, message| {
                            element.child(div().text_color(theme::color(theme::palette().ui_muted)).child(message))
                        })
                        .child(div().id("hub-list").flex_1().min_h_0().overflow_y_scroll().pb(px(20.)).child(list)),
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
        .px(px(10.))
        .py(px(4.))
        .rounded(px(4.))
        .cursor_pointer()
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
