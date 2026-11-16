use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::tools::find_tool;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Engine {
    Typst,
    Tectonic,
    PdfLatex,
    LuaLatex,
    Pandoc,
}

impl Engine {
    pub fn label(self) -> &'static str {
        match self {
            Engine::Typst => "Typst",
            Engine::Tectonic => "Tectonic",
            Engine::PdfLatex => "pdfLaTeX",
            Engine::LuaLatex => "LuaLaTeX",
            Engine::Pandoc => "Pandoc",
        }
    }

    /// Picks the engine from the file type. LaTeX prefers an installed TeX
    /// distribution and falls back to Tectonic.
    pub fn for_path(path: &Path) -> Option<Self> {
        match path.extension()?.to_str()? {
            "typ" => Some(Engine::Typst),
            "md" | "markdown" => Some(Engine::Pandoc),
            "tex" | "latex" => {
                if find_tool("latexmk").is_some() || find_tool("pdflatex").is_some() {
                    Some(Engine::PdfLatex)
                } else {
                    Some(Engine::Tectonic)
                }
            }
            _ => None,
        }
    }

    pub fn has_synctex(self) -> bool {
        matches!(self, Engine::Tectonic | Engine::PdfLatex | Engine::LuaLatex)
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Trigger {
    #[default]
    Save,
    Type,
}

/// Project settings from `.editor.toml`.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub main: Option<PathBuf>,
    pub engine: Option<Engine>,
    pub trigger: Trigger,
    pub debounce_ms: u64,
    /// Whether `.editor.toml` names the trigger, which then wins over the
    /// user's choice for all projects.
    #[serde(skip)]
    pub trigger_set: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            main: None,
            engine: None,
            trigger: Trigger::Save,
            debounce_ms: 300,
            trigger_set: false,
        }
    }
}

impl Config {
    /// Reads `.editor.toml`. A missing file gives the defaults.
    pub fn load(root: &Path) -> Result<Self, String> {
        match std::fs::read_to_string(root.join(".editor.toml")) {
            Ok(text) => {
                let mut config: Self = toml::from_str(&text).map_err(|error| format!(".editor.toml: {error}"))?;
                config.trigger_set = toml::from_str::<toml::Table>(&text).is_ok_and(|table| table.contains_key("trigger"));
                Ok(config)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(format!(".editor.toml: {error}")),
        }
    }
}

/// Sets `main` in the text of `.editor.toml` and keeps every other line, comments too.
pub fn with_main(text: &str, main: &str) -> String {
    let line = format!("main = {}", toml::Value::String(main.to_string()));
    let mut done = false;
    let mut lines: Vec<String> = text
        .lines()
        .map(|current| {
            let key = current.split('=').next().unwrap_or("").trim();
            if !done && key == "main" && current.contains('=') {
                done = true;
                line.clone()
            } else {
                current.to_string()
            }
        })
        .collect();
    if !done {
        lines.insert(0, line);
    }
    lines.join("\n") + "\n"
}

/// Makes `main`, relative to the project, the file that every build compiles.
pub fn write_main(root: &Path, main: &Path) -> Result<(), String> {
    let path = root.join(".editor.toml");
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(format!(".editor.toml: {error}")),
    };
    let main = main.to_string_lossy().replace('\\', "/");
    std::fs::write(&path, with_main(&text, &main)).map_err(|error| format!(".editor.toml: {error}"))
}

/// Finds the file to compile: the configured one, else the document root that
/// belongs to the active file.
pub fn find_main(root: &Path, config: &Config, active: Option<&Path>) -> Option<PathBuf> {
    if let Some(main) = &config.main {
        return Some(root.join(main));
    }
    let extension = active?.extension()?.to_str()?;
    match extension {
        "tex" | "latex" => {
            let active = active?;
            if is_latex_root(active) {
                return Some(active.to_path_buf());
            }
            let main = root.join("main.tex");
            if is_latex_root(&main) {
                return Some(main);
            }
            let mut candidates: Vec<PathBuf> = std::fs::read_dir(root)
                .ok()?
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| path.extension().is_some_and(|ext| ext == "tex"))
                .filter(|path| is_latex_root(path))
                .collect();
            candidates.sort();
            candidates.into_iter().next().or_else(|| Some(active.to_path_buf()))
        }
        "typ" => {
            let main = root.join("main.typ");
            Some(if main.is_file() { main } else { active?.to_path_buf() })
        }
        "md" | "markdown" => active.map(Path::to_path_buf),
        _ => None,
    }
}

fn is_latex_root(path: &Path) -> bool {
    std::fs::read_to_string(path).is_ok_and(|text| {
        text.lines()
            .any(|line| line.trim_start().starts_with("\\documentclass"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn main_replaces_its_line_and_keeps_the_rest() {
        let text = "# build with lua\nengine = 'lualatex'\nmain = 'old.tex'\n";
        assert_eq!(with_main(text, "parts/new.tex"), "# build with lua\nengine = 'lualatex'\nmain = \"parts/new.tex\"\n");
        assert_eq!(with_main("", "a.tex"), "main = \"a.tex\"\n");
        let config: Config = toml::from_str(&with_main("trigger = 'type'\n", "a.tex")).unwrap();
        assert_eq!(config.main, Some(PathBuf::from("a.tex")));
    }

    #[test]
    fn reads_settings_and_rejects_unknown_keys() {
        let config: Config = toml::from_str("main = 'a.tex'\nengine = 'lualatex'\ntrigger = 'type'").unwrap();
        assert_eq!(config.engine, Some(Engine::LuaLatex));
        assert_eq!(config.trigger, Trigger::Type);
        assert_eq!(config.debounce_ms, 300);
        assert!(toml::from_str::<Config>("nope = 1").is_err());
    }

    #[test]
    fn included_file_compiles_through_its_root() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("paper.tex"), "\\documentclass{article}").unwrap();
        std::fs::write(dir.path().join("sub/part.tex"), "text").unwrap();
        let main = find_main(dir.path(), &Config::default(), Some(&dir.path().join("sub/part.tex")));
        assert_eq!(main, Some(dir.path().join("paper.tex")));
    }
}
