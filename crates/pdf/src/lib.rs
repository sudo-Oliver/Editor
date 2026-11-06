//! Renders PDF pages with PDFium. All PDFium handles stay on one worker thread.

mod content;
mod recolor;

use content::PageFrame;
pub use content::{LinkTarget, PageChar, PageContent, PageLink, Rect};
pub use recolor::Recolor;
use recolor::PixelRect;

use std::{
    collections::{HashMap, VecDeque},
    path::Path,
    sync::mpsc::{self, RecvError, TryRecvError},
};

use pdfium_render::prelude::*;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PageRequest {
    pub page: usize,
    pub width_px: u32,
    /// Draws the page in dark colors, see `recolor`.
    pub recolor: Option<Recolor>,
}

#[derive(Debug)]
pub enum PdfEvent {
    /// `pages` holds the size of each page in PDF points.
    Loaded { generation: u64, pages: Vec<(f32, f32)> },
    Rendered { generation: u64, request: PageRequest, height_px: u32, bgra: Vec<u8> },
    PageContent { generation: u64, page: usize, content: PageContent },
    Failed { generation: u64, message: String },
}

enum Command {
    Load { generation: u64, bytes: Vec<u8> },
    Render { generation: u64, pages: Vec<PageRequest> },
    Content { generation: u64, page: usize },
}

pub struct PdfWorker {
    commands: mpsc::Sender<Command>,
}

impl PdfWorker {
    /// Starts the worker. `library` is the directory that holds the PDFium library.
    pub fn spawn(library: &Path) -> Result<(Self, UnboundedReceiver<PdfEvent>), String> {
        let library = library.to_path_buf();
        let (commands, receiver) = mpsc::channel();
        let (events, event_receiver) = unbounded_channel();
        let (ready, ready_receiver) = mpsc::channel();
        std::thread::Builder::new()
            .name("pdfium".into())
            .spawn(move || {
                let path = Pdfium::pdfium_platform_library_name_at_path(&library);
                match Pdfium::bind_to_library(path) {
                    Ok(bindings) => {
                        ready.send(Ok(())).ok();
                        run(Pdfium::new(bindings), receiver, events);
                    }
                    Err(error) => {
                        let message = format!("Cannot load PDFium from {}: {error}", library.display());
                        ready.send(Err(message)).ok();
                    }
                }
            })
            .map_err(|error| format!("Cannot start the PDF thread: {error}"))?;
        ready_receiver
            .recv()
            .map_err(|_| "The PDF thread stopped.".to_string())??;
        Ok((Self { commands }, event_receiver))
    }

    pub fn load(&self, generation: u64, bytes: Vec<u8>) {
        self.commands.send(Command::Load { generation, bytes }).ok();
    }

    /// Replaces the pending page list, so that scrolling drops outdated work.
    pub fn render(&self, generation: u64, pages: Vec<PageRequest>) {
        self.commands.send(Command::Render { generation, pages }).ok();
    }

    /// Asks for the text and links of a page. Commands go before queued renders, so
    /// the page under the pointer does not wait for scrolling to finish.
    pub fn content(&self, generation: u64, page: usize) {
        self.commands.send(Command::Content { generation, page }).ok();
    }
}

fn run(pdfium: Pdfium, commands: mpsc::Receiver<Command>, events: UnboundedSender<PdfEvent>) {
    let mut document: Option<(u64, PdfDocument)> = None;
    let mut frames: Vec<PageFrame> = Vec::new();
    // Found on the first dark render of a page, since it walks all objects.
    let mut images: HashMap<usize, Vec<Rect>> = HashMap::new();
    let mut queue: VecDeque<PageRequest> = VecDeque::new();
    let mut pending: Option<Command> = None;
    loop {
        let next = match pending.take() {
            Some(command) => Ok(command),
            None if queue.is_empty() => commands.recv(),
            None => match commands.try_recv() {
                Ok(command) => Ok(command),
                Err(TryRecvError::Empty) => {
                    if let (Some((generation, document)), Some(request)) = (&document, queue.pop_front()) {
                        render_page(document, *generation, request, &frames, &mut images, &events);
                    }
                    continue;
                }
                Err(TryRecvError::Disconnected) => Err(RecvError),
            },
        };
        let Ok(command) = next else {
            return;
        };
        match command {
            Command::Load { generation, bytes } => {
                queue.clear();
                document = None;
                images.clear();
                match pdfium.load_pdf_from_byte_vec(bytes, None) {
                    Ok(loaded) => {
                        let pages: Vec<(f32, f32)> = loaded
                            .pages()
                            .iter()
                            .map(|page| (page.width().value, page.height().value))
                            .collect();
                        frames = loaded.pages().iter().filter_map(|page| PageFrame::of(&page).ok()).collect();
                        document = Some((generation, loaded));
                        events.send(PdfEvent::Loaded { generation, pages }).ok();
                    }
                    Err(error) => {
                        events
                            .send(PdfEvent::Failed { generation, message: format!("Cannot read the PDF: {error}") })
                            .ok();
                    }
                }
            }
            Command::Render { generation, pages } => {
                if document.as_ref().is_some_and(|(current, _)| *current == generation) {
                    queue = pages.into();
                }
            }
            Command::Content { generation, page } => {
                if let Some((current, document)) = &document
                    && *current == generation
                {
                    send_content(document, generation, page, &frames, &events);
                }
            }
        }
    }
}

fn send_content(
    document: &PdfDocument,
    generation: u64,
    page: usize,
    frames: &[PageFrame],
    events: &UnboundedSender<PdfEvent>,
) {
    let content = u16::try_from(page)
        .map_err(|_| PdfiumError::PageIndexOutOfBounds)
        .and_then(|index| document.pages().get(index))
        .and_then(|loaded| {
            let frame = frames.get(page).ok_or(PdfiumError::PageIndexOutOfBounds)?;
            content::extract(&loaded, frame, frames)
        })
        // A page without readable text still shows, it only cannot be selected.
        .unwrap_or_default();
    events.send(PdfEvent::PageContent { generation, page, content }).ok();
}

fn render_page(
    document: &PdfDocument,
    generation: u64,
    request: PageRequest,
    frames: &[PageFrame],
    images: &mut HashMap<usize, Vec<Rect>>,
    events: &UnboundedSender<PdfEvent>,
) {
    let config = PdfRenderConfig::new()
        .set_target_width(request.width_px as Pixels)
        .set_format(PdfBitmapFormat::BGRA)
        // pdfium-render turns this on by default, which swaps red and blue.
        .set_reverse_byte_order(false);
    let page = u16::try_from(request.page)
        .map_err(|_| PdfiumError::PageIndexOutOfBounds)
        .and_then(|index| document.pages().get(index));
    let rendered = page.as_ref().map_err(|_| PdfiumError::PageIndexOutOfBounds).and_then(|page| page.render_with_config(&config));
    match rendered {
        Ok(bitmap) => {
            let mut bgra = bitmap.as_raw_bytes();
            if let (Some(colors), Ok(page)) = (request.recolor, &page) {
                let rects = images.entry(request.page).or_insert_with(|| {
                    frames.get(request.page).map_or_else(Vec::new, |frame| content::image_rects(page, frame))
                });
                let (width, height) = (bitmap.width() as u32, bitmap.height() as u32);
                let scale = width as f32 / page.width().value;
                let to_pixel = |value: f32, limit: u32| (value * scale).clamp(0., limit as f32);
                let keep: Vec<PixelRect> = rects
                    .iter()
                    .map(|rect| PixelRect {
                        left: to_pixel(rect.left, width).floor() as u32,
                        top: to_pixel(rect.top, height).floor() as u32,
                        right: to_pixel(rect.right, width).ceil() as u32,
                        bottom: to_pixel(rect.bottom, height).ceil() as u32,
                    })
                    .collect();
                recolor::recolor(&mut bgra, width, colors, &keep);
            }
            events
                .send(PdfEvent::Rendered {
                    generation,
                    request,
                    height_px: bitmap.height() as u32,
                    bgra,
                })
                .ok();
        }
        Err(error) => {
            events
                .send(PdfEvent::Failed {
                    generation,
                    message: format!("Cannot render page {}: {error}", request.page + 1),
                })
                .ok();
        }
    }
}
