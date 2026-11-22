//! Keeps the build folder out of the way: out of backups and the search index,
//! and free of what main files that are gone left behind. Nothing here may fail
//! a build, and nothing here touches a file outside the build folder.

use std::{fs, path::Path};

use crate::{preamble, runner::LATEX_STATE_EXTENSIONS};

/// Spotlight skips a folder that holds this file.
const NEVER_INDEX: &str = ".metadata_never_index";
/// Time Machine skips a folder with this attribute, which moves along with it.
#[cfg(target_os = "macos")]
const BACKUP_ATTRIBUTE: &str = "com.apple.metadata:com_apple_backup_excludeItem";
/// What `tmutil addexclusion` writes: a binary plist of `com.apple.backupd`.
#[cfg(target_os = "macos")]
const BACKUP_EXCLUSION: &[u8] = b"bplist00_\x10\x11com.apple.backupd\x08\0\0\0\0\0\0\x01\x01\0\0\0\0\0\0\0\x01\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\x1c";
/// Endings of main files. A source with any of them keeps its output.
const SOURCE_EXTENSIONS: &[&str] = &["tex", "latex", "ltx", "typ", "md", "markdown"];
/// What a main file leaves besides LaTeX's state.
const OUTPUT_EXTENSIONS: &[&str] = &["pdf", "synctex.gz", "synctex(busy)", "synctex", "glo", "gls", "glg"];
/// How many entries the search for a main file reads at most. Past that,
/// nothing is removed.
const MAX_SEARCHED: usize = 20_000;

/// Keeps a build folder out of Time Machine and Spotlight. The second file
/// marks it as done, so this costs one check per build.
pub(crate) fn keep_out_of_backups(directory: &Path) {
    let marker = directory.join(NEVER_INDEX);
    if marker.exists() {
        return;
    }
    exclude_from_backups(directory);
    fs::write(marker, "").ok();
}

#[cfg(target_os = "macos")]
fn exclude_from_backups(directory: &Path) {
    let (Ok(path), Ok(name)) = (std::ffi::CString::new(directory.as_os_str().as_encoded_bytes()), std::ffi::CString::new(BACKUP_ATTRIBUTE)) else {
        return;
    };
    // SAFETY: both names end in a zero, and the value lives through the call.
    unsafe {
        libc::setxattr(path.as_ptr(), name.as_ptr(), BACKUP_EXCLUSION.as_ptr().cast(), BACKUP_EXCLUSION.len(), 0, 0);
    }
}

#[cfg(not(target_os = "macos"))]
fn exclude_from_backups(_directory: &Path) {}

/// Removes the output of main files that are gone, such as after a rename.
/// A file goes only when it is named after a main file with a known ending,
/// and no source of that name is left in `cwd` or the folders below it.
pub(crate) fn remove_orphans(cwd: &Path, output_directory: &Path, current: &str) {
    if output_directory.file_name().is_none_or(|name| name != crate::runner::OUTPUT_DIRECTORY) {
        return;
    }
    let Ok(entries) = fs::read_dir(output_directory) else {
        return;
    };
    let outputs: Vec<(String, String)> = entries
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let stem = output_stem(&name)?.to_string();
            (stem != current).then_some((name, stem))
        })
        .collect();
    let mut stems: Vec<String> = outputs.iter().map(|(_, stem)| stem.clone()).collect();
    stems.sort();
    stems.dedup();
    let mut budget = MAX_SEARCHED;
    if stems.is_empty() || !drop_found_sources(cwd, &mut stems, &mut budget) {
        return;
    }
    for (name, stem) in outputs {
        if stems.contains(&stem) {
            fs::remove_file(output_directory.join(name)).ok();
        }
    }
}

/// The main file's name in an output's, or `None` for files of other kinds.
fn output_stem(name: &str) -> Option<&str> {
    if name.starts_with('.') || name.starts_with(preamble::PREFIX) {
        return None;
    }
    let extensions = OUTPUT_EXTENSIONS.iter().chain(LATEX_STATE_EXTENSIONS);
    extensions.filter_map(|extension| name.strip_suffix(extension)?.strip_suffix('.')).find(|stem| !stem.is_empty())
}

/// Takes every stem with a source out of `stems`. False when the search ended
/// early, so that the rest cannot count as gone.
fn drop_found_sources(folder: &Path, stems: &mut Vec<String>, budget: &mut usize) -> bool {
    let Ok(entries) = fs::read_dir(folder) else {
        return false;
    };
    let mut folders = Vec::new();
    for entry in entries.flatten() {
        if stems.is_empty() {
            return true;
        }
        let Some(left) = budget.checked_sub(1) else {
            return false;
        };
        *budget = left;
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            return false;
        };
        // Hidden folders hold build output and version history, not sources.
        if kind.is_dir() && !entry.file_name().to_string_lossy().starts_with('.') {
            folders.push(path);
        } else if path.extension().is_some_and(|extension| SOURCE_EXTENSIONS.iter().any(|known| extension == *known))
            && let Some(stem) = path.file_stem()
        {
            stems.retain(|known| known.as_str() != stem);
        }
    }
    folders.iter().all(|folder| stems.is_empty() || drop_found_sources(folder, stems, budget))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(folder: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(folder).unwrap().map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        names
    }

    #[test]
    fn outputs_name_their_main_file() {
        assert_eq!(output_stem("thesis.synctex.gz"), Some("thesis"));
        assert_eq!(output_stem("a.b.run.xml"), Some("a.b"));
        assert_eq!(output_stem("thesis.pdf"), Some("thesis"));
        for other in ["pdftex.map", "preamble-0123.log", ".metadata_never_index", ".pdf", "notes.txt"] {
            assert_eq!(output_stem(other), None, "{other}");
        }
    }

    #[test]
    fn only_outputs_of_gone_main_files_go() {
        let directory = tempfile::tempdir().unwrap();
        let cwd = directory.path();
        let output = cwd.join(".build");
        fs::create_dir_all(output.join("chapters")).unwrap();
        fs::create_dir_all(cwd.join("deep/er")).unwrap();
        fs::write(cwd.join("main.tex"), "").unwrap();
        fs::write(cwd.join("deep/er/poster.typ"), "").unwrap();
        // Same names as gone outputs, but no main file.
        fs::write(cwd.join("old.bib"), "").unwrap();
        fs::create_dir(cwd.join("old.tex")).unwrap();
        let kept = [
            "chapters", "main.aux", "main.log", "main.pdf", "pdftex.map", "poster.pdf", "preamble-0123.fmt", "preamble-0123.log",
            "renamed.pdf",
        ];
        for name in kept.iter().chain(&["old.aux", "old.pdf", "old.synctex.gz", "old.fdb_latexmk"]) {
            if *name != "chapters" {
                fs::write(output.join(name), "").unwrap();
            }
        }
        fs::write(output.join("chapters/old.aux"), "").unwrap();
        // The current main file has no source yet, as before its first save.
        remove_orphans(cwd, &output, "renamed");
        assert_eq!(names(&output), kept);
        assert_eq!(names(&output.join("chapters")), ["old.aux"]);
        assert_eq!(names(cwd), [".build", "deep", "main.tex", "old.bib", "old.tex"]);
    }

    #[test]
    fn nothing_goes_outside_a_build_folder_or_after_a_cut_off_search() {
        let directory = tempfile::tempdir().unwrap();
        let cwd = directory.path();
        fs::write(cwd.join("gone.pdf"), "").unwrap();
        remove_orphans(cwd, cwd, "main");
        assert_eq!(names(cwd), ["gone.pdf"]);

        let output = cwd.join(".build");
        fs::create_dir(&output).unwrap();
        fs::write(output.join("gone.pdf"), "").unwrap();
        let mut stems = vec!["gone".to_string()];
        assert!(!drop_found_sources(cwd, &mut stems, &mut 1));
        let mut budget = MAX_SEARCHED;
        assert!(drop_found_sources(cwd, &mut stems, &mut budget));
        assert_eq!(stems, ["gone"]);
    }

    #[test]
    fn a_build_folder_is_marked_once() {
        let directory = tempfile::tempdir().unwrap();
        keep_out_of_backups(directory.path());
        assert!(directory.path().join(NEVER_INDEX).is_file());
        #[cfg(target_os = "macos")]
        {
            let output = std::process::Command::new("xattr").arg("-px").arg(BACKUP_ATTRIBUTE).arg(directory.path()).output().unwrap();
            let hex: String = BACKUP_EXCLUSION.iter().map(|byte| format!("{byte:02X}")).collect();
            let shown: String = String::from_utf8_lossy(&output.stdout).split_whitespace().collect();
            assert_eq!(shown, hex);
        }
        keep_out_of_backups(directory.path());
        assert_eq!(names(directory.path()), [NEVER_INDEX]);
    }
}
