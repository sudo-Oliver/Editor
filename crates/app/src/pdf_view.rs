use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use gpui::{
    Bounds, Context, Corners, EventEmitter, MouseButton, MouseDownEvent, Pixels, Render, RenderImage,
    ScrollEvent, Task, Window, canvas, div, fill, point, prelude::*, px, rgba, size,
};
use pdf::{PageRequest, PdfEvent, PdfWorker};
use synctex::PdfLocation;

use crate::theme;

const PAGE_GAP: f32 = 12.0;
const SIDE_MARGIN: f32 = 21.0;
const MIN_ZOOM: f32 = 0.1;
const MAX_ZOOM: f32 = 8.0;
pub const ZOOM_STEP: f32 = 1.2;
const MAX_RENDER_WIDTH: f32 = 8000.0;
/// Pages this far outside the viewport stay rendered.
const KEEP_PAGES: usize = 2;
/// Zooming scales the existing texture. A sharp render follows after this pause.
const ZOOM_SETTLE: Duration = Duration::from_millis(150);
const HIGHLIGHT_TIME: Duration = Duration::from_millis(1500);

pub enum PdfViewEvent {
    /// A secondary click at a position in PDF points on a zero-based page.
    BackwardSearch { page: usize, x: f32, y: f32 },
    Failed(String),
}

struct PageImage {
    image: Arc<RenderImage>,
    generation: u64,
    request: PageRequest,
}

struct PaintList {
    pages: Vec<(Bounds<Pixels>, Option<Arc<RenderImage>>)>,
    highlight: Option<Bounds<Pixels>>,
}

pub struct PdfView {
    worker: Option<PdfWorker>,
    generation: u64,
    pages: Vec<(f32, f32)>,
    images: HashMap<usize, PageImage>,
    pending_drop: Vec<Arc<RenderImage>>,
    last_requested: Vec<PageRequest>,
    zoom: f32,
    fit_width: bool,
    zoomed_at: Option<Instant>,
    settle_task: Option<Task<()>>,
    invert: bool,
    scroll: gpui::Point<Pixels>,
    bounds: Option<Bounds<Pixels>>,
    pending_location: Option<PdfLocation>,
    highlight: Option<PdfLocation>,
    highlight_task: Option<Task<()>>,
    error: Option<String>,
    load_task: Option<Task<()>>,
    _events_task: Option<Task<()>>,
}

impl EventEmitter<PdfViewEvent> for PdfView {}

impl PdfView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let mut view = Self {
            worker: None,
            generation: 0,
            pages: Vec::new(),
            images: HashMap::new(),
            pending_drop: Vec::new(),
            last_requested: Vec::new(),
            zoom: 1.0,
            fit_width: true,
            zoomed_at: None,
            settle_task: None,
            invert: false,
            scroll: point(px(0.), px(0.)),
            bounds: None,
            pending_location: None,
            highlight: None,
            highlight_task: None,
            error: None,
            load_task: None,
            _events_task: None,
        };
        match PdfWorker::spawn(&library_directory()) {
            Ok((worker, mut events)) => {
                view.worker = Some(worker);
                view._events_task = Some(cx.spawn(async move |this, cx| {
                    while let Some(event) = events.recv().await {
                        if this.update(cx, |this, cx| this.handle_event(event, cx)).is_err() {
                            return;
                        }
                    }
                }));
            }
            Err(error) => {
                view.error = Some(format!("{error} Run scripts/fetch-pdfium.sh."));
            }
        }
        view
    }

    /// Loads a new PDF. The old pages stay visible until it is ready.
    pub fn load_file(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.load_task = Some(cx.spawn(async move |this, cx| {
            let read_path = path.clone();
            let bytes = cx.background_spawn(async move { std::fs::read(read_path) }).await;
            this.update(cx, |this, cx| match bytes {
                Ok(bytes) => {
                    this.generation += 1;
                    if let Some(worker) = &this.worker {
                        worker.load(this.generation, bytes);
                    }
                }
                Err(error) => {
                    cx.emit(PdfViewEvent::Failed(format!("Cannot read {}: {error}", path.display())));
                }
            })
            .ok();
        }));
    }

    /// Removes the document. A failed build of another document ends up here,
    /// because showing the old one would look like the result of this build.
    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.generation += 1;
        self.load_task = None;
        self.pages.clear();
        self.last_requested.clear();
        self.pending_drop.extend(self.images.drain().map(|(_, image)| image.image));
        cx.notify();
    }

    pub fn has_document(&self) -> bool {
        !self.pages.is_empty()
    }

    pub fn page_count(&self) -> usize {
        self.pages.len()
    }

    pub fn zoom_percent(&self) -> u32 {
        (self.zoom * 100.0).round() as u32
    }

    pub fn current_page(&self) -> usize {
        let Some(bounds) = self.bounds else {
            return 0;
        };
        let center = f32::from(self.scroll.y + bounds.size.height / 2.);
        let mut top = PAGE_GAP;
        for (index, (_, height)) in self.pages.iter().enumerate() {
            top += height * self.zoom + PAGE_GAP;
            if center < top {
                return index;
            }
        }
        self.pages.len().saturating_sub(1)
    }

    pub fn go_to_page(&mut self, page: usize, cx: &mut Context<Self>) {
        if page < self.pages.len() {
            self.scroll.y = px(self.page_top(page) - PAGE_GAP);
            cx.notify();
        }
    }

    pub fn zoom_by(&mut self, factor: f32, cx: &mut Context<Self>) {
        let anchor = self.bounds.map_or(point(px(0.), px(0.)), |bounds| {
            point(bounds.size.width / 2., bounds.size.height / 2.)
        });
        self.zoom_around(factor, anchor, cx);
    }

    pub fn set_fit_width(&mut self, cx: &mut Context<Self>) {
        self.fit_width = true;
        self.zoomed_at = Some(Instant::now());
        self.schedule_settle(cx);
        cx.notify();
    }

    pub fn toggle_invert(&mut self, cx: &mut Context<Self>) {
        self.invert = !self.invert;
        cx.notify();
    }

    /// Scrolls to a location and flashes it.
    pub fn reveal(&mut self, location: PdfLocation, cx: &mut Context<Self>) {
        self.pending_location = Some(location);
        self.highlight = Some(location);
        self.highlight_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(HIGHLIGHT_TIME).await;
            this.update(cx, |this, cx| {
                this.highlight = None;
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn handle_event(&mut self, event: PdfEvent, cx: &mut Context<Self>) {
        match event {
            PdfEvent::Loaded { generation, pages } => {
                if generation == self.generation {
                    self.pages = pages;
                    self.error = None;
                    self.last_requested.clear();
                    let count = self.pages.len();
                    self.images.retain(|page, image| {
                        let keep = *page < count;
                        if !keep {
                            self.pending_drop.push(image.image.clone());
                        }
                        keep
                    });
                    cx.notify();
                }
            }
            PdfEvent::Rendered { generation, request, height_px, bgra } => {
                if generation != self.generation {
                    return;
                }
                let Some(buffer) = image::RgbaImage::from_raw(request.width_px, height_px, bgra) else {
                    return;
                };
                let image = Arc::new(RenderImage::new([image::Frame::new(buffer)]));
                let page = PageImage { image, generation, request };
                if let Some(old) = self.images.insert(request.page, page) {
                    self.pending_drop.push(old.image);
                }
                cx.notify();
            }
            PdfEvent::Failed { generation, message } => {
                if generation == self.generation {
                    cx.emit(PdfViewEvent::Failed(message));
                }
            }
        }
    }

    fn page_top(&self, page: usize) -> f32 {
        let above: f32 = self.pages[..page].iter().map(|(_, height)| height * self.zoom + PAGE_GAP).sum();
        PAGE_GAP + above
    }

    fn content_size(&self) -> (f32, f32) {
        let width = self.pages.iter().map(|(width, _)| *width).fold(0.0, f32::max) * self.zoom;
        let height = self.pages.iter().map(|(_, height)| height * self.zoom + PAGE_GAP).sum::<f32>() + PAGE_GAP;
        (width + SIDE_MARGIN * 2.0, height)
    }

    fn page_left(&self, page: usize, viewport: Pixels) -> f32 {
        let width = self.pages[page].0 * self.zoom;
        let (content_width, _) = self.content_size();
        ((f32::from(viewport).max(content_width) - width) / 2.0).max(SIDE_MARGIN)
    }

    fn clamp_scroll(&mut self, viewport: gpui::Size<Pixels>) {
        let (width, height) = self.content_size();
        self.scroll.x = self.scroll.x.clamp(px(0.), px((width - f32::from(viewport.width)).max(0.0)));
        self.scroll.y = self.scroll.y.clamp(px(0.), px((height - f32::from(viewport.height)).max(0.0)));
    }

    fn zoom_around(&mut self, factor: f32, anchor: gpui::Point<Pixels>, cx: &mut Context<Self>) {
        let zoom = (self.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        let ratio = zoom / self.zoom;
        self.scroll.x = (self.scroll.x + anchor.x) * ratio - anchor.x;
        self.scroll.y = (self.scroll.y + anchor.y) * ratio - anchor.y;
        self.zoom = zoom;
        self.fit_width = false;
        self.zoomed_at = Some(Instant::now());
        self.schedule_settle(cx);
        cx.notify();
    }

    fn schedule_settle(&mut self, cx: &mut Context<Self>) {
        self.settle_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(ZOOM_SETTLE).await;
            this.update(cx, |this, cx| {
                this.zoomed_at = None;
                cx.notify();
            })
            .ok();
        }));
    }

    fn on_scroll(&mut self, event: &ScrollEvent, _: &mut Window, cx: &mut Context<Self>) {
        let delta = event.delta.pixel_delta(px(theme::CODE_LINE_HEIGHT));
        if event.modifiers.secondary() {
            let anchor = self.bounds.map_or(point(px(0.), px(0.)), |bounds| {
                point(event.position.x - bounds.left(), event.position.y - bounds.top())
            });
            self.zoom_around((f32::from(delta.y) * 0.01).exp(), anchor, cx);
            return;
        }
        self.scroll.x -= delta.x;
        self.scroll.y -= delta.y;
        cx.notify();
    }

    fn on_mouse_down(&mut self, event: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        if !event.modifiers.secondary() {
            return;
        }
        let Some(bounds) = self.bounds else {
            return;
        };
        let x = event.position.x - bounds.left() + self.scroll.x;
        let y = event.position.y - bounds.top() + self.scroll.y;
        for page in 0..self.pages.len() {
            let top = self.page_top(page);
            let left = self.page_left(page, bounds.size.width);
            let (width, height) = self.pages[page];
            let inside_x = f32::from(x) >= left && f32::from(x) <= left + width * self.zoom;
            if inside_x && f32::from(y) >= top && f32::from(y) <= top + height * self.zoom {
                cx.emit(PdfViewEvent::BackwardSearch {
                    page,
                    x: (f32::from(x) - left) / self.zoom,
                    y: (f32::from(y) - top) / self.zoom,
                });
                return;
            }
        }
    }

    fn prepare_paint(&mut self, bounds: Bounds<Pixels>, window: &mut Window) -> PaintList {
        self.bounds = Some(bounds);
        for image in self.pending_drop.drain(..) {
            window.drop_image(image).ok();
        }
        if self.pages.is_empty() {
            return PaintList { pages: Vec::new(), highlight: None };
        }
        if self.fit_width {
            let widest = self.pages.iter().map(|(width, _)| *width).fold(1.0, f32::max);
            self.zoom = ((f32::from(bounds.size.width) - SIDE_MARGIN * 2.0) / widest).clamp(MIN_ZOOM, MAX_ZOOM);
        }
        if let Some(location) = self.pending_location.take()
            && location.page < self.pages.len()
        {
            self.scroll.y = px(self.page_top(location.page) + location.y * self.zoom - f32::from(bounds.size.height) / 3.0);
        }
        self.clamp_scroll(bounds.size);

        let view_top = f32::from(self.scroll.y);
        let view_bottom = view_top + f32::from(bounds.size.height);
        let mut visible = Vec::new();
        let mut paint = Vec::new();
        let mut highlight = None;
        for page in 0..self.pages.len() {
            let top = self.page_top(page);
            let (width, height) = self.pages[page];
            let (width, height) = (width * self.zoom, height * self.zoom);
            if top + height < view_top || top > view_bottom {
                continue;
            }
            visible.push(page);
            let origin = point(
                bounds.left() + px(self.page_left(page, bounds.size.width)) - self.scroll.x,
                bounds.top() + px(top) - self.scroll.y,
            );
            paint.push((
                Bounds::new(origin, size(px(width), px(height))),
                self.images.get(&page).map(|image| image.image.clone()),
            ));
            if let Some(location) = self.highlight.filter(|location| location.page == page) {
                let left = location.x * self.zoom;
                highlight = Some(Bounds::new(
                    point(origin.x + px(left), origin.y + px(location.y * self.zoom)),
                    size(px((location.width * self.zoom).min(width - left)), px(location.height * self.zoom)),
                ));
            }
        }

        let first = visible.first().copied().unwrap_or(0);
        let last = visible.last().copied().unwrap_or(0);
        let far: Vec<usize> = self
            .images
            .keys()
            .copied()
            .filter(|page| *page + KEEP_PAGES < first || *page > last + KEEP_PAGES)
            .collect();
        for page in far {
            if let Some(image) = self.images.remove(&page) {
                self.pending_drop.push(image.image);
            }
        }
        self.request_pages(first, last, window.scale_factor());
        PaintList { pages: paint, highlight }
    }

    /// Asks for visible pages first, then their neighbors, at the current resolution.
    fn request_pages(&mut self, first: usize, last: usize, scale: f32) {
        let is_zooming = self.zoomed_at.is_some();
        let before = first.saturating_sub(1)..first;
        let after = (last + 1)..(last + 2).min(self.pages.len());
        let wanted: Vec<PageRequest> = (first..=last)
            .chain(after)
            .chain(before)
            .filter_map(|page| {
                let request = PageRequest {
                    page,
                    width_px: (self.pages[page].0 * self.zoom * scale).min(MAX_RENDER_WIDTH).round() as u32,
                    invert: self.invert,
                };
                let up_to_date = self.images.get(&page).is_some_and(|image| {
                    image.generation == self.generation
                        && (image.request == request || (is_zooming && image.request.invert == self.invert))
                });
                (!up_to_date && request.width_px > 0).then_some(request)
            })
            .collect();
        if wanted != self.last_requested {
            if let Some(worker) = &self.worker {
                worker.render(self.generation, wanted.clone());
            }
            self.last_requested = wanted;
        }
    }
}

fn library_directory() -> PathBuf {
    if let Some(directory) = std::env::var_os("EDITOR_PDFIUM_DIR") {
        return directory.into();
    }
    let name = pdfium_file_name();
    let beside_binary = std::env::current_exe()
        .ok()
        .and_then(|binary| binary.parent().map(Path::to_path_buf))
        .filter(|directory| directory.join(name).is_file());
    beside_binary.unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../vendor/pdfium/lib"))
}

#[cfg(target_os = "macos")]
fn pdfium_file_name() -> &'static str {
    "libpdfium.dylib"
}

#[cfg(not(target_os = "macos"))]
fn pdfium_file_name() -> &'static str {
    "libpdfium.so"
}

impl Render for PdfView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let message = self.error.clone().or_else(|| {
            self.pages.is_empty().then(|| "No PDF yet. Press Recompile.".to_string())
        });
        let view = cx.entity();
        div()
            .id("pdf")
            .relative()
            .size_full()
            .overflow_hidden()
            .on_scroll(cx.listener(Self::on_scroll))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .child(
                canvas(
                    move |bounds, window, cx| view.update(cx, |view, _| view.prepare_paint(bounds, window)),
                    |bounds, list, window, _| {
                        window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
                            for (page, image) in list.pages {
                                match image {
                                    Some(image) => {
                                        window
                                            .paint_image(bounds, page, Corners::default(), image, 0, false)
                                            .ok();
                                    }
                                    None => window.paint_quad(fill(page, theme::color(0xffffff))),
                                }
                            }
                            if let Some(highlight) = list.highlight {
                                window.paint_quad(fill(highlight, rgba(0xf5bb7766)));
                            }
                        });
                    },
                )
                .size_full(),
            )
            .when_some(message, |element, message| {
                element.child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .p(px(16.))
                        .text_color(theme::color(theme::UI_MUTED))
                        .child(message),
                )
            })
    }
}
