//! The title bar on Windows. The editor draws its own bar, as on the Mac, so
//! Windows shows neither its buttons nor a place to drag the window: the bar
//! marks itself as the drag area and these buttons as Windows' own. Windows
//! then does the rest: snap layouts when the pointer rests on maximize, a
//! double click on the bar to maximize, and dragging to the screen edges.

use gpui::{AnyElement, Hsla, IntoElement, ParentElement, Rgba, Styled, Window, WindowControlArea, div, prelude::*, px};

use crate::theme;

/// Room on the left of the bar: the Mac draws its traffic lights there.
pub const LEFT_SPACE: f32 = if cfg!(target_os = "macos") { 78.0 } else { 8.0 };

/// Marks `element` as the bar that moves the window, on Windows.
pub fn drag_area<E: InteractiveElement>(element: E) -> E {
    if cfg!(windows) { element.window_control_area(WindowControlArea::Drag) } else { element }
}

/// Minimize, maximize or restore, and close, `height` tall, on Windows only.
pub fn buttons(window: &Window, height: f32) -> Option<AnyElement> {
    if !cfg!(windows) {
        return None;
    }
    let maximized = window.is_maximized();
    Some(
        div()
            .id("window-controls")
            .flex()
            .flex_none()
            .h(px(height))
            .font_family(icon_font())
            .child(button("window-minimize", "\u{e921}", WindowControlArea::Min, false))
            .child(button("window-maximize", if maximized { "\u{e923}" } else { "\u{e922}" }, WindowControlArea::Max, false))
            .child(button("window-close", "\u{e8bb}", WindowControlArea::Close, true))
            .into_any_element(),
    )
}

fn button(id: &'static str, glyph: &'static str, area: WindowControlArea, close: bool) -> impl IntoElement {
    let palette = theme::palette();
    let red: Hsla = Rgba { r: 232.0 / 255.0, g: 17.0 / 255.0, b: 32.0 / 255.0, a: 1.0 }.into();
    let (hover, hover_text) = if close { (red, gpui::white()) } else { (theme::color(palette.divider), theme::color(palette.ui_text)) };
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .w(px(46.))
        .h_full()
        .occlude()
        .text_size(px(10.))
        .text_color(theme::color(palette.ui_text))
        .hover(move |style| style.bg(hover).text_color(hover_text))
        .active(move |style| style.bg(hover.opacity(0.8)))
        .window_control_area(area)
        .child(glyph)
}

/// Windows 11 has Segoe Fluent Icons, Windows 10 the older MDL2 set with the
/// same glyphs.
fn icon_font() -> &'static str {
    static FONT: std::sync::OnceLock<&'static str> = std::sync::OnceLock::new();
    FONT.get_or_init(read_icon_font)
}

fn read_icon_font() -> &'static str {
    let build = crate::platform::system().split('.').nth(2).and_then(|build| build.split(|ch: char| !ch.is_ascii_digit()).next()?.parse::<u32>().ok());
    if build.is_some_and(|build| build >= 22000) { "Segoe Fluent Icons" } else { "Segoe MDL2 Assets" }
}
