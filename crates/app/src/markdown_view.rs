//! Draws the blocks of `markdown`, for add-on panels and for the preview of
//! Markdown files. Math comes as Unicode text, so no layout engine is needed.

use std::path::{Path, PathBuf};

use gpui::{AnyElement, FontStyle, FontWeight, HighlightStyle, InteractiveText, UnderlineStyle, ObjectFit, SharedString, StyledText, div, img, prelude::*, px, relative};

use crate::{
    image_view::is_image,
    markdown::{Align, Block, Inline},
    theme,
};

/// How big blocks are drawn: small in a side panel, as a page in the preview.
#[derive(Clone, Copy)]
pub struct Look {
    pub text_size: f32,
    pub headings: [f32; 3],
    pub gap: f32,
}

impl Look {
    pub const PANEL: Look = Look { text_size: theme::UI_FONT_SIZE, headings: [18., 16., 14.], gap: 6. };
    pub const PAGE: Look = Look { text_size: 16., headings: [28., 22., 18.], gap: 12. };

    pub fn heading_size(&self, level: u8) -> f32 {
        self.headings[(level as usize).clamp(1, 3) - 1]
    }
}

/// One block. `code_bar` gives a code block a bar above its text, as the panel's
/// copy and insert buttons; `base` is where image paths start from.
pub fn block(index: usize, block: &Block, look: Look, base: Option<&Path>, code_bar: impl FnOnce(&SharedString, &SharedString) -> Option<AnyElement>) -> AnyElement {
    let palette = theme::palette();
    match block {
        Block::Heading { level, text } => div()
            .flex_none()
            .pt(px(look.gap * 0.6))
            .text_size(px(look.heading_size(*level)))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(theme::color(palette.heading))
            .child(rich_text(index, text))
            .into_any_element(),
        Block::Paragraph(text) => div().flex_none().child(rich_text(index, text)).into_any_element(),
        Block::Quote(text) => div()
            .flex_none()
            .pl(px(look.gap + 2.))
            .border_l_2()
            .border_color(theme::color(palette.divider))
            .text_color(theme::color(palette.ui_muted))
            .child(rich_text(index, text))
            .into_any_element(),
        Block::Rule => div().flex_none().h(px(1.)).bg(theme::color(palette.divider)).into_any_element(),
        Block::List { ordered, start, items } => div()
            .flex()
            .flex_col()
            .flex_none()
            .gap(px(look.gap / 3.))
            .children(items.iter().enumerate().map(|(position, item)| {
                let marker = if *ordered { format!("{}.", *start as usize + position) } else { "\u{2022}".to_string() };
                div()
                    .flex()
                    .gap(px(6.))
                    .child(div().flex_none().min_w(px(look.text_size + 2.)).text_color(theme::color(palette.ui_muted)).child(marker))
                    .child(div().flex_1().min_w_0().child(rich_text(index * 1000 + position, item)))
            }))
            .into_any_element(),
        Block::Code { language, text } => div()
            .flex()
            .flex_col()
            .flex_none()
            .rounded(px(6.))
            .bg(theme::color(palette.panel))
            .border_1()
            .border_color(theme::color(palette.divider))
            .children(code_bar(language, text))
            .child(
                div()
                    .px(px(8.))
                    .py(px(6.))
                    .font_family(theme::code_font())
                    .text_size(px(theme::code_font_size()))
                    .child(text.clone()),
            )
            .into_any_element(),
        Block::Math { text, .. } => div()
            .flex_none()
            .flex()
            .flex_col()
            .items_center()
            .py(px(look.gap / 2.))
            .text_size(px(look.text_size * 1.15))
            .children(text.split('\n').map(|line| div().child(SharedString::from(line.to_string()))))
            .into_any_element(),
        Block::Table { align, header, rows } => table(index, align, header, rows, look),
        Block::Image { alt, url } => image(alt, url, base, look),
    }
}

fn table(index: usize, align: &[Align], header: &[Inline], rows: &[Vec<Inline>], look: Look) -> AnyElement {
    let palette = theme::palette();
    let columns = header.len().max(1);
    let row = |cells: &[Inline], number: usize, head: bool| {
        div()
            .flex()
            .when(!head, |row| row.border_t_1().border_color(theme::color(palette.divider)))
            .when(head, |row| row.font_weight(FontWeight::SEMIBOLD).bg(theme::color(palette.panel)))
            .children((0..columns).map(|column| {
                let cell = div().flex_1().min_w_0().px(px(8.)).py(px(look.gap / 3. + 1.)).flex();
                let cell = match align.get(column).copied().unwrap_or_default() {
                    Align::Left => cell.justify_start(),
                    Align::Center => cell.justify_center(),
                    Align::Right => cell.justify_end(),
                };
                cell.children(cells.get(column).map(|text| rich_text(index * 100_000 + number * 100 + column, text)))
            }))
    };
    div()
        .flex()
        .flex_col()
        .flex_none()
        .rounded(px(4.))
        .border_1()
        .border_color(theme::color(palette.divider))
        .overflow_hidden()
        .child(row(header, 0, true))
        .children(rows.iter().enumerate().map(|(number, cells)| row(cells, number + 1, false)))
        .into_any_element()
}

/// A local image the decoders know; any other shows its description.
fn image(alt: &SharedString, url: &str, base: Option<&Path>, look: Look) -> AnyElement {
    let palette = theme::palette();
    let path = base.filter(|_| !url.contains("://")).map(|base| base.join(url)).filter(|path| is_image(path));
    let caption = (!alt.is_empty()).then(|| div().text_size(px(look.text_size * 0.85)).text_color(theme::color(palette.ui_muted)).child(alt.clone()));
    match path {
        Some(path) => div()
            .flex()
            .flex_col()
            .flex_none()
            .items_center()
            .gap(px(4.))
            .child(img(PathBuf::from(path)).max_w(relative(1.)).max_h(px(480.)).object_fit(ObjectFit::Contain))
            .children(caption)
            .into_any_element(),
        None => div().flex_none().italic().text_color(theme::color(palette.ui_muted)).child(if alt.is_empty() { SharedString::from(url.to_string()) } else { alt.clone() }).into_any_element(),
    }
}

/// Text with its marks, and links that open in the browser.
pub fn rich_text(index: usize, text: &Inline) -> AnyElement {
    let palette = theme::palette();
    let highlights: Vec<_> = text
        .marks
        .iter()
        .map(|(range, marks)| {
            let mut style = HighlightStyle::default();
            if marks.bold {
                style.font_weight = Some(FontWeight::SEMIBOLD);
            }
            if marks.italic {
                style.font_style = Some(FontStyle::Italic);
            }
            if marks.code {
                style.background_color = Some(theme::color(palette.panel));
            }
            if marks.link.is_some() {
                style.color = Some(theme::color(palette.accent));
                style.underline = Some(UnderlineStyle { thickness: px(1.), color: Some(theme::color(palette.accent)), wavy: false });
            }
            (range.clone(), style)
        })
        .collect();
    let styled = StyledText::new(text.text.clone()).with_highlights(highlights);
    let links: Vec<_> = text.marks.iter().filter_map(|(range, marks)| Some((range.clone(), marks.link.clone()?))).collect();
    if links.is_empty() {
        return styled.into_any_element();
    }
    let ranges = links.iter().map(|(range, _)| range.clone()).collect();
    InteractiveText::new(("markdown-text", index), styled)
        .on_click(ranges, move |clicked, _, cx| {
            if let Some((_, url)) = links.get(clicked)
                && (url.starts_with("https://") || url.starts_with("http://"))
            {
                cx.open_url(url);
            }
        })
        .into_any_element()
}
