//! Typesets the display formulas of the Markdown preview with the LaTeX that
//! is already installed, one run for all new formulas, and keeps each as a
//! cropped alpha mask that takes the color of the theme. Nothing of it runs
//! before a formula shows, and without LaTeX the Unicode text stays.

use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

/// Longer than any sane run of formulas, which takes well under a second.
const TIMEOUT: Duration = Duration::from_secs(20);

/// Bitmap pixels per PDF point. 1.7 logical pixels per point set 10 pt math
/// a little above the 16 px text, and twice that keeps it sharp on Retina.
pub const PIXELS_PER_POINT: f32 = 3.4;
/// Logical pixels per bitmap pixel.
pub const LOGICAL_PER_PIXEL: f32 = 0.5;
/// The width of each formula's page, 10 in, as `document` sets it.
const PAGE_WIDTH: f32 = 720.0;

pub fn page_width_px() -> u32 {
    (PAGE_WIDTH * PIXELS_PER_POINT) as u32
}

/// A formula as coverage: 0 is paper, 255 is ink.
pub struct Mask {
    pub width: u32,
    pub height: u32,
    pub alpha: Vec<u8>,
}

impl Mask {
    /// BGRA pixels in `color`, as `0xRRGGBB`, for a `RenderImage`.
    pub fn tinted(&self, color: u32) -> Vec<u8> {
        let [_, red, green, blue] = color.to_be_bytes();
        self.alpha.iter().flat_map(|alpha| [blue, green, red, *alpha]).collect()
    }
}

/// The first of these that is installed. Typst reads no LaTeX math.
pub fn find_engine() -> Option<PathBuf> {
    ["pdflatex", "lualatex", "xelatex", "tectonic"].into_iter().find_map(build::find_tool)
}

/// One page per formula. Returns the document and the line each formula starts on.
fn document(formulas: &[&str]) -> (String, Vec<usize>) {
    let mut text = String::from(concat!(
        "\\documentclass{article}\n",
        "\\usepackage{amsmath,amssymb}\n",
        "\\pagestyle{empty}\n",
        "\\ifdefined\\pdfpagewidth\\pdfpagewidth=10in\\pdfpageheight=4in\\else\\pagewidth=10in\\pageheight=4in\\fi\n",
        "\\hoffset=-0.8in\\voffset=-0.8in\\oddsidemargin=0pt\\topmargin=0pt\\headheight=0pt\\headsep=0pt\n",
        "\\textwidth=9.5in\\textheight=3.6in\\parindent=0pt\n",
        "\\begin{document}\n",
    ));
    let mut lines = Vec::new();
    for formula in formulas {
        // An environment of its own, such as `align`, stands as it is.
        let alone = ["align", "gather", "equation", "multline", "flalign"].iter().any(|name| formula.trim_start().starts_with(&format!("\\begin{{{name}")));
        text.push_str("\\null\n");
        lines.push(text.lines().count() + 1);
        // No blank line may end the paragraph inside display math.
        let body: String = formula.lines().filter(|line| !line.trim().is_empty()).collect::<Vec<_>>().join("\n");
        if alone {
            text.push_str(&body);
        } else {
            text.push_str("\\[\n");
            text.push_str(&body);
            text.push_str("\n\\]");
        }
        text.push_str("\n\\clearpage\n");
    }
    text.push_str("\\end{document}\n");
    (text, lines)
}

/// The formulas the log has an error in, by the `l.<line>` TeX names.
fn failed(log: &str, lines: &[usize]) -> Vec<usize> {
    let mut out: Vec<usize> = log
        .lines()
        .filter_map(|line| line.strip_prefix("l.")?.split(|ch: char| !ch.is_ascii_digit()).next()?.parse::<usize>().ok())
        .filter_map(|line| lines.iter().rposition(|start| *start <= line))
        .collect();
    out.dedup();
    out
}

/// Whether braces and environments pair up, which a run of several formulas
/// needs: one open brace would swallow all formulas after it.
pub fn balanced(formula: &str) -> bool {
    let mut depth = 0i32;
    let mut chars = formula.chars();
    while let Some(ch) = chars.next() {
        match ch {
            '\\' => {
                chars.next();
            }
            '%' => return false,
            '{' => depth += 1,
            '}' => depth -= 1,
            _ => {}
        }
        if depth < 0 {
            return false;
        }
    }
    depth == 0 && formula.matches("\\begin{").count() == formula.matches("\\end{").count() && !formula.contains("\\end{document}")
}

/// Typesets `formulas` in a scratch folder. Returns the PDF, one page per
/// formula, and the formulas that had an error.
pub fn typeset(engine: &Path, formulas: &[&str]) -> Result<(Vec<u8>, Vec<usize>), String> {
    let directory = tempfile::tempdir().map_err(|error| error.to_string())?;
    let (text, lines) = document(formulas);
    std::fs::write(directory.path().join("math.tex"), text).map_err(|error| error.to_string())?;
    let tectonic = engine.file_stem().is_some_and(|stem| stem == "tectonic");
    let mut command = Command::new(engine);
    if tectonic {
        command.args(["--keep-logs", "math.tex"]);
    } else {
        command.args(["-interaction=nonstopmode", "-no-shell-escape", "math.tex"]);
    }
    command.current_dir(directory.path()).env("PATH", build::child_path()).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    let mut child = command.spawn().map_err(|error| error.to_string())?;
    // A formula can loop forever, as `\\def\\a{\\a}\\a` does.
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|error| error.to_string())? {
            break status;
        }
        if started.elapsed() > TIMEOUT {
            child.kill().ok();
            child.wait().ok();
            return Err("LaTeX took too long.".to_string());
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let pdf = std::fs::read(directory.path().join("math.pdf")).map_err(|_| format!("LaTeX made no PDF ({status})."))?;
    let log = std::fs::read_to_string(directory.path().join("math.log")).unwrap_or_default();
    Ok((pdf, failed(&log, &lines)))
}

/// Crops a white BGRA page to its ink, with a little room, as a mask.
pub fn crop(bgra: &[u8], width: u32, height: u32) -> Option<Mask> {
    let (width, height) = (width as usize, height as usize);
    let ink = |x: usize, y: usize| {
        let pixel = &bgra[(y * width + x) * 4..][..3];
        255 - pixel.iter().map(|channel| *channel as u32).sum::<u32>() / 3
    };
    let (mut left, mut top, mut right, mut bottom) = (width, height, 0, 0);
    for y in 0..height {
        for x in 0..width {
            if ink(x, y) > 8 {
                left = left.min(x);
                right = right.max(x + 1);
                top = top.min(y);
                bottom = bottom.max(y + 1);
            }
        }
    }
    if right <= left {
        return None;
    }
    let pad = 2;
    let (left, top) = (left.saturating_sub(pad), top.saturating_sub(pad));
    let (right, bottom) = ((right + pad).min(width), (bottom + pad).min(height));
    let alpha = (top..bottom).flat_map(|y| (left..right).map(move |x| (y, x))).map(|(y, x)| ink(x, y) as u8).collect();
    Some(Mask { width: (right - left) as u32, height: (bottom - top) as u32, alpha })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_point_at_their_formula() {
        let (text, lines) = document(&["x^2", "\\frac{1}{", "\\begin{align}a&=b\\end{align}"]);
        let numbered: Vec<&str> = text.lines().collect();
        assert_eq!(numbered[lines[0] - 1], "\\[");
        assert!(numbered[lines[2] - 1].starts_with("\\begin{align}"));
        let log = format!("! Missing }} inserted.\nl.{} \\frac{{1}}{{\n", lines[1] + 1);
        assert_eq!(failed(&log, &lines), [1]);
    }

    #[test]
    fn unbalanced_formulas_stay_out() {
        assert!(balanced(r"\frac{1}{2} \{x\}"));
        assert!(!balanced(r"\frac{1}{"));
        assert!(!balanced(r"a}{b"));
        assert!(!balanced(r"\begin{pmatrix}1"));
        assert!(!balanced(r"x % comment"));
        assert!(!balanced(r"\end{document}\begin{document}"));
    }

    #[test]
    fn crop_keeps_only_the_ink() {
        let (width, height) = (10, 6);
        let mut page = vec![255u8; width * height * 4];
        for (x, y) in [(4, 2), (6, 3)] {
            page[(y * width + x) * 4..][..3].copy_from_slice(&[0, 0, 0]);
        }
        let mask = crop(&page, width as u32, height as u32).unwrap();
        assert_eq!((mask.width, mask.height), (7, 6));
        assert_eq!(mask.alpha[2 * 7 + 2], 255);
        assert_eq!(mask.tinted(0x102030)[..4], [0x30, 0x20, 0x10, 0]);
        assert!(crop(&[255u8; 16 * 4], 4, 4).is_none());
    }

    /// Needs `pdflatex`: cargo test -p app math_render -- --ignored
    #[test]
    #[ignore]
    fn typesets_with_latex() {
        let engine = find_engine().expect("LaTeX is installed");
        let (pdf, failed) = typeset(&engine, &["\\int_0^1 x^2\\,dx", "\\undefinedmacro x", "\\begin{pmatrix}1&0\\\\0&1\\end{pmatrix}"]).unwrap();
        assert!(pdf.starts_with(b"%PDF"));
        assert_eq!(failed, [1]);
    }
}
