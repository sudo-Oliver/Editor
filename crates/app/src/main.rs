mod actions;
mod addon_panel;
mod addon_reports;
mod addons;
mod app_location;
mod assets;
mod atomic;
mod bib;
mod completion;
mod editor_view;
mod export;
mod file_tree;
mod git_login;
mod git_panel;
mod github;
mod home_view;
mod history;
mod image_view;
mod keymap;
mod markdown;
mod markdown_preview;
mod markdown_view;
mod math_text;
mod marketplace;
mod material_icons;
mod nvim_themes;
mod outline;
mod paper;
mod package_check;
mod package_commands;
mod pdf_text;
mod pdf_view;
mod picker;
mod problems;
mod projects;
mod secrets;
mod recovery;
mod settings;
mod setup;
mod shell_command;
mod shortcuts;
mod smear;
mod snippets;
mod status_bar;
mod storage;
mod texlive;
mod text_file;
mod theme;
mod toolchain;
mod tour;
mod updater;
mod workspace;

use std::{ffi::OsString, os::unix::ffi::OsStringExt, path::PathBuf};

use gpui::{
    App, Bounds, Menu, MenuItem, TitlebarOptions, WindowBounds, WindowOptions, point, prelude::*,
    px, size,
};

fn main() {
    // Git may start the editor only to ask for a login.
    git_login::answer_if_asked();
    // Old macOS versions add a process serial number when Finder starts the app.
    let root = std::env::args_os().skip(1).find(|argument| !argument.to_string_lossy().starts_with("-psn_")).map(PathBuf::from);
    // Before anything reads its settings or tools from the old place.
    storage::migrate();
    std::thread::spawn(storage::sweep);
    toolchain::register_installed();

    // Files and folders from Finder, the Dock and the `editor` command arrive
    // here, at the start as well as while the app runs.
    let (open_sender, mut opened) = tokio::sync::mpsc::unbounded_channel::<PathBuf>();
    let application = gpui_platform::application().with_assets(assets::Assets);
    application.on_open_urls(move |urls| {
        for path in urls.iter().filter_map(|url| path_from_url(url)) {
            let _ = open_sender.send(path);
        }
    });
    application.run(move |cx: &mut App| {
            cx.text_system()
                .add_fonts(assets::FONTS.iter().map(|font| std::borrow::Cow::Borrowed(*font)).collect())
                .expect("the bundled fonts load");
            cx.on_action(|_: &actions::Quit, cx| cx.quit());
            let mut items = Vec::new();
            // For the keyboard too, through the menu bar, as the offer itself is a bar.
            if app_location::running().is_some() {
                items.push(MenuItem::action("Move to Applications…", actions::MoveToApplications));
            }
            items.push(MenuItem::action("Quit", actions::Quit));
            cx.set_menus([Menu::new("Editor").items(items)]);

            let bounds = Bounds::centered(None, size(px(1400.), px(900.)), cx);
            let root = root.clone();
            let window = cx.open_window(
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
            cx.spawn(async move |cx| {
                while let Some(path) = opened.recv().await {
                    let shown = window.update(cx, |workspace, window, cx| {
                        workspace.open_path(path, window, cx);
                        window.activate_window();
                    });
                    if shown.is_err() {
                        return;
                    }
                }
            })
            .detach();
            cx.activate(true);
            cx.on_window_closed(|cx, _| cx.quit()).detach();
        });
}

/// The path in a `file://` URL, with `%20` and the like decoded.
fn path_from_url(url: &str) -> Option<PathBuf> {
    let encoded = url.strip_prefix("file://")?;
    let encoded = encoded.strip_prefix("localhost").unwrap_or(encoded);
    let bytes = encoded.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let hex = |byte: u8| (byte as char).to_digit(16);
        match (bytes[index], bytes.get(index + 1).copied().and_then(hex), bytes.get(index + 2).copied().and_then(hex)) {
            (b'%', Some(high), Some(low)) => {
                decoded.push((high * 16 + low) as u8);
                index += 3;
            }
            (byte, _, _) => {
                decoded.push(byte);
                index += 1;
            }
        }
    }
    Some(PathBuf::from(OsString::from_vec(decoded)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_urls_become_paths() {
        assert_eq!(path_from_url("file:///Users/ada/My%20Thesis/main.tex"), Some(PathBuf::from("/Users/ada/My Thesis/main.tex")));
        assert_eq!(path_from_url("file:///tmp/%C3%BCber/"), Some(PathBuf::from("/tmp/über/")));
        assert_eq!(path_from_url("file://localhost/tmp/a"), Some(PathBuf::from("/tmp/a")));
        assert_eq!(path_from_url("https://example.com"), None);
    }
}
