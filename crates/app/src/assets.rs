use std::borrow::Cow;

use gpui::{AssetSource, Result, SharedString};

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
    "folder-plus", "git-branch", "help", "image", "layout", "link", "log", "logo", "math", "minus",
    "outline", "plus", "position", "puzzle", "quote", "redo", "refresh", "settings", "sigma", "table", "undo", "upload", "vim",
);

pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        // The icon of an add-on, which is the only absolute path asked for.
        if path.starts_with('/') {
            return Ok(std::fs::read(path).ok().map(Cow::Owned));
        }
        Ok(ICONS
            .iter()
            .find(|(name, _)| *name == path)
            .map(|(_, bytes)| Cow::Borrowed(*bytes)))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok(ICONS
            .iter()
            .filter(|(name, _)| name.starts_with(path))
            .map(|(name, _)| SharedString::from(*name))
            .collect())
    }
}
