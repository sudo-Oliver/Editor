//! Icons of the Material Icon Theme for the file tree, a small part of it chosen
//! for papers and their projects, see `assets/material/LICENSE`.

macro_rules! material {
    ($($name:literal),* $(,)?) => {
        pub const ICONS: &[(&str, &[u8])] = &[
            $((concat!("material/", $name, ".svg"), include_bytes!(concat!("../assets/material/", $name, ".svg")))),*
        ];
    };
}

material!(
    "bibliography", "changelog", "console", "css", "docker", "document", "editorconfig", "file", "font",
    "git", "html", "image", "javascript", "json", "julia", "jupyter", "latexmk", "license", "lock", "log",
    "lua", "makefile", "markdown", "matlab", "nix", "pdf", "powerpoint", "python", "r", "readme", "rust",
    "settings", "svg", "table", "tex", "toml", "typescript", "typst", "word", "xml", "yaml", "zip",
    "folder", "folder-open", "folder-archive", "folder-archive-open", "folder-config", "folder-config-open",
    "folder-dist", "folder-dist-open", "folder-docs", "folder-docs-open", "folder-examples",
    "folder-examples-open", "folder-font", "folder-font-open", "folder-git", "folder-git-open",
    "folder-github", "folder-github-open", "folder-images", "folder-images-open", "folder-include",
    "folder-include-open", "folder-lib", "folder-lib-open", "folder-resource", "folder-resource-open",
    "folder-scripts", "folder-scripts-open", "folder-src", "folder-src-open", "folder-temp",
    "folder-temp-open", "folder-template", "folder-template-open", "folder-test", "folder-test-open",
);

/// Asks for an icon in two tones of ink, for paper mode.
pub const INK_PREFIX: &str = "material-ink/";

/// The asset path of the icon for a file name.
pub fn file(name: &str) -> String {
    format!("material/{}.svg", file_icon(name))
}

/// The asset path of the icon for a folder name, open or closed.
pub fn folder(name: &str, open: bool) -> String {
    let icon = folder_icon(name);
    if open { format!("material/{icon}-open.svg") } else { format!("material/{icon}.svg") }
}

/// The same icon drawn with the ink of the text and the paper behind it.
pub fn inked(path: &str, ink: u32, paper: u32) -> String {
    format!("{INK_PREFIX}{ink:06x}/{paper:06x}/{}", path.trim_start_matches("material/"))
}

fn file_icon(name: &str) -> &'static str {
    let lower = name.to_ascii_lowercase();
    let stem = lower.split('.').next().unwrap_or("");
    match lower.as_str() {
        ".gitignore" | ".gitattributes" | ".gitmodules" | ".gitkeep" => return "git",
        "latexmkrc" | ".latexmkrc" => return "latexmk",
        ".editorconfig" => return "editorconfig",
        "makefile" | "gnumakefile" => return "makefile",
        "dockerfile" => return "docker",
        "typst.toml" => return "typst",
        _ => {}
    }
    match stem {
        "readme" => return "readme",
        "license" | "licence" | "copying" => return "license",
        "changelog" | "changes" => return "changelog",
        _ => {}
    }
    let extension = lower.rsplit_once('.').map_or("", |(_, extension)| extension);
    match extension {
        "tex" | "ltx" | "cls" | "sty" | "dtx" | "ins" => "tex",
        "bib" | "bbl" | "bcf" | "blg" => "bibliography",
        "typ" => "typst",
        "pdf" => "pdf",
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "tif" | "tiff" | "ico" | "eps" => "image",
        "svg" => "svg",
        "md" | "markdown" => "markdown",
        "toml" => "toml",
        "json" => "json",
        "yaml" | "yml" => "yaml",
        "txt" => "document",
        "log" => "log",
        "lua" => "lua",
        "py" => "python",
        "r" => "r",
        "jl" => "julia",
        "js" | "mjs" | "cjs" => "javascript",
        "ts" | "mts" | "cts" => "typescript",
        "rs" => "rust",
        "sh" | "bash" | "zsh" | "fish" => "console",
        "html" | "htm" => "html",
        "css" => "css",
        "csv" | "tsv" => "table",
        "zip" | "tar" | "gz" | "tgz" | "7z" | "rar" => "zip",
        "ini" | "cfg" | "conf" => "settings",
        "doc" | "docx" | "odt" => "word",
        "ppt" | "pptx" | "odp" => "powerpoint",
        "ttf" | "otf" | "woff" | "woff2" => "font",
        "m" => "matlab",
        "nix" => "nix",
        "lock" => "lock",
        "xml" => "xml",
        "ipynb" => "jupyter",
        _ => "file",
    }
}

fn folder_icon(name: &str) -> &'static str {
    let lower = name.to_ascii_lowercase();
    match lower.trim_start_matches(['.', '_']) {
        "images" | "image" | "imgs" | "img" | "icons" | "figures" | "figure" | "figs" | "fig" | "pictures"
        | "pics" | "photos" | "screenshots" => "folder-images",
        "src" | "source" | "sources" | "code" => "folder-src",
        "doc" | "docs" | "document" | "documents" | "documentation" | "notes" | "articles" => "folder-docs",
        "res" | "resource" | "resources" | "asset" | "assets" | "static" => "folder-resource",
        "inc" | "include" | "includes" | "partial" | "partials" => "folder-include",
        "script" | "scripts" => "folder-scripts",
        "test" | "tests" | "testing" | "spec" | "specs" => "folder-test",
        "git" | "patches" | "githooks" | "submodules" => "folder-git",
        "github" => "folder-github",
        "cfg" | "conf" | "config" | "configs" | "settings" | "options" => "folder-config",
        "lib" | "libs" | "library" | "libraries" | "vendor" | "external" => "folder-lib",
        "dist" | "out" | "output" | "build" | "builds" | "release" | "bin" | "target" => "folder-dist",
        "font" | "fonts" => "folder-font",
        "temp" | "tmp" | "cache" | "cached" => "folder-temp",
        "example" | "examples" | "sample" | "samples" | "demo" | "demos" => "folder-examples",
        "template" | "templates" => "folder-template",
        "arc" | "archive" | "archives" | "old" => "folder-archive",
        _ => "folder",
    }
}

/// Loads an icon asked for under [`INK_PREFIX`]: the dark colors of the icon
/// turn to ink and the light ones to paper, so a motive stays readable on its shape.
pub fn load_inked(path: &str) -> Option<Vec<u8>> {
    let mut parts = path.strip_prefix(INK_PREFIX)?.splitn(3, '/');
    let ink = u32::from_str_radix(parts.next()?, 16).ok()?;
    let paper = u32::from_str_radix(parts.next()?, 16).ok()?;
    let source = format!("material/{}", parts.next()?);
    let (_, bytes) = ICONS.iter().find(|(name, _)| *name == source)?;
    Some(ink_svg(std::str::from_utf8(bytes).ok()?, ink, paper).into_bytes())
}

fn ink_svg(svg: &str, ink: u32, paper: u32) -> String {
    let colors: Vec<(&str, f32)> = hex_fills(svg).map(|hex| (hex, luminance(hex))).collect();
    let darkest = colors.iter().map(|(_, light)| *light).fold(f32::MAX, f32::min);
    let lightest = colors.iter().map(|(_, light)| *light).fold(f32::MIN, f32::max);
    let middle = (darkest + lightest) / 2.;
    let mut out = String::with_capacity(svg.len());
    let mut rest = svg;
    while let Some(start) = rest.find("fill=\"#") {
        let value = start + "fill=\"".len();
        let end = value + rest[value..].find('"').unwrap_or(0);
        out.push_str(&rest[..value]);
        let light = luminance(&rest[value..end]);
        let color = if lightest - darkest > 0.1 && light > middle { paper } else { ink };
        out.push_str(&format!("#{color:06x}"));
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

fn hex_fills(svg: &str) -> impl Iterator<Item = &str> {
    svg.split("fill=\"").skip(1).filter_map(|part| part.split('"').next()).filter(|value| value.starts_with('#'))
}

fn luminance(hex: &str) -> f32 {
    let digits = hex.trim_start_matches('#');
    let rgb = match digits.len() {
        3 => u32::from_str_radix(&digits.chars().flat_map(|digit| [digit, digit]).collect::<String>(), 16),
        _ => u32::from_str_radix(digits, 16),
    }
    .unwrap_or(0);
    let channel = |shift: u32| ((rgb >> shift) & 0xff) as f32 / 255.;
    0.2126 * channel(16) + 0.7152 * channel(8) + 0.0722 * channel(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn papers_get_their_own_icons() {
        assert_eq!(file("main.tex"), "material/tex.svg");
        assert_eq!(file("refs.BIB"), "material/bibliography.svg");
        assert_eq!(file("README.md"), "material/readme.svg");
        assert_eq!(file(".latexmkrc"), "material/latexmk.svg");
        assert_eq!(file("notes"), "material/file.svg");
        assert_eq!(folder("Figures", false), "material/folder-images.svg");
        assert_eq!(folder(".git", true), "material/folder-git-open.svg");
        assert_eq!(folder("chapters", true), "material/folder-open.svg");
    }

    #[test]
    fn every_icon_is_bundled_with_its_open_folder() {
        let bundled = |path: &str| ICONS.iter().any(|(name, _)| *name == path);
        let names = ["a.tex", "a.bib", "a.typ", "a.pdf", "a.png", "a.svg", "a.md", "a.toml", "a.json", "a.yml", "a.txt",
            "a.log", "a.lua", "a.py", "a.r", "a.jl", "a.js", "a.ts", "a.rs", "a.sh", "a.html", "a.css", "a.csv", "a.zip",
            "a.ini", "a.docx", "a.pptx", "a.otf", "a.m", "a.nix", "a.lock", "a.xml", "a.ipynb", "x", ".gitignore",
            "latexmkrc", ".editorconfig", "Makefile", "Dockerfile", "README", "LICENSE", "CHANGELOG.md"];
        for name in names {
            assert!(bundled(&file(name)), "{name}");
        }
        let folders = ["images", "src", "docs", "assets", "include", "scripts", "tests", "git", "github", "config",
            "lib", "build", "fonts", "tmp", "examples", "templates", "archive", "x"];
        for name in folders {
            assert!(bundled(&folder(name, false)) && bundled(&folder(name, true)), "{name}");
        }
    }

    #[test]
    fn ink_keeps_the_motive_on_its_shape() {
        let svg = r##"<svg><path fill="#009688" d="a"/><path fill="#b2dfdb" d="b"/></svg>"##;
        assert_eq!(ink_svg(svg, 0x111111, 0xeeeeee), r##"<svg><path fill="#111111" d="a"/><path fill="#eeeeee" d="b"/></svg>"##);
        // One color is all ink, also when it is light.
        assert_eq!(ink_svg(r##"<path fill="#fff"/>"##, 0x111111, 0xeeeeee), r##"<path fill="#111111"/>"##);
        let inked = load_inked(&inked(&folder("images", true), 0x222222, 0xf0e6d2)).unwrap();
        assert!(String::from_utf8(inked).unwrap().contains("#f0e6d2"));
    }
}
