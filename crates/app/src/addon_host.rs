//! What add-ons can ask of the editor beyond reading and editing files: the
//! project, the build log, the PDF, the bibliography, text at the cursor and
//! files from outside. And the events the editor sends to add-ons that want
//! them. Nothing here runs for an add-on that did not ask.

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    time::Duration,
};

use addon_api::{
    BibInfo, CommandMenu, DiffBase, EventKind, FileOutline, GitCommitInfo, GitFileInfo, GitStatusInfo, HostMessage, LineRange, OpenDocument, OutlineEntry,
    PackageHintInfo, PdfOutlineEntry, PdfRect, ProjectInfo, Reply, Request, SelectionInfo, SourcePlace, TextPoint,
};
use editor_core::{HeadingKind, Language, SymbolKind};
use build::{Engine, Fix, Severity};
use gpui::{AppContext, Context, Entity, Pixels, Point, Window};
use pdf::{PdfEvent, PdfWorker};

use super::{LeftPanel, Menu, MenuEntry, Workspace, entry, menu_separator};
use crate::{
    actions::{AddonBinding, Copy, Cut, Paste, RunAddonCommand},
    addon_panel::AddonPanel,
    addon_reports, addons, bib,
    editor_view::{DragContent, DragItem, EditorView, is_save_conflict},
    file_tree, git_panel, outline, package_check, snippets, toolchain,
};

/// The cursor has to rest this long before add-ons hear where it is.
const CONTEXT_DELAY: Duration = Duration::from_millis(250);
const BUILD_LOG_BYTES: usize = 64 * 1024;
const PDF_PAGE_WIDTH: u32 = 1200;
const PDF_PAGE_WIDTH_MAX: u32 = 3000;
/// Typing has to pause this long before add-ons hear that a document changed.
const DOCUMENT_DELAY: Duration = Duration::from_millis(500);
const GIT_LOG_COMMITS: usize = 50;
const GIT_DIFF_BYTES: usize = 256 * 1024;
const PDF_HITS: usize = 200;

/// Where the user is, as add-ons hear it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct AddonContext {
    path: Option<String>,
    line: usize,
    column: usize,
    selected_chars: usize,
}

impl Workspace {
    /// The keys of the commands of the add-ons that are on.
    pub(super) fn addon_bindings(&self, cx: &gpui::App) -> Vec<AddonBinding> {
        self.addon_panels
            .iter()
            .flat_map(|entry| {
                let manifest = &entry.panel.read(cx).add_on.manifest;
                manifest.commands.iter().map(|command| AddonBinding { addon: manifest.id.clone(), command: command.id.clone(), keys: command.keys.clone() })
            })
            .collect()
    }

    /// Shows the add-on's panel and tells it to run the command, starting it
    /// first when it does not run.
    pub(super) fn run_addon_command(&mut self, action: &RunAddonCommand, cx: &mut Context<Self>) {
        let Some(index) = self.addon_panels.iter().position(|entry| entry.panel.read(cx).add_on.manifest.id == action.addon) else {
            return;
        };
        let context = self.active_file_info(cx);
        self.left_panel = LeftPanel::Addon(index);
        self.show_file_panel = true;
        let panel = self.addon_panels[index].panel.clone();
        panel.update(cx, |panel, cx| panel.run_command(&action.command, context, cx));
        cx.notify();
    }

    /// The right-click menu of the editor.
    pub(super) fn open_editor_menu(&mut self, position: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        self.open_menu = Some(Menu::EditorContext);
        self.menu_origin = position;
        window.focus(&self.menu_focus, cx);
        cx.notify();
    }

    pub(super) fn editor_menu_entries(&self, cx: &gpui::App) -> Vec<MenuEntry> {
        let clipboard = |action: fn() -> Box<dyn gpui::Action>| {
            move |this: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>| {
                this.focus_active(window, cx);
                window.dispatch_action(action(), cx);
            }
        };
        let mut entries = vec![
            entry("Cut", self.shortcut("cut"), clipboard(|| Box::new(Cut))),
            entry("Copy", self.shortcut("copy"), clipboard(|| Box::new(Copy))),
            entry("Paste", self.shortcut("paste"), clipboard(|| Box::new(Paste))),
        ];
        let commands: Vec<MenuEntry> = self
            .addon_panels
            .iter()
            .flat_map(|panel| {
                let manifest = &panel.panel.read(cx).add_on.manifest;
                manifest.commands.iter().filter(|command| command.menus.contains(&CommandMenu::Editor)).map(|command| {
                    let action = RunAddonCommand { addon: manifest.id.clone(), command: command.id.clone() };
                    let keys = command.keys.first().map(|keys| crate::keymap::display(keys, crate::keymap::host_os())).unwrap_or_default();
                    entry(&command.label, keys, move |this, _, cx| this.run_addon_command(&action, cx))
                })
            })
            .collect();
        if !commands.is_empty() {
            entries.push(menu_separator());
            entries.extend(commands);
        }
        entries
    }

    /// Places what was dropped into a document at its cursor: text as it is, a
    /// picture as a figure, copied into the project when it lies outside.
    pub(super) fn drop_into(&mut self, view: &Entity<EditorView>, item: DragItem, cx: &mut Context<Self>) {
        self.active = self.documents.iter().position(|document| &document.view == view);
        match item.content {
            DragContent::Text(text) => view.update(cx, |view, cx| view.insert_text(&text, cx)),
            DragContent::File(path) => {
                let language = view.read(cx).state.language();
                if !snippets::is_figure_file(language, &path) {
                    return self.set_status("Only pictures can be dropped into a document.", cx);
                }
                let path = match self.copy_into_figures(&path) {
                    Ok(path) => path,
                    Err(error) => return self.set_status(error, cx),
                };
                // A figure starts a line of its own.
                if view.read(cx).cursor_position().1 > 1 {
                    view.update(cx, |view, cx| view.insert_text("\n", cx));
                }
                self.insert_figure(&path, cx);
                self.refresh_from_disk(cx);
            }
        }
        if let Some((addon, node)) = item.origin {
            let view = view.read(cx);
            let path = view.path.as_deref().map(|path| self.relative_to_project(path)).unwrap_or_default();
            let line = view.cursor_position().0;
            if let Some(entry) = self.addon_panels.iter().find(|entry| entry.panel.read(cx).add_on.manifest.id == addon) {
                entry.panel.read(cx).send_event(HostMessage::Dropped { id: node, path, line });
            }
        }
        cx.notify();
    }

    /// A picture inside the project stays where it is; one from elsewhere is
    /// copied into its `figures` folder.
    fn copy_into_figures(&self, path: &Path) -> Result<PathBuf, String> {
        let root = self.root.as_deref().ok_or("Open a project first.")?;
        if path.starts_with(root) {
            return Ok(path.to_path_buf());
        }
        let name = path.file_name().ok_or("The file has no name.")?;
        let target = free_name(&root.join("figures").join(name));
        std::fs::create_dir_all(root.join("figures")).map_err(|error| format!("Cannot make the figures folder: {error}"))?;
        std::fs::copy(path, &target).map_err(|error| format!("Cannot copy {}: {error}", path.display()))?;
        Ok(target)
    }

    fn panels_wanting(&self, kind: EventKind, cx: &gpui::App) -> Vec<Entity<AddonPanel>> {
        self.addon_panels.iter().filter(|entry| entry.panel.read(cx).wants(kind)).map(|entry| entry.panel.clone()).collect()
    }

    fn addon_context(&self, cx: &gpui::App) -> AddonContext {
        let Some(view) = self.active_view() else {
            return AddonContext::default();
        };
        let view = view.read(cx);
        let (line, column) = view.cursor_position();
        let selected = view.state.selections().primary().range();
        AddonContext {
            path: view.path.as_deref().map(|path| self.relative_to_project(path)),
            line,
            column,
            selected_chars: view.state.buffer().slice(selected).chars().count(),
        }
    }

    /// Called when the cursor or the active document may have changed. Waits
    /// until the cursor rests, and only while some add-on listens.
    pub(super) fn addon_context_changed(&mut self, cx: &mut Context<Self>) {
        let wanted = self.addon_panels.iter().any(|entry| {
            let panel = entry.panel.read(cx);
            panel.wants(EventKind::ActiveFile) || panel.wants(EventKind::Selection)
        });
        if !wanted {
            return;
        }
        let context = self.addon_context(cx);
        if self.addon_context_pending.as_ref() == Some(&context) {
            return;
        }
        self.addon_context_pending = Some(context);
        self.addon_context_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(CONTEXT_DELAY).await;
            this.update(cx, |this, cx| this.send_addon_context(cx)).ok();
        }));
    }

    fn send_addon_context(&mut self, cx: &mut Context<Self>) {
        let context = self.addon_context(cx);
        let previous = std::mem::replace(&mut self.addon_context_sent, context.clone());
        if previous.path != context.path {
            for panel in self.panels_wanting(EventKind::ActiveFile, cx) {
                panel.read(cx).send_event(HostMessage::ActiveFileChanged { path: context.path.clone() });
            }
        }
        if let Some(path) = context.path.clone().filter(|_| previous != context) {
            for panel in self.panels_wanting(EventKind::Selection, cx) {
                panel.read(cx).send_event(HostMessage::SelectionChanged {
                    path: path.clone(),
                    line: context.line,
                    column: context.column,
                    selected_chars: context.selected_chars,
                });
            }
        }
    }

    pub(super) fn addon_build_finished(&self, success: bool, cx: &mut Context<Self>) {
        let errors = self.diagnostics.iter().filter(|diagnostic| diagnostic.severity == Severity::Error).count();
        let warnings = self.diagnostics.len() - errors;
        for panel in self.panels_wanting(EventKind::Build, cx) {
            panel.read(cx).send_event(HostMessage::BuildFinished { success, errors, warnings });
        }
    }

    pub(super) fn addon_project_info(&self, cx: &gpui::App) -> Result<Reply, String> {
        let root = self.root.clone().ok_or("No project is open.")?;
        let active = self.active_view().and_then(|view| view.read(cx).path.clone());
        let main = build::find_main(&root, &self.config, active.as_deref());
        let engine = main.as_deref().and_then(|main| self.config.engine.or_else(|| build::Engine::for_path(main)));
        let language = main.as_deref().and_then(Path::extension).and_then(|extension| match extension.to_str()? {
            "tex" | "latex" => Some("latex"),
            "typ" => Some("typst"),
            "md" | "markdown" => Some("markdown"),
            _ => None,
        });
        let compiler_ready = match language {
            Some("latex") => toolchain::language_ready(toolchain::Tool::Tectonic),
            Some("typst") => toolchain::language_ready(toolchain::Tool::Typst),
            Some("markdown") => toolchain::language_ready(toolchain::Tool::Pandoc),
            _ => false,
        };
        Ok(Reply::Project {
            project: ProjectInfo {
                root: root.display().to_string(),
                main: main.as_deref().map(|main| self.relative_to_project(main)),
                language: language.map(str::to_string),
                compiler: engine.map(|engine| engine.label().to_string()),
                compiler_ready,
                engine: engine.map(|engine| engine_name(engine).to_string()),
                building: self.building,
            },
        })
    }

    pub(super) fn addon_build_log(&self, max_bytes: Option<usize>) -> Reply {
        let limit = max_bytes.unwrap_or(BUILD_LOG_BYTES).max(1);
        let log = &self.build_log;
        let start = log.ceil_char_boundary(log.len().saturating_sub(limit));
        Reply::Text { text: log[start..].to_string() }
    }

    pub(super) fn addon_pdf_info(&self, cx: &gpui::App) -> Result<Reply, String> {
        let pdf = self.pdf.read(cx);
        if !pdf.has_document() {
            return Err("There is no PDF yet. Compile first.".to_string());
        }
        Ok(Reply::Pdf { pages: pdf.page_count(), page: pdf.current_page() + 1 })
    }

    /// Opens the file, which need not be open yet, and returns its view.
    fn addon_document(&mut self, relative: &str, cx: &mut Context<Self>) -> Result<Entity<EditorView>, String> {
        let path = self.addon_file(relative)?;
        let index = match self.document_at(&path, cx) {
            Some(index) => index,
            None => {
                let text = addons::read_file_limited(&path)?;
                self.add_document(path, &text, cx);
                self.documents.len() - 1
            }
        };
        Ok(self.documents[index].view.clone())
    }

    pub(super) fn addon_reveal(&mut self, relative: &str, line: Option<usize>, column: Option<usize>, cx: &mut Context<Self>) -> Result<Reply, String> {
        let view = self.addon_document(relative, cx)?;
        self.active = self.documents.iter().position(|document| document.view == view);
        self.show_hub = false;
        if let Some(line) = line {
            view.update(cx, |view, cx| view.place_cursor(line.saturating_sub(1), column.unwrap_or(1).saturating_sub(1), cx));
        }
        // The focus needs the window, which the next frame has.
        self.focus_requested = true;
        cx.notify();
        Ok(Reply::Done)
    }

    pub(super) fn addon_insert_text(&mut self, text: &str, cx: &mut Context<Self>) -> Result<Reply, String> {
        let view = self.active_view().ok_or("No document is open.")?;
        view.update(cx, |view, cx| view.insert_text(text, cx));
        Ok(Reply::Done)
    }

    pub(super) fn addon_insert_at(&mut self, relative: &str, line: usize, column: usize, text: &str, cx: &mut Context<Self>) -> Result<Reply, String> {
        let view = self.addon_document(relative, cx)?;
        view.update(cx, |view, cx| {
            let buffer = view.state.buffer();
            let line = line.saturating_sub(1);
            // Columns count characters, as people see them.
            let line_text = buffer.line_text(line.min(buffer.line_count().saturating_sub(1)));
            let bytes = line_text.char_indices().nth(column.saturating_sub(1)).map_or(line_text.trim_end_matches(['\n', '\r']).len(), |(at, _)| at);
            let offset = buffer.point_to_offset(line, bytes);
            view.edit_from_outside(offset..offset, text, cx);
        });
        cx.notify();
        Ok(Reply::Done)
    }

    pub(super) fn answer_copy_into_project(&self, panel: Entity<AddonPanel>, id: u64, source: String, dest: String, cx: &mut Context<Self>) {
        let target = self.addon_file(&dest);
        let root = self.root.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let target = free_name(&target?);
                    let source = PathBuf::from(&source);
                    if !source.is_file() {
                        return Err(format!("{} is not a file.", source.display()));
                    }
                    if let Some(parent) = target.parent() {
                        std::fs::create_dir_all(parent).map_err(|error| format!("Cannot make {}: {error}", parent.display()))?;
                    }
                    std::fs::copy(&source, &target).map_err(|error| format!("Cannot copy {}: {error}", source.display()))?;
                    let root = root.ok_or("No project is open.")?;
                    Ok(target.strip_prefix(&root).unwrap_or(&target).to_string_lossy().into_owned())
                })
                .await;
            this.update(cx, |this, cx| this.refresh_from_disk(cx)).ok();
            panel.update(cx, |panel, _| panel.reply(id, result.map(|path| Reply::File { path })));
        })
        .detach();
    }

    /// The map of the project. Open documents count with their unsaved text;
    /// the rest is read in the background.
    pub(super) fn answer_outline(&self, panel: Entity<AddonPanel>, id: u64, cx: &mut Context<Self>) {
        let root = self.root.clone();
        let open: Vec<(PathBuf, String)> =
            self.documents.iter().filter_map(|document| Some((document.view.read(cx).path.clone()?, document.view.read(cx).state.buffer().to_string()))).collect();
        let active = self.active_view().and_then(|view| view.read(cx).path.clone());
        let main = root.as_deref().and_then(|root| build::find_main(root, &self.config, active.as_deref()));
        self.answer_in_background(panel, id, cx, move || {
            let root = root.ok_or("No project is open.")?;
            let mut files: Vec<FileOutline> = file_tree::scan(&root, &HashSet::new())
                .into_iter()
                .filter(|entry| !entry.is_dir && is_source(&entry.path))
                .filter_map(|entry| {
                    let text = match open.iter().find(|(path, _)| *path == entry.path) {
                        Some((_, text)) => text.clone(),
                        None => addons::read_file_limited(&entry.path).ok()?,
                    };
                    let relative = entry.path.strip_prefix(&root).unwrap_or(&entry.path).to_string_lossy().into_owned();
                    Some(file_outline(&entry.path, relative, &text, &root))
                })
                .collect();
            let main = main.as_deref().map(|main| main.strip_prefix(&root).unwrap_or(main).to_string_lossy().into_owned());
            files.sort_by_key(|file| (Some(&file.path) != main.as_ref(), file.path.clone()));
            Ok(Reply::Outline { main, files })
        });
    }

    pub(super) fn answer_bib_entries(&self, panel: Entity<AddonPanel>, id: u64, cx: &mut Context<Self>) {
        let root = self.root.clone();
        self.answer_in_background(panel, id, cx, move || {
            let root = root.ok_or("No project is open.")?;
            let mut entries = Vec::new();
            for file in file_tree::scan(&root, &HashSet::new()).into_iter().filter(|entry| !entry.is_dir && entry.path.extension().is_some_and(|extension| extension == "bib")) {
                let Ok(text) = std::fs::read_to_string(&file.path) else {
                    continue;
                };
                let relative = file.path.strip_prefix(&root).unwrap_or(&file.path).to_string_lossy().into_owned();
                entries.extend(bib::parse(&text).into_iter().map(|entry| BibInfo {
                    key: entry.key,
                    authors: entry.authors,
                    year: entry.year,
                    title: entry.title,
                    venue: entry.venue,
                    file: relative.clone(),
                    entry_type: entry.kind,
                    doi: entry.doi,
                    url: entry.url,
                    raw: entry.raw,
                }));
            }
            Ok(Reply::Bib { entries })
        });
    }

    /// The text or a picture of a page, from a PDFium worker of its own, so
    /// that the PDF view keeps its place and pages.
    pub(super) fn answer_from_pdf(&self, panel: Entity<AddonPanel>, id: u64, page: Option<usize>, image_width: Option<u32>, cx: &mut Context<Self>) {
        let pdf = self.last_pdf.clone();
        let current = self.pdf.read(cx).current_page();
        cx.spawn(async move |_, cx| {
            let result = async {
                let pdf = pdf.ok_or("There is no PDF yet. Compile first.")?;
                let (worker, mut events) = cx.background_spawn(async move { PdfWorker::spawn(&crate::pdf_view::library_directory()) }).await?;
                worker.open(0, pdf);
                let pages = loop {
                    match events.recv().await {
                        Some(PdfEvent::Loaded { pages, .. }) => break pages.len(),
                        Some(PdfEvent::Failed { message, .. }) => return Err(message),
                        Some(_) => {}
                        None => return Err("The PDF could not be read.".to_string()),
                    }
                };
                let index = page.map_or(current, |page| page.saturating_sub(1));
                if index >= pages {
                    return Err(if pages == 1 { "The PDF has 1 page.".to_string() } else { format!("The PDF has {pages} pages.") });
                }
                match image_width {
                    None => {
                        worker.content(0, index);
                        loop {
                            match events.recv().await {
                                Some(PdfEvent::PageContent { content, .. }) => break Ok(Reply::Text { text: content.chars.iter().map(|char| char.ch).collect() }),
                                Some(PdfEvent::Failed { message, .. }) => return Err(message),
                                Some(_) => {}
                                None => return Err("The PDF could not be read.".to_string()),
                            }
                        }
                    }
                    Some(width) => {
                        worker.render(0, vec![pdf::PageRequest { page: index, width_px: width, recolor: None }]);
                        let (height, bgra) = loop {
                            match events.recv().await {
                                Some(PdfEvent::Rendered { height_px, bgra, .. }) => break (height_px, bgra),
                                Some(PdfEvent::Failed { message, .. }) => return Err(message),
                                Some(_) => {}
                                None => return Err("The PDF could not be drawn.".to_string()),
                            }
                        };
                        let path = cx.background_spawn(async move { save_png(width, height, bgra, index + 1) }).await?;
                        Ok(Reply::File { path: path.display().to_string() })
                    }
                }
            }
            .await;
            panel.update(cx, |panel, _| panel.reply(id, result));
        })
        .detach();
    }
}

impl Workspace {
    pub(super) fn addon_build_started(&self, cx: &mut Context<Self>) {
        for panel in self.panels_wanting(EventKind::BuildStart, cx) {
            panel.read(cx).send_event(HostMessage::BuildStarted);
        }
    }

    pub(super) fn addon_diagnostics_changed(&self, cx: &mut Context<Self>) {
        let errors = self.diagnostics.iter().filter(|diagnostic| diagnostic.severity == Severity::Error).count();
        let warnings = self.diagnostics.len() - errors;
        for panel in self.panels_wanting(EventKind::Diagnostics, cx) {
            panel.read(cx).send_event(HostMessage::DiagnosticsChanged { errors, warnings });
        }
    }

    pub(super) fn addon_project_closed(&self, cx: &mut Context<Self>) {
        for panel in self.panels_wanting(EventKind::Project, cx) {
            panel.read(cx).send_event(HostMessage::ProjectClosed);
        }
    }

    /// Notes the change and tells the add-ons that listen once typing pauses.
    pub(super) fn addon_document_changed(&mut self, view: &Entity<EditorView>, cx: &mut Context<Self>) {
        if !self.addon_panels.iter().any(|entry| entry.panel.read(cx).wants(EventKind::Document)) {
            return;
        }
        let Some(path) = view.read(cx).path.clone() else {
            return;
        };
        self.addon_changed.insert(path);
        self.addon_changed_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(DOCUMENT_DELAY).await;
            this.update(cx, |this, cx| {
                let changed = std::mem::take(&mut this.addon_changed);
                for panel in this.panels_wanting(EventKind::Document, cx) {
                    for path in &changed {
                        panel.read(cx).send_event(HostMessage::DocumentChanged { path: this.relative_to_project(path) });
                    }
                }
            })
            .ok();
        }));
    }

    /// The paths and texts of the open documents, for work in the background.
    fn addon_open_texts(&self, cx: &gpui::App) -> Vec<(PathBuf, String)> {
        self.documents
            .iter()
            .filter_map(|document| {
                let view = document.view.read(cx);
                Some((view.path.clone()?, view.state.buffer().to_string()))
            })
            .collect()
    }

    pub(super) fn addon_open_documents(&self, cx: &gpui::App) -> Reply {
        let documents = self
            .documents
            .iter()
            .enumerate()
            .filter_map(|(index, document)| {
                let view = document.view.read(cx);
                Some(OpenDocument { path: self.relative_to_project(view.path.as_deref()?), unsaved: view.state.is_dirty(), active: self.active == Some(index) })
            })
            .collect();
        Reply::Documents { documents }
    }

    pub(super) fn addon_save(&mut self, relative: &str, cx: &mut Context<Self>) -> Result<Reply, String> {
        let path = self.addon_file(relative)?;
        let index = self.document_at(&path, cx).ok_or_else(|| format!("{relative} is not open, so it has nothing to save."))?;
        let view = self.documents[index].view.clone();
        let result = match view.update(cx, |view, cx| view.save(cx)) {
            Ok(saved) => {
                if saved {
                    self.notify_addons_saved(&view, cx);
                    self.reload_key_files(&view, cx);
                }
                Ok(Reply::Done)
            }
            Err(error) if is_save_conflict(&error) => Err(format!("{relative} changed on disk and was not saved. The user has to decide which to keep.")),
            Err(error) => Err(format!("Cannot save {relative}: {error}")),
        };
        self.forget_saved_recovery(cx);
        result
    }

    /// Moves the entry on disk, then the open documents, the tracked edits and
    /// the main document follow it, as after a rename in the file tree.
    pub(super) fn addon_rename(&mut self, from: &str, to: &str, cx: &mut Context<Self>) -> Result<Reply, String> {
        let source = self.addon_file(from)?;
        let target = self.addon_file(to)?;
        if self.root.as_deref() == Some(source.as_path()) {
            return Err("The project itself cannot be renamed.".to_string());
        }
        if !source.exists() {
            return Err(format!("{from} does not exist."));
        }
        let same = source.to_string_lossy().eq_ignore_ascii_case(&target.to_string_lossy());
        if target.exists() && !same {
            return Err(format!("{to} already exists."));
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|error| format!("Cannot make {}: {error}", parent.display()))?;
        }
        std::fs::rename(&source, &target).map_err(|error| format!("Cannot rename {from}: {error}"))?;
        for tracked in self.tracked_edits.values_mut() {
            tracked.originals = std::mem::take(&mut tracked.originals)
                .into_iter()
                .map(|(path, text)| (file_tree::moved_path(&path, &source, &target).unwrap_or(path), text))
                .collect();
            for path in &mut tracked.created {
                if let Some(moved) = file_tree::moved_path(path, &source, &target) {
                    *path = moved;
                }
            }
        }
        self.follow_rename(&source, &target, cx);
        Ok(Reply::File { path: self.relative_to_project(&target) })
    }

    /// Moves the entry to the Trash without asking, which the permission to
    /// change documents allows, and closes its documents.
    pub(super) fn answer_delete(&mut self, panel: Entity<AddonPanel>, id: u64, relative: &str, cx: &mut Context<Self>) {
        let checked = self.addon_file(relative).and_then(|path| {
            if self.root.as_deref() == Some(path.as_path()) {
                return Err("The project itself cannot be deleted.".to_string());
            }
            if !path.exists() {
                return Err(format!("{relative} does not exist."));
            }
            let unsaved = self.documents.iter().any(|document| {
                let view = document.view.read(cx);
                view.state.is_dirty() && view.path.as_deref().is_some_and(|open| open.starts_with(&path))
            });
            if unsaved {
                return Err(format!("{relative} has unsaved changes."));
            }
            Ok(path)
        });
        let path = match checked {
            Ok(path) => path,
            Err(error) => return panel.update(cx, |panel, _| panel.reply(id, Err(error))),
        };
        let name = path.file_name().map_or_else(String::new, |name| name.to_string_lossy().into_owned());
        cx.spawn(async move |this, cx| {
            let target = path.clone();
            let trashed = cx
                .background_spawn(async move {
                    // The same copy the file tree keeps, since an add-on deletes without asking.
                    if std::fs::metadata(&target).is_ok_and(|metadata| metadata.is_file() && metadata.len() <= crate::text_file::HISTORY_LIMIT)
                        && let Some(directory) = crate::history::directory()
                    {
                        crate::history::keep_disk(&directory, &target).ok();
                    }
                    trash::delete(&target)
                })
                .await;
            let result = this
                .update(cx, |this, cx| match trashed {
                    Err(error) => Err(format!("Cannot move {name} to the Trash: {error}")),
                    Ok(()) => {
                        this.close_everything_in(&path, cx);
                        this.set_status(format!("Moved {name} to the Trash"), cx);
                        Ok(Reply::Done)
                    }
                })
                .unwrap_or_else(|_| Err("The editor closed.".to_string()));
            panel.update(cx, |panel, _| panel.reply(id, result));
        })
        .detach();
    }

    /// Closes the documents and pictures of a path that went away, without a
    /// window: the focus follows with the next frame.
    fn close_everything_in(&mut self, path: &Path, cx: &mut Context<Self>) {
        while let Some(index) = self.documents.iter().rposition(|document| document.view.read(cx).path.as_deref().is_some_and(|open| open.starts_with(path))) {
            self.documents.remove(index);
            self.move_images_for_removed_document(index);
            self.active = match self.active {
                Some(active) if active == index => self.documents.len().checked_sub(1).map(|last| index.min(last)),
                Some(active) if active > index => Some(active - 1),
                other => other,
            };
        }
        while let Some(index) = self.images.iter().rposition(|image| image.view.read(cx).path().starts_with(path)) {
            self.images.remove(index);
            self.shown_image = match self.shown_image {
                Some(shown) if shown == index => None,
                Some(shown) if shown > index => Some(shown - 1),
                other => other,
            };
        }
        self.collapsed.retain(|folder| !folder.starts_with(path));
        self.refresh_from_disk(cx);
        self.tree_cursor = self.tree_cursor.min(self.entries.len().saturating_sub(1));
        self.focus_requested = true;
        cx.notify();
    }

    pub(super) fn addon_selection(&self, cx: &gpui::App) -> Reply {
        let selection = self.active_view().and_then(|view| {
            let view = view.read(cx);
            let path = view.path.as_deref()?;
            let buffer = view.state.buffer();
            let range = view.state.selections().primary().range();
            let point = |offset: usize| {
                let (line, byte) = buffer.offset_to_point(offset);
                TextPoint { line: line + 1, column: buffer.line_text(line).get(..byte).map_or(byte, |text| text.chars().count()) + 1 }
            };
            let (line, column) = view.cursor_position();
            Some(SelectionInfo {
                path: self.relative_to_project(path),
                cursor: TextPoint { line, column },
                start: point(range.start),
                end: point(range.end),
                text: buffer.slice(range),
                visible: view.visible_lines().map(|lines| LineRange { first: lines.start + 1, last: lines.end }),
            })
        });
        Reply::Selection { selection }
    }

    pub(super) fn addon_source_to_pdf(&self, relative: &str, line: usize) -> Result<Reply, String> {
        if let Some(reason) = self.synctex_unavailable_reason() {
            return Err(reason);
        }
        let path = self.addon_file(relative)?;
        let synctex = self.synctex.as_ref().ok_or("PDF navigation needs a finished LaTeX build.")?;
        let location = synctex
            .forward(&path, line.max(1))
            .or_else(|| synctex.forward(&path.canonicalize().map(crate::platform::plain_path).ok()?, line.max(1)))
            .ok_or("This line has no position in the PDF.")?;
        Ok(Reply::PdfPlace {
            page: location.page + 1,
            rect: PdfRect { left: location.x, top: location.y, right: location.x + location.width, bottom: location.y + location.height },
        })
    }

    pub(super) fn addon_pdf_to_source(&self, page: usize, x: f32, y: f32) -> Result<Reply, String> {
        if let Some(reason) = self.synctex_unavailable_reason() {
            return Err(reason);
        }
        let location = self
            .synctex
            .as_ref()
            .and_then(|synctex| synctex.backward(page.saturating_sub(1), x, y))
            .ok_or("This position has no source line.")?;
        Ok(Reply::SourcePlace { place: SourcePlace { path: self.relative_to_project(&location.file), line: location.line } })
    }

    /// Applies a fix by its name. A package hint knows its file; anything else
    /// goes to the main file, as the fix in the error panel does.
    pub(super) fn addon_apply_fix(&mut self, id: &str, cx: &mut Context<Self>) -> Result<Reply, String> {
        let fix = match id.split_once(':') {
            Some(("add_package", package)) => Fix::AddPackage(package.to_string()),
            Some(("remove_package", package)) => Fix::RemovePackage(package.to_string()),
            Some(("install_tool", _)) => return Err("Installing a compiler needs the user. Ask them to use the fix in the editor.".to_string()),
            Some(("install_latex_package", _)) => return Err("Installing a LaTeX package needs the Mac password. Ask the user to use the fix in the editor.".to_string()),
            None if id == "update_latex" => return Err("Updating LaTeX needs the Mac password. Ask the user to use the fix in the editor.".to_string()),
            _ => return Err(format!("No fix is called {id}.")),
        };
        let path = self.package_hints.iter().find(|hint| hint.fix == fix).map(|hint| hint.path.clone());
        let target = path.clone().or_else(|| self.last_main.clone()).ok_or("Compile once, so that the main file is known.")?;
        let text = match self.document_at(&target, cx) {
            Some(index) => self.documents[index].view.read(cx).state.buffer().to_string(),
            None => addons::read_file_limited(&target)?,
        };
        if package_check::edit_for(&fix, &text).is_none() {
            return Err(format!("Cannot apply this: {}.", fix.label()));
        }
        self.apply_fix(&fix, path.as_deref(), cx);
        Ok(Reply::Done)
    }

    pub(super) fn answer_from_git(&self, panel: Entity<AddonPanel>, id: u64, request: Request, cx: &mut Context<Self>) {
        let root = self.root.clone();
        let open = self.addon_open_texts(cx);
        let dirty: Vec<PathBuf> =
            self.documents.iter().filter_map(|document| document.view.read(cx).state.is_dirty().then(|| document.view.read(cx).path.clone()).flatten()).collect();
        self.answer_in_background(panel, id, cx, move || {
            let root = root.ok_or("No project is open.")?;
            // Unsaved changes need no repository.
            if let Request::GitDiff { path, against: DiffBase::Working } = &request {
                let files = match path {
                    Some(path) => vec![addons::resolve_path(&root, path)?],
                    None => dirty,
                };
                let diff = files
                    .iter()
                    .map(|file| {
                        let relative = file.strip_prefix(&root).unwrap_or(file).to_string_lossy().into_owned();
                        let saved = std::fs::read_to_string(file).unwrap_or_default();
                        let current = read_with_open(file, &open).unwrap_or_default();
                        addon_reports::unified_diff(&relative, &saved, &current)
                    })
                    .collect::<String>();
                return Ok(Reply::Text { text: limit_diff(diff) });
            }
            let program = git_panel::program().ok_or("Git is not installed.")?;
            let repository = program.discover(&root)?.ok_or("The project is not in a Git repository.")?;
            match request {
                Request::GitLog { max } => {
                    let commits = repository
                        .log(0)?
                        .into_iter()
                        .take(max.unwrap_or(GIT_LOG_COMMITS))
                        .map(|commit| GitCommitInfo { hash: commit.hash, author: commit.author, age: commit.age, refs: commit.refs, subject: commit.subject })
                        .collect();
                    Ok(Reply::Commits { commits })
                }
                Request::GitDiff { path, .. } => {
                    let top = repository.root().canonicalize().map(crate::platform::plain_path).unwrap_or_else(|_| repository.root().to_path_buf());
                    let files: Vec<String> = match path {
                        Some(path) => {
                            let file = addons::resolve_path(&root, &path)?;
                            let real = file.canonicalize().map(crate::platform::plain_path).unwrap_or(file);
                            vec![real.strip_prefix(&top).map_err(|_| format!("{path} is not in the repository."))?.to_string_lossy().into_owned()]
                        }
                        None => repository.status()?.files.into_iter().map(|file| file.path).collect(),
                    };
                    let open: Vec<(PathBuf, String)> = open.into_iter().map(|(path, text)| (path.canonicalize().map(crate::platform::plain_path).unwrap_or(path), text)).collect();
                    let diff = files
                        .iter()
                        .map(|file| {
                            let committed = repository.file_at_head(file).unwrap_or_default();
                            let current = read_with_open(&top.join(file), &open).unwrap_or_default();
                            addon_reports::unified_diff(file, &committed, &current)
                        })
                        .collect::<String>();
                    Ok(Reply::Text { text: limit_diff(diff) })
                }
                _ => {
                    let status = repository.status()?;
                    let letter = |letter: Option<char>| letter.map(String::from);
                    Ok(Reply::GitStatus {
                        status: GitStatusInfo {
                            branch: status.branch,
                            upstream: status.upstream,
                            ahead: status.ahead,
                            behind: status.behind,
                            files: status
                                .files
                                .into_iter()
                                .map(|file| GitFileInfo {
                                    path: file.path,
                                    old_path: file.old_path,
                                    staged: letter(file.staged),
                                    unstaged: letter(file.unstaged),
                                    conflicted: file.conflicted,
                                })
                                .collect(),
                        },
                    })
                }
            }
        });
    }

    /// Checks the packages of the whole project now, as after a build.
    pub(super) fn answer_package_hints(&self, panel: Entity<AddonPanel>, id: u64, cx: &mut Context<Self>) {
        let root = self.root.clone();
        let active = self.active_view().and_then(|view| view.read(cx).path.clone());
        let main = root.as_deref().and_then(|root| build::find_main(root, &self.config, active.as_deref()));
        let open = self.addon_open_texts(cx);
        self.answer_in_background(panel, id, cx, move || {
            let root = root.ok_or("No project is open.")?;
            let main = main.ok_or("The project has no main file.")?;
            if Language::from_path(&main) != Language::Latex {
                return Ok(Reply::PackageHints { hints: Vec::new() });
            }
            let mut sources: Vec<(PathBuf, String)> = Vec::new();
            let mut has_own_style_files = false;
            for entry in file_tree::scan(&root, &HashSet::new()).into_iter().filter(|entry| !entry.is_dir) {
                let extension = entry.path.extension().and_then(|extension| extension.to_str());
                has_own_style_files |= matches!(extension, Some("sty" | "cls"));
                if !matches!(extension, Some("tex" | "latex")) {
                    continue;
                }
                let too_big = std::fs::metadata(&entry.path).is_ok_and(|metadata| metadata.len() > super::PACKAGE_CHECK_FILE_LIMIT);
                if let Some(text) = open.iter().find(|(path, _)| *path == entry.path).map(|(_, text)| text.clone()).or_else(|| (!too_big).then(|| std::fs::read_to_string(&entry.path).ok()).flatten()) {
                    sources.push((entry.path, text));
                }
            }
            let report = package_check::hints(&main, &sources, has_own_style_files);
            let hints = report
                .hints
                .into_iter()
                .map(|hint| {
                    let text = sources.iter().find(|(path, _)| *path == hint.path).map_or("", |(_, text)| text.as_str());
                    let (line, column) = text_point(text, hint.range.start);
                    PackageHintInfo {
                        path: hint.path.strip_prefix(&root).unwrap_or(&hint.path).to_string_lossy().into_owned(),
                        line,
                        column,
                        message: hint.message,
                        fix: addon_reports::fix_info(&hint.fix),
                    }
                })
                .collect();
            Ok(Reply::PackageHints { hints })
        });
    }

    pub(super) fn answer_references(&self, panel: Entity<AddonPanel>, id: u64, cx: &mut Context<Self>) {
        let root = self.root.clone();
        let open = self.addon_open_texts(cx);
        self.answer_in_background(panel, id, cx, move || {
            let root = root.ok_or("No project is open.")?;
            let mut sources = Vec::new();
            let mut keys = Vec::new();
            for entry in file_tree::scan(&root, &HashSet::new()).into_iter().filter(|entry| !entry.is_dir) {
                let extension = entry.path.extension().and_then(|extension| extension.to_str());
                let relative = entry.path.strip_prefix(&root).unwrap_or(&entry.path).to_string_lossy().into_owned();
                match extension {
                    Some("tex" | "latex" | "typ") => sources.extend(read_with_open(&entry.path, &open).map(|text| (relative, text))),
                    Some("bib") => keys.extend(read_with_open(&entry.path, &open).into_iter().flat_map(|text| bib::parse(&text)).map(|entry| entry.key)),
                    _ => {}
                }
            }
            Ok(Reply::References { references: addon_reports::references(&sources, &keys) })
        });
    }

    /// The main document with what it includes, or the file asked for alone.
    pub(super) fn answer_word_count(&self, panel: Entity<AddonPanel>, id: u64, path: Option<String>, cx: &mut Context<Self>) {
        let root = self.root.clone();
        let active = self.active_view().and_then(|view| view.read(cx).path.clone());
        let main = match path {
            Some(path) => self.addon_file(&path).map(Some),
            None => Ok(root.as_deref().and_then(|root| build::find_main(root, &self.config, active.as_deref()))),
        };
        let open = self.addon_open_texts(cx);
        self.answer_in_background(panel, id, cx, move || {
            let root = root.ok_or("No project is open.")?;
            let main = main?.ok_or("This file type has no word count.")?;
            // LaTeX runs in the main file's folder, and Typst resolves `/` from the project.
            let base = if Language::from_path(&main) == Language::Typst { root.clone() } else { main.parent().unwrap_or(&root).to_path_buf() };
            let open: HashMap<PathBuf, String> = open.into_iter().filter_map(|(path, text)| Some((path.canonicalize().map(crate::platform::plain_path).ok()?, text))).collect();
            let mut read = |path: &Path| path.canonicalize().map(crate::platform::plain_path).ok().and_then(|path| open.get(&path).cloned().or_else(|| std::fs::read_to_string(&path).ok()));
            let text = read(&main).ok_or_else(|| format!("Cannot read {}", main.display()))?;
            let count = editor_core::word_count::count_words(&main, &text, &base, &mut read);
            let relative = main.strip_prefix(&root).unwrap_or(&main).to_string_lossy().into_owned();
            Ok(Reply::WordCount { count: addon_reports::word_count_info(relative, &count) })
        });
    }

    pub(super) fn answer_pdf_outline(&self, panel: Entity<AddonPanel>, id: u64, cx: &mut Context<Self>) {
        let pdf = self.last_pdf.clone();
        cx.spawn(async move |_, cx| {
            let result = async {
                let (worker, mut events, _) = open_pdf(pdf, cx).await?;
                worker.outline(0);
                loop {
                    match events.recv().await {
                        Some(PdfEvent::Outline { entries, .. }) => {
                            let entries = entries.into_iter().map(|entry| PdfOutlineEntry { title: entry.title, level: entry.level, page: entry.page + 1 }).collect();
                            break Ok(Reply::PdfOutline { entries });
                        }
                        Some(PdfEvent::Failed { message, .. }) => break Err(message),
                        Some(_) => {}
                        None => break Err("The PDF could not be read.".to_string()),
                    }
                }
            }
            .await;
            panel.update(cx, |panel, _| panel.reply(id, result));
        })
        .detach();
    }

    /// Reads the text of one page after another until enough hits are found.
    pub(super) fn answer_pdf_search(&self, panel: Entity<AddonPanel>, id: u64, query: String, cx: &mut Context<Self>) {
        let pdf = self.last_pdf.clone();
        cx.spawn(async move |_, cx| {
            let result = async {
                if query.trim().is_empty() {
                    return Err("Search for some text.".to_string());
                }
                let (worker, mut events, pages) = open_pdf(pdf, cx).await?;
                let mut hits = Vec::new();
                for page in 0..pages {
                    worker.content(0, page);
                    let content = loop {
                        match events.recv().await {
                            Some(PdfEvent::PageContent { content, .. }) => break content,
                            Some(PdfEvent::Failed { message, .. }) => return Err(message),
                            Some(_) => {}
                            None => return Err("The PDF could not be read.".to_string()),
                        }
                    };
                    hits.extend(addon_reports::page_hits(page + 1, &content, &query, PDF_HITS - hits.len()));
                    if hits.len() >= PDF_HITS {
                        break;
                    }
                }
                Ok(Reply::PdfHits { hits })
            }
            .await;
            panel.update(cx, |panel, _| panel.reply(id, result));
        })
        .detach();
    }
}

/// A diff of a whole project can be long; its start says enough.
fn limit_diff(mut diff: String) -> String {
    if diff.len() > GIT_DIFF_BYTES {
        diff.truncate(diff.floor_char_boundary(GIT_DIFF_BYTES));
        diff.push_str("\n… The diff goes on. Ask for one file to see all of it.\n");
    }
    diff
}

/// The tracked edits of a group of an add-on, the default group without a name.
pub(super) fn edit_group(add_on: &str, group: Option<String>) -> (String, String) {
    (add_on.to_string(), group.unwrap_or_default())
}

/// As `.editor.toml` writes it.
fn engine_name(engine: Engine) -> &'static str {
    match engine {
        Engine::Typst => "typst",
        Engine::Tectonic => "tectonic",
        Engine::PdfLatex => "pdflatex",
        Engine::LuaLatex => "lualatex",
        Engine::Pandoc => "pandoc",
    }
}

/// Opens the PDF in a PDFium worker of its own and waits until it is read.
async fn open_pdf(pdf: Option<PathBuf>, cx: &mut gpui::AsyncApp) -> Result<(PdfWorker, tokio::sync::mpsc::UnboundedReceiver<PdfEvent>, usize), String> {
    let pdf = pdf.ok_or("There is no PDF yet. Compile first.")?;
    let (worker, mut events) = cx.background_spawn(async move { PdfWorker::spawn(&crate::pdf_view::library_directory()) }).await?;
    worker.open(0, pdf);
    loop {
        match events.recv().await {
            Some(PdfEvent::Loaded { pages, .. }) => return Ok((worker, events, pages.len())),
            Some(PdfEvent::Failed { message, .. }) => return Err(message),
            Some(_) => {}
            None => return Err("The PDF could not be read.".to_string()),
        }
    }
}

/// Line and character column from 1 of a byte offset in `text`.
fn text_point(text: &str, offset: usize) -> (usize, usize) {
    let before = &text[..text.floor_char_boundary(offset.min(text.len()))];
    let line_start = before.rfind('\n').map_or(0, |newline| newline + 1);
    (before.matches('\n').count() + 1, before[line_start..].chars().count() + 1)
}

/// The text of each file, open documents with their unsaved changes.
fn read_with_open(path: &Path, open: &[(PathBuf, String)]) -> Option<String> {
    match open.iter().find(|(open, _)| open == path) {
        Some((_, text)) => Some(text.clone()),
        None => addons::read_file_limited(path).ok(),
    }
}

fn is_source(path: &Path) -> bool {
    path.extension().and_then(|extension| extension.to_str()).is_some_and(|extension| matches!(extension, "tex" | "latex" | "typ" | "md" | "markdown"))
}

fn file_outline(path: &Path, relative: String, text: &str, root: &Path) -> FileOutline {
    let language = Language::from_path(path);
    let (headings, symbols) = editor_core::outline_of(language, text);
    let entries = outline::outline(&headings, language)
        .into_iter()
        .map(|item| OutlineEntry {
            kind: match item.kind {
                HeadingKind::Section => "section",
                HeadingKind::Figure => "figure",
                HeadingKind::Table => "table",
                HeadingKind::References => "references",
            }
            .to_string(),
            level: if item.kind == HeadingKind::Section { item.level } else { 0 },
            title: item.title,
            line: item.line + 1,
        })
        .collect();
    let labels = symbols.into_iter().filter(|symbol| symbol.kind == SymbolKind::Label).map(|symbol| symbol.name).collect();
    // Prose words, as the word count of the editor counts them, where it can.
    let words = match language {
        Language::Latex => editor_core::word_count::count_words(path, text, root, &mut |_| None).total(),
        _ => text.split_whitespace().count(),
    };
    FileOutline { path: relative, words, lines: text.lines().count(), entries, labels }
}

pub(super) fn pdf_page_width(width: Option<u32>) -> u32 {
    width.unwrap_or(PDF_PAGE_WIDTH).clamp(100, PDF_PAGE_WIDTH_MAX)
}

fn save_png(width: u32, height: u32, mut bgra: Vec<u8>, page: usize) -> Result<PathBuf, String> {
    for pixel in bgra.as_chunks_mut::<4>().0 {
        pixel.swap(0, 2);
    }
    let image = image::RgbaImage::from_raw(width, height, bgra).ok_or("The page came out broken.")?;
    let path = addons::private_file(&format!("page-{page}"), "png")?;
    image.save(&path).map_err(|error| format!("Cannot save the page: {error}"))?;
    Ok(path)
}

/// `figure.png`, or `figure-2.png` and on when that exists.
fn free_name(path: &Path) -> PathBuf {
    if !path.exists() {
        return path.to_path_buf();
    }
    let stem = path.file_stem().map(|stem| stem.to_string_lossy().into_owned()).unwrap_or_default();
    let extension = path.extension().map(|extension| format!(".{}", extension.to_string_lossy())).unwrap_or_default();
    (2..)
        .map(|number| path.with_file_name(format!("{stem}-{number}{extension}")))
        .find(|candidate| !candidate.exists())
        .unwrap_or_else(|| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copies_get_a_free_name() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("figure.png");
        assert_eq!(free_name(&path), path);
        std::fs::write(&path, "").unwrap();
        assert_eq!(free_name(&path), directory.path().join("figure-2.png"));
        std::fs::write(directory.path().join("figure-2.png"), "").unwrap();
        assert_eq!(free_name(&path), directory.path().join("figure-3.png"));
    }

    #[test]
    fn the_outline_of_a_chapter() {
        let text = "\\chapter{Intro}\\label{ch:intro}\nSome words here.\n\\section{Why \\emph{this}}\nMore words.\n\\begin{figure}\\caption{A plot}\\label{fig:plot}\\end{figure}\n";
        let outline = file_outline(Path::new("/p/intro.tex"), "intro.tex".to_string(), text, Path::new("/p"));
        let titles: Vec<_> = outline.entries.iter().map(|entry| (entry.kind.as_str(), entry.title.as_str(), entry.line)).collect();
        assert_eq!(titles, [("section", "Intro", 1), ("section", "Why this", 3), ("figure", "A plot", 5)]);
        assert_eq!(outline.labels, ["ch:intro", "fig:plot"]);
        assert!(outline.words >= 5, "{}", outline.words);
    }

    /// A book of about 500 pages: how long the map takes and how big it is.
    /// cargo test -p app outline_of_a_book -- --ignored --nocapture
    #[test]
    #[ignore]
    fn outline_of_a_book() {
        let paragraph = "This sentence stands for the prose of a long book, with \\emph{some} markup and a citation \\cite{knuth84}. ".repeat(12);
        let mut chapters = Vec::new();
        for chapter in 0..30 {
            let mut text = format!("\\chapter{{Chapter {chapter}}}\\label{{ch:{chapter}}}\n");
            for section in 0..8 {
                text.push_str(&format!("\\section{{Section {chapter}.{section}}}\\label{{sec:{chapter}:{section}}}\n"));
                for _ in 0..6 {
                    text.push_str(&paragraph);
                    text.push_str("\n\n");
                }
                text.push_str(&format!("\\begin{{figure}}\\caption{{Figure in {chapter}.{section}}}\\label{{fig:{chapter}:{section}}}\\end{{figure}}\n"));
            }
            chapters.push(text);
        }
        let started = std::time::Instant::now();
        let files: Vec<FileOutline> = chapters.iter().enumerate().map(|(index, text)| file_outline(Path::new(&format!("/p/ch{index}.tex")), format!("ch{index}.tex"), text, Path::new("/p"))).collect();
        let elapsed = started.elapsed();
        let words: usize = files.iter().map(|file| file.words).sum();
        let json = serde_json::to_string(&Reply::Outline { main: Some("main.tex".into()), files }).unwrap();
        println!("{words} words, {} KB of source, map {} KB (about {} tokens), in {elapsed:?}", chapters.iter().map(String::len).sum::<usize>() / 1024, json.len() / 1024, json.len() / 4);
    }

    #[test]
    fn page_pictures_stay_in_a_sane_size() {
        assert_eq!(pdf_page_width(None), PDF_PAGE_WIDTH);
        assert_eq!(pdf_page_width(Some(10)), 100);
        assert_eq!(pdf_page_width(Some(99_999)), PDF_PAGE_WIDTH_MAX);
    }
}
