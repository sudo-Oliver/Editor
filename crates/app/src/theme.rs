use gpui::{Hsla, Rgba, rgb};

use editor_core::HighlightKind;

pub const BG: u32 = 0x1c222b;
pub const PANEL: u32 = 0x313a4b;
pub const DIVIDER: u32 = 0x4b5364;
pub const TAB_INACTIVE: u32 = 0x262d3a;
pub const ACTIVE_LINE: u32 = 0x444759;
pub const GREEN: u32 = 0x3b8549;
pub const GREEN_DARK: u32 = 0x2c5739;
pub const UI_TEXT: u32 = 0xf4f5f6;
pub const UI_MUTED: u32 = 0xa8abae;
pub const CODE_TEXT: u32 = 0xf8f8f3;
pub const LINE_NUMBER: u32 = 0x909194;
pub const COMMENT: u32 = 0x6472a1;
pub const CYAN: u32 = 0x9fe6fb;
pub const PINK: u32 = 0xef83c3;
pub const ORANGE: u32 = 0xf5bb77;
pub const ERROR: u32 = 0xec381e;
pub const ERROR_PANEL: u32 = 0x4a2529;
pub const WARNING: u32 = 0xe2a93b;
pub const CARET: u32 = 0x4a9cd4;
pub const SELECTION: u32 = 0x3d5a80;
pub const SEARCH_MATCH: u32 = 0x6b5a1e;

// NvChad statusline, in the tokyodark colors of base46.
pub const ST_BG: u32 = 0x161722;
pub const ST_LIGHT: u32 = 0x2a2b36;
pub const ST_GREY: u32 = 0x474853;
pub const ST_MUTED: u32 = 0x5c5d68;
pub const ST_TEXT: u32 = 0xa0a8cd;
pub const ST_BLACK: u32 = 0x11121d;
pub const ST_ONE_BG: u32 = 0x1d1e29;
pub const ST_RED: u32 = 0xee6d85;
pub const ST_YELLOW: u32 = 0xd7a65f;
pub const ST_GREEN: u32 = 0x98c379;
pub const ST_NORMAL: u32 = 0x648ce1;
pub const ST_INSERT: u32 = 0x9071c9;
pub const ST_VISUAL: u32 = 0x38a89d;

pub const CODE_FONT_SIZE: f32 = 12.0;
pub const CODE_LINE_HEIGHT: f32 = 19.2;
pub const UI_FONT_SIZE: f32 = 13.0;

#[cfg(target_os = "macos")]
pub const CODE_FONT: &str = "Menlo";
#[cfg(not(target_os = "macos"))]
pub const CODE_FONT: &str = "DejaVu Sans Mono";

pub fn color(value: u32) -> Hsla {
    let color: Rgba = rgb(value);
    color.into()
}

/// Returns the color and the italic flag of a syntax kind.
pub fn highlight_style(kind: Option<HighlightKind>) -> (u32, bool) {
    use HighlightKind::*;
    match kind {
        None | Some(Heading | Punctuation | Variable | Strong) => (CODE_TEXT, false),
        Some(Comment) => (COMMENT, false),
        Some(Keyword | Operator) => (PINK, false),
        Some(Function | Tag) => (CYAN, true),
        Some(Type | String | Link) => (ORANGE, true),
        Some(Number | Constant) => (ORANGE, false),
        Some(Emphasis) => (CODE_TEXT, true),
        Some(Raw) => (ORANGE, false),
        Some(Math) => (PINK, false),
    }
}
