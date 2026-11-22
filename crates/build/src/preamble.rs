//! A precompiled preamble. LaTeX spends most of a small edit on loading packages,
//! and a saved format skips that. Everything here may fail: the normal run is always
//! there, so a format is only used where it has proven harmless.

use std::{
    fs,
    hash::{DefaultHasher, Hasher},
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    time::SystemTime,
};

pub(crate) const PREFIX: &str = "preamble-";
const BEGIN_DOCUMENT: &str = "\\begin{document}";
const FORMAT_VERSION: &[u8] = b"2";
/// Things that only work in a normal run, because a format keeps no open files and
/// freezes the name of the job.
const NOT_FOR_FORMATS: &[&str] = &["\\jobname", "\\openout", "\\write18"];

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum State {
    /// Not seen before, so it may still be changing. Worth nothing yet.
    New,
    /// Used twice, so a format pays off.
    Wanted,
    Unsuitable,
    /// The format exists, but it has not led to a good PDF yet.
    Untested,
    Trusted,
}

pub(crate) struct Preamble {
    key: u64,
    directory: PathBuf,
}

impl Preamble {
    /// `None` when the main file has no preamble that can be cut off safely.
    pub(crate) fn of(main: &Path, directory: &Path) -> Option<Self> {
        let text = fs::read(main).ok()?;
        let preamble = preamble_text(&String::from_utf8_lossy(&text))?.to_string();
        let mut hasher = DefaultHasher::new();
        hasher.write(preamble.as_bytes());
        // A new way of building formats replaces the ones built the old way.
        hasher.write(FORMAT_VERSION);
        Some(Self { key: hasher.finish(), directory: directory.to_path_buf() })
    }

    pub(crate) fn name(&self) -> String {
        format!("{PREFIX}{:016x}", self.key)
    }

    fn record_path(&self) -> PathBuf {
        self.directory.join(format!("{}.inputs", self.name()))
    }

    fn format_path(&self) -> PathBuf {
        self.directory.join(format!("{}.fmt", self.name()))
    }

    pub(crate) fn state(&self) -> State {
        let Ok(record) = fs::read_to_string(self.record_path()) else {
            return State::New;
        };
        let mut lines = record.lines();
        let status = lines.next().unwrap_or_default();
        match status {
            "unsuitable" => return State::Unsuitable,
            "untested" | "trusted" => {}
            _ => return State::Wanted,
        }
        // A changed package or a new compiler needs a new format.
        if !self.format_path().is_file() || !lines.all(input_is_unchanged) {
            return State::Wanted;
        }
        if status == "trusted" { State::Trusted } else { State::Untested }
    }

    fn set_status(&self, status: &str, inputs: &str) {
        fs::write(self.record_path(), format!("{status}\n{inputs}")).ok();
    }

    pub(crate) fn mark_seen(&self) {
        self.set_status("seen", "");
    }

    pub(crate) fn mark_unsuitable(&self) {
        self.set_status("unsuitable", "");
        fs::remove_file(self.format_path()).ok();
    }

    pub(crate) fn mark_trusted(&self) {
        if let Ok(record) = fs::read_to_string(self.record_path())
            && let Some(inputs) = record.strip_prefix("untested\n")
        {
            self.set_status("trusted", inputs);
        }
    }

    /// Reads what the format needed, right after building it. A format that wrote
    /// other files, such as an index, would lose them in the real run without any
    /// error, so it is not used.
    pub(crate) fn finish_format(&self, cwd: &Path, main: &Path, compiler: &Path, started: SystemTime) -> bool {
        let recorded = fs::read_to_string(self.directory.join(format!("{}.fls", self.name()))).unwrap_or_default();
        let inputs = recorded_inputs(&recorded);
        if inputs.is_empty() || self.wrote_other_files(started) {
            self.mark_unsuitable();
            return false;
        }
        let main = main.canonicalize().ok();
        let mut lines = String::new();
        let compiler = std::iter::once(compiler.to_path_buf());
        for input in compiler.chain(inputs.iter().map(|input| cwd.join(input))) {
            // The main file changes all the time, and the key covers its preamble.
            if input.canonicalize().ok() == main {
                continue;
            }
            if let Some(line) = input_line(&input) {
                lines.push_str(&line);
                lines.push('\n');
            }
        }
        self.set_status("untested", &lines);
        true
    }

    fn wrote_other_files(&self, started: SystemTime) -> bool {
        let Ok(entries) = fs::read_dir(&self.directory) else {
            return true;
        };
        let name = self.name();
        let own = [format!("{name}.fmt"), format!("{name}.log"), format!("{name}.fls"), format!("{name}.inputs")];
        entries.flatten().any(|entry| {
            let is_own = own.iter().any(|file| entry.file_name() == file.as_str());
            let is_new = entry.metadata().and_then(|metadata| metadata.modified()).is_ok_and(|time| time >= started);
            !is_own && is_new && entry.file_type().is_ok_and(|kind| kind.is_file())
        })
    }

    /// Formats are large, and old ones rarely come back.
    pub(crate) fn remove_others(&self) {
        remove_formats(&self.directory, Some(&self.name()));
    }
}

/// Removes every format and what belongs to it, except `keep`.
pub(crate) fn remove_formats(directory: &Path, keep: Option<&str>) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with(PREFIX) && !keep.is_some_and(|keep| name.starts_with(keep)) {
            fs::remove_file(entry.path()).ok();
        }
    }
}

/// True if the compiler could not use the format at all, which says nothing about
/// the document.
pub(crate) fn format_failed(log: Option<&str>) -> bool {
    log.is_none_or(|log| log.contains("format file") || log.contains("stymied"))
}

/// The text before `\begin{document}`, if that line stands on its own.
fn preamble_text(text: &str) -> Option<&str> {
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let code = line.split('%').next().unwrap_or_default();
        if code.contains(BEGIN_DOCUMENT) {
            // In a macro or behind other text, the real start could lie elsewhere.
            if !code.trim_start().starts_with(BEGIN_DOCUMENT) {
                return None;
            }
            let preamble = &text[..offset];
            let comments_removed = preamble.lines().map(|line| line.split('%').next().unwrap_or_default());
            let harmless = !comments_removed.into_iter().any(|line| NOT_FOR_FORMATS.iter().any(|word| line.contains(word)));
            return (harmless && preamble.contains("\\documentclass")).then_some(preamble);
        }
        offset += line.len();
    }
    None
}

/// What `-recorder` says the format read.
fn recorded_inputs(recorded: &str) -> Vec<String> {
    let mut inputs: Vec<String> = Vec::new();
    for path in recorded.lines().filter_map(|line| line.strip_prefix("INPUT ")) {
        if !inputs.iter().any(|input| input == path) {
            inputs.push(path.to_string());
        }
    }
    inputs
}

fn input_line(path: &Path) -> Option<String> {
    let metadata = fs::metadata(path).ok()?;
    Some(format!("{} {} {}", metadata.len(), metadata.mtime_nsec() + metadata.mtime() * 1_000_000_000, path.display()))
}

fn input_is_unchanged(line: &str) -> bool {
    let mut parts = line.splitn(3, ' ');
    let (_, _, Some(path)) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    input_line(Path::new(path)).is_some_and(|now| now == line)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_preamble_ends_at_the_line_that_starts_the_document() {
        let text = "\\documentclass{article}\n% \\begin{document}\n\\usepackage{a}\n  \\begin{document}\nx\n";
        assert_eq!(preamble_text(text), Some("\\documentclass{article}\n% \\begin{document}\n\\usepackage{a}\n"));
        assert_eq!(preamble_text("\\documentclass{a}\n\\def\\x{\\begin{document}}\n\\x\n"), None);
        assert_eq!(preamble_text("just text\n"), None);
        assert_eq!(preamble_text("\\begin{document}\n"), None);
    }

    #[test]
    fn some_preambles_never_fit_a_format() {
        for line in ["\\immediate\\openout\\f=x.y", "\\edef\\name{\\jobname}"] {
            assert_eq!(preamble_text(&format!("\\documentclass{{a}}\n{line}\n\\begin{{document}}\n")), None);
        }
        let commented = "\\documentclass{a}\n% \\jobname\n\\begin{document}\n";
        assert!(preamble_text(commented).is_some());
    }

    #[test]
    fn inputs_come_once_each() {
        let recorded = "PWD /p\nINPUT a.sty\nINPUT a.sty\nOUTPUT preamble-1.fmt\nINPUT /b.cls\n";
        assert_eq!(recorded_inputs(recorded), ["a.sty", "/b.cls"]);
    }

    #[test]
    fn a_format_that_writes_other_files_is_not_used() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let main = root.join("main.tex");
        fs::write(&main, "\\documentclass{a}\n\\begin{document}\n").unwrap();
        let old = root.join("old.aux");
        fs::write(&old, "from an earlier run").unwrap();
        let started = SystemTime::now() + std::time::Duration::from_secs(5);
        let preamble = Preamble::of(&main, &root).unwrap();
        fs::write(root.join(format!("{}.fls", preamble.name())), "INPUT /a.sty\n").unwrap();
        fs::write(preamble.format_path(), "format").unwrap();
        // Everything so far is older than this start, so nothing looks new.
        assert!(preamble.finish_format(&root, &main, &main, started));
        let early = SystemTime::now() - std::time::Duration::from_secs(5);
        assert!(!preamble.finish_format(&root, &main, &main, early));
        assert_eq!(preamble.state(), State::Unsuitable);
    }

    #[test]
    fn a_format_goes_through_its_states_and_follows_its_inputs() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let main = root.join("main.tex");
        let package = root.join("local.sty");
        let compiler = root.join("compiler");
        fs::write(&main, "\\documentclass{a}\n\\usepackage{local}\n\\begin{document}\nx\n").unwrap();
        fs::write(&package, "one").unwrap();
        fs::write(&compiler, "c").unwrap();

        let preamble = Preamble::of(&main, &root).unwrap();
        assert_eq!(preamble.state(), State::New);
        preamble.mark_seen();
        assert_eq!(preamble.state(), State::Wanted);

        fs::write(preamble.format_path(), "format").unwrap();
        fs::write(root.join(format!("{}.fls", preamble.name())), "INPUT local.sty\nINPUT main.tex\n").unwrap();
        let started = SystemTime::now() + std::time::Duration::from_secs(5);
        assert!(preamble.finish_format(&root, &main, &compiler, started));
        assert_eq!(preamble.state(), State::Untested);
        preamble.mark_trusted();
        assert_eq!(preamble.state(), State::Trusted);
        // Text after the preamble is not part of the key, and the main file no input.
        fs::write(&main, "\\documentclass{a}\n\\usepackage{local}\n\\begin{document}\ny\n").unwrap();
        assert_eq!(Preamble::of(&main, &root).unwrap().name(), preamble.name());
        assert_eq!(preamble.state(), State::Trusted);

        fs::write(&package, "changed").unwrap();
        assert_eq!(preamble.state(), State::Wanted);

        preamble.mark_unsuitable();
        assert_eq!(preamble.state(), State::Unsuitable);
        fs::write(&main, "\\documentclass{b}\n\\begin{document}\n").unwrap();
        assert_ne!(Preamble::of(&main, &root).unwrap().name(), preamble.name());
    }

    #[test]
    fn only_the_current_format_stays() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let main = root.join("main.tex");
        fs::write(&main, "\\documentclass{a}\n\\begin{document}\n").unwrap();
        let preamble = Preamble::of(&main, root).unwrap();
        let name = preamble.name();
        let own = [format!("{name}.fmt"), format!("{name}.inputs")];
        let others = ["preamble-00000000000000aa.fmt", "preamble-00000000000000aa.inputs", "preamble-00000000000000bb.log"];
        for file in own.iter().map(String::as_str).chain(others).chain(["main.pdf", "main.aux"]) {
            fs::write(root.join(file), "").unwrap();
        }
        preamble.remove_others();
        let mut left: Vec<String> = fs::read_dir(root).unwrap().map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned()).collect();
        left.sort();
        let mut expected = vec!["main.aux".to_string(), "main.pdf".to_string(), "main.tex".to_string()];
        expected.extend(own.iter().cloned());
        expected.sort();
        assert_eq!(left, expected);

        remove_formats(root, None);
        assert!(!root.join(&own[0]).exists() && root.join("main.pdf").exists());
    }

    #[test]
    fn only_a_format_error_blames_the_format() {
        assert!(format_failed(None));
        assert!(format_failed(Some("Fatal format file error; I'm stymied")));
        assert!(!format_failed(Some("! Undefined control sequence.")));
    }
}
