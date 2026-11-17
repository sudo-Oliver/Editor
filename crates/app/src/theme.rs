use std::{collections::HashMap, sync::RwLock};

use gpui::{FontWeight, Hsla, PathBuilder, Pixels, Point, Rgba, SharedString, rgb};
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
/// Lines and the fill of secondary buttons and switches against their ground.
const LINE_CONTRAST: f32 = 1.35;
/// A selected row or tab against its neighbors.
const SURFACE_CONTRAST: f32 = 1.3;
/// A filled accent, such as a switch that is on, against the panel it sits on.
const FILL_CONTRAST: f32 = 2.0;

impl Palette {
    /// The familiar dark look, with the statusline in the tokyodark colors of base46.
    pub const OVERLEAF_DARK: Self = Self {
        bg: 0x1c222b,
        panel: 0x313a4b,
        divider: 0x4b5364,
        tab_active: 0x313a4b,
        tab_inactive: 0x1f2530,
        ui_text: 0xf4f5f6,
        ui_muted: 0xa8abae,
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
        divider: 0xc8cdd6,
        tab_active: 0xffffff,
        tab_inactive: 0xdbdfe6,
        ui_text: 0x1b222c,
        ui_muted: 0x5d6879,
        accent: 0x098842,
        on_accent: 0xffffff,
        accent_soft: 0xb5e0c4,
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

    /// Ink on warm paper, in the spirit of an e-ink screen: low glare, muted
    /// inks for the syntax and a deep ink for whatever is chosen.
    pub const PAPER: Self = Self {
        bg: 0xf4f3f0,
        panel: 0xeae8e2,
        divider: 0xc9c5bc,
        tab_active: 0xf4f3f0,
        tab_inactive: 0xd8d4cb,
        ui_text: 0x2c2b2a,
        ui_muted: 0x6f6a61,
        accent: 0x1a1918,
        on_accent: 0xf4f3f0,
        accent_soft: 0xcfcabf,
        focus: 0x3a4a6b,
        error: 0x9e3426,
        on_error: 0xffffff,
        error_panel: 0xf3e3de,
        warning: 0x7a5512,
        on_warning: 0xffffff,
        code_text: 0x2c2b2a,
        caret: 0x1a1918,
        line_number: 0x8a857b,
        line_number_active: 0x2c2b2a,
        active_line: 0xedebe6,
        selection: 0xdcd7cb,
        search_match: 0xebddb0,
        comment: 0x857f74,
        keyword: 0x8b2e3c,
        function: 0x2e4057,
        string: 0x56662b,
        number: 0x8a5a2b,
        kind: 0x2e4057,
        math: 0x8b2e3c,
        heading: 0x1a1918,
        st_bg: 0xe6e3dc,
        st_light: 0xdad6cd,
        st_grey: 0xc9c4ba,
        st_muted: 0x6a645a,
        st_text: 0x2c2b2a,
        st_black: 0xf4f3f0,
        st_one_bg: 0xf4f3f0,
        st_red: 0x9e3426,
        st_yellow: 0x7a5512,
        st_green: 0x4f6b2e,
        st_normal: 0x2c2b2a,
        st_insert: 0x6b3a5a,
        st_visual: 0x2e5566,
    };

    /// Paper mode in daylight: warm parchment, graphite ink and a blue-black
    /// fountain-pen ink for commands.
    pub const PAPER_INK: Self = Self {
        bg: 0xe6e1d7,
        panel: 0xf6f4ee,
        divider: 0xbeb8ad,
        tab_active: 0xe6e1d7,
        tab_inactive: 0xccc5b8,
        ui_text: 0x333333,
        ui_muted: 0x6e6e6e,
        accent: 0x111111,
        on_accent: 0xf6f4ee,
        accent_soft: 0xc9c2b4,
        focus: 0x2b4570,
        error: 0x9e3426,
        on_error: 0xffffff,
        error_panel: 0xf1dfd8,
        warning: 0x7a5512,
        on_warning: 0xffffff,
        code_text: 0x111111,
        caret: 0x111111,
        line_number: 0x6e6e6e,
        line_number_active: 0x333333,
        active_line: 0xdfd9cd,
        selection: 0xcfc8ba,
        search_match: 0xe3cf98,
        comment: 0x666666,
        keyword: 0x2b4570,
        function: 0x2b4570,
        string: 0x5e5444,
        number: 0x7a5426,
        kind: 0x333333,
        math: 0x8a2f35,
        heading: 0x111111,
        st_bg: 0xebe7df,
        st_light: 0xdcd6ca,
        st_grey: 0xbeb8ad,
        st_muted: 0x666666,
        st_text: 0x333333,
        st_black: 0xf6f4ee,
        st_one_bg: 0xf6f4ee,
        st_red: 0x9e3426,
        st_yellow: 0x7a5512,
        st_green: 0x4f6b2e,
        st_normal: 0x333333,
        st_insert: 0x2b4570,
        st_visual: 0x6b3a5a,
    };

    /// Paper mode at night: dark paper and pale ink, the same roles inverted.
    pub const PAPER_INK_DARK: Self = Self {
        bg: 0x1c1b19,
        panel: 0x141312,
        divider: 0x3d3a35,
        tab_active: 0x1c1b19,
        tab_inactive: 0x3a3732,
        ui_text: 0xd6d1c6,
        ui_muted: 0x8e8980,
        accent: 0xece8df,
        on_accent: 0x141312,
        accent_soft: 0x34312c,
        focus: 0x9db4d6,
        error: 0xe0806e,
        on_error: 0x141312,
        error_panel: 0x33201c,
        warning: 0xd6ac62,
        on_warning: 0x141312,
        code_text: 0xece8df,
        caret: 0xece8df,
        line_number: 0x807b72,
        line_number_active: 0xd6d1c6,
        active_line: 0x24231f,
        selection: 0x3a3732,
        search_match: 0x4a3f22,
        comment: 0x8e8980,
        keyword: 0x9fb7da,
        function: 0x9fb7da,
        string: 0xc2b59c,
        number: 0xd9b27c,
        kind: 0xd6d1c6,
        math: 0xe0a092,
        heading: 0xf7f4ec,
        st_bg: 0x24231f,
        st_light: 0x2c2a26,
        st_grey: 0x3d3a35,
        st_muted: 0x8e8980,
        st_text: 0xd6d1c6,
        st_black: 0x141312,
        st_one_bg: 0x141312,
        st_red: 0xe0806e,
        st_yellow: 0xd6ac62,
        st_green: 0xa3b87a,
        st_normal: 0xd6d1c6,
        st_insert: 0x9fb7da,
        st_visual: 0xc79bbd,
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
        // A scheme often gives floats and tabs the same background as the editor,
        // and some a dark selection in a light theme, which no text reads on
        // together with the rest.
        let fits = |color: &u32| *color != back && is_light(*color) == is_light(back);
        let apart = |color: Option<u32>, amount: f32| color.filter(fits).unwrap_or_else(|| tint(amount));
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
            accent,
            on_accent: text,
            accent_soft: bg(&["PmenuSel"]).filter(fits).or(bg(&["Visual"]).filter(fits)).unwrap_or_else(|| mix(back, accent, 0.3)),
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
        self.panel = settle(self.panel, self.ui_text, self.bg, TEXT_CONTRAST);
        // Controls have to stand out from what they sit on, or a switch and the
        // halves of a choice blur into one field. Then the text follows them.
        let (bg, panel, text) = (self.bg, self.panel, self.ui_text);
        self.divider = distinct(self.divider, [bg, panel], text, LINE_CONTRAST);
        self.accent_soft = distinct(self.accent_soft, [bg, panel], text, SURFACE_CONTRAST);
        self.tab_active = distinct(self.tab_active, [self.tab_inactive, self.tab_inactive], text, SURFACE_CONTRAST);
        self.ui_text = readable_on(self.ui_text, &[self.bg, self.panel, self.divider, self.tab_active, self.accent_soft]);
        // Where no text color reads on all of them, readable text wins.
        for surface in [&mut self.panel, &mut self.divider, &mut self.tab_active, &mut self.accent_soft, &mut self.error_panel] {
            *surface = settle(*surface, self.ui_text, self.bg, TEXT_CONTRAST);
        }
        for surface in [&mut self.active_line, &mut self.selection, &mut self.search_match] {
            *surface = settle(*surface, self.code_text, self.bg, TEXT_CONTRAST);
        }
        self.tab_inactive = settle(self.tab_inactive, self.ui_muted, self.bg, MARK_CONTRAST);
        self.ui_muted = readable(self.ui_muted, self.panel, MARK_CONTRAST);
        for mark in [&mut self.accent, &mut self.focus, &mut self.caret, &mut self.error, &mut self.warning, &mut self.line_number] {
            *mark = readable(*mark, self.bg, MARK_CONTRAST);
        }
        self.accent = readable(self.accent, self.panel, FILL_CONTRAST);
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

/// A tone above the panel for the chosen row, like a sheet lifted off the desk.
/// Text drawn on the block cursor, so the character under it stays readable.
pub fn on_caret(palette: &Palette) -> u32 {
    readable(palette.bg, palette.caret, TEXT_CONTRAST)
}

/// The open file in the file tree. On a dark panel the raised shade is too
/// faint to find, so the row takes more of the text color.
pub fn selected_row(palette: &Palette) -> u32 {
    if is_light(palette.panel) { raised(palette) } else { mix(palette.panel, palette.ui_text, 0.24) }
}

pub fn raised(palette: &Palette) -> u32 {
    if is_light(palette.panel) { mix(palette.panel, 0xffffff, 0.7) } else { mix(palette.panel, palette.ui_text, 0.12) }
}

pub fn is_light(color: u32) -> bool {
    luminance(color) > 0.4
}

fn contrast(left: u32, right: u32) -> f32 {
    let (left, right) = (luminance(left), luminance(right));
    (left.max(right) + 0.05) / (left.min(right) + 0.05)
}

/// `color`, moved toward white or black until it stands out from `back`. It
/// keeps its side: dark text gets darker, light text lighter.
fn readable(color: u32, back: u32, wanted: f32) -> u32 {
    let toward = |extreme: u32| (0..=10).map(|step| mix(color, extreme, step as f32 / 10.)).find(|color| contrast(*color, back) >= wanted);
    let (first, second) = if luminance(color) < luminance(back) { (0x000000, 0xffffff) } else { (0xffffff, 0x000000) };
    toward(first).or_else(|| toward(second)).unwrap_or(if is_light(back) { 0x000000 } else { 0xffffff })
}

/// `color`, moved toward white or black until it reads on all `backs`, of which
/// the first is the ground of the app and must work.
fn readable_on(color: u32, backs: &[u32]) -> u32 {
    let reads = |color: &u32| backs.iter().all(|back| contrast(*color, *back) >= TEXT_CONTRAST);
    let toward = |extreme: u32| (0..=10).map(|step| mix(color, extreme, step as f32 / 10.)).find(reads);
    let (first, second) = if luminance(color) < luminance(backs[0]) { (0x000000, 0xffffff) } else { (0xffffff, 0x000000) };
    toward(first).or_else(|| toward(second)).unwrap_or_else(|| readable(color, backs[0], TEXT_CONTRAST))
}

/// `surface`, moved toward `text` until it stands apart from both `grounds`.
/// It stops at 60 percent, so that the text keeps room to stay readable.
fn distinct(surface: u32, grounds: [u32; 2], text: u32, wanted: f32) -> u32 {
    (0..=6)
        .map(|step| mix(surface, text, step as f32 / 10.))
        .find(|surface| grounds.iter().all(|ground| contrast(*surface, *ground) >= wanted))
        .unwrap_or_else(|| mix(surface, text, 0.6))
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
pub const BUILT_IN: [(&str, Palette); 3] =
    [("overleaf-dark", Palette::OVERLEAF_DARK), ("overleaf-light", Palette::OVERLEAF_LIGHT), ("paper", Palette::PAPER)];

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
/// Ships with the app for paper mode, under the SIL Open Font License.
pub const PAPER_FONT: &str = "iA Writer Quattro S";
/// The typewriter of the hub's project folders, from the system.
#[cfg(any(target_os = "macos", target_os = "windows"))]
pub const MONO_FONT: &str = "Courier New";
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub const MONO_FONT: &str = "Liberation Mono";
/// A book's rhythm: lines further apart than in a code editor.
const PAPER_LINE_SPACING: f32 = 1.7;

/// How text looks and whether the editor is in paper mode. The settings set it.
#[derive(Clone, Debug, PartialEq)]
pub struct Look {
    pub code_font: SharedString,
    /// None keeps the system font of the interface.
    pub ui_font: Option<SharedString>,
    pub code_size: f32,
    pub line_height: f32,
    pub paper: bool,
    pub ruled_lines: bool,
}

impl Look {
    pub fn new(paper: bool, ruled_lines: bool, code_font: Option<&str>, ui_font: Option<&str>, code_size: Option<f32>) -> Self {
        let code_size = code_size.unwrap_or(if paper { 14. } else { CODE_FONT_SIZE });
        let spacing = if paper { PAPER_LINE_SPACING } else { CODE_LINE_HEIGHT / CODE_FONT_SIZE };
        Self {
            code_font: code_font.unwrap_or(if paper { PAPER_FONT } else { CODE_FONT }).to_string().into(),
            ui_font: ui_font.or(paper.then_some(PAPER_FONT)).map(|font| font.to_string().into()),
            code_size,
            line_height: (code_size * spacing).round(),
            paper,
            ruled_lines: paper && ruled_lines,
        }
    }
}

static LOOK: RwLock<Option<Look>> = RwLock::new(None);

pub fn look() -> Look {
    LOOK.read().unwrap_or_else(|error| error.into_inner()).clone().unwrap_or_else(|| Look::new(false, false, None, None, None))
}

pub fn set_look(look: Look) {
    *LOOK.write().unwrap_or_else(|error| error.into_inner()) = Some(look);
}

pub fn code_font() -> SharedString {
    look().code_font
}

pub fn code_font_size() -> f32 {
    look().code_size
}

pub fn code_line_height() -> f32 {
    look().line_height
}
/// The title of a page in the hub or the setup.
pub const TITLE_FONT_SIZE: f32 = 22.0;
pub const RADIUS_SM: f32 = 4.0;
pub const RADIUS_MD: f32 = 6.0;
pub const RADIUS_LG: f32 = 10.0;

#[cfg(target_os = "macos")]
pub const CODE_FONT: &str = "Menlo";
#[cfg(not(target_os = "macos"))]
pub const CODE_FONT: &str = "DejaVu Sans Mono";

/// A filled shape through `points`, or `None` when it has no area to fill.
pub fn polygon(points: impl IntoIterator<Item = Point<Pixels>>) -> Option<gpui::Path> {
    let mut points = points.into_iter();
    let mut builder = PathBuilder::fill();
    builder.move_to(points.next()?);
    for point in points {
        builder.line_to(point);
    }
    builder.close();
    builder.build().ok()
}

pub fn color(value: u32) -> Hsla {
    let color: Rgba = rgb(value);
    color.into()
}

/// Returns the color, the italic flag and the weight of a syntax kind. Paper
/// mode carries structure by weight, as ink on a page has one color.
pub fn highlight_style(kind: Option<HighlightKind>) -> (u32, bool, FontWeight) {
    use HighlightKind::*;
    let palette = palette();
    if look().paper {
        // Ink tells commands from text, slant tells arguments from commands,
        // and weight is kept for what the document itself makes bold.
        let (color, italic) = match kind {
            None | Some(Punctuation | Variable | Strong | Emphasis) => (palette.code_text, kind == Some(Emphasis)),
            Some(Heading) => (palette.heading, false),
            Some(Comment) => (palette.comment, true),
            Some(Keyword | Operator | Function | Tag) => (palette.keyword, false),
            Some(Type) => (palette.kind, true),
            Some(String | Link | Raw) => (palette.string, true),
            Some(Number | Constant) => (palette.number, false),
            Some(Math) => (palette.math, false),
        };
        let weight = if matches!(kind, Some(Heading | Strong)) { FontWeight::BOLD } else { FontWeight::NORMAL };
        return (color, italic, weight);
    }
    let (color, italic) = match kind {
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
    };
    let weight = if kind == Some(Strong) { FontWeight::BOLD } else { FontWeight::NORMAL };
    (color, italic, weight)
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
    }

    /// The surfaces `make_readable` keeps apart, where the scheme leaves room for it.
    pub(crate) fn assert_distinct(palette: &Palette, name: &str) {
        let apart = [
            (palette.divider, palette.bg, LINE_CONTRAST),
            (palette.divider, palette.panel, LINE_CONTRAST),
            (palette.accent_soft, palette.bg, SURFACE_CONTRAST),
            (palette.accent_soft, palette.panel, SURFACE_CONTRAST),
            (palette.tab_active, palette.tab_inactive, SURFACE_CONTRAST),
            (palette.accent, palette.bg, MARK_CONTRAST),
            (palette.accent, palette.panel, FILL_CONTRAST),
        ];
        for (index, (surface, ground, wanted)) in apart.into_iter().enumerate() {
            assert!(contrast(surface, ground) >= wanted - 0.05, "{name}: surface {index} {surface:06x} on {ground:06x}");
        }
    }

    /// Everything a real theme has to give.
    pub(crate) fn assert_theme(palette: &Palette, name: &str) {
        assert_readable(palette, name);
        assert_distinct(palette, name);
        assert_marks(palette, name);
    }

    fn assert_marks(palette: &Palette, name: &str) {
        for mark in [palette.comment, palette.keyword, palette.function, palette.string, palette.caret, palette.focus, palette.line_number] {
            assert!(contrast(mark, palette.bg) >= MARK_CONTRAST - 0.05, "{name}: {mark:06x} on {:06x}", palette.bg);
        }
    }

    #[test]
    fn text_reads_on_the_raised_row() {
        for (name, palette) in BUILT_IN {
            assert!(contrast(palette.ui_text, raised(&palette)) >= TEXT_CONTRAST, "{name}");
        }
    }

    #[test]
    fn the_open_file_stands_out_on_a_dark_panel() {
        let paper = [("paper-ink-dark", Palette::PAPER_INK_DARK)];
        for (name, palette) in BUILT_IN.into_iter().chain(paper).filter(|(_, palette)| !is_light(palette.panel)) {
            let row = selected_row(&palette);
            assert!(contrast(row, palette.panel) >= 1.6, "{name}");
            assert!(contrast(palette.ui_text, row) >= TEXT_CONTRAST, "{name}");
        }
    }

    #[test]
    fn the_character_under_the_block_cursor_reads() {
        let paper = [("paper-ink", Palette::PAPER_INK), ("paper-ink-dark", Palette::PAPER_INK_DARK)];
        for (name, palette) in BUILT_IN.into_iter().chain(paper) {
            assert!(contrast(on_caret(&palette), palette.caret) >= TEXT_CONTRAST, "{name}");
        }
    }

    #[test]
    fn built_in_themes_are_readable() {
        for (name, palette) in BUILT_IN {
            assert_theme(&palette, name);
        }
        assert_theme(&Palette::PAPER_INK, "paper-ink");
        assert_theme(&Palette::PAPER_INK_DARK, "paper-ink-dark");
    }

    #[test]
    fn paper_mode_spaces_lines_like_a_book() {
        let look = Look::new(true, true, None, None, None);
        assert_eq!(look.code_font.as_ref(), PAPER_FONT);
        assert!(look.line_height >= look.code_size * 1.6);
        assert!(!Look::new(false, true, None, None, None).ruled_lines);
        assert_eq!(Look::new(false, false, Some("Fira Code"), None, Some(13.)).code_font.as_ref(), "Fira Code");
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
        assert_theme(&palette, "vesper-like");
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
            let palette = Palette::from_groups(scheme);
            assert_readable(&palette, &format!("scheme {index}"));
            assert_marks(&palette, &format!("scheme {index}"));
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
