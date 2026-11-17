use std::borrow::Cow;

use gpui::{AssetSource, Result, SharedString};

use crate::material_icons;

macro_rules! icons {
    ($($name:literal),* $(,)?) => {
        const ICONS: &[(&str, &[u8])] = &[
            $((concat!("icons/", $name, ".svg"), include_bytes!(concat!("../assets/icons/", $name, ".svg")))),*
        ];
    };
}

icons!(
    "addon", "arrow-left", "arrow-right", "book", "chevron-down", "chevron-left", "chevron-right",
    "chevron-up", "close", "contrast", "dots", "download", "file", "file-bib", "file-default",
    "file-markdown", "file-md", "file-plus", "file-tex", "file-toml", "file-txt", "file-typ", "folder",
    "folder-plus", "git-branch", "help", "home", "image", "layout", "link", "log", "logo", "math", "minus",
    "notepad", "outline", "people", "plus", "position", "puzzle", "quote", "redo", "refresh", "settings", "sigma", "table", "undo", "upload", "vim",
    "check-circle", "code", "grid", "list", "play", "wrench", "paperclip", "binder-clip",
);

/// iA Writer Quattro for paper mode, under the SIL Open Font License, see `assets/fonts/LICENSE.md`.
pub const FONTS: &[&[u8]] = &[
    include_bytes!("../assets/fonts/iAWriterQuattroS-Regular.ttf"),
    include_bytes!("../assets/fonts/iAWriterQuattroS-Italic.ttf"),
    include_bytes!("../assets/fonts/iAWriterQuattroS-Bold.ttf"),
    include_bytes!("../assets/fonts/iAWriterQuattroS-BoldItalic.ttf"),
];

pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        // The icon of an add-on, which is the only absolute path asked for.
        if path.starts_with('/') {
            return Ok(std::fs::read(path).ok().map(Cow::Owned));
        }
        if path.starts_with(material_icons::INK_PREFIX) {
            return Ok(material_icons::load_inked(path).map(Cow::Owned));
        }
        Ok(ICONS
            .iter()
            .chain(material_icons::ICONS)
            .find(|(name, _)| *name == path)
            .map(|(_, bytes)| Cow::Borrowed(*bytes)))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok(ICONS
            .iter()
            .chain(material_icons::ICONS)
            .filter(|(name, _)| name.starts_with(path))
            .map(|(name, _)| SharedString::from(*name))
            .collect())
    }
}
