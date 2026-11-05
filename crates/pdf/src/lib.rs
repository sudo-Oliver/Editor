//! Renders PDF pages with PDFium. All PDFium handles stay on one worker thread.

mod content;

pub use content::{LinkTarget, PageChar, PageContent, PageLink, Rect};

use std::{
    collections::VecDeque,
    path::Path,
    sync::mpsc::{self, RecvError, TryRecvError},
};

use pdfium_render::prelude::*;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PageRequest {
    pub page: usize,
    pub width_px: u32,
    pub invert: bool,
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
                        render_page(document, *generation, request, &events);
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
                match pdfium.load_pdf_from_byte_vec(bytes, None) {
                    Ok(loaded) => {
                        let pages = page_sizes(&loaded);
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
                    send_content(document, generation, page, &events);
                }
            }
        }
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
            if request.invert {
                for pixel in bgra.as_chunks_mut::<4>().0 {
                    pixel[0] = 255 - pixel[0];
                    pixel[1] = 255 - pixel[1];
                    pixel[2] = 255 - pixel[2];
                }
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

    #[test]
    fn sizes_match_the_opened_pages() {
        let library = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../vendor/pdfium/lib");
        let Ok(bindings) = Pdfium::bind_to_library(Pdfium::pdfium_platform_library_name_at_path(&library)) else {
            return;
        };
        let pdfium = Pdfium::new(bindings);
        let document = pdfium.load_pdf_from_byte_slice(ROTATED_AND_CROPPED, None).unwrap();
        let opened: Vec<(f32, f32)> = document.pages().iter().map(|page| (page.width().value, page.height().value)).collect();
        assert_eq!(page_sizes(&document), opened);
        assert_eq!(opened, [(100.0, 200.0), (150.0, 120.0)]);
    }
}
