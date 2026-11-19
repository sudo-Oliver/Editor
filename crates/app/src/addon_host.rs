//! What add-ons can ask of the editor beyond reading and editing files: the
//! project, the build log, the PDF, the bibliography, text at the cursor and
//! files from outside. And the events the editor sends to add-ons that want
//! them. Nothing here runs for an add-on that did not ask.

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    time::Duration,
};

use addon_api::{BibInfo, EventKind, FileOutline, HostMessage, OutlineEntry, ProjectInfo, Reply};
use editor_core::{HeadingKind, Language, SymbolKind};
use build::Severity;
use gpui::{AppContext, Context, Entity};
use pdf::{PdfEvent, PdfWorker};

use super::Workspace;
use crate::{addon_panel::AddonPanel, addons, bib, editor_view::EditorView, file_tree, outline, toolchain};

/// The cursor has to rest this long before add-ons hear where it is.
const CONTEXT_DELAY: Duration = Duration::from_millis(250);
const BUILD_LOG_BYTES: usize = 64 * 1024;
const PDF_PAGE_WIDTH: u32 = 1200;
const PDF_PAGE_WIDTH_MAX: u32 = 3000;

/// Where the user is, as add-ons hear it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct AddonContext {
    path: Option<String>,
    line: usize,
    column: usize,
    selected_chars: usize,
}

impl Workspace {
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
    for pixel in bgra.chunks_exact_mut(4) {
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
