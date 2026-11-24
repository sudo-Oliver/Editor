//! The preview of a Markdown file: its blocks drawn as a page beside the
//! editor, without a browser and without a build. Only the blocks on screen
//! are laid out, and an edit reads the text again only from the block it is in.
//! Display formulas are typeset by LaTeX in the background, see `math_render`.

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::Arc,
};

use gpui::{AnyElement, Context, EntityId, EventEmitter, ListAlignment, ListState, MouseButton, Render, RenderImage, SharedString, Window, div, img, list, prelude::*, px};
use pdf::{PageRequest, PdfEvent, PdfWorker};

use crate::{
    markdown::{Block, Document},
    markdown_view::{self, Look},
    math_render::{self, Mask},
    theme,
};

/// A display formula on its way to an image.
enum Formula {
    /// Waits for a run; `alone` once a shared run lost track of its pages.
    Pending { alone: bool },
    Ready { mask: Arc<Mask>, image: Option<(u32, Arc<RenderImage>)> },
    /// LaTeX failed on it or is not there: the Unicode text stays.
    Failed,
}

/// The bold of iA Writer Quattro claims a regular weight, so the page takes
/// the system font, which has every weight.
#[cfg(target_os = "macos")]
const PAGE_FONT: Option<&str> = Some(".SystemUIFont");
#[cfg(target_os = "windows")]
const PAGE_FONT: Option<&str> = Some("Segoe UI");
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
const PAGE_FONT: Option<&str> = None;

/// The widest a line of the page gets, for comfortable reading.
const PAGE_WIDTH: f32 = 720.0;

pub enum MarkdownPreviewEvent {
    /// A block was clicked: the editor goes to where it starts.
    Clicked(usize),
}

pub struct MarkdownPreview {
    document: Document,
    /// The document and the version of its text that `document` shows.
    shown: Option<(EntityId, u64)>,
    /// Where image paths start from: the folder of the file.
    base: Option<PathBuf>,
    list: ListState,
    formulas: HashMap<SharedString, Formula>,
    typesetting: bool,
    /// Looked for at the first formula: the LaTeX program, if one is installed.
    engine: Option<Option<PathBuf>>,
}

impl EventEmitter<MarkdownPreviewEvent> for MarkdownPreview {}

impl MarkdownPreview {
    pub fn new(_cx: &mut Context<Self>) -> Self {
        Self {
            document: Document::file(),
            shown: None,
            base: None,
            list: ListState::new(0, ListAlignment::Top, px(600.)),
            formulas: HashMap::new(),
            typesetting: false,
            engine: None,
        }
    }

    /// Whether the preview already shows this version of this document.
    pub fn shows(&self, view: EntityId, version: u64) -> bool {
        self.shown == Some((view, version))
    }

    /// Takes the text of `view` at `version`. Another document starts at its top.
    pub fn show(&mut self, view: EntityId, version: u64, path: Option<&Path>, text: &str, cx: &mut Context<Self>) {
        let other = self.shown.is_none_or(|(shown, _)| shown != view);
        self.shown = Some((view, version));
        self.base = path.and_then(Path::parent).map(Path::to_path_buf);
        if other {
            self.document = Document::file();
        }
        let (replaced, count) = self.document.update(text);
        let fresh = if other { 0..self.document.blocks.len() } else { replaced.start..replaced.start + count };
        if other {
            self.list.reset(self.document.blocks.len());
        } else {
            self.list.splice(replaced, count);
        }
        for block in &self.document.blocks[fresh] {
            if let Block::Math { source, .. } = block
                && self.engine != Some(None)
            {
                let formula = if math_render::balanced(source) { Formula::Pending { alone: false } } else { Formula::Failed };
                self.formulas.entry(source.clone()).or_insert(formula);
            }
        }
        self.typeset_pending(cx);
        cx.notify();
    }

    /// Starts one LaTeX run for the formulas that wait, unless one runs. Formulas
    /// no longer in the text, as those typed on the way, are dropped first.
    fn typeset_pending(&mut self, cx: &mut Context<Self>) {
        if self.typesetting || !self.formulas.values().any(|formula| matches!(formula, Formula::Pending { .. })) {
            return;
        }
        let shown: HashSet<&SharedString> = self.document.blocks.iter().filter_map(|block| if let Block::Math { source, .. } = block { Some(source) } else { None }).collect();
        self.formulas.retain(|source, _| shown.contains(source));
        let alone = self.formulas.iter().find(|(_, formula)| matches!(formula, Formula::Pending { alone: true })).map(|(source, _)| source.clone());
        let batch: Vec<SharedString> = match alone {
            Some(source) => vec![source],
            None => self.formulas.iter().filter(|(_, formula)| matches!(formula, Formula::Pending { .. })).map(|(source, _)| source.clone()).collect(),
        };
        if batch.is_empty() {
            return;
        }
        self.typesetting = true;
        let engine = self.engine.clone();
        cx.spawn(async move |this, cx| {
            let sources = batch.clone();
            let (engine, typeset) = cx
                .background_spawn(async move {
                    let engine = engine.unwrap_or_else(math_render::find_engine);
                    let formulas: Vec<&str> = sources.iter().map(|source| source.as_ref()).collect();
                    let typeset = engine.as_deref().map(|engine| math_render::typeset(engine, &formulas));
                    (engine, typeset)
                })
                .await;
            // Each formula alone would take as long again: they all keep their text.
            let no_latex = engine.is_none() || matches!(&typeset, Some(Err(error)) if error == math_render::TOO_LONG);
            let pages = match typeset {
                Some(Ok((pdf, failed))) => {
                    this.update(cx, |this, _| {
                        for index in &failed {
                            this.formulas.insert(batch[*index].clone(), Formula::Failed);
                        }
                    })
                    .ok();
                    let wanted: Vec<usize> = (0..batch.len()).filter(|index| !failed.contains(index)).collect();
                    Self::rasterize(&this, cx, pdf, &batch, &wanted).await
                }
                _ => Err(()),
            };
            this.update(cx, |this, cx| {
                this.engine = Some(engine);
                for source in &batch {
                    let lost = pages.is_err() || matches!(this.formulas.get(source), Some(Formula::Pending { .. }));
                    if lost {
                        // A run of several that failed or lost its pages tries each alone.
                        let formula = if batch.len() > 1 && !no_latex { Formula::Pending { alone: true } } else { Formula::Failed };
                        this.formulas.insert(source.clone(), formula);
                    }
                }
                this.typesetting = false;
                this.typeset_pending(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Renders the pages of `wanted` formulas and keeps their masks. Fails when
    /// the PDF has not one page per formula, since then pages and formulas mismatch.
    async fn rasterize(this: &gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp, pdf: Vec<u8>, batch: &[SharedString], wanted: &[usize]) -> Result<(), ()> {
        if wanted.is_empty() {
            return Ok(());
        }
        let (worker, mut events) = cx.background_spawn(async move { PdfWorker::spawn(&crate::pdf_view::library_directory()) }).await.map_err(|_| ())?;
        worker.load(0, pdf);
        let mut left = wanted.len();
        while let Some(event) = events.recv().await {
            match event {
                PdfEvent::Loaded { pages, .. } => {
                    if pages.len() != batch.len() {
                        return Err(());
                    }
                    worker.render(0, wanted.iter().map(|page| PageRequest { page: *page, width_px: math_render::page_width_px(), recolor: None }).collect());
                }
                PdfEvent::Rendered { request, height_px, bgra, .. } => {
                    let mask = cx.background_spawn(async move { math_render::crop(&bgra, request.width_px, height_px) }).await;
                    let source = batch[request.page].clone();
                    this.update(cx, |this, cx| {
                        let formula = match mask {
                            Some(mask) => Formula::Ready { mask: Arc::new(mask), image: None },
                            None => Formula::Failed,
                        };
                        this.formulas.insert(source, formula);
                        cx.notify();
                    })
                    .map_err(|_| ())?;
                    left -= 1;
                    if left == 0 {
                        return Ok(());
                    }
                }
                PdfEvent::Failed { .. } => return Err(()),
                PdfEvent::PageContent { .. } | PdfEvent::Outline { .. } => {}
            }
        }
        Err(())
    }

    /// The typeset formula, in the color of the text, once it is there.
    fn formula(&mut self, source: &SharedString, look: Look) -> Option<AnyElement> {
        let Some(Formula::Ready { mask, image }) = self.formulas.get_mut(source) else {
            return None;
        };
        let color = theme::palette().code_text;
        if image.as_ref().is_none_or(|(tint, _)| *tint != color) {
            let pixels = image::RgbaImage::from_raw(mask.width, mask.height, mask.tinted(color))?;
            *image = Some((color, Arc::new(RenderImage::new([image::Frame::new(pixels)]))));
        }
        let (_, picture) = image.as_ref()?;
        let scale = math_render::LOGICAL_PER_PIXEL;
        Some(
            div()
                .flex()
                .justify_center()
                .py(px(look.gap / 2.))
                .overflow_hidden()
                .child(img(picture.clone()).flex_none().w(px(mask.width as f32 * scale)).h(px(mask.height as f32 * scale)))
                .into_any_element(),
        )
    }

    /// Scrolls to the block that holds `offset`, unless it is on screen.
    pub fn reveal(&mut self, offset: usize, cx: &mut Context<Self>) {
        let starts = self.document.starts();
        let index = starts.partition_point(|start| *start <= offset).saturating_sub(1);
        if index < starts.len() {
            self.list.scroll_to_reveal_item(index);
            cx.notify();
        }
    }

    fn render_item(&mut self, index: usize, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(block) = self.document.blocks.get(index) else {
            return div().into_any_element();
        };
        let start = self.document.starts()[index];
        let look = Look::PAGE;
        let math = if let Block::Math { source, .. } = block { Some(source.clone()) } else { None };
        let content = match math.and_then(|source| self.formula(&source, look)) {
            Some(formula) => formula,
            None => markdown_view::block(index, &self.document.blocks[index], look, self.base.as_deref(), |_, _| None),
        };
        div()
            .id(("markdown-block", index))
            // A column, so that the page gets a definite width and its text wraps.
            .flex()
            .flex_col()
            .items_center()
            .px(px(32.))
            .pt(px(if index == 0 { 32. } else { look.gap }))
            .when(index + 1 == self.document.blocks.len(), |element| element.pb(px(48.)))
            .on_mouse_down(MouseButton::Left, cx.listener(move |_, event: &gpui::MouseDownEvent, _, cx| {
                // A double click goes to the source, so that links and selection keep the single one.
                if event.click_count == 2 {
                    cx.emit(MarkdownPreviewEvent::Clicked(start));
                }
            }))
            .child(div().w_full().max_w(px(PAGE_WIDTH)).min_w_0().child(content))
            .into_any_element()
    }
}

impl Render for MarkdownPreview {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = theme::palette();
        div()
            .size_full()
            .bg(theme::color(palette.bg))
            .text_color(theme::color(palette.code_text))
            .when_some(PAGE_FONT, |page, font| page.font_family(font))
            .text_size(px(Look::PAGE.text_size))
            .line_height(gpui::relative(1.55))
            .child(list(self.list.clone(), cx.processor(Self::render_item)).size_full())
    }
}
