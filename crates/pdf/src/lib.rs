//! Renders PDF pages with PDFium. All PDFium handles stay on one thread, which
//! every `PdfWorker` of the process shares.

mod content;
mod recolor;

pub use content::{LinkTarget, PageChar, PageContent, PageLink, Rect};
pub use recolor::Recolor;
use recolor::PixelRect;

use std::{
    collections::{HashMap, VecDeque},
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc::{self, RecvError, TryRecvError},
    },
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
    Outline { generation: u64, entries: Vec<OutlineEntry> },
    Failed { generation: u64, message: String },
}

/// A bookmark of the PDF, such as a chapter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutlineEntry {
    pub title: String,
    /// Zero for the top level.
    pub level: usize,
    pub page: usize,
}

/// Bookmarks deeper than this stay out of the outline.
const OUTLINE_DEPTH: usize = 5;
const OUTLINE_LIMIT: usize = 500;

enum Command {
    Load { generation: u64, bytes: Vec<u8> },
    Open { generation: u64, path: PathBuf },
    Outline { generation: u64 },
    Render { generation: u64, pages: Vec<PageRequest> },
    Content { generation: u64, page: usize },
}

/// What the PDFium thread hears from its workers.
enum Message {
    Attach { client: u64, events: UnboundedSender<PdfEvent> },
    Detach { client: u64 },
    Command { client: u64, command: Command },
}

/// The one PDFium thread of the process. PDFium holds a global lock from its
/// start to its end, so a second instance would wait forever; workers share it.
static SHARED: Mutex<Option<mpsc::Sender<Message>>> = Mutex::new(None);
static NEXT_CLIENT: AtomicU64 = AtomicU64::new(0);

/// One document on the shared thread, with its own events. Dropping it frees the document.
pub struct PdfWorker {
    client: u64,
    messages: mpsc::Sender<Message>,
}

impl PdfWorker {
    /// Starts the PDFium thread unless it runs already, and joins it.
    /// `library` is the directory that holds the PDFium library.
    pub fn spawn(library: &Path) -> Result<(Self, UnboundedReceiver<PdfEvent>), String> {
        let client = NEXT_CLIENT.fetch_add(1, Ordering::Relaxed);
        let (events, event_receiver) = unbounded_channel();
        let mut shared = SHARED.lock().unwrap_or_else(|error| error.into_inner());
        let mut attach = Message::Attach { client, events };
        if let Some(messages) = shared.as_ref() {
            match messages.send(attach) {
                Ok(()) => return Ok((Self { client, messages: messages.clone() }, event_receiver)),
                // The thread is gone; start it again.
                Err(mpsc::SendError(message)) => attach = message,
            }
        }
        let messages = start(library)?;
        messages.send(attach).map_err(|_| "The PDF thread stopped.".to_string())?;
        *shared = Some(messages.clone());
        Ok((Self { client, messages }, event_receiver))
    }

    fn send(&self, command: Command) {
        self.messages.send(Message::Command { client: self.client, command }).ok();
    }

    pub fn load(&self, generation: u64, bytes: Vec<u8>) {
        self.send(Command::Load { generation, bytes });
    }

    /// Opens a PDF file. PDFium then reads only the parts it needs, which for a
    /// long book is far less than the whole file.
    pub fn open(&self, generation: u64, path: PathBuf) {
        self.send(Command::Open { generation, path });
    }

    /// Asks for the bookmarks, answered with `PdfEvent::Outline`.
    pub fn outline(&self, generation: u64) {
        self.send(Command::Outline { generation });
    }

    /// Replaces the pending page list, so that scrolling drops outdated work.
    pub fn render(&self, generation: u64, pages: Vec<PageRequest>) {
        self.send(Command::Render { generation, pages });
    }

    /// Asks for the text and links of a page. Commands go before queued renders, so
    /// the page under the pointer does not wait for scrolling to finish.
    pub fn content(&self, generation: u64, page: usize) {
        self.send(Command::Content { generation, page });
    }
}

impl Drop for PdfWorker {
    fn drop(&mut self) {
        self.messages.send(Message::Detach { client: self.client }).ok();
    }
}

fn start(library: &Path) -> Result<mpsc::Sender<Message>, String> {
    let library = library.to_path_buf();
    let (messages, receiver) = mpsc::channel();
    let (ready, ready_receiver) = mpsc::channel();
    std::thread::Builder::new()
        .name("pdfium".into())
        .spawn(move || {
            let path = Pdfium::pdfium_platform_library_name_at_path(&library);
            match Pdfium::bind_to_library(path) {
                Ok(bindings) => {
                    ready.send(Ok(())).ok();
                    run(Pdfium::new(bindings), receiver);
                }
                Err(error) => {
                    let message = format!("Cannot load PDFium from {}: {error}", library.display());
                    ready.send(Err(message)).ok();
                }
            }
        })
        .map_err(|error| format!("Cannot start the PDF thread: {error}"))?;
    ready_receiver.recv().map_err(|_| "The PDF thread stopped.".to_string())??;
    Ok(messages)
}

/// A worker's document and the pages it still wants.
struct Client<'a> {
    events: UnboundedSender<PdfEvent>,
    document: Option<(u64, PdfDocument<'a>)>,
    // Found on the first dark render of a page, since it walks all objects.
    images: HashMap<usize, Vec<Rect>>,
    queue: VecDeque<PageRequest>,
}

fn run(pdfium: Pdfium, messages: mpsc::Receiver<Message>) {
    let mut clients: HashMap<u64, Client> = HashMap::new();
    loop {
        let busy = clients.values().any(|client| !client.queue.is_empty());
        let next = if busy {
            match messages.try_recv() {
                Ok(message) => Ok(message),
                Err(TryRecvError::Empty) => {
                    // A page for each worker in turn, so that one long document
                    // does not hold up the others.
                    for client in clients.values_mut() {
                        if let (Some((generation, document)), Some(request)) = (&client.document, client.queue.pop_front()) {
                            render_page(document, *generation, request, &mut client.images, &client.events);
                        }
                    }
                    continue;
                }
                Err(TryRecvError::Disconnected) => Err(RecvError),
            }
        } else {
            messages.recv()
        };
        let Ok(message) = next else {
            return;
        };
        let (id, command) = match message {
            Message::Attach { client, events } => {
                clients.insert(client, Client { events, document: None, images: HashMap::new(), queue: VecDeque::new() });
                continue;
            }
            Message::Detach { client } => {
                clients.remove(&client);
                continue;
            }
            Message::Command { client, command } => (client, command),
        };
        let Some(client) = clients.get_mut(&id) else {
            continue;
        };
        match command {
            Command::Load { .. } | Command::Open { .. } => {
                client.queue.clear();
                client.document = None;
                client.images.clear();
                let (generation, loaded) = match command {
                    Command::Load { generation, bytes } => (generation, pdfium.load_pdf_from_byte_vec(bytes, None)),
                    Command::Open { generation, path } => (generation, pdfium.load_pdf_from_file(&path, None)),
                    _ => unreachable!(),
                };
                match loaded {
                    Ok(loaded) => {
                        let pages = page_sizes(&loaded);
                        client.document = Some((generation, loaded));
                        client.events.send(PdfEvent::Loaded { generation, pages }).ok();
                    }
                    Err(error) => {
                        client
                            .events
                            .send(PdfEvent::Failed { generation, message: format!("Cannot read the PDF: {error}") })
                            .ok();
                    }
                }
            }
            Command::Render { generation, pages } => {
                if client.document.as_ref().is_some_and(|(current, _)| *current == generation) {
                    client.queue = pages.into();
                }
            }
            Command::Content { generation, page } => {
                if let Some((current, document)) = &client.document
                    && *current == generation
                {
                    send_content(document, generation, page, &client.events);
                }
            }
            Command::Outline { generation } => {
                if let Some((current, document)) = &client.document
                    && *current == generation
                {
                    let mut entries = Vec::new();
                    outline(document.bookmarks().root(), 0, &mut entries);
                    client.events.send(PdfEvent::Outline { generation, entries }).ok();
                }
            }
        }
    }
}

/// Walks `first` and its siblings, and their children down to `OUTLINE_DEPTH`.
/// PDFium's own sibling iterator leaves out the bookmark it starts from.
fn outline(first: Option<PdfBookmark>, level: usize, entries: &mut Vec<OutlineEntry>) {
    let mut next = first;
    while let Some(bookmark) = next {
        if entries.len() >= OUTLINE_LIMIT {
            return;
        }
        let page = match bookmark.destination() {
            Some(destination) => destination.page_index().ok(),
            None => match bookmark.action() {
                Some(PdfAction::LocalDestination(action)) => action.destination().ok().and_then(|destination| destination.page_index().ok()),
                _ => None,
            },
        };
        if let (Some(title), Some(page)) = (bookmark.title(), page) {
            entries.push(OutlineEntry { title: title.trim().to_string(), level, page: page as usize });
        }
        if level + 1 < OUTLINE_DEPTH {
            outline(bookmark.first_child(), level + 1, entries);
        }
        next = bookmark.next_sibling();
    }
}

/// Asks for the sizes without opening the pages, which takes a thousandth of the time.
fn page_sizes(document: &PdfDocument) -> Vec<(f32, f32)> {
    let pages = document.pages();
    (0..pages.len())
        .map(|index| pages.page_size(index).map_or((0.0, 0.0), |size| (size.width().value, size.height().value)))
        .collect()
}

fn send_content(
    document: &PdfDocument,
    generation: u64,
    page: usize,
    events: &UnboundedSender<PdfEvent>,
) {
    let content = u16::try_from(page)
        .map_err(|_| PdfiumError::PageIndexOutOfBounds)
        .and_then(|index| document.pages().get(index))
        .and_then(|loaded| content::extract(&loaded, document))
        // A page without readable text still shows, it only cannot be selected.
        .unwrap_or_default();
    events.send(PdfEvent::PageContent { generation, page, content }).ok();
}

fn render_page(
    document: &PdfDocument,
    generation: u64,
    request: PageRequest,
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
                    content::PageFrame::of(page).map_or_else(|_| Vec::new(), |frame| content::image_rects(page, &frame))
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

#[cfg(test)]
mod tests {
    use super::*;

    const ROTATED_AND_CROPPED: &[u8] = b"%PDF-1.4
1 0 obj<</Type/Catalog/Pages 2 0 R>>endobj
2 0 obj<</Type/Pages/Kids[3 0 R 4 0 R]/Count 2>>endobj
3 0 obj<</Type/Page/Parent 2 0 R/MediaBox[0 0 200 100]/Rotate 90>>endobj
4 0 obj<</Type/Page/Parent 2 0 R/MediaBox[0 0 300 400]/CropBox[0 0 150 120]>>endobj
trailer<</Root 1 0 R>>";

    /// Two chapters, the first with a section, each bookmark on its own page.
    const BOOKMARKED: &[u8] = b"%PDF-1.4
1 0 obj<</Type/Catalog/Pages 2 0 R/Outlines 5 0 R>>endobj
2 0 obj<</Type/Pages/Kids[3 0 R 4 0 R]/Count 2>>endobj
3 0 obj<</Type/Page/Parent 2 0 R/MediaBox[0 0 200 200]>>endobj
4 0 obj<</Type/Page/Parent 2 0 R/MediaBox[0 0 200 200]>>endobj
5 0 obj<</Type/Outlines/First 6 0 R/Last 7 0 R/Count 3>>endobj
6 0 obj<</Title(One)/Parent 5 0 R/Next 7 0 R/First 8 0 R/Last 8 0 R/Count 1/Dest[3 0 R/Fit]>>endobj
7 0 obj<</Title(Two)/Parent 5 0 R/Prev 6 0 R/Dest[4 0 R/Fit]>>endobj
8 0 obj<</Title(Inner)/Parent 6 0 R/Dest[4 0 R/Fit]>>endobj
trailer<</Root 1 0 R>>";

    #[test]
    fn the_outline_keeps_the_first_bookmark_of_each_level() {
        let library = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../vendor/pdfium/lib");
        let Ok((worker, mut events)) = PdfWorker::spawn(&library) else {
            return;
        };
        worker.load(0, BOOKMARKED.to_vec());
        worker.outline(0);
        let entries = loop {
            match events.blocking_recv() {
                Some(PdfEvent::Outline { entries, .. }) => break entries,
                Some(PdfEvent::Failed { message, .. }) => panic!("{message}"),
                Some(_) => {}
                None => panic!("the PDF thread stopped"),
            }
        };
        let entry = |title: &str, level, page| OutlineEntry { title: title.to_string(), level, page };
        assert_eq!(entries, [entry("One", 0, 0), entry("Inner", 1, 1), entry("Two", 0, 1)]);
    }

    #[test]
    fn sizes_match_the_opened_pages() {
        // Through the shared thread, as PDFium allows one instance per process.
        let library = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../vendor/pdfium/lib");
        let Ok((worker, mut events)) = PdfWorker::spawn(&library) else {
            return;
        };
        worker.load(0, ROTATED_AND_CROPPED.to_vec());
        let pages = loop {
            match events.blocking_recv() {
                Some(PdfEvent::Loaded { pages, .. }) => break pages,
                Some(PdfEvent::Failed { message, .. }) => panic!("{message}"),
                Some(_) => {}
                None => panic!("the PDF thread stopped"),
            }
        };
        // Rotated by 90 degrees, and cropped: the sizes of the pages as opened.
        assert_eq!(pages, [(100.0, 200.0), (150.0, 120.0)]);
    }
}

