use std::{collections::HashMap, sync::RwLock};

use gpui::{Hsla, Rgba, rgb};
use serde::{Deserialize, Serialize};

use editor_core::HighlightKind;

/// The colors of the app by role. Neovim colorschemes fill them through
/// `from_groups`, see `nvim_themes`.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct Palette {
    // Surfaces.
    pub bg: u32,
    pub panel: u32,
    /// Lines, borders and the background of secondary buttons.
    pub divider: u32,
    pub tab_active: u32,
    pub tab_inactive: u32,
    // Interface text.
    pub ui_text: u32,
    pub ui_muted: u32,
    pub folder: u32,
    // Accent: primary buttons, switches, the selected row and focus marks.
    pub accent: u32,
    pub on_accent: u32,
    pub accent_soft: u32,
    pub focus: u32,
    // Diagnostics.
    pub error: u32,
    pub on_error: u32,
    pub error_panel: u32,
    pub warning: u32,
    pub on_warning: u32,
    // Editor.
    pub code_text: u32,
    pub caret: u32,
    pub line_number: u32,
    pub line_number_active: u32,
    pub active_line: u32,
    pub selection: u32,
    pub search_match: u32,
    // Syntax.
    pub comment: u32,
    pub keyword: u32,
    pub function: u32,
    pub string: u32,
    pub number: u32,
    pub kind: u32,
    pub math: u32,
    pub heading: u32,
    // The NvChad statusline.
    pub st_bg: u32,
    pub st_light: u32,
    pub st_grey: u32,
    pub st_muted: u32,
    pub st_text: u32,
    /// Text on the colored mode and position segments.
    pub st_black: u32,
    pub st_one_bg: u32,
    pub st_red: u32,
    pub st_yellow: u32,
    pub st_green: u32,
    pub st_normal: u32,
    pub st_insert: u32,
    pub st_visual: u32,
}

/// Text needs this contrast to its background, as in WCAG AA.
const TEXT_CONTRAST: f32 = 4.5;
/// Enough for marks, muted labels and large text.
const MARK_CONTRAST: f32 = 3.0;

impl Palette {
    /// The familiar dark look, with the statusline in the tokyodark colors of base46.
    pub const OVERLEAF_DARK: Self = Self {
        bg: 0x1c222b,
        panel: 0x313a4b,
        divider: 0x4b5364,
        tab_active: 0x313a4b,
        tab_inactive: 0x262d3a,
        ui_text: 0xf4f5f6,
        ui_muted: 0xa8abae,
        folder: 0xf4f5f6,
        accent: 0x3b8549,
        on_accent: 0xffffff,
        accent_soft: 0x2c5739,
        focus: 0x4a9cd4,
        error: 0xd63a22,
        on_error: 0xffffff,
        error_panel: 0x4a2529,
        warning: 0xe2a93b,
        on_warning: 0x1c222b,
        code_text: 0xf8f8f3,
        caret: 0x4a9cd4,
        line_number: 0x909194,
        line_number_active: 0xf4f5f6,
        active_line: 0x444759,
        selection: 0x3d5a80,
        search_match: 0x6b5a1e,
        comment: 0x6472a1,
        keyword: 0xef83c3,
        function: 0x9fe6fb,
        string: 0xf5bb77,
        number: 0xf5bb77,
        kind: 0xf5bb77,
        math: 0xef83c3,
        heading: 0xf8f8f3,
        st_bg: 0x161722,
        st_light: 0x2a2b36,
        st_grey: 0x474853,
        st_muted: 0x5c5d68,
        st_text: 0xa0a8cd,
        st_black: 0x11121d,
        st_one_bg: 0x1d1e29,
        st_red: 0xee6d85,
        st_yellow: 0xd7a65f,
        st_green: 0x98c379,
        st_normal: 0x648ce1,
        st_insert: 0x9071c9,
        st_visual: 0x38a89d,
    };

    /// The familiar light look: white paper, grey chrome, green buttons.
    pub const OVERLEAF_LIGHT: Self = Self {
        bg: 0xffffff,
        panel: 0xf2f4f7,
        divider: 0xd5d9e0,
        tab_active: 0xffffff,
        tab_inactive: 0xe4e7ec,
        ui_text: 0x1b222c,
        ui_muted: 0x5d6879,
        folder: 0x1b222c,
        accent: 0x098842,
        on_accent: 0xffffff,
        accent_soft: 0xd3eedd,
        focus: 0x2f6bc4,
        error: 0xb42318,
        on_error: 0xffffff,
        error_panel: 0xfce9e7,
        warning: 0x8a5a00,
        on_warning: 0xffffff,
        code_text: 0x1f2328,
        caret: 0x1f2328,
        line_number: 0x8a93a3,
        line_number_active: 0x1b222c,
        active_line: 0xf0f4fa,
        selection: 0xc9ddf6,
        search_match: 0xfbe3a1,
        comment: 0x6e7781,
        keyword: 0x8250df,
        function: 0x0550ae,
        string: 0x953800,
        number: 0x0a7571,
        kind: 0x0550ae,
        math: 0x0a7571,
        heading: 0x1f2328,
        st_bg: 0xe9ecf1,
        st_light: 0xdce0e7,
        st_grey: 0xc5cbd5,
        st_muted: 0x5d6879,
        st_text: 0x2e3440,
        st_black: 0xffffff,
        st_one_bg: 0xffffff,
        st_red: 0xb42318,
        st_yellow: 0x8a5a00,
        st_green: 0x1a7f37,
        st_normal: 0x2f6bc4,
        st_insert: 0x8250df,
        st_visual: 0x0a7571,
    };

    /// Maps Neovim highlight groups to the roles. A group the scheme leaves out
    /// falls back to a related one, and in the end to a mix of `Normal`.
    pub fn from_groups(groups: &HashMap<String, Highlight>) -> Self {
        let fg = |names: &[&str]| names.iter().find_map(|name| groups.get(*name).and_then(|group| group.fg));
        let bg = |names: &[&str]| names.iter().find_map(|name| groups.get(*name).and_then(|group| group.bg));
        let base = if bg(&["Normal"]).is_some_and(|back| luminance(back) > 0.5) { Self::OVERLEAF_LIGHT } else { Self::OVERLEAF_DARK };
        let back = bg(&["Normal"]).unwrap_or(base.bg);
        let text = fg(&["Normal"]).unwrap_or(base.code_text);
        let tint = |amount: f32| mix(back, text, amount);
        // A scheme often gives floats and tabs the same background as the editor.
        let apart = |color: Option<u32>, amount: f32| color.filter(|color| *color != back).unwrap_or_else(|| tint(amount));
        let keyword = fg(&["@keyword", "Keyword", "Statement"]).unwrap_or(base.keyword);
        let function = fg(&["@function", "Function"]).unwrap_or(base.function);
        let string = fg(&["@string", "String"]).unwrap_or(base.string);
        let number = fg(&["@number", "Number", "Constant"]).unwrap_or(string);
        let kind = fg(&["@type", "Type"]).unwrap_or(function);
        let comment = fg(&["Comment"]).unwrap_or_else(|| tint(0.45));
        let ok = fg(&["DiagnosticOk", "String"]).unwrap_or(base.st_green);
        let error = fg(&["DiagnosticError", "Error"]).unwrap_or(base.error);
        let warning = fg(&["DiagnosticWarn", "WarningMsg"]).unwrap_or(base.warning);
        let status = apart(bg(&["StatusLine"]), 0.05);
        let panel = apart(bg(&["NormalFloat", "Pmenu"]), 0.07);
        let accent = function;
        let mut palette = Self {
            bg: back,
            panel,
            divider: fg(&["WinSeparator", "VertSplit", "FloatBorder"]).filter(|line| *line != back).unwrap_or_else(|| tint(0.2)),
            tab_active: apart(bg(&["TabLineSel"]), 0.1),
            tab_inactive: apart(bg(&["TabLine", "TabLineFill"]), 0.04),
            ui_text: text,
            ui_muted: fg(&["NonText", "LineNr"]).filter(|muted| contrast(*muted, back) >= MARK_CONTRAST).unwrap_or_else(|| tint(0.6)),
            folder: fg(&["Directory"]).unwrap_or(text),
            accent,
            on_accent: text,
            accent_soft: bg(&["PmenuSel", "Visual"]).unwrap_or_else(|| mix(back, accent, 0.3)),
            focus: accent,
            error,
            on_error: text,
            error_panel: bg(&["DiagnosticVirtualTextError"]).filter(|panel| *panel != back).unwrap_or_else(|| mix(back, error, 0.18)),
            warning,
            on_warning: text,
            code_text: text,
            // `Cursor` is often black on black, while Neovim then draws it inverted.
            caret: bg(&["Cursor"]).filter(|caret| contrast(*caret, back) >= MARK_CONTRAST).unwrap_or(text),
            line_number: fg(&["LineNr"]).unwrap_or(comment),
            line_number_active: fg(&["CursorLineNr"]).unwrap_or(text),
            active_line: apart(bg(&["CursorLine"]), 0.06),
            selection: apart(bg(&["Visual"]), 0.2),
            search_match: bg(&["Search"]).unwrap_or_else(|| mix(back, warning, 0.35)),
            comment,
            keyword,
            function,
            string,
            number,
            kind,
            math: fg(&["@markup.math", "Special"]).unwrap_or(number),
            heading: fg(&["@markup.heading", "Title"]).unwrap_or(text),
            st_bg: status,
            st_light: mix(status, text, 0.08),
            st_grey: mix(status, text, 0.16),
            st_muted: mix(status, text, 0.5),
            st_text: fg(&["StatusLine"]).unwrap_or(text),
            st_black: back,
            st_one_bg: back,
            st_red: error,
            st_yellow: warning,
            st_green: ok,
            st_normal: function,
            st_insert: keyword,
            st_visual: kind,
        };
        palette.make_readable();
        palette
    }

    /// Fixes every pair the app draws, so that no theme hides a label. Text
    /// moves away from its background, and filled surfaces move toward `bg`
    /// when the interface text would vanish on them.
    fn make_readable(&mut self) {
        self.ui_text = readable(self.ui_text, self.bg, TEXT_CONTRAST);
        self.code_text = readable(self.code_text, self.bg, TEXT_CONTRAST);
        for surface in [&mut self.panel, &mut self.divider, &mut self.tab_active, &mut self.accent_soft, &mut self.error_panel] {
            *surface = settle(*surface, self.ui_text, self.bg, TEXT_CONTRAST);
        }
        for surface in [&mut self.active_line, &mut self.selection, &mut self.search_match] {
            *surface = settle(*surface, self.code_text, self.bg, TEXT_CONTRAST);
        }
        self.tab_inactive = settle(self.tab_inactive, self.ui_muted, self.bg, MARK_CONTRAST);
        self.ui_muted = readable(self.ui_muted, self.panel, MARK_CONTRAST);
        self.folder = readable(self.folder, self.panel, TEXT_CONTRAST);
        for mark in [&mut self.accent, &mut self.focus, &mut self.caret, &mut self.error, &mut self.warning, &mut self.line_number] {
            *mark = readable(*mark, self.bg, MARK_CONTRAST);
        }
        // Buttons and badges: the best theme color for the label, then the
        // fill moves until even that one reads.
        let (ui_text, bg) = (self.ui_text, self.bg);
        for (fill, on) in [(&mut self.accent, &mut self.on_accent), (&mut self.error, &mut self.on_error), (&mut self.warning, &mut self.on_warning)] {
            *on = text_on(*fill, [ui_text, bg]);
            *fill = readable(*fill, *on, TEXT_CONTRAST);
        }
        self.line_number_active = readable(self.line_number_active, self.bg, TEXT_CONTRAST);
        for token in [&mut self.comment, &mut self.keyword, &mut self.function, &mut self.string, &mut self.number, &mut self.kind, &mut self.math, &mut self.heading] {
            *token = readable(*token, self.bg, MARK_CONTRAST);
        }
        self.st_text = readable(self.st_text, self.st_light, TEXT_CONTRAST);
        self.st_muted = readable(self.st_muted, self.st_bg, MARK_CONTRAST);
        // Labels on the bar first, since the segments below are filled with
        // the same colors and must win.
        for label in [&mut self.st_red, &mut self.st_yellow] {
            *label = readable(*label, self.st_bg, MARK_CONTRAST);
        }
        self.st_green = readable(self.st_green, self.st_light, MARK_CONTRAST);
        self.st_black = text_on(self.st_normal, [self.st_black, self.ui_text]);
        for segment in [&mut self.st_normal, &mut self.st_insert, &mut self.st_visual, &mut self.st_green, &mut self.st_red] {
            *segment = readable(*segment, self.st_black, TEXT_CONTRAST);
        }
        self.st_one_bg = text_on(self.st_red, [self.st_one_bg, self.ui_text]);
        self.st_red = readable(self.st_red, self.st_one_bg, TEXT_CONTRAST);
    }
}

/// One highlight group of Neovim, as `nvim_get_hl` gives it.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize)]
pub struct Highlight {
    pub fg: Option<u32>,
    pub bg: Option<u32>,
}

/// `amount` of `to` over `from`, per channel.
fn mix(from: u32, to: u32, amount: f32) -> u32 {
    let channel = |shift: u32| {
        let (from, to) = (((from >> shift) & 0xff) as f32, ((to >> shift) & 0xff) as f32);
        ((from + (to - from) * amount).round() as u32) << shift
    };
    channel(16) | channel(8) | channel(0)
}

/// Relative luminance after WCAG 2.
fn luminance(color: u32) -> f32 {
    let channel = |shift: u32| {
        let value = ((color >> shift) & 0xff) as f32 / 255.;
        if value <= 0.03928 { value / 12.92 } else { ((value + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * channel(16) + 0.7152 * channel(8) + 0.0722 * channel(0)
}

pub fn is_light(color: u32) -> bool {
    luminance(color) > 0.4
}

fn contrast(left: u32, right: u32) -> f32 {
    let (left, right) = (luminance(left), luminance(right));
    (left.max(right) + 0.05) / (left.min(right) + 0.05)
}

/// `color`, moved toward white or black until it stands out from `back`.
fn readable(color: u32, back: u32, wanted: f32) -> u32 {
    let extreme = if luminance(back) > 0.4 { 0x000000 } else { 0xffffff };
    (0..=10).map(|step| mix(color, extreme, step as f32 / 10.)).find(|color| contrast(*color, back) >= wanted).unwrap_or(extreme)
}

/// `color` as a mark that stands out from `back`, for colors outside the palette.
pub fn legible(color: u32, back: u32) -> u32 {
    readable(color, back, MARK_CONTRAST)
}

/// `surface`, moved toward `base` until `text` reads on it. `base` is assumed readable.
fn settle(surface: u32, text: u32, base: u32, wanted: f32) -> u32 {
    (0..=10).map(|step| mix(surface, base, step as f32 / 10.)).find(|surface| contrast(text, *surface) >= wanted).unwrap_or(base)
}

/// The theme color that reads best on `fill`, or black or white when none does.
fn text_on(fill: u32, candidates: [u32; 2]) -> u32 {
    let best = candidates.into_iter().chain([0x000000, 0xffffff]).max_by(|left, right| contrast(*left, fill).total_cmp(&contrast(*right, fill)));
    candidates.into_iter().find(|color| contrast(*color, fill) >= TEXT_CONTRAST).or(best).unwrap_or(0xffffff)
}

/// The theme without a choice in the settings.
pub const DEFAULT_THEME: &str = "overleaf-dark";

/// The themes that ship with the app, by the name the picker shows.
pub const BUILT_IN: [(&str, Palette); 2] = [("overleaf-dark", Palette::OVERLEAF_DARK), ("overleaf-light", Palette::OVERLEAF_LIGHT)];

pub fn built_in(name: &str) -> Option<Palette> {
    BUILT_IN.iter().find(|(built_in, _)| *built_in == name).map(|(_, palette)| *palette)
}

static ACTIVE: RwLock<Palette> = RwLock::new(Palette::OVERLEAF_DARK);

pub fn palette() -> Palette {
    *ACTIVE.read().unwrap_or_else(|error| error.into_inner())
}

pub fn set_palette(palette: Palette) {
    *ACTIVE.write().unwrap_or_else(|error| error.into_inner()) = palette;
}

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
    let palette = palette();
    match kind {
        None | Some(Punctuation | Variable | Strong) => (palette.code_text, false),
        Some(Heading) => (palette.heading, false),
        Some(Comment) => (palette.comment, false),
        Some(Keyword | Operator) => (palette.keyword, false),
        Some(Function | Tag) => (palette.function, true),
        Some(Type) => (palette.kind, true),
        Some(String | Link) => (palette.string, true),
        Some(Number | Constant) => (palette.number, false),
        Some(Emphasis) => (palette.code_text, true),
        Some(Raw) => (palette.string, false),
        Some(Math) => (palette.math, false),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn groups(entries: &[(&str, Option<u32>, Option<u32>)]) -> HashMap<String, Highlight> {
        entries.iter().map(|(name, fg, bg)| (name.to_string(), Highlight { fg: *fg, bg: *bg })).collect()
    }

    /// The pairs `make_readable` promises, checked on any palette.
    pub(crate) fn assert_readable(palette: &Palette, name: &str) {
        let text = [
            (palette.ui_text, palette.bg),
            (palette.ui_text, palette.panel),
            (palette.ui_text, palette.divider),
            (palette.ui_text, palette.tab_active),
            (palette.ui_text, palette.accent_soft),
            (palette.ui_text, palette.error_panel),
            (palette.code_text, palette.bg),
            (palette.code_text, palette.active_line),
            (palette.code_text, palette.selection),
            (palette.code_text, palette.search_match),
            (palette.on_accent, palette.accent),
            (palette.on_error, palette.error),
            (palette.on_warning, palette.warning),
            (palette.st_text, palette.st_light),
            (palette.st_black, palette.st_normal),
            (palette.st_black, palette.st_insert),
            (palette.st_black, palette.st_visual),
            (palette.st_black, palette.st_green),
            (palette.st_one_bg, palette.st_red),
        ];
        for (index, (fg, bg)) in text.into_iter().enumerate() {
            assert!(contrast(fg, bg) >= TEXT_CONTRAST - 0.05, "{name}: pair {index} {fg:06x} on {bg:06x}");
        }
        for mark in [palette.comment, palette.keyword, palette.function, palette.string, palette.caret, palette.focus, palette.line_number] {
            assert!(contrast(mark, palette.bg) >= MARK_CONTRAST - 0.05, "{name}: {mark:06x} on {:06x}", palette.bg);
        }
    }

    #[test]
    fn built_in_themes_are_readable() {
        for (name, palette) in BUILT_IN {
            assert_readable(&palette, name);
        }
    }

    #[test]
    fn a_bright_accent_gets_dark_text() {
        // Like vesper: mint strings and peach functions on near black.
        let palette = Palette::from_groups(&groups(&[
            ("Normal", Some(0xcccccc), Some(0x101010)),
            ("Function", Some(0xffc799), None),
            ("String", Some(0x99ffe4), None),
            ("Cursor", Some(0x101010), Some(0x101010)),
        ]));
        assert_eq!(palette.on_accent, 0x101010);
        assert_eq!(palette.caret, 0xcccccc);
        assert_readable(&palette, "vesper-like");
    }

    #[test]
    fn hostile_schemes_still_read() {
        let schemes = [
            groups(&[("Normal", Some(0x777777), Some(0x666666)), ("Function", Some(0x6a6a6a), None), ("Visual", None, Some(0x777777))]),
            groups(&[("Normal", Some(0x000000), Some(0xf7f7f7)), ("Function", Some(0xffff00), None), ("PmenuSel", None, Some(0x000000))]),
            groups(&[("Normal", Some(0xffffff), Some(0x000000)), ("StatusLine", Some(0x000000), Some(0x000000))]),
            HashMap::new(),
        ];
        for (index, scheme) in schemes.iter().enumerate() {
            assert_readable(&Palette::from_groups(scheme), &format!("scheme {index}"));
        }
    }

    #[test]
    fn groups_fall_back_to_their_relatives() {
        let palette = Palette::from_groups(&groups(&[
            ("Normal", Some(0xffffff), Some(0x000000)),
            ("Statement", Some(0xff4040), None),
            ("@function", Some(0x00ff00), None),
            ("Function", Some(0x0000ff), None),
        ]));
        assert_eq!((palette.bg, palette.code_text), (0x000000, 0xffffff));
        assert_eq!((palette.keyword, palette.function, palette.kind), (0xff4040, 0x00ff00, 0x00ff00));
    }

    #[test]
    fn light_schemes_start_from_the_light_theme() {
        let palette = Palette::from_groups(&groups(&[("Normal", Some(0x202020), Some(0xfafafa))]));
        assert_eq!(palette.error, readable(Palette::OVERLEAF_LIGHT.error, palette.on_error, TEXT_CONTRAST));
    }

    #[test]
    fn mix_blends_each_channel() {
        assert_eq!(mix(0x000000, 0xffffff, 0.5), 0x808080);
        assert_eq!(mix(0x102030, 0x102030, 0.7), 0x102030);
    }
}
