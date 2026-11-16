//! Colorschemes from the Neovim of the user. Neovim loads them, since they are
//! Lua plugins, and writes their highlight groups to a file. Only the theme
//! picker asks Neovim. Everything else reads the cache.

use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use crate::theme::{self, Highlight, Palette};
/// A config that installs plugins on start may take a while, but never forever.
const SCAN_TIMEOUT: Duration = Duration::from_secs(15);

/// Applies each colorscheme that does not ship with Neovim and writes the
/// groups the app needs as JSON to `$EDITOR_THEMES_OUT`.
const SCRIPT: &str = r#"
local groups = { "Normal", "NormalFloat", "Pmenu", "PmenuSel", "WinSeparator", "VertSplit", "FloatBorder", "TabLine",
  "TabLineSel", "TabLineFill", "NonText", "Directory", "CursorLine", "CursorLineNr", "LineNr", "Visual", "Search",
  "Cursor", "Comment", "@keyword", "Keyword", "Statement", "@function", "Function", "@string", "String", "@number",
  "Number", "Constant", "@type", "Type", "@markup.math", "Special", "@markup.heading", "Title", "Error", "WarningMsg",
  "DiagnosticError", "DiagnosticWarn", "DiagnosticOk", "DiagnosticVirtualTextError", "StatusLine" }
local builtin = {}
for _, file in ipairs(vim.fn.globpath(vim.env.VIMRUNTIME .. "/colors", "*", false, true)) do
  builtin[vim.fn.fnamemodify(file, ":t:r")] = true
end
local themes = vim.empty_dict()
for _, name in ipairs(vim.fn.getcompletion("", "color")) do
  if not builtin[name] then
    vim.cmd("highlight clear")
    if pcall(vim.cmd.colorscheme, name) then
      local colors = vim.empty_dict()
      for _, group in ipairs(groups) do
        local hl = vim.api.nvim_get_hl(0, { name = group, link = false })
        local color = vim.empty_dict()
        color.fg = hl.fg
        color.bg = hl.bg
        colors[group] = color
      end
      themes[name] = colors
    end
  end
end
vim.fn.writefile({ vim.json.encode(themes) }, vim.env.EDITOR_THEMES_OUT)
"#;

pub type Themes = BTreeMap<String, Palette>;

pub fn cache_path() -> Option<PathBuf> {
    Some(crate::settings::cache_dir()?.join("themes.json"))
}

/// A missing or broken cache means no themes yet.
pub fn load_cache(path: &Path) -> Themes {
    std::fs::read_to_string(path).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default()
}

pub fn save_cache(path: &Path, themes: &Themes) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, serde_json::to_string(themes).map_err(std::io::Error::other)?)
}

/// Runs Neovim with the config of the user. Blocks, so call it in the background.
pub fn scan() -> Result<Themes, String> {
    let directory = tempfile::tempdir().map_err(|error| error.to_string())?;
    let script = directory.path().join("themes.lua");
    let output = directory.path().join("themes.json");
    std::fs::write(&script, SCRIPT).map_err(|error| error.to_string())?;
    let mut child = Command::new("nvim")
        .args(["--headless", "-n", "-i", "NONE", "-c"])
        .arg(format!("luafile {}", script.display()))
        .args(["-c", "qa!"])
        .env("EDITOR_THEMES_OUT", &output)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "Neovim was not found.".to_string())?;
    let started = Instant::now();
    while child.try_wait().map_err(|error| error.to_string())?.is_none() {
        if started.elapsed() > SCAN_TIMEOUT {
            child.kill().ok();
            child.wait().ok();
            return Err("Neovim took too long to list its themes.".to_string());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let text = std::fs::read_to_string(&output).map_err(|_| "Neovim did not list its themes.".to_string())?;
    parse(&text)
}

fn parse(text: &str) -> Result<Themes, String> {
    let schemes: HashMap<String, HashMap<String, Highlight>> =
        serde_json::from_str(text).map_err(|error| error.to_string())?;
    Ok(schemes
        .into_iter()
        .filter(|(name, _)| theme::built_in(name).is_none())
        .map(|(name, groups)| (name, Palette::from_groups(&groups)))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_neovim_output_becomes_palettes() {
        let themes = parse(r#"{"night":{"Normal":{"fg":16777215,"bg":0},"Comment":{}},"empty":{},"overleaf-dark":{}}"#).unwrap();
        assert_eq!(themes.keys().collect::<Vec<_>>(), ["empty", "night"]);
        assert_eq!((themes["night"].bg, themes["night"].code_text), (0, 0xffffff));
    }

    #[test]
    #[ignore = "needs Neovim with colorschemes installed"]
    fn installed_themes_are_readable() {
        let themes = scan().unwrap();
        assert!(!themes.is_empty());
        for (name, palette) in &themes {
            crate::theme::tests::assert_theme(palette, name);
        }
    }

    #[test]
    fn the_cache_survives_a_restart() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("nested").join("themes.json");
        let themes = Themes::from([("night".to_string(), Palette::OVERLEAF_DARK)]);
        save_cache(&path, &themes).unwrap();
        assert_eq!(load_cache(&path), themes);
        assert!(load_cache(&directory.path().join("missing.json")).is_empty());
    }
}
