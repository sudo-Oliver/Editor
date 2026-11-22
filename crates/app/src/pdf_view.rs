use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use gpui::{
    Bounds, ClipboardItem, Context, Corners, EventEmitter, FocusHandle, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, NavigationDirection, Pixels, Render, RenderImage, ScrollEvent, Task, Window, canvas, div, fill,
    point, prelude::*, px, rgba, size,
};
use pdf::{LinkTarget, PageContent, PageRequest, PdfEvent, PdfWorker, Recolor, Rect};
use synctex::PdfLocation;

use crate::{
    actions,
    pdf_text::{self, Selection, TextPoint},
    theme,
};

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
const SCROLL_TIME: Duration = Duration::from_millis(180);
/// A press that moves less than this is a click.
const DRAG_DISTANCE: f32 = 3.0;
const MAX_HISTORY: usize = 100;
/// Space above the target of a link.
const LINK_MARGIN: f32 = 16.0;
const LINK_FLASH_HEIGHT: f32 = 14.0;
const SELECTION_COLOR: u32 = 0x4a9cd466;
const LINK_HOVER_COLOR: u32 = 0x4a9cd426;
/// How far a wheel or trackpad moves to turn a slide.
const SLIDE_SCROLL: f32 = 60.0;
/// The swing of a trackpad after one slide turn must not turn the next.
const SLIDE_PAUSE: Duration = Duration::from_millis(350);

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
    selection: Vec<Bounds<Pixels>>,
    hovered_link: Option<Bounds<Pixels>>,
    highlight: Option<Bounds<Pixels>>,
}

#[derive(Clone, Copy)]
enum Placement {
    /// A third down the viewport.
    Third,
    /// Centered, unless it already is near the middle, so that moving the cursor
    /// through a paragraph leaves the page at rest.
    Center,
    /// This many pixels below the top of the viewport.
    Top(f32),
}

#[derive(Clone, Copy)]
struct ScrollRequest {
    page: usize,
    y: f32,
    placement: Placement,
}

#[derive(Clone, Copy)]
struct ScrollAnimation {
    from: f32,
    to: f32,
    start: Instant,
}

/// A position in PDF points on a zero-based page. It stays valid when the zoom changes.
#[derive(Clone, Copy)]
struct PagePoint {
    page: usize,
    x: f32,
    y: f32,
    inside: bool,
}

#[derive(Clone, Copy)]
struct Press {
    position: gpui::Point<Pixels>,
    point: PagePoint,
    link: Option<usize>,
    moved: bool,
}

#[derive(Clone, Copy)]
struct ViewPosition {
    page: usize,
    y: f32,
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
    pending_scroll: Option<ScrollRequest>,
    scroll_animation: Option<ScrollAnimation>,
    highlight: Option<PdfLocation>,
    highlight_task: Option<Task<()>>,
    error: Option<String>,
    load_task: Option<Task<()>>,
    _events_task: Option<Task<()>>,
    /// Text and links of the pages that the pointer reached.
    content: HashMap<usize, PageContent>,
    requested_content: HashSet<usize>,
    selection: Option<Selection>,
    selection_rects: HashMap<usize, Box<[Rect]>>,
    press: Option<Press>,
    /// Page and index of the link under the pointer.
    hovered_link: Option<(usize, usize)>,
    back: Vec<ViewPosition>,
    forward: Vec<ViewPosition>,
    focus: FocusHandle,
    /// One page fills the view, for a talk.
    presenting: bool,
    present_page: usize,
    /// Zoom and fit from before the talk.
    before_present: (f32, bool),
    slide_scroll: f32,
    slide_turned_at: Option<Instant>,
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
            pending_scroll: None,
            scroll_animation: None,
            highlight: None,
            highlight_task: None,
            error: None,
            load_task: None,
            _events_task: None,
            content: HashMap::new(),
            requested_content: HashSet::new(),
            selection: None,
            selection_rects: HashMap::new(),
            press: None,
            hovered_link: None,
            back: Vec::new(),
            forward: Vec::new(),
            focus: cx.focus_handle(),
            presenting: false,
            present_page: 0,
            before_present: (1.0, true),
            slide_scroll: 0.0,
            slide_turned_at: None,
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
                eprintln!("PDFium did not load: {error}");
                view.error = Some(if cfg!(debug_assertions) {
                    format!("{error} Run scripts/fetch-pdfium.sh.")
                } else {
                    "The PDF viewer is missing from this copy of the app. Install Editor again from the download page.".to_string()
                });
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
        self.forget_content();
        self.back.clear();
        self.forward.clear();
        cx.notify();
    }

    pub fn focus_handle(&self) -> FocusHandle {
        self.focus.clone()
    }

    pub fn is_focused(&self, window: &Window) -> bool {
        self.focus.is_focused(window)
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
        if self.presenting {
            return self.present_page;
        }
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
            self.scroll_animation = None;
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

    /// Shows one page at a time, as large as it fits. Leaving goes back to the
    /// zoom from before, at the page the talk ended on.
    pub fn set_presenting(&mut self, on: bool, cx: &mut Context<Self>) {
        if on == self.presenting {
            return;
        }
        if on {
            self.present_page = self.current_page();
            self.before_present = (self.zoom, self.fit_width);
            self.selection = None;
            self.selection_rects.clear();
            self.press = None;
            self.hovered_link = None;
            self.slide_scroll = 0.0;
        } else {
            (self.zoom, self.fit_width) = self.before_present;
            self.pending_scroll = Some(ScrollRequest { page: self.present_page, y: 0.0, placement: Placement::Top(PAGE_GAP) });
        }
        self.presenting = on;
        self.scroll_animation = None;
        self.zoomed_at = Some(Instant::now());
        self.schedule_settle(cx);
        cx.notify();
    }

    fn turn_slide(&mut self, forward: bool, cx: &mut Context<Self>) {
        let last = self.pages.len().saturating_sub(1);
        let page = if forward { (self.present_page + 1).min(last) } else { self.present_page.saturating_sub(1) };
        if page != self.present_page {
            self.present_page = page;
            cx.notify();
        }
    }

    pub fn toggle_invert(&mut self, cx: &mut Context<Self>) {
        self.invert = !self.invert;
        cx.notify();
    }

    /// Scrolls to a location and flashes it.
    pub fn reveal(&mut self, location: PdfLocation, cx: &mut Context<Self>) {
        self.pending_scroll = Some(ScrollRequest { page: location.page, y: location.y, placement: Placement::Third });
        self.flash(location, cx);
    }

    /// Keeps a location in view while the cursor moves, without flashing it.
    pub fn follow(&mut self, location: PdfLocation, cx: &mut Context<Self>) {
        self.pending_scroll = Some(ScrollRequest { page: location.page, y: location.y, placement: Placement::Center });
        cx.notify();
    }

    fn flash(&mut self, location: PdfLocation, cx: &mut Context<Self>) {
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

    fn forget_content(&mut self) {
        self.content.clear();
        self.requested_content.clear();
        self.selection = None;
        self.selection_rects.clear();
        self.press = None;
        self.hovered_link = None;
    }

    fn handle_event(&mut self, event: PdfEvent, cx: &mut Context<Self>) {
        match event {
            PdfEvent::Loaded { generation, pages } => {
                if generation == self.generation {
                    self.pages = pages;
                    self.error = None;
                    self.last_requested.clear();
                    self.forget_content();
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
            PdfEvent::PageContent { generation, page, content } => {
                if generation == self.generation {
                    self.content.insert(page, content);
                }
            }
            PdfEvent::Outline { .. } => {}
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
        self.scroll_animation = None;
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
        let delta = event.delta.pixel_delta(px(theme::code_line_height()));
        if self.presenting {
            if self.slide_turned_at.is_some_and(|at| at.elapsed() < SLIDE_PAUSE) {
                return;
            }
            self.slide_scroll -= f32::from(delta.y);
            if self.slide_scroll.abs() >= SLIDE_SCROLL {
                self.turn_slide(self.slide_scroll > 0.0, cx);
                self.slide_scroll = 0.0;
                self.slide_turned_at = Some(Instant::now());
            }
            return;
        }
        if event.modifiers.secondary(cx) {
            let anchor = self.bounds.map_or(point(px(0.), px(0.)), |bounds| {
                point(event.position.x - bounds.left(), event.position.y - bounds.top())
            });
            self.zoom_around((f32::from(delta.y) * 0.01).exp(), anchor, cx);
            return;
        }
        self.scroll_animation = None;
        self.scroll.x -= delta.x;
        self.scroll.y -= delta.y;
        cx.notify();
    }

    /// The page at a height in the content. A gap belongs to the page above it.
    fn page_near(&self, content_y: f32) -> Option<(usize, f32)> {
        let mut top = PAGE_GAP;
        for (page, (_, height)) in self.pages.iter().enumerate() {
            let next = top + height * self.zoom + PAGE_GAP;
            if content_y < next {
                return Some((page, top));
            }
            top = next;
        }
        let last = self.pages.len().checked_sub(1)?;
        Some((last, top - self.pages[last].1 * self.zoom - PAGE_GAP))
    }

    /// None during a talk, where the pages are not where scrolling puts them.
    fn page_point(&self, position: gpui::Point<Pixels>) -> Option<PagePoint> {
        if self.presenting {
            return None;
        }
        let bounds = self.bounds?;
        let content_x = f32::from(position.x - bounds.left() + self.scroll.x);
        let content_y = f32::from(position.y - bounds.top() + self.scroll.y);
        let (page, top) = self.page_near(content_y)?;
        let x = (content_x - self.page_left(page, bounds.size.width)) / self.zoom;
        let y = (content_y - top) / self.zoom;
        let (width, height) = self.pages[page];
        let inside = x >= 0.0 && x <= width && y >= 0.0 && y <= height;
        Some(PagePoint { page, x, y, inside })
    }

    fn request_content(&mut self, page: usize) {
        if self.requested_content.insert(page)
            && let Some(worker) = &self.worker
        {
            worker.content(self.generation, page);
        }
    }

    fn link_at(&self, point: &PagePoint) -> Option<usize> {
        let links = &self.content.get(&point.page)?.links;
        links.iter().position(|link| link.rect.contains(point.x, point.y))
    }

    fn text_point(&self, point: &PagePoint) -> Option<TextPoint> {
        let chars = &self.content.get(&point.page)?.chars;
        let index = pdf_text::char_at(chars, point.x, point.y)?;
        Some(TextPoint { page: point.page, index })
    }

    fn set_selection(&mut self, selection: Option<Selection>, cx: &mut Context<Self>) {
        if self.selection != selection {
            self.selection = selection;
            self.selection_rects.clear();
            cx.notify();
        }
    }

    fn on_mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        let Some(point) = self.page_point(event.position) else {
            return;
        };
        if event.modifiers.secondary(cx) {
            if point.inside {
                cx.emit(PdfViewEvent::BackwardSearch { page: point.page, x: point.x, y: point.y });
            }
            return;
        }
        self.request_content(point.page);
        let link = if point.inside { self.link_at(&point) } else { None };
        self.press = Some(Press { position: event.position, point, link, moved: false });
        self.set_selection(None, cx);
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(point) = self.page_point(event.position) else {
            return;
        };
        if event.pressed_button == Some(MouseButton::Left) {
            self.drag(event.position, point, cx);
            return;
        }
        self.press = None;
        if point.inside {
            self.request_content(point.page);
        }
        let link = if point.inside { self.link_at(&point) } else { None };
        let hovered = link.map(|link| (point.page, link));
        if self.hovered_link != hovered {
            self.hovered_link = hovered;
            cx.notify();
        }
    }

    fn drag(&mut self, position: gpui::Point<Pixels>, point: PagePoint, cx: &mut Context<Self>) {
        let Some(press) = self.press.as_mut() else {
            return;
        };
        if !press.moved {
            let distance = position - press.position;
            if f32::from(distance.x).hypot(f32::from(distance.y)) < DRAG_DISTANCE {
                return;
            }
            press.moved = true;
        }
        let press = *press;
        self.request_content(point.page);
        // The content of the first page can still be on its way. The next move tries again.
        let anchor = self.selection.map(|selection| selection.anchor).or_else(|| self.text_point(&press.point));
        if let (Some(anchor), Some(head)) = (anchor, self.text_point(&point)) {
            self.set_selection(Some(Selection { anchor, head }), cx);
        }
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(press) = self.press.take() else {
            return;
        };
        if press.moved {
            return;
        }
        let target = press.link.and_then(|link| {
            Some(self.content.get(&press.point.page)?.links.get(link)?.target.clone())
        });
        if let Some(target) = target {
            self.open_link(target, cx);
        }
    }

    fn open_link(&mut self, target: LinkTarget, cx: &mut Context<Self>) {
        match target {
            LinkTarget::Page { page, y } if page < self.pages.len() => {
                if let Some(position) = self.view_position() {
                    self.forward.clear();
                    push_position(&mut self.back, position);
                }
                let placement = Placement::Top(if y.is_some() { LINK_MARGIN } else { PAGE_GAP });
                self.pending_scroll = Some(ScrollRequest { page, y: y.unwrap_or(0.0), placement });
                match y {
                    Some(y) => {
                        let width = self.pages[page].0;
                        let location = PdfLocation { page, x: 0.0, y, width, height: LINK_FLASH_HEIGHT };
                        self.flash(location, cx);
                    }
                    None => cx.notify(),
                }
            }
            LinkTarget::Page { .. } => {}
            LinkTarget::Uri(uri) if pdf_text::is_openable_uri(&uri) => cx.open_url(&uri),
            LinkTarget::Uri(uri) => cx.emit(PdfViewEvent::Failed(format!("Not opening the link {uri}"))),
        }
    }

    fn view_position(&self) -> Option<ViewPosition> {
        let content_y = f32::from(self.scroll.y);
        let (page, top) = self.page_near(content_y)?;
        Some(ViewPosition { page, y: (content_y - top) / self.zoom })
    }

    fn go_back(&mut self, cx: &mut Context<Self>) {
        if let Some(position) = self.back.pop() {
            self.restore_position(position, true, cx);
        }
    }

    fn go_forward(&mut self, cx: &mut Context<Self>) {
        if let Some(position) = self.forward.pop() {
            self.restore_position(position, false, cx);
        }
    }

    fn restore_position(&mut self, position: ViewPosition, going_back: bool, cx: &mut Context<Self>) {
        if let Some(current) = self.view_position() {
            push_position(if going_back { &mut self.forward } else { &mut self.back }, current);
        }
        // A rebuild can make the document shorter.
        if let Some(last) = self.pages.len().checked_sub(1) {
            let page = position.page.min(last);
            self.pending_scroll = Some(ScrollRequest { page, y: position.y, placement: Placement::Top(0.0) });
            cx.notify();
        }
    }

    /// The selected text of the pages whose text is loaded.
    pub fn selected_text(&self) -> Option<String> {
        let selection = self.selection.as_ref()?;
        Some(pdf_text::selected_text(selection, |page| self.content.get(&page)))
    }

    fn copy_selection(&mut self, cx: &mut Context<Self>) {
        if let Some(selection) = &self.selection {
            let text = pdf_text::selected_text(selection, |page| self.content.get(&page));
            if !text.is_empty() {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
            }
        }
    }

    /// Starts the requested scroll and moves on with the running one.
    fn advance_scroll(&mut self, viewport: gpui::Size<Pixels>, window: &mut Window) {
        if let Some(request) = self.pending_scroll.take()
            && request.page < self.pages.len()
        {
            let target = self.page_top(request.page) + request.y * self.zoom;
            let height = f32::from(viewport.height);
            let current = f32::from(self.scroll.y);
            let to = match request.placement {
                Placement::Third => Some(target - height / 3.0),
                Placement::Center => {
                    let in_middle = target >= current + height * 0.25 && target <= current + height * 0.75;
                    (!in_middle).then_some(target - height / 2.0)
                }
                Placement::Top(margin) => Some(target - margin),
            };
            if let Some(to) = to {
                let to = to.clamp(0.0, (self.content_size().1 - height).max(0.0));
                self.scroll_animation = Some(ScrollAnimation { from: current, to, start: Instant::now() });
            }
        }
        if let Some(animation) = self.scroll_animation {
            let progress = (animation.start.elapsed().as_secs_f32() / SCROLL_TIME.as_secs_f32()).min(1.0);
            let eased = 1.0 - (1.0 - progress).powi(3);
            self.scroll.y = px(animation.from + (animation.to - animation.from) * eased);
            if progress < 1.0 {
                window.request_animation_frame();
            } else {
                self.scroll_animation = None;
            }
        }
    }

    fn prepare_paint(&mut self, bounds: Bounds<Pixels>, window: &mut Window) -> PaintList {
        self.bounds = Some(bounds);
        for image in self.pending_drop.drain(..) {
            window.drop_image(image).ok();
        }
        if self.pages.is_empty() {
            return PaintList { pages: Vec::new(), selection: Vec::new(), hovered_link: None, highlight: None };
        }
        if self.presenting {
            return self.prepare_slide(bounds, window);
        }
        if self.fit_width {
            let widest = self.pages.iter().map(|(width, _)| *width).fold(1.0, f32::max);
            self.zoom = ((f32::from(bounds.size.width) - SIDE_MARGIN * 2.0) / widest).clamp(MIN_ZOOM, MAX_ZOOM);
        }
        self.advance_scroll(bounds.size, window);
        self.clamp_scroll(bounds.size);

        let view_top = f32::from(self.scroll.y);
        let view_bottom = view_top + f32::from(bounds.size.height);
        let mut visible = Vec::new();
        let mut paint = Vec::new();
        let mut selection_bounds = Vec::new();
        let mut hovered_link = None;
        let mut highlight = None;
        let mut next_top = PAGE_GAP;
        for page in 0..self.pages.len() {
            let top = next_top;
            let (width, height) = self.pages[page];
            let (width, height) = (width * self.zoom, height * self.zoom);
            next_top = top + height + PAGE_GAP;
            if top > view_bottom {
                break;
            }
            if top + height < view_top {
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
            if let Some(selection) = &self.selection
                && let Some(content) = self.content.get(&page)
            {
                let rects = self.selection_rects.entry(page).or_insert_with(|| {
                    selection
                        .range_on(page, content.chars.len())
                        .map_or_else(Box::default, |range| pdf_text::selection_rects(&content.chars, range))
                });
                selection_bounds.extend(rects.iter().map(|rect| rect_bounds(rect, origin, self.zoom)));
            }
            if let Some((_, link)) = self.hovered_link.filter(|(link_page, _)| *link_page == page) {
                let rect = self.content.get(&page).and_then(|content| content.links.get(link));
                hovered_link = rect.map(|link| rect_bounds(&link.rect, origin, self.zoom));
            }
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
        self.keep_near(first, last, window.scale_factor());
        PaintList { pages: paint, selection: selection_bounds, hovered_link, highlight }
    }

    fn prepare_slide(&mut self, bounds: Bounds<Pixels>, window: &mut Window) -> PaintList {
        let page = self.present_page.min(self.pages.len() - 1);
        self.present_page = page;
        let viewport = (f32::from(bounds.size.width), f32::from(bounds.size.height));
        let (zoom, left, top) = fit_page(self.pages[page], viewport);
        self.zoom = zoom;
        let (width, height) = self.pages[page];
        let slide = Bounds::new(
            point(bounds.left() + px(left), bounds.top() + px(top)),
            size(px(width * zoom), px(height * zoom)),
        );
        let image = self.images.get(&page).map(|image| image.image.clone());
        self.keep_near(page, page, window.scale_factor());
        PaintList { pages: vec![(slide, image)], selection: Vec::new(), hovered_link: None, highlight: None }
    }

    /// Lets go of pages far from `first..=last` and asks for the ones in it.
    fn keep_near(&mut self, first: usize, last: usize, scale: f32) {
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
        // An open selection needs the text of its pages for copying.
        if self.selection.is_none() && self.press.is_none() {
            self.content.retain(|page, _| *page + KEEP_PAGES >= first && *page <= last + KEEP_PAGES);
            self.requested_content.retain(|page| *page + KEEP_PAGES >= first && *page <= last + KEEP_PAGES);
        }
        self.request_pages(first, last, scale);
    }

    /// The colors of dark pages. A light theme borrows the dark one, so that the
    /// button always gives dark pages.
    fn recolor(&self) -> Option<Recolor> {
        self.invert.then(|| {
            let palette = theme::palette();
            let palette = if theme::is_light(palette.bg) { theme::Palette::OVERLEAF_DARK } else { palette };
            Recolor { paper: palette.bg, ink: palette.code_text }
        })
    }

    /// Asks for visible pages first, then their neighbors, at the current resolution.
    fn request_pages(&mut self, first: usize, last: usize, scale: f32) {
        let is_zooming = self.zoomed_at.is_some();
        let recolor = self.recolor();
        let before = first.saturating_sub(1)..first;
        let after = (last + 1)..(last + 2).min(self.pages.len());
        let wanted: Vec<PageRequest> = (first..=last)
            .chain(after)
            .chain(before)
            .filter_map(|page| {
                let request = PageRequest {
                    page,
                    width_px: (self.pages[page].0 * self.zoom * scale).min(MAX_RENDER_WIDTH).round() as u32,
                    recolor,
                };
                let up_to_date = self.images.get(&page).is_some_and(|image| {
                    image.generation == self.generation
                        && (image.request == request || (is_zooming && image.request.recolor == recolor))
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

/// The zoom that fits a whole page into the viewport, and where the page then
/// starts so that it sits in the middle.
fn fit_page((width, height): (f32, f32), (view_width, view_height): (f32, f32)) -> (f32, f32, f32) {
    if width <= 0.0 || height <= 0.0 {
        return (1.0, 0.0, 0.0);
    }
    let zoom = (view_width / width).min(view_height / height).clamp(MIN_ZOOM, MAX_ZOOM);
    (zoom, (view_width - width * zoom) / 2.0, (view_height - height * zoom) / 2.0)
}

fn push_position(positions: &mut Vec<ViewPosition>, position: ViewPosition) {
    if positions.len() == MAX_HISTORY {
        positions.remove(0);
    }
    positions.push(position);
}

fn rect_bounds(rect: &Rect, origin: gpui::Point<Pixels>, zoom: f32) -> Bounds<Pixels> {
    Bounds::new(
        point(origin.x + px(rect.left * zoom), origin.y + px(rect.top * zoom)),
        size(px((rect.right - rect.left) * zoom), px((rect.bottom - rect.top) * zoom)),
    )
}

/// Where PDFium is: named by `EDITOR_PDFIUM_DIR`, next to the program, or in
/// the app bundle's `Frameworks` folder. Builds made with cargo also look in the
/// checkout's `vendor` folder.
pub(crate) fn library_directory() -> PathBuf {
    if let Some(directory) = std::env::var_os("EDITOR_PDFIUM_DIR") {
        return directory.into();
    }
    let name = pdfium_file_name();
    let binary_directory = std::env::current_exe().ok().and_then(|binary| binary.parent().map(Path::to_path_buf));
    let candidates = binary_directory.iter().flat_map(|directory| [directory.clone(), directory.join("../Frameworks")]);
    let vendor = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../vendor/pdfium/lib");
    let found = candidates.chain(cfg!(debug_assertions).then_some(vendor.clone())).find(|directory| directory.join(name).is_file());
    found.unwrap_or(vendor)
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
        let has_pages = !self.pages.is_empty();
        let over_link = self.hovered_link.is_some();
        let presenting = self.presenting;
        div()
            .id("pdf")
            .relative()
            .size_full()
            .overflow_hidden()
            .track_focus(&self.focus)
            .key_context(if presenting { "PdfView Presentation" } else { "PdfView" })
            .when(has_pages && !over_link && !presenting, |element| element.cursor_text())
            .when(over_link, |element| element.cursor_pointer())
            .on_action(cx.listener(|this, _: &actions::Copy, _, cx| this.copy_selection(cx)))
            .on_action(cx.listener(|this, _: &actions::PdfBack, _, cx| this.go_back(cx)))
            .on_action(cx.listener(|this, _: &actions::PdfForward, _, cx| this.go_forward(cx)))
            .on_action(cx.listener(|this, _: &actions::PdfNextPage, _, cx| this.turn_slide(true, cx)))
            .on_action(cx.listener(|this, _: &actions::PdfPreviousPage, _, cx| this.turn_slide(false, cx)))
            .on_scroll(cx.listener(Self::on_scroll))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(|this, _, _, _| this.press = None))
            .on_mouse_down(
                MouseButton::Navigate(NavigationDirection::Back),
                cx.listener(|this, _, _, cx| this.go_back(cx)),
            )
            .on_mouse_down(
                MouseButton::Navigate(NavigationDirection::Forward),
                cx.listener(|this, _, _, cx| this.go_forward(cx)),
            )
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
                            for rect in list.selection {
                                window.paint_quad(fill(rect, rgba(SELECTION_COLOR)));
                            }
                            if let Some(rect) = list.hovered_link {
                                window.paint_quad(fill(rect, rgba(LINK_HOVER_COLOR)));
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
                        .text_color(theme::color(theme::palette().ui_muted))
                        .child(message),
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_slide_fits_whole_and_sits_in_the_middle() {
        // A 4:3 Beamer slide on a 16:10 screen leaves room left and right.
        let (zoom, left, top) = fit_page((364.0, 273.0), (1440.0, 900.0));
        assert!((273.0 * zoom - 900.0).abs() < 0.01);
        assert!(364.0 * zoom <= 1440.0);
        assert!((left * 2.0 + 364.0 * zoom - 1440.0).abs() < 0.01);
        assert!(top.abs() < 0.01);
        // A portrait page in a wide view fits by height, a landscape one in a tall view by width.
        let (zoom, left, _) = fit_page((595.0, 842.0), (1000.0, 500.0));
        assert!((842.0 * zoom - 500.0).abs() < 0.01 && left > 0.0);
        let (zoom, _, top) = fit_page((842.0, 595.0), (500.0, 1000.0));
        assert!((842.0 * zoom - 500.0).abs() < 0.01 && top > 0.0);
    }

    #[test]
    fn an_empty_page_does_not_divide_by_zero() {
        assert_eq!(fit_page((0.0, 0.0), (800.0, 600.0)), (1.0, 0.0, 0.0));
    }
}
