mod actions;
mod addon_panel;
mod addons;
mod assets;
mod atomic;
mod completion;
mod editor_view;
mod export;
mod file_tree;
mod git_panel;
mod image_view;
mod marketplace;
mod outline;
mod package_check;
mod pdf_text;
mod pdf_view;
mod picker;
mod problems;
mod recovery;
mod settings;
mod shortcuts;
mod smear;
mod snippets;
mod status_bar;
mod theme;
mod workspace;

use std::path::PathBuf;

use gpui::{
    App, Bounds, Menu, MenuItem, TitlebarOptions, WindowBounds, WindowOptions, point, prelude::*,
    px, size,
};

fn main() {
    let root = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .and_then(|path| path.canonicalize().ok());

    gpui_platform::application()
        .with_assets(assets::Assets)
        .run(move |cx: &mut App| {
            actions::bind_keys(cx);
            cx.on_action(|_: &actions::Quit, cx| cx.quit());
            cx.set_menus([Menu::new("Editor").items([MenuItem::action("Quit", actions::Quit)])]);

            let bounds = Bounds::centered(None, size(px(1400.), px(900.)), cx);
            let root = root.clone();
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    titlebar: Some(TitlebarOptions {
                        title: Some("Editor".into()),
                        appears_transparent: true,
                        traffic_light_position: Some(point(px(12.), px(12.))),
                    }),
                    ..Default::default()
                },
                move |window, cx| cx.new(|cx| workspace::Workspace::new(root, window, cx)),
            )
            .expect("window opens");
            cx.activate(true);
            cx.on_window_closed(|cx, _| cx.quit()).detach();
        });
}
