//! The NvChad statusline with `theme = "default"`. The separators are drawn,
//! because the app must not depend on a Nerd Font.

use editor_core::vim::Mode;
use gpui::{Bounds, FontWeight, Path as GpuiPath, Pixels, canvas, div, point, prelude::*, px, svg};

use crate::theme;

const HEIGHT: f32 = 22.0;
const ROUND_STEPS: usize = 14;

pub struct StatusInfo {
    pub mode: Mode,
    pub pending: String,
    pub file: Option<String>,
    pub file_icon: (&'static str, u32),
    pub dirty: bool,
    pub branch: Option<String>,
    pub errors: usize,
    pub warnings: usize,
    pub project: String,
    pub line: usize,
    pub column: usize,
    pub command_line: Option<String>,
}

fn mode_color(mode: Mode) -> u32 {
    match mode {
        Mode::Normal => theme::ST_NORMAL,
        Mode::Insert => theme::ST_INSERT,
        Mode::Visual | Mode::VisualLine => theme::ST_VISUAL,
        Mode::Command => theme::ST_GREEN,
    }
}

/// `U+E0BC`: a triangle in the upper left corner, in `color`, on `background`.
fn slant(color: u32, background: u32) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |bounds: Bounds<Pixels>, _, window, _| {
            let mut path = GpuiPath::new(bounds.origin);
            path.line_to(point(bounds.right(), bounds.top()));
            path.line_to(point(bounds.left(), bounds.bottom()));
            window.paint_path(path, theme::color(color));
        },
    )
    .flex_none()
    .w(px(HEIGHT / 2.))
    .h_full()
    .bg(theme::color(background))
}

/// `U+E0B6`: a half circle that bulges to the left, in `color`, on `background`.
fn round(color: u32, background: u32) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |bounds: Bounds<Pixels>, _, window, _| {
            let radius = bounds.size.height / 2.;
            let center = point(bounds.right(), bounds.top() + radius);
            let mut path = GpuiPath::new(point(bounds.right(), bounds.top()));
            for step in 1..=ROUND_STEPS {
                let angle = std::f32::consts::PI * step as f32 / ROUND_STEPS as f32;
                path.line_to(point(center.x - radius * angle.sin(), center.y - radius * angle.cos()));
            }
            window.paint_path(path, theme::color(color));
        },
    )
    .flex_none()
    .w(px(HEIGHT / 2.))
    .h_full()
    .bg(theme::color(background))
}

fn segment(background: u32, color: u32) -> gpui::Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .h_full()
        .px(px(6.))
        .gap(px(5.))
        .bg(theme::color(background))
        .text_color(theme::color(color))
}

pub fn render(info: StatusInfo) -> impl IntoElement {
    let color = mode_color(info.mode);
    let file_label = info.file.map(|name| if info.dirty { format!("{name} [+]") } else { name });
    let (file_icon, file_color) = info.file_icon;
    let position = format!("{}/{}", info.line, info.column);
    div()
        .flex()
        .flex_col()
        .flex_none()
        .font_family(theme::CODE_FONT)
        .text_size(px(theme::CODE_FONT_SIZE))
        .child(
            div()
                .flex()
                .items_center()
                .h(px(HEIGHT))
                .bg(theme::color(theme::ST_BG))
                .child(
                    segment(color, theme::ST_BLACK)
                        .pl(px(10.))
                        .font_weight(FontWeight::BOLD)
                        .child(
                            svg()
                                .path("icons/vim.svg")
                                .size(px(13.))
                                .flex_none()
                                .text_color(theme::color(theme::ST_BLACK)),
                        )
                        .child(info.mode.label()),
                )
                .child(slant(color, theme::ST_GREY))
                .child(slant(theme::ST_GREY, theme::ST_LIGHT))
                .child(segment(theme::ST_LIGHT, theme::ST_TEXT).when_some(file_label, |element, label| {
                    element
                        .child(
                            svg()
                                .path(file_icon)
                                .size(px(13.))
                                .flex_none()
                                .text_color(theme::color(file_color)),
                        )
                        .child(label)
                }))
                .child(slant(theme::ST_LIGHT, theme::ST_BG))
                .when_some(info.branch, |element, branch| {
                    element.child(segment(theme::ST_BG, theme::ST_MUTED).font_weight(FontWeight::BOLD).child(branch))
                })
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .justify_center()
                        .truncate()
                        .text_color(theme::color(theme::ST_TEXT))
                        .child(info.pending),
                )
                .when(info.errors > 0, |element| {
                    element.child(segment(theme::ST_BG, theme::ST_RED).child(format!("E {}", info.errors)))
                })
                .when(info.warnings > 0, |element| {
                    element.child(segment(theme::ST_BG, theme::ST_YELLOW).child(format!("W {}", info.warnings)))
                })
                .child(round(theme::ST_RED, theme::ST_BG))
                .child(
                    segment(theme::ST_RED, theme::ST_ONE_BG).pl(px(0.)).pr(px(5.)).child(
                        svg()
                            .path("icons/folder.svg")
                            .size(px(13.))
                            .flex_none()
                            .text_color(theme::color(theme::ST_ONE_BG)),
                    ),
                )
                .child(segment(theme::ST_LIGHT, theme::ST_TEXT).child(info.project))
                .child(round(theme::ST_GREEN, theme::ST_LIGHT))
                .child(
                    segment(theme::ST_GREEN, theme::ST_BLACK)
                        .pl(px(0.))
                        .pr(px(5.))
                        .child(
                            svg()
                                .path("icons/position.svg")
                                .size(px(11.))
                                .flex_none()
                                .text_color(theme::color(theme::ST_BLACK)),
                        ),
                )
                .child(segment(theme::ST_LIGHT, theme::ST_GREEN).pr(px(10.)).child(position)),
        )
        .when_some(info.command_line, |element, line| {
            let is_error = line.starts_with('E') && line.contains(':');
            element.child(
                div()
                    .flex()
                    .items_center()
                    .h(px(HEIGHT))
                    .px(px(8.))
                    .bg(theme::color(theme::BG))
                    .text_color(theme::color(if is_error { theme::ST_RED } else { theme::CODE_TEXT }))
                    .child(line),
            )
        })
}

/// The icons and colors of `nvim-web-devicons`, with the fallback of the NvChad statusline.
pub fn file_icon(extension: Option<&str>) -> (&'static str, u32) {
    match extension {
        Some("tex") => ("icons/file-tex.svg", 0x3d6117),
        Some("typ") => ("icons/file-typ.svg", 0x0dbcc0),
        Some("md") => ("icons/file-md.svg", 0xdddddd),
        Some("markdown") => ("icons/file-markdown.svg", 0xdddddd),
        Some("bib") => ("icons/file-bib.svg", 0xcbcb41),
        Some("toml") => ("icons/file-toml.svg", 0x9c4221),
        Some("txt") => ("icons/file-txt.svg", 0x89e051),
        _ => ("icons/file-default.svg", theme::ST_TEXT),
    }
}

#[cfg(test)]
mod tests {
    use gpui::AssetSource;

    use super::*;
    use crate::assets::Assets;

    #[test]
    fn every_file_icon_is_bundled() {
        for extension in ["tex", "typ", "md", "markdown", "bib", "toml", "txt", "unknown"] {
            let (path, _) = file_icon(Some(extension));
            assert!(Assets.load(path).unwrap().is_some(), "{path}");
        }
    }

    #[test]
    fn unknown_files_get_the_fallback() {
        assert_eq!(file_icon(None).0, "icons/file-default.svg");
    }
}
