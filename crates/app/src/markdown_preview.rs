//! The preview of a Markdown file: its blocks drawn as a page beside the
//! editor, without a browser and without a build. Only the blocks on screen
//! are laid out, and an edit reads the text again only from the block it is in.

use std::path::{Path, PathBuf};

use gpui::{AnyElement, Context, EntityId, EventEmitter, ListAlignment, ListState, MouseButton, Render, Window, div, list, prelude::*, px};

use crate::{
    markdown::Document,
    markdown_view::{self, Look},
    theme,
};

/// The bold of iA Writer Quattro claims a regular weight, so the page takes
/// the system font, which has every weight.
#[cfg(target_os = "macos")]
const PAGE_FONT: Option<&str> = Some(".SystemUIFont");
#[cfg(not(target_os = "macos"))]
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
}

impl EventEmitter<MarkdownPreviewEvent> for MarkdownPreview {}

impl MarkdownPreview {
    pub fn new(_cx: &mut Context<Self>) -> Self {
        Self { document: Document::default(), shown: None, base: None, list: ListState::new(0, ListAlignment::Top, px(600.)) }
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
            self.document = Document::default();
        }
        let (replaced, count) = self.document.update(text);
        if other {
            self.list.reset(self.document.blocks.len());
        } else {
            self.list.splice(replaced, count);
        }
        cx.notify();
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
            .child(div().w_full().max_w(px(PAGE_WIDTH)).min_w_0().child(markdown_view::block(index, block, look, self.base.as_deref(), |_, _| None)))
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
