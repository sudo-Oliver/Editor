//! Shows an image from the file tree in place of the editor.

use std::{
    io::Cursor,
    path::{Component, Path, PathBuf},
    sync::Arc,
};

use build::Engine;
use gpui::{
    ClipboardItem, Context, Corners, EventEmitter, FocusHandle, FontWeight, ObjectFit, Render, RenderImage,
    SharedString, SvgRenderer, Task, Window, canvas, div, prelude::*, px,
};
use image::{
    DynamicImage, ImageResult,
    codecs::{jpeg::JpegDecoder, png::PngDecoder, webp::WebPDecoder},
};

use crate::theme;

const IMAGE_EXTENSIONS: [&str; 5] = ["png", "jpg", "jpeg", "webp", "svg"];
/// Device pixels per SVG unit, which keeps vector images sharp on HiDPI screens.
const SVG_SCALE: f32 = 2.0;

pub fn is_image(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| IMAGE_EXTENSIONS.iter().any(|known| extension.eq_ignore_ascii_case(known)))
}

pub enum ImageViewEvent {
    Status(String),
}

pub struct ImageView {
    path: PathBuf,
    /// Code that includes this image in the document.
    include: String,
    details: Option<String>,
    image: Option<Arc<RenderImage>>,
    error: Option<String>,
    focus: FocusHandle,
    _load_task: Task<()>,
    export_task: Option<Task<()>>,
}

impl EventEmitter<ImageViewEvent> for ImageView {}

impl ImageView {
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn new(path: PathBuf, include: String, cx: &mut Context<Self>) -> Self {
        let read_path = path.clone();
        let svg_renderer = cx.svg_renderer();
        let load_task = cx.spawn(async move |this, cx| {
            let loaded = cx.background_spawn(async move { load(&read_path, svg_renderer) }).await;
            this.update(cx, |this, cx| {
                match loaded {
                    Ok((image, details)) => {
                        this.image = Some(image);
                        this.details = Some(details);
                    }
                    Err(error) => this.error = Some(error),
                }
                cx.notify();
            })
            .ok();
        });
        cx.on_release(|this, cx| {
            if let Some(image) = this.image.take() {
                cx.drop_image(image, None);
            }
        })
        .detach();
        Self {
            path,
            include,
            details: None,
            image: None,
            error: None,
            focus: cx.focus_handle(),
            _load_task: load_task,
            export_task: None,
        }
    }

    pub fn focus_handle(&self) -> &FocusHandle {
        &self.focus
    }

    fn export(&mut self, cx: &mut Context<Self>) {
        let source = self.path.clone();
        let directory = source.parent().unwrap_or(Path::new("/")).to_path_buf();
        let name = source.file_name().and_then(|name| name.to_str()).map(str::to_string);
        let answer = cx.prompt_for_new_path(&directory, name.as_deref());
        self.export_task = Some(cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(target))) = answer.await else {
                return;
            };
            let shown = target.display().to_string();
            let copied = cx.background_spawn(async move { std::fs::copy(source, target) }).await;
            this.update(cx, |_, cx| {
                cx.emit(ImageViewEvent::Status(match copied {
                    Ok(_) => format!("Exported to {shown}"),
                    Err(error) => format!("Cannot export to {shown}: {error}"),
                }));
            })
            .ok();
        }));
    }

    fn action(
        &self,
        id: &'static str,
        label: &'static str,
        cx: &mut Context<Self>,
        handler: impl Fn(&mut Self, &mut Context<Self>) + 'static,
    ) -> impl IntoElement {
        div()
            .id(id)
            .flex_none()
            .px(px(10.))
            .py(px(4.))
            .rounded(px(4.))
            .cursor_pointer()
            .bg(theme::color(theme::palette().panel))
            .hover(|style| style.bg(theme::color(theme::palette().divider)))
            .on_click(cx.listener(move |this, _, _, cx| handler(this, cx)))
            .child(label)
    }
}

impl Render for ImageView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let name: SharedString = self
            .path
            .file_name()
            .map_or_else(String::new, |name| name.to_string_lossy().into_owned())
            .into();
        div()
            .flex()
            .flex_col()
            .size_full()
            .track_focus(&self.focus)
            .bg(theme::color(theme::palette().bg))
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(8.))
                    .p(px(8.))
                    .text_color(theme::color(theme::palette().ui_text))
                    .child(div().flex_none().font_weight(FontWeight::SEMIBOLD).child(name))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(theme::color(theme::palette().ui_muted))
                            .child(self.details.clone().unwrap_or_default()),
                    )
                    .child(self.action("copy-include", "Copy include", cx, |this, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(this.include.clone()));
                        cx.emit(ImageViewEvent::Status(format!("Copied {}", this.include)));
                    }))
                    .child(self.action("export-image", "Export…", cx, |this, cx| this.export(cx))),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .p(px(16.))
                    .when_some(self.image.clone(), |element, image| {
                        element.child(
                            canvas(
                                |_, _, _| {},
                                move |bounds, _, window, _| {
                                    let fitted = ObjectFit::ScaleDown.get_bounds(bounds, image.size(0));
                                    window.paint_image(bounds, fitted, Corners::default(), image, 0, false).ok();
                                },
                            )
                            .size_full(),
                        )
                    })
                    .when_some(self.error.clone(), |element, error| {
                        element.text_color(theme::color(theme::palette().ui_muted)).child(error)
                    }),
            )
    }
}

fn load(path: &Path, svg_renderer: SvgRenderer) -> Result<(Arc<RenderImage>, String), String> {
    let cannot = |error: String| format!("Cannot show this image: {error}");
    let bytes = std::fs::read(path).map_err(|error| cannot(error.to_string()))?;
    let extension = path.extension().map_or_else(String::new, |extension| extension.to_string_lossy().to_lowercase());
    if extension == "svg" {
        // An SVG has no fixed resolution.
        let image = svg_renderer.render_single_frame(&bytes, SVG_SCALE).map_err(|error| cannot(error.to_string()))?;
        return Ok((image, details(&extension, bytes.len(), None)));
    }
    let decoded = decode(&bytes, &extension).map_err(|error| cannot(error.to_string()))?;
    let mut pixels = decoded.into_rgba8();
    let resolution = pixels.dimensions();
    // GPUI wants BGRA.
    for pixel in pixels.as_chunks_mut::<4>().0 {
        pixel.swap(0, 2);
    }
    let image = Arc::new(RenderImage::new([image::Frame::new(pixels)]));
    Ok((image, details(&extension, bytes.len(), Some(resolution))))
}

/// Names the codecs, because the generic `image` functions link every format that
/// GPUI turns on, which adds megabytes to the app.
fn decode(bytes: &[u8], extension: &str) -> ImageResult<DynamicImage> {
    let reader = Cursor::new(bytes);
    match extension {
        "png" => DynamicImage::from_decoder(PngDecoder::new(reader)?),
        "webp" => DynamicImage::from_decoder(WebPDecoder::new(reader)?),
        _ => DynamicImage::from_decoder(JpegDecoder::new(reader)?),
    }
}

fn details(extension: &str, bytes: usize, resolution: Option<(u32, u32)>) -> String {
    let format = extension.to_uppercase();
    let size = human_size(bytes as u64);
    match resolution {
        Some((width, height)) => format!("{format} · {width} × {height} · {size}"),
        None => format!("{format} · {size}"),
    }
}

fn human_size(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    let value = bytes as f64;
    if value < KIB {
        format!("{bytes} B")
    } else if value < KIB * KIB {
        format!("{:.1} KB", value / KIB)
    } else {
        format!("{:.1} MB", value / KIB / KIB)
    }
}

/// The path from a directory to a file, with forward slashes for all engines.
pub fn relative_path(directory: &Path, file: &Path) -> String {
    let from: Vec<Component> = directory.components().collect();
    let to: Vec<Component> = file.components().collect();
    let shared = from.iter().zip(&to).take_while(|(left, right)| left == right).count();
    let up = std::iter::repeat_n("..".to_string(), from.len() - shared);
    let down = to[shared..].iter().map(|part| part.as_os_str().to_string_lossy().into_owned());
    up.chain(down).collect::<Vec<_>>().join("/")
}

/// The code that includes an image. Without a known engine, LaTeX is the best guess.
pub fn include_snippet(engine: Option<Engine>, relative: &str) -> String {
    match engine {
        Some(Engine::Typst) => format!("#image(\"{}\")", relative.replace('\\', "\\\\").replace('"', "\\\"")),
        Some(Engine::Pandoc) if relative.contains(' ') => format!("![](<{relative}>)"),
        Some(Engine::Pandoc) => format!("![]({relative})"),
        _ if relative.to_ascii_lowercase().ends_with(".svg") => format!("\\includesvg{{{relative}}}"),
        _ => format!("\\includegraphics{{{relative}}}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn images_are_recognized_in_any_case() {
        assert!(is_image(Path::new("a/photo.PNG")));
        assert!(is_image(Path::new("logo.svg")));
        assert!(!is_image(Path::new("main.tex")));
        assert!(!is_image(Path::new("png")));
    }

    #[test]
    fn snippets_fit_the_engine() {
        let photo = "images/photo.png";
        assert_eq!(include_snippet(Some(Engine::PdfLatex), photo), "\\includegraphics{images/photo.png}");
        assert_eq!(include_snippet(None, photo), "\\includegraphics{images/photo.png}");
        assert_eq!(include_snippet(Some(Engine::Tectonic), "a.SVG"), "\\includesvg{a.SVG}");
        assert_eq!(include_snippet(Some(Engine::Typst), photo), "#image(\"images/photo.png\")");
        assert_eq!(include_snippet(Some(Engine::Typst), "a\"b.png"), "#image(\"a\\\"b.png\")");
        assert_eq!(include_snippet(Some(Engine::Pandoc), photo), "![](images/photo.png)");
        assert_eq!(include_snippet(Some(Engine::Pandoc), "my photo.png"), "![](<my photo.png>)");
    }

    #[test]
    fn relative_paths_leave_the_directory_when_needed() {
        assert_eq!(relative_path(Path::new("/p"), Path::new("/p/images/a.png")), "images/a.png");
        assert_eq!(relative_path(Path::new("/p/src"), Path::new("/p/images/a.png")), "../images/a.png");
        assert_eq!(relative_path(Path::new("/p"), Path::new("/p/a.png")), "a.png");
    }

    #[test]
    fn sizes_are_readable() {
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(2048), "2.0 KB");
        assert_eq!(human_size(3 * 1024 * 1024), "3.0 MB");
    }

    #[test]
    fn every_raster_format_decodes() {
        let directory = tempfile::tempdir().unwrap();
        for extension in ["png", "jpg", "webp"] {
            let path = directory.path().join(format!("a.{extension}"));
            image::RgbImage::new(3, 2).save(&path).unwrap();
            let decoded = decode(&std::fs::read(&path).unwrap(), extension).unwrap();
            assert_eq!((decoded.width(), decoded.height()), (3, 2), "{extension}");
        }
    }

    #[test]
    fn a_broken_file_is_an_error() {
        assert!(decode(b"not an image", "png").is_err());
    }

    #[test]
    fn details_show_the_resolution_when_there_is_one() {
        assert_eq!(details("png", 2048, Some((1920, 1080))), "PNG · 1920 × 1080 · 2.0 KB");
        assert_eq!(details("svg", 6, None), "SVG · 6 B");
    }
}
