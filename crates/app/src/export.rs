//! Writes exports. The project is only read, and a result reaches its target
//! only complete and checked, so a failure leaves an existing file as it was.

use std::{
    ffi::OsString,
    fs::{self, File},
    io::{self, BufReader, BufWriter, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    process::Stdio,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use crate::atomic::{self, TEMP_PREFIX};
use sha2::{Digest, Sha256};
use tempfile::NamedTempFile;
use zip::{CompressionMethod, DateTime, ZipArchive, ZipWriter, write::SimpleFileOptions};

/// Never part of a source archive: version history, build output and Finder data.
const EXCLUDED: &[&str] = &[".git", ".build", ".DS_Store"];
/// Formats that are compressed already, so Deflate only costs time.
const STORED_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "webp", "gif", "pdf", "zip", "gz", "docx", "xlsx", "pptx", "mp4"];
/// Earlier exports do not make a PDF out of date.
const EXPORT_EXTENSIONS: &[&str] = &["zip", "docx", "html"];
const PANDOC_TIME_LIMIT: Duration = Duration::from_secs(120);
const CHUNK: usize = 64 * 1024;

fn start_temp(target: &Path) -> Result<NamedTempFile, String> {
    let folder = target.parent().ok_or_else(|| format!("{} has no folder", target.display()))?;
    tempfile::Builder::new()
        .prefix(TEMP_PREFIX)
        .tempfile_in(folder)
        .map_err(|error| format!("Cannot write to {}: {error}", folder.display()))
}

fn commit(temp: NamedTempFile, target: &Path) -> Result<(), String> {
    atomic::finish(temp, target).map_err(|error| format!("Cannot write {}: {error}", target.display()))
}

fn hash_file(path: &Path) -> io::Result<Vec<u8>> {
    let mut hasher = Sha256::new();
    io::copy(&mut File::open(path)?, &mut hasher)?;
    Ok(hasher.finalize().to_vec())
}

/// A PDF starts with its version and ends with an end marker, which a cut-off file lacks.
fn check_pdf(path: &Path) -> Result<(), String> {
    let invalid = || format!("{} is not a complete PDF", path.display());
    let mut file = File::open(path).map_err(|error| format!("Cannot read {}: {error}", path.display()))?;
    let mut head = [0; 5];
    file.read_exact(&mut head).map_err(|_| invalid())?;
    if &head != b"%PDF-" {
        return Err(invalid());
    }
    let size = file.seek(SeekFrom::End(0)).map_err(|_| invalid())?;
    file.seek(SeekFrom::Start(size.saturating_sub(1024))).map_err(|_| invalid())?;
    let mut tail = Vec::new();
    file.read_to_end(&mut tail).map_err(|_| invalid())?;
    if !tail.windows(5).any(|window| window == b"%%EOF") {
        return Err(invalid());
    }
    Ok(())
}

/// Copies the PDF byte for byte, so that its layout is the preview's.
pub fn copy_pdf(source: &Path, target: &Path) -> Result<(), String> {
    check_pdf(source)?;
    let mut temp = start_temp(target)?;
    let failed = |error: io::Error| format!("Cannot write {}: {error}", target.display());
    let mut input = File::open(source).map_err(failed)?;
    io::copy(&mut input, temp.as_file_mut()).map_err(failed)?;
    temp.as_file().sync_all().map_err(failed)?;
    if hash_file(source).map_err(failed)? != hash_file(temp.path()).map_err(failed)? {
        return Err(format!("{} changed during the export", source.display()));
    }
    check_pdf(temp.path())?;
    commit(temp, target)
}

/// One file or empty folder of a project.
#[derive(Clone, Debug)]
pub struct Entry {
    /// With `/` between folders, as an archive stores it.
    pub relative: String,
    source: PathBuf,
    is_dir: bool,
    modified: Option<SystemTime>,
    size: u64,
    mode: u32,
}

#[derive(Default)]
struct Walk {
    entries: Vec<Entry>,
    problems: Vec<String>,
    skip: Option<PathBuf>,
    /// The folders being walked, to notice links that lead back.
    chain: Vec<PathBuf>,
}

fn mode_of(metadata: &fs::Metadata) -> u32 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o777
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        0o644
    }
}

impl Walk {
    fn visit(&mut self, directory: &Path, prefix: &str) {
        let listing = match fs::read_dir(directory) {
            Ok(listing) => listing,
            Err(error) => {
                self.problems.push(format!("{prefix}: {error}"));
                return;
            }
        };
        let mut children: Vec<_> = listing.flatten().collect();
        children.sort_by_key(|child| child.file_name());
        let before = self.entries.len();
        for child in children {
            let file_name = child.file_name();
            let Some(name) = file_name.to_str() else {
                self.problems.push(format!("{prefix}{}: the name is not valid Unicode", file_name.to_string_lossy()));
                continue;
            };
            if EXCLUDED.contains(&name) || name.starts_with(TEMP_PREFIX) {
                continue;
            }
            let path = child.path();
            let relative = format!("{prefix}{name}");
            if let Some(skip) = &self.skip
                && skip.file_name() == Some(file_name.as_os_str())
                && fs::canonicalize(&path).is_ok_and(|canonical| canonical == *skip)
            {
                continue;
            }
            // Follows links: an archive holds what the link points at.
            let metadata = match fs::metadata(&path) {
                Ok(metadata) => metadata,
                Err(error) => {
                    self.problems.push(format!("{relative}: {error}"));
                    continue;
                }
            };
            if metadata.is_dir() {
                let Ok(canonical) = fs::canonicalize(&path) else {
                    self.problems.push(format!("{relative}: cannot be resolved"));
                    continue;
                };
                if self.chain.contains(&canonical) {
                    self.problems.push(format!("{relative}: the link leads back to a parent folder"));
                    continue;
                }
                self.chain.push(canonical);
                self.visit(&path, &format!("{relative}/"));
                self.chain.pop();
            } else if metadata.is_file() {
                self.entries.push(Entry {
                    relative,
                    source: path,
                    is_dir: false,
                    modified: metadata.modified().ok(),
                    size: metadata.len(),
                    mode: mode_of(&metadata),
                });
            } else {
                self.problems.push(format!("{relative}: not a regular file"));
            }
        }
        if self.entries.len() == before && !prefix.is_empty() {
            self.entries.push(Entry {
                relative: prefix.to_string(),
                source: directory.to_path_buf(),
                is_dir: true,
                modified: None,
                size: 0,
                mode: 0o755,
            });
        }
    }
}

fn walk(root: &Path, skip: Option<PathBuf>) -> Walk {
    let mut walk = Walk { skip, ..Walk::default() };
    walk.chain.extend(fs::canonicalize(root));
    walk.visit(root, "");
    walk
}

/// The target as it will resolve, also before it exists.
fn resolved_target(target: &Path) -> Option<PathBuf> {
    Some(fs::canonicalize(target.parent()?).ok()?.join(target.file_name()?))
}

/// Everything an archive of the project holds. `.gitignore` does not count,
/// because generated figures that a build needs are often listed there. What
/// cannot be archived fails the whole export, as a partial archive looks complete.
pub fn project_entries(root: &Path, target: &Path) -> Result<Vec<Entry>, String> {
    let found = walk(root, resolved_target(target));
    match found.problems.first() {
        Some(problem) => Err(format!("Cannot archive {problem}")),
        None => Ok(found.entries),
    }
}

fn is_suspicious(name: &str) -> bool {
    let name = name.to_lowercase();
    name.starts_with(".env")
        || name.starts_with("id_rsa")
        || name.starts_with("credentials")
        || name == ".netrc"
        || [".pem", ".key", ".p12"].iter().any(|ending| name.ends_with(ending))
}

/// Files that often hold secrets, which an archive for others should not carry by accident.
pub fn suspicious_files(entries: &[Entry]) -> Vec<String> {
    entries
        .iter()
        .filter(|entry| !entry.is_dir && is_suspicious(entry.relative.rsplit('/').next().unwrap_or(&entry.relative)))
        .map(|entry| entry.relative.clone())
        .collect()
}

/// Whether a build of the archive has to download packages.
pub fn uses_online_packages(entries: &[Entry]) -> bool {
    entries.iter().any(|entry| {
        entry.relative.ends_with(".typ") && fs::read_to_string(&entry.source).is_ok_and(|text| text.contains("@preview/"))
    })
}

/// An archive stores local dates without a zone. UTC is close enough for files.
fn archive_date(modified: Option<SystemTime>) -> Option<DateTime> {
    let seconds = modified?.duration_since(UNIX_EPOCH).ok()?.as_secs();
    // Howard Hinnant's conversion from days to a civil date.
    let shifted = (seconds / 86_400) as i64 + 719_468;
    let era = shifted / 146_097;
    let day_of_era = shifted - era * 146_097;
    let year_of_era = (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 { shifted_month + 3 } else { shifted_month - 9 };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    let clock = seconds % 86_400;
    DateTime::from_date_and_time(
        u16::try_from(year).ok()?,
        month as u8,
        day as u8,
        (clock / 3600) as u8,
        (clock / 60 % 60) as u8,
        (clock % 60) as u8,
    )
    .ok()
}

fn method_for(relative: &str) -> CompressionMethod {
    let extension = relative.rsplit_once('.').map_or(String::new(), |(_, extension)| extension.to_lowercase());
    if STORED_EXTENSIONS.contains(&extension.as_str()) { CompressionMethod::Stored } else { CompressionMethod::Deflated }
}

fn write_zip(entries: &[Entry], file: &mut File) -> Result<(), String> {
    let mut writer = ZipWriter::new(BufWriter::with_capacity(CHUNK, file));
    for entry in entries {
        let failed = |error: &dyn std::fmt::Display| format!("Cannot archive {}: {error}", entry.relative);
        let mut options = SimpleFileOptions::default()
            .compression_method(method_for(&entry.relative))
            .large_file(entry.size >= u64::from(u32::MAX))
            .unix_permissions(entry.mode);
        if let Some(date) = archive_date(entry.modified) {
            options = options.last_modified_time(date);
        }
        if entry.is_dir {
            writer.add_directory(entry.relative.as_str(), options).map_err(|error| failed(&error))?;
            continue;
        }
        let mut input = File::open(&entry.source).map_err(|error| failed(&error))?;
        writer.start_file(entry.relative.as_str(), options).map_err(|error| failed(&error))?;
        io::copy(&mut input, &mut writer).map_err(|error| failed(&error))?;
    }
    let mut buffer = writer.finish().map_err(|error| format!("Cannot finish the archive: {error}"))?;
    buffer.flush().map_err(|error| format!("Cannot finish the archive: {error}"))
}

/// Compares two streams without holding either in memory.
fn same_bytes(first: &mut impl Read, second: &mut impl Read) -> io::Result<bool> {
    fn fill(reader: &mut impl Read, buffer: &mut [u8]) -> io::Result<usize> {
        let mut filled = 0;
        while filled < buffer.len() {
            match reader.read(&mut buffer[filled..])? {
                0 => break,
                count => filled += count,
            }
        }
        Ok(filled)
    }
    let mut left = vec![0; CHUNK];
    let mut right = vec![0; CHUNK];
    loop {
        let (left_count, right_count) = (fill(first, &mut left)?, fill(second, &mut right)?);
        if left[..left_count] != right[..right_count] {
            return Ok(false);
        }
        if left_count == 0 {
            return Ok(true);
        }
    }
}

/// Opens the archive again and compares every entry with its original.
pub fn verify_zip(archive: &Path, entries: &[Entry]) -> Result<(), String> {
    let file = File::open(archive).map_err(|error| format!("Cannot check the archive: {error}"))?;
    let mut opened =
        ZipArchive::new(BufReader::new(file)).map_err(|error| format!("The archive is damaged: {error}"))?;
    if opened.len() != entries.len() {
        return Err(format!("The archive holds {} entries instead of {}", opened.len(), entries.len()));
    }
    for entry in entries {
        let damaged = |error: &dyn std::fmt::Display| format!("The archive is damaged at {}: {error}", entry.relative);
        let mut packed = opened.by_name(&entry.relative).map_err(|error| damaged(&error))?;
        if entry.is_dir {
            if !packed.is_dir() {
                return Err(damaged(&"expected a folder"));
            }
            continue;
        }
        let mut original = File::open(&entry.source).map_err(|error| format!("Cannot read {}: {error}", entry.relative))?;
        if !same_bytes(&mut packed, &mut original).map_err(|error| damaged(&error))? {
            return Err(format!("{} changed during the export", entry.relative));
        }
    }
    Ok(())
}

/// Packs `entries` and checks the result before it reaches `target`.
pub fn zip_project(entries: &[Entry], target: &Path) -> Result<(), String> {
    let mut temp = start_temp(target)?;
    write_zip(entries, temp.as_file_mut())?;
    temp.as_file().sync_all().map_err(|error| format!("Cannot write {}: {error}", target.display()))?;
    verify_zip(temp.path(), entries)?;
    commit(temp, target)
}

/// Whether writing to `target` would replace one of `sources`.
pub fn is_source(target: &Path, sources: &[PathBuf]) -> bool {
    let Some(target) = resolved_target(target) else {
        return false;
    };
    let target = fs::canonicalize(&target).unwrap_or(target);
    sources.iter().any(|source| fs::canonicalize(source).is_ok_and(|source| source == target))
}

/// What the build left behind, for the check before a PDF goes out.
pub struct PdfState<'a> {
    pub building: bool,
    pub unsaved: bool,
    pub has_errors: bool,
    pub last_good: bool,
    pub newest_source: Option<SystemTime>,
    pub pdf_modified: Option<SystemTime>,
    pub warnings: &'a [String],
}

fn counted(count: usize, name: &str) -> String {
    format!("{count} undefined {name}{}", if count == 1 { "" } else { "s" })
}

/// Why this PDF may not be what its author expects. Empty when it is current.
pub fn pdf_problems(state: &PdfState) -> Vec<String> {
    let mut problems = Vec::new();
    if state.building {
        problems.push("A build is running".to_string());
    }
    if state.unsaved {
        problems.push("Unsaved changes".to_string());
    }
    if state.has_errors {
        problems.push("This PDF has errors".to_string());
    }
    if state.last_good {
        problems.push("Last good PDF, not the current source".to_string());
    }
    if let (Some(source), Some(pdf)) = (state.newest_source, state.pdf_modified)
        && source > pdf
    {
        problems.push("Files changed since the build".to_string());
    }
    let is_undefined = |start: &str| {
        let mut found: Vec<&String> =
            state.warnings.iter().filter(|warning| warning.contains(start) && warning.contains("undefined")).collect();
        found.sort();
        found.dedup();
        found.len()
    };
    let references = is_undefined("Reference `");
    let citations = is_undefined("Citation `");
    if references > 0 {
        problems.push(counted(references, "reference"));
    }
    if citations > 0 {
        problems.push(counted(citations, "citation"));
    }
    if references == 0 && citations == 0 && state.warnings.iter().any(|warning| warning.contains("undefined references")) {
        problems.push("Undefined references".to_string());
    }
    if state.warnings.iter().any(|warning| warning.to_lowercase().contains("rerun")) {
        problems.push("References may be out of date".to_string());
    }
    problems
}

/// When a file of the project last changed, apart from earlier exports, which
/// carry the PDF's name or a draft's ending. The build's own PDF is in `.build`.
pub fn newest_source(root: &Path, pdf_name: &str) -> Option<SystemTime> {
    walk(root, None)
        .entries
        .iter()
        .filter(|entry| {
            let name = entry.relative.rsplit('/').next().unwrap_or(&entry.relative);
            let extension = name.rsplit_once('.').map_or("", |(_, extension)| extension);
            let (stem, _) = name.rsplit_once('.').unwrap_or((name, ""));
            name != pdf_name && !stem.ends_with("-draft") && !EXPORT_EXTENSIONS.contains(&extension)
        })
        .filter_map(|entry| entry.modified)
        .max()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DraftFormat {
    Word,
    Html,
    Markdown,
}

impl DraftFormat {
    pub fn extension(self) -> &'static str {
        match self {
            DraftFormat::Word => "docx",
            DraftFormat::Html => "html",
            DraftFormat::Markdown => "md",
        }
    }

    fn writer(self) -> &'static str {
        match self {
            DraftFormat::Word => "docx",
            DraftFormat::Html => "html",
            DraftFormat::Markdown => "markdown",
        }
    }
}

/// Pandoc's name for the format of `main`, when it reads that format.
pub fn pandoc_reader(main: &Path) -> Option<&'static str> {
    match main.extension()?.to_str()? {
        "tex" => Some("latex"),
        "typ" => Some("typst"),
        "md" | "markdown" => Some("markdown"),
        _ => None,
    }
}

/// `input` is relative to Pandoc's folder, so that its report stays readable.
fn pandoc_args(reader: &str, format: DraftFormat, input: &Path, output: &Path) -> Vec<OsString> {
    let mut args: Vec<OsString> = ["-f", reader, "-t", format.writer(), "--standalone", "--citeproc", "--verbose"]
        .into_iter()
        .map(OsString::from)
        .collect();
    if format == DraftFormat::Html {
        args.extend(["--embed-resources", "--mathml"].map(OsString::from));
    }
    args.extend([OsString::from("-o"), output.into(), input.into()]);
    args
}

const REPORT_LINE_LIMIT: usize = 120;

/// What Pandoc left out or could not do. It finishes successfully either way,
/// so this log is the only sign of a loss.
pub fn parse_pandoc_report(stderr: &str) -> Vec<String> {
    let mut report: Vec<String> = Vec::new();
    for line in stderr.lines() {
        let Some(message) = line.strip_prefix("[WARNING] ").or_else(|| {
            line.strip_prefix("[INFO] ").filter(|message| message.starts_with("Skipped ") || message.starts_with("Ignored "))
        }) else {
            continue;
        };
        let message = message.strip_prefix("Skipped ").unwrap_or(message).trim().replace(" at ./", " at ");
        // The place comes last and must survive when a long passage is cut.
        let (passage, place) = message.rsplit_once(" at ").map_or((message.as_str(), None), |(passage, place)| (passage, Some(place)));
        let mut short: String = passage.chars().take(REPORT_LINE_LIMIT).collect();
        if short.len() < passage.len() {
            short.push('…');
        }
        if let Some(place) = place {
            short = format!("{short} at {place}");
        }
        if !report.contains(&short) {
            report.push(short);
        }
    }
    report
}

/// A finished draft that has not reached its target yet.
pub struct Draft {
    temp: NamedTempFile,
    pub report: Vec<String>,
}

impl Draft {
    pub fn keep(self, target: &Path) -> Result<(), String> {
        commit(self.temp, target)
    }
}

fn check_draft(path: &Path, format: DraftFormat) -> Result<(), String> {
    let invalid = |detail: &dyn std::fmt::Display| format!("Pandoc wrote no valid file: {detail}");
    let size = fs::metadata(path).map_err(|error| invalid(&error))?.len();
    if size == 0 {
        return Err(invalid(&"the file is empty"));
    }
    if format == DraftFormat::Word {
        let file = File::open(path).map_err(|error| invalid(&error))?;
        let mut archive = ZipArchive::new(BufReader::new(file)).map_err(|error| invalid(&error))?;
        archive.by_name("word/document.xml").map_err(|error| invalid(&error))?;
    }
    Ok(())
}

/// Runs Pandoc beside the target. A failure or a timeout leaves no file behind.
pub fn run_pandoc(pandoc: &Path, main: &Path, format: DraftFormat, target: &Path) -> Result<Draft, String> {
    let reader = pandoc_reader(main).ok_or_else(|| format!("Pandoc cannot read {}", main.display()))?;
    let temp = start_temp(target)?;
    let name = main.file_name().ok_or_else(|| format!("{} has no file name", main.display()))?;
    let mut command = crate::platform::command(pandoc);
    command
        .args(pandoc_args(reader, format, &Path::new(".").join(name), temp.path()))
        .envs(build::child_env())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    if let Some(folder) = main.parent() {
        command.current_dir(folder);
    }
    let mut child = command.spawn().map_err(|error| format!("Cannot start Pandoc: {error}"))?;
    let mut stderr = child.stderr.take().ok_or("Cannot read Pandoc's output")?;
    // The pipe has to drain while Pandoc runs, or a long log blocks it.
    let log = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stderr.read_to_end(&mut bytes).ok();
        String::from_utf8_lossy(&bytes).into_owned()
    });
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() >= PANDOC_TIME_LIMIT => {
                child.kill().ok();
                child.wait().ok();
                return Err(format!("Pandoc did not finish within {} seconds", PANDOC_TIME_LIMIT.as_secs()));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(error) => return Err(format!("Cannot wait for Pandoc: {error}")),
        }
    };
    let log = log.join().unwrap_or_default();
    if !status.success() {
        let message: Vec<&str> = log.lines().filter(|line| !line.starts_with("[INFO]")).take(8).collect();
        return Err(if message.is_empty() { format!("Pandoc failed: {status}") } else { message.join("\n") });
    }
    check_draft(temp.path(), format)?;
    let mut report = parse_pandoc_report(&log);
    if reader == "typst" {
        // Pandoc reports nothing about these, as seen with `rect` and `#show heading`.
        report.push("Typst shapes, show rules and layout may be missing. Compare with the PDF.".to_string());
    }
    if format == DraftFormat::Markdown {
        report.push("Images stay links to the project's files".to_string());
    }
    Ok(Draft { temp, report })
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tempfile::tempdir;

    use super::*;

    const PDF: &[u8] = b"%PDF-1.7\nbody\n%%EOF\n";

    fn names(entries: &[Entry]) -> Vec<&str> {
        entries.iter().map(|entry| entry.relative.as_str()).collect()
    }

    fn folder_is_only(folder: &Path, expected: &[&str]) {
        let mut found: Vec<String> =
            fs::read_dir(folder).unwrap().map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned()).collect();
        found.sort();
        assert_eq!(found, expected);
    }

    #[test]
    fn a_pdf_copy_is_identical() {
        let folder = tempdir().unwrap();
        let (source, target) = (folder.path().join("a.pdf"), folder.path().join("b.pdf"));
        fs::write(&source, PDF).unwrap();
        copy_pdf(&source, &target).unwrap();
        assert_eq!(fs::read(&target).unwrap(), PDF);
        folder_is_only(folder.path(), &["a.pdf", "b.pdf"]);
    }

    #[cfg(unix)]
    #[test]
    fn an_export_is_readable_and_keeps_the_rights_it_replaces() {
        use std::os::unix::fs::PermissionsExt;
        let folder = tempdir().unwrap();
        let (source, target, kept) = (folder.path().join("a.pdf"), folder.path().join("b.pdf"), folder.path().join("c.pdf"));
        fs::write(&source, PDF).unwrap();
        fs::write(&kept, "old").unwrap();
        fs::set_permissions(&kept, fs::Permissions::from_mode(0o640)).unwrap();
        copy_pdf(&source, &target).unwrap();
        copy_pdf(&source, &kept).unwrap();
        assert_eq!(fs::metadata(&target).unwrap().permissions().mode() & 0o777, 0o644);
        assert_eq!(fs::metadata(&kept).unwrap().permissions().mode() & 0o777, 0o640);
    }

    #[test]
    fn a_cut_off_pdf_leaves_the_target_as_it_was() {
        let folder = tempdir().unwrap();
        let (source, target) = (folder.path().join("a.pdf"), folder.path().join("b.pdf"));
        fs::write(&target, "old").unwrap();
        for broken in [&b"%PDF-1.7\nbody"[..], b"", b"%%EOF"] {
            fs::write(&source, broken).unwrap();
            assert!(copy_pdf(&source, &target).is_err());
            assert_eq!(fs::read_to_string(&target).unwrap(), "old");
        }
        folder_is_only(folder.path(), &["a.pdf", "b.pdf"]);
    }

    #[test]
    fn a_missing_target_folder_is_an_error() {
        let folder = tempdir().unwrap();
        let source = folder.path().join("a.pdf");
        fs::write(&source, PDF).unwrap();
        let error = copy_pdf(&source, &folder.path().join("gone/b.pdf")).unwrap_err();
        assert!(error.contains("Cannot write to"), "{error}");
    }

    /// A project with what makes archives go wrong.
    fn project() -> (tempfile::TempDir, PathBuf) {
        let folder = tempdir().unwrap();
        let root = folder.path().join("project");
        for directory in ["kapitel/ä b", "leer/tiefer", ".git/objects", ".build", "nur_ds"] {
            fs::create_dir_all(root.join(directory)).unwrap();
        }
        fs::write(root.join("main.tex"), "\\documentclass{article}\n").unwrap();
        fs::write(root.join("kapitel/ä b/teil 1.tex"), "Größe\n").unwrap();
        fs::write(root.join("bild.png"), (0..=255u8).cycle().take(200_000).collect::<Vec<u8>>()).unwrap();
        fs::write(root.join(".gitignore"), "ignoriert.pdf\n").unwrap();
        fs::write(root.join("ignoriert.pdf"), "needed").unwrap();
        fs::write(root.join(".git/objects/x"), "history").unwrap();
        fs::write(root.join(".build/main.pdf"), PDF).unwrap();
        fs::write(root.join(".DS_Store"), "finder").unwrap();
        fs::write(root.join("nur_ds/.DS_Store"), "finder").unwrap();
        (folder, root)
    }

    #[test]
    fn an_archive_holds_the_project_and_nothing_else() {
        let (folder, root) = project();
        let target = folder.path().join("out.zip");
        let entries = project_entries(&root, &target).unwrap();
        assert_eq!(
            names(&entries),
            [".gitignore", "bild.png", "ignoriert.pdf", "kapitel/ä b/teil 1.tex", "leer/tiefer/", "main.tex", "nur_ds/"]
        );
        zip_project(&entries, &target).unwrap();
        let mut archive = ZipArchive::new(File::open(&target).unwrap()).unwrap();
        let mut text = String::new();
        archive.by_name("kapitel/ä b/teil 1.tex").unwrap().read_to_string(&mut text).unwrap();
        assert_eq!(text, "Größe\n");
        assert!(archive.by_name("leer/tiefer/").unwrap().is_dir());
        assert_eq!(archive.by_name("bild.png").unwrap().compression(), CompressionMethod::Stored);
        assert_eq!(archive.by_name("main.tex").unwrap().compression(), CompressionMethod::Deflated);
    }

    #[test]
    fn an_archive_in_the_project_does_not_pack_itself() {
        let (_folder, root) = project();
        let target = root.join("out.zip");
        fs::write(&target, "old").unwrap();
        let entries = project_entries(&root, &target).unwrap();
        assert!(!names(&entries).contains(&"out.zip"));
        zip_project(&entries, &target).unwrap();
        assert!(ZipArchive::new(File::open(&target).unwrap()).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn links_hold_what_they_point_at() {
        let (folder, root) = project();
        std::os::unix::fs::symlink(root.join("main.tex"), root.join("link.tex")).unwrap();
        let elsewhere = folder.path().join("figures");
        fs::create_dir(&elsewhere).unwrap();
        fs::write(elsewhere.join("a.txt"), "shared").unwrap();
        std::os::unix::fs::symlink(&elsewhere, root.join("figures")).unwrap();
        let target = folder.path().join("out.zip");
        let entries = project_entries(&root, &target).unwrap();
        zip_project(&entries, &target).unwrap();
        let mut archive = ZipArchive::new(File::open(&target).unwrap()).unwrap();
        let mut text = String::new();
        archive.by_name("link.tex").unwrap().read_to_string(&mut text).unwrap();
        assert!(text.starts_with("\\documentclass"));
        assert!(archive.by_name("figures/a.txt").is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn what_cannot_be_archived_names_itself() {
        let (folder, root) = project();
        let target = folder.path().join("out.zip");
        std::os::unix::fs::symlink(&root, root.join("kapitel/zurueck")).unwrap();
        assert!(project_entries(&root, &target).unwrap_err().contains("kapitel/zurueck"));
        fs::remove_file(root.join("kapitel/zurueck")).unwrap();
        std::os::unix::fs::symlink(root.join("gone"), root.join("kaputt")).unwrap();
        assert!(project_entries(&root, &target).unwrap_err().contains("kaputt"));
    }

    #[cfg(unix)]
    #[test]
    fn an_unreadable_file_stops_the_archive() {
        use std::os::unix::fs::PermissionsExt;
        let (folder, root) = project();
        let target = folder.path().join("out.zip");
        fs::write(&target, "old").unwrap();
        let entries = project_entries(&root, &target).unwrap();
        let locked = root.join("main.tex");
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
        let result = zip_project(&entries, &target);
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(result.unwrap_err().contains("main.tex"));
        assert_eq!(fs::read_to_string(&target).unwrap(), "old");
        folder_is_only(folder.path(), &["out.zip", "project"]);
    }

    #[test]
    fn checking_notices_changed_and_damaged_archives() {
        let (folder, root) = project();
        let target = folder.path().join("out.zip");
        let entries = project_entries(&root, &target).unwrap();
        zip_project(&entries, &target).unwrap();
        verify_zip(&target, &entries).unwrap();
        fs::write(root.join("main.tex"), "changed since").unwrap();
        assert!(verify_zip(&target, &entries).unwrap_err().contains("main.tex changed"));
        fs::write(root.join("main.tex"), "\\documentclass{article}\n").unwrap();
        assert!(verify_zip(&target, &entries[..entries.len() - 1]).unwrap_err().contains("entries"));
        let mut bytes = fs::read(&target).unwrap();
        // The stored picture lies in the middle, so this hits its data and not the headers.
        let middle = bytes.windows(4).position(|window| window == [0, 1, 2, 3]).unwrap() + 500;
        bytes[middle] ^= 0xff;
        fs::write(&target, bytes).unwrap();
        assert!(verify_zip(&target, &entries).is_err());
    }

    #[test]
    fn large_files_compare_in_chunks() {
        let data = vec![7u8; CHUNK * 2 + 3];
        let mut other = data.clone();
        assert!(same_bytes(&mut &data[..], &mut &other[..]).unwrap());
        *other.last_mut().unwrap() = 8;
        assert!(!same_bytes(&mut &data[..], &mut &other[..]).unwrap());
        assert!(!same_bytes(&mut &data[..], &mut &data[..CHUNK]).unwrap());
    }

    #[test]
    fn a_source_is_no_target() {
        let (folder, root) = project();
        let sources = [root.join("main.tex")];
        assert!(is_source(&root.join("kapitel/../main.tex"), &sources));
        assert!(!is_source(&root.join("main.pdf"), &sources));
        assert!(!is_source(&folder.path().join("gone/main.tex"), &sources));
    }

    #[test]
    fn secrets_are_noticed() {
        let (folder, root) = project();
        for name in [".env", ".env.local", "server.PEM", "my.key", "a.p12", "id_rsa", "id_rsa.pub", ".netrc", "credentials.json"] {
            fs::write(root.join(name), "x").unwrap();
        }
        fs::write(root.join("kapitel/.env"), "x").unwrap();
        fs::write(root.join("keys.tex"), "x").unwrap();
        let found = suspicious_files(&project_entries(&root, &folder.path().join("o.zip")).unwrap());
        assert_eq!(found.len(), 10, "{found:?}");
        assert!(found.contains(&"kapitel/.env".to_string()));
    }

    #[test]
    fn online_packages_are_noticed_in_typst_only() {
        let (folder, root) = project();
        let target = folder.path().join("o.zip");
        assert!(!uses_online_packages(&project_entries(&root, &target).unwrap()));
        fs::write(root.join("a.typ"), "#import \"@preview/cetz:0.3.0\"").unwrap();
        assert!(uses_online_packages(&project_entries(&root, &target).unwrap()));
    }

    #[test]
    fn dates_convert_to_the_calendar() {
        let date = archive_date(Some(UNIX_EPOCH + Duration::from_secs(1_772_368_496))).unwrap();
        assert_eq!((date.year(), date.month(), date.day()), (2026, 3, 1));
        assert_eq!((date.hour(), date.minute(), date.second()), (12, 34, 56));
        assert!(archive_date(Some(UNIX_EPOCH)).is_none());
    }

    fn current() -> PdfState<'static> {
        PdfState { building: false, unsaved: false, has_errors: false, last_good: false, newest_source: None, pdf_modified: None, warnings: &[] }
    }

    #[test]
    fn a_current_pdf_has_no_problem() {
        let now = SystemTime::now();
        let state = PdfState { newest_source: Some(now), pdf_modified: Some(now), ..current() };
        assert_eq!(pdf_problems(&state), Vec::<String>::new());
    }

    #[test]
    fn each_problem_is_named() {
        let now = SystemTime::now();
        let cases = [
            (PdfState { building: true, ..current() }, "A build is running"),
            (PdfState { unsaved: true, ..current() }, "Unsaved changes"),
            (PdfState { has_errors: true, ..current() }, "This PDF has errors"),
            (PdfState { last_good: true, ..current() }, "Last good PDF, not the current source"),
            (
                PdfState { newest_source: Some(now + Duration::from_secs(1)), pdf_modified: Some(now), ..current() },
                "Files changed since the build",
            ),
        ];
        for (state, expected) in cases {
            assert_eq!(pdf_problems(&state), [expected]);
        }
    }

    #[test]
    fn undefined_references_are_counted() {
        let warnings: Vec<String> = [
            "LaTeX Warning: Reference `a' on page 1 undefined",
            "LaTeX Warning: Reference `a' on page 1 undefined",
            "LaTeX Warning: Reference `b' on page 2 undefined",
            "LaTeX Warning: Citation `k' on page 1 undefined",
            "LaTeX Warning: There were undefined references.",
            "LaTeX Warning: Label(s) may have changed. Rerun to get cross-references right.",
        ]
        .map(String::from)
        .to_vec();
        let state = PdfState { warnings: &warnings, ..current() };
        assert_eq!(pdf_problems(&state), ["2 undefined references", "1 undefined citation", "References may be out of date"]);
        let only_summary = ["LaTeX Warning: There were undefined references.".to_string()];
        assert_eq!(pdf_problems(&PdfState { warnings: &only_summary, ..current() }), ["Undefined references"]);
    }

    #[test]
    fn a_new_source_counts_but_an_old_export_does_not() {
        let (_folder, root) = project();
        let before = newest_source(&root, "main.pdf").unwrap();
        std::thread::sleep(Duration::from_millis(20));
        for export in ["out.zip", "main.pdf", "kapitel/main.pdf", "main-draft.md", "main-draft.docx"] {
            fs::write(root.join(export), "x").unwrap();
        }
        assert_eq!(newest_source(&root, "main.pdf").unwrap(), before);
        fs::write(root.join("figure.pdf"), "x").unwrap();
        assert!(newest_source(&root, "main.pdf").unwrap() > before);
    }

    #[test]
    fn pandoc_reports_what_it_left_out() {
        let log = "[INFO] Loaded main.tex\n[INFO] Skipped '\\begin{tikzpicture}\\draw;\\end{tikzpicture}' at line 5 column 20\n[INFO] Skipped '\\unknowncmd{x}' at line 6 column 15\n[INFO] Skipped '\\unknowncmd{x}' at line 6 column 15\n[INFO] Ignored element parameters of place\n[WARNING] Could not fetch resource a.png\n  more detail\n";
        assert_eq!(
            parse_pandoc_report(log),
            [
                "'\\begin{tikzpicture}\\draw;\\end{tikzpicture}' at line 5 column 20",
                "'\\unknowncmd{x}' at line 6 column 15",
                "Ignored element parameters of place",
                "Could not fetch resource a.png",
            ]
        );
        assert_eq!(parse_pandoc_report("[INFO] Loaded a\n"), Vec::<String>::new());
    }

    #[test]
    fn a_report_keeps_the_place_of_a_long_passage() {
        let log = format!("[INFO] Skipped '{}' at ./main.tex line 21 column 11\n", "x".repeat(500));
        let report = parse_pandoc_report(&log);
        assert!(report[0].ends_with("… at main.tex line 21 column 11"), "{}", report[0]);
    }

    #[cfg(unix)]
    #[test]
    fn a_read_only_folder_leaves_the_target_as_it_was() {
        use std::os::unix::fs::PermissionsExt;
        let folder = tempdir().unwrap();
        let locked = folder.path().join("locked");
        fs::create_dir(&locked).unwrap();
        let (source, target) = (folder.path().join("a.pdf"), locked.join("b.pdf"));
        fs::write(&source, PDF).unwrap();
        fs::write(&target, "old").unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o555)).unwrap();
        let result = copy_pdf(&source, &target);
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(result.unwrap_err().contains("Cannot write to"));
        assert_eq!(fs::read_to_string(&target).unwrap(), "old");
    }

    #[test]
    fn a_long_report_line_is_cut_at_a_character() {
        let log = format!("[WARNING] {}", "ä".repeat(500));
        let report = parse_pandoc_report(&log);
        assert_eq!(report[0].chars().count(), REPORT_LINE_LIMIT + 1);
        assert!(report[0].ends_with('…'));
    }

    #[test]
    fn html_embeds_what_it_shows() {
        let args = pandoc_args("typst", DraftFormat::Html, Path::new("./m.typ"), Path::new("/o/t"));
        let args: Vec<&str> = args.iter().map(|arg| arg.to_str().unwrap()).collect();
        assert_eq!(args[..4], ["-f", "typst", "-t", "html"]);
        assert!(args.contains(&"--embed-resources") && args.contains(&"--mathml") && args.contains(&"--verbose"));
        assert_eq!(args[args.len() - 3..], ["-o", "/o/t", "./m.typ"]);
        let word = pandoc_args("latex", DraftFormat::Word, Path::new("/m.tex"), Path::new("/t"));
        assert!(!word.iter().any(|arg| arg == "--embed-resources"));
    }

    #[test]
    fn only_known_sources_have_a_reader() {
        assert_eq!(pandoc_reader(Path::new("a.tex")), Some("latex"));
        assert_eq!(pandoc_reader(Path::new("a.typ")), Some("typst"));
        assert_eq!(pandoc_reader(Path::new("a.md")), Some("markdown"));
        assert_eq!(pandoc_reader(Path::new("a.png")), None);
    }

    #[cfg(unix)]
    fn fake_pandoc(folder: &Path, script: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = folder.join("pandoc");
        fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[cfg(unix)]
    #[test]
    fn a_failing_pandoc_leaves_nothing() {
        let folder = tempdir().unwrap();
        let tools = tempdir().unwrap();
        let main = tools.path().join("m.tex");
        fs::write(&main, "x").unwrap();
        let target = folder.path().join("out.html");
        fs::write(&target, "old").unwrap();
        let pandoc = fake_pandoc(tools.path(), "echo '[INFO] noise' >&2; echo 'real problem' >&2; exit 3");
        let error = run_pandoc(&pandoc, &main, DraftFormat::Html, &target).err().unwrap();
        assert_eq!(error, "real problem");
        assert_eq!(fs::read_to_string(&target).unwrap(), "old");
        folder_is_only(folder.path(), &["out.html"]);
    }

    #[cfg(unix)]
    #[test]
    fn an_empty_result_is_a_failure_and_a_word_file_must_be_one() {
        let folder = tempdir().unwrap();
        let tools = tempdir().unwrap();
        let main = tools.path().join("m.tex");
        fs::write(&main, "x").unwrap();
        let pandoc = fake_pandoc(tools.path(), "exit 0");
        assert!(run_pandoc(&pandoc, &main, DraftFormat::Html, &folder.path().join("o.html")).is_err());
        let pandoc = fake_pandoc(tools.path(), "for last; do :; done; while [ \"$1\" != -o ]; do shift; done; echo not-a-zip > \"$2\"");
        assert!(run_pandoc(&pandoc, &main, DraftFormat::Word, &folder.path().join("o.docx")).is_err());
        folder_is_only(folder.path(), &[]);
    }

    #[cfg(unix)]
    #[test]
    fn a_draft_reaches_its_target_only_when_kept() {
        let folder = tempdir().unwrap();
        let tools = tempdir().unwrap();
        let main = tools.path().join("m.tex");
        fs::write(&main, "x").unwrap();
        let target = folder.path().join("o.md");
        let pandoc = fake_pandoc(tools.path(), "echo \"[INFO] Skipped 'x' at line 1\" >&2; while [ \"$1\" != -o ]; do shift; done; echo done > \"$2\"");
        let draft = run_pandoc(&pandoc, &main, DraftFormat::Markdown, &target).unwrap();
        assert_eq!(draft.report, ["'x' at line 1", "Images stay links to the project's files"]);
        assert!(!target.exists());
        drop(draft);
        folder_is_only(folder.path(), &[]);
        run_pandoc(&pandoc, &main, DraftFormat::Markdown, &target).unwrap().keep(&target).unwrap();
        assert_eq!(fs::read_to_string(&target).unwrap(), "done\n");
        folder_is_only(folder.path(), &["o.md"]);
    }

    /// Fetches Pandoc as the app does and turns the sample project, with its
    /// figure and bibliography, and a Markdown file into Word documents.
    #[test]
    #[ignore = "downloads Pandoc"]
    fn word_from_latex_and_markdown_with_real_pandoc() {
        use std::sync::atomic::{AtomicBool, AtomicU64};
        let tools = tempdir().unwrap();
        let pandoc = crate::toolchain::install(crate::toolchain::Tool::Pandoc, tools.path(), &AtomicU64::new(0), &AtomicBool::new(false)).unwrap();
        let sample = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/overleaf-sample");
        let project = tempdir().unwrap();
        for name in ["main.tex", "sample.bib", "frog.jpg"] {
            fs::copy(sample.join(name), project.path().join(name)).unwrap();
        }
        fs::write(project.path().join("notes.md"), "# Notizen über Ü\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n$$x^2$$\n").unwrap();
        for (main, expected) in [("main.tex", "Introduction"), ("notes.md", "Notizen über Ü")] {
            let target = project.path().join(Path::new(main).with_extension("docx"));
            let draft = run_pandoc(&pandoc, &project.path().join(main), DraftFormat::Word, &target).unwrap();
            println!("{main}: {:?}", draft.report);
            draft.keep(&target).unwrap();
            let mut archive = ZipArchive::new(File::open(&target).unwrap()).unwrap();
            let mut document = String::new();
            archive.by_name("word/document.xml").unwrap().read_to_string(&mut document).unwrap();
            assert!(document.contains(expected), "{main} lost its text");
        }
    }
}
