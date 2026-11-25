//! A small font map per project. pdfTeX reads its whole map, some 46 000
//! lines, on every run, which is half of what a small edit costs. A map with
//! only the font families the project uses skips that. It lies in the output
//! directory, and `TEXFONTMAPS` makes pdfTeX find it before the system's. The families come from
//! the fonts the last run loaded, which `-recorder` lists. A font outside them
//! falls back to a bitmap, which the log shows, and then the map grows and
//! LaTeX runs again: the PDF never keeps a bitmap font.

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::OnceLock,
    time::SystemTime,
};

use crate::tools::find_tool;

/// Named like the system's, so that `TEXFONTMAPS` puts it in its place.
pub(crate) const FILE: &str = "pdftex.map";
/// Sections every document needs: the 14 base fonts, and Computer Modern,
/// which LaTeX loads into its format, so that no run lists it.
const ALWAYS: &[&str] = &["pdftex_dl14.map", "ps2pk35.map", "cm.map"];

/// The map of the TeX distribution, found once per start.
fn system_map() -> Option<&'static Path> {
    static MAP: OnceLock<Option<PathBuf>> = OnceLock::new();
    MAP.get_or_init(|| {
        let output = crate::process::no_window(Command::new(find_tool("kpsewhich")?).arg("pdftex.map")).output().ok()?;
        let path = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
        (output.status.success() && path.is_file()).then_some(path)
    })
    .as_deref()
}

/// The map split at its `% name.map` comments, which updmap writes before
/// the lines of each package.
fn sections(map: &str) -> Vec<(&str, Vec<&str>)> {
    let mut sections: Vec<(&str, Vec<&str>)> = Vec::new();
    for line in map.lines() {
        match line.strip_prefix("% ").filter(|name| name.trim_end_matches(':').ends_with(".map")) {
            Some(name) => sections.push((name, Vec::new())),
            None if !line.starts_with('%') => match sections.last_mut() {
                Some((_, lines)) => lines.push(line),
                None => sections.push(("", vec![line])),
            },
            None => {}
        }
    }
    sections
}

/// The fonts a run loaded, by TeX name, from its `.fls` file.
pub(crate) fn fonts_used(fls: &str) -> BTreeSet<String> {
    fls.lines()
        .filter_map(|line| line.strip_prefix("INPUT "))
        .filter_map(|path| path.strip_suffix(".tfm"))
        .filter_map(|path| path.rsplit('/').next())
        .map(str::to_string)
        .collect()
}

/// The sections of `map` that a document needs for `fonts`.
fn select(map: &str, fonts: &BTreeSet<String>) -> String {
    let mut text = String::new();
    for (name, lines) in sections(map) {
        let file = name.trim_end_matches(':').rsplit('/').next().unwrap_or_default();
        let needed = ALWAYS.contains(&file)
            || lines.iter().any(|line| line.split_whitespace().next().is_some_and(|font| fonts.contains(font)));
        if needed {
            for line in lines {
                text.push_str(line);
                text.push('\n');
            }
        }
    }
    text
}

/// The project's map, if it is there and newer than the system's.
pub(crate) fn current(output_directory: &Path) -> Option<PathBuf> {
    let path = output_directory.join(FILE);
    let modified = |path: &Path| fs::metadata(path).and_then(|metadata| metadata.modified()).ok();
    let own = modified(&path)?;
    let system = system_map().and_then(modified).unwrap_or(SystemTime::UNIX_EPOCH);
    (own >= system).then_some(path)
}

/// Writes the project's map for the fonts of the last run. `false` when it
/// cannot, and the next run then reads the whole map as before.
pub(crate) fn update(output_directory: &Path, stem: &str) -> bool {
    let Some(system) = system_map() else {
        return false;
    };
    let (Ok(fls), Ok(map)) = (fs::read_to_string(output_directory.join(format!("{stem}.fls"))), fs::read_to_string(system)) else {
        return false;
    };
    let mut fonts = fonts_used(&fls);
    // The fonts of a preamble format load when it is made, so its own list has them.
    for entry in fs::read_dir(output_directory).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|extension| extension == "fls") && path.file_name().is_some_and(|name| name.to_string_lossy().starts_with("preamble-")) {
            fonts.extend(fs::read_to_string(&path).map(|fls| fonts_used(&fls)).unwrap_or_default());
        }
    }
    fs::write(output_directory.join(FILE), select(&map, &fonts)).is_ok()
}

/// Whether the map lacked a font of the run. With `-no-mktex=pk`, pdfTeX
/// stops at once instead of making a bitmap font, which takes long.
pub(crate) fn missed_a_font(log: &str) -> bool {
    log.lines().any(|line| line.contains("pdfTeX error") && line.contains(" not found")) || log.contains("pk>")
}

pub(crate) fn remove(output_directory: &Path) {
    fs::remove_file(output_directory.join(FILE)).ok();
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAP: &str = "% /texmf/pdftex_dl14.map:\nptmr8r Times-Roman \"TeXBase1Encoding ReEncodeFont\" <8r.enc\n% ps2pk35.map\n% cm.map\ncmbx10 CMBX10 <cmbx10.pfb\ncmr10 CMR10 <cmr10.pfb\n% bbold.map\nbbold10 BBOLD10 <bbold10.pfb\n% amsfonts.map\nmsbm10 MSBM10 <msbm10.pfb\n";

    #[test]
    fn only_the_families_in_use_are_kept() {
        let fonts = fonts_used("PWD /p\nINPUT /tex/fonts/tfm/public/cm/cmr10.tfm\nINPUT ./main.tex\nOUTPUT main.pdf\n");
        assert_eq!(fonts.into_iter().collect::<Vec<_>>(), ["cmr10"]);
        let map = select(MAP, &BTreeSet::from(["cmr10".to_string()]));
        // The whole cm family comes along, so that bold needs no new map.
        assert!(map.contains("cmbx10") && map.contains("Times-Roman"));
        assert!(!map.contains("bbold10") && !map.contains("msbm10"));
        let map = select(MAP, &BTreeSet::from(["msbm10".to_string()]));
        assert!(map.contains("msbm10") && map.contains("cmr10") && !map.contains("bbold10"));
    }

    #[test]
    fn a_bitmap_font_shows_the_map_was_short() {
        assert!(missed_a_font(" </tex/fonts/pk/ljfour/public/cm/dpi600/cmbx10.pk></tex/cmr10.pfb>"));
        assert!(missed_a_font(" </texmf-var/fonts/pk/ljfour/public/amsfonts/symbols/msbm10.600pk>"));
        assert!(missed_a_font("!pdfTeX error: pdflatex (file msbm10): Font msbm10 at 600 not found\n"));
        assert!(!missed_a_font(" </tex/fonts/type1/public/amsfonts/cm/cmr10.pfb>"));
    }
}
