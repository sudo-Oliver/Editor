//! Installed add-ons and the programs behind them. An add-on is a folder with
//! `addon.toml`. See `addon_api` for how it talks to the editor.

use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    path::{Component, Path, PathBuf},
    process::Stdio,
    sync::mpsc as std_mpsc,
    thread,
    time::{Duration, Instant},
};

use addon_api::{API_VERSION, AddonMessage, HostMessage, Manifest, OLDEST_API_VERSION};
use tokio::sync::mpsc;

const MANIFEST: &str = "addon.toml";
/// Marks an add-on as turned on. New ones start turned off.
pub const ENABLED_MARKER: &str = ".enabled";
/// How long a program gets to exit by itself after its stdin closes.
const EXIT_GRACE: Duration = Duration::from_millis(500);
/// How long an add-on gets when the whole app ends.
const QUIT_GRACE: Duration = Duration::from_millis(300);
/// Larger than any useful message, but a broken add-on cannot fill the memory.
const MAX_MESSAGE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_ERROR_LINE_BYTES: u64 = 16 * 1024;
const EVENT_CAPACITY: usize = 256;
const MAX_READ_BYTES: u64 = 8 << 20;
/// As much as a download from the marketplace may be.
const MAX_INSTALL_BYTES: u64 = 200 << 20;
/// Development leftovers that an add-on folder may hold and never needs.
const NOT_INSTALLED: [&str; 3] = [".git", "target", "node_modules"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Installed {
    pub manifest: Manifest,
    pub directory: PathBuf,
    pub enabled: bool,
}

impl Installed {
    pub fn icon(&self) -> Option<PathBuf> {
        self.manifest.icon.as_deref().map(|icon| self.directory.join(icon))
    }

    pub fn program(&self) -> PathBuf {
        self.directory.join(&self.manifest.program)
    }
}

/// A folder for files the editor hands to add-ons, such as pasted pictures and
/// PDF pages. Only this user can open it, and it goes when the editor quits.
pub fn private_directory() -> Result<PathBuf, String> {
    static DIRECTORY: std::sync::OnceLock<Result<PathBuf, String>> = std::sync::OnceLock::new();
    DIRECTORY
        .get_or_init(|| {
            let path = std::env::temp_dir().join(format!("editor-addons-{}", std::process::id()));
            #[cfg_attr(not(unix), allow(unused_mut))]
            let mut builder = std::fs::DirBuilder::new();
            // Windows keeps the temporary folder to its user already.
            #[cfg(unix)]
            std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
            match builder.create(&path) {
                Ok(()) => Ok(path),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(path),
                Err(error) => Err(format!("Cannot make a folder for add-on files: {error}")),
            }
        })
        .clone()
}

/// Removes that folder, at quit.
pub fn remove_private_directory() {
    std::fs::remove_dir_all(std::env::temp_dir().join(format!("editor-addons-{}", std::process::id()))).ok();
}

/// A new name in the private folder, such as `pasted-3.png`.
pub fn private_file(stem: &str, extension: &str) -> Result<PathBuf, String> {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let number = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    Ok(private_directory()?.join(format!("{stem}-{number}.{extension}")))
}

pub fn default_directory() -> Option<PathBuf> {
    Some(crate::settings::default_path()?.parent()?.join("addons"))
}

pub fn parse_manifest(text: &str) -> Result<Manifest, String> {
    let manifest: Manifest = toml::from_str(text).map_err(|error| format!("{MANIFEST}: {error}"))?;
    if manifest.id.is_empty() || !manifest.id.chars().all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-') {
        return Err(format!("\"{}\" is not a valid add-on id. Use a-z, 0-9 and -.", manifest.id));
    }
    if !(OLDEST_API_VERSION..=API_VERSION).contains(&manifest.api) {
        return Err(format!("{} needs add-on API {}, and this editor has {OLDEST_API_VERSION} to {API_VERSION}.", manifest.name, manifest.api));
    }
    let valid_id = |id: &str| !id.is_empty() && id.chars().all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-' || ch == '_');
    if let Some(command) = manifest.commands.iter().find(|command| !valid_id(&command.id)) {
        return Err(format!("{}: \"{}\" is not a valid command id. Use a-z, 0-9, - and _.", manifest.name, command.id));
    }
    if !stays_inside(Path::new(&manifest.program)) || manifest.icon.as_deref().is_some_and(|icon| !stays_inside(Path::new(icon))) {
        return Err(format!("{}: files must stay inside the add-on folder.", manifest.name));
    }
    Ok(manifest)
}

fn stays_inside(path: &Path) -> bool {
    path.components().next().is_some() && path.components().all(|part| matches!(part, Component::Normal(_) | Component::CurDir))
}

/// The add-ons in `directory`, by name. A broken one gives an error instead.
pub fn load_all(directory: &Path) -> (Vec<Installed>, Vec<String>) {
    let (mut installed, mut errors) = (Vec::new(), Vec::new());
    let Ok(entries) = fs::read_dir(directory) else {
        return (installed, errors);
    };
    for entry in entries.flatten() {
        let folder = entry.path();
        let Ok(text) = fs::read_to_string(folder.join(MANIFEST)) else {
            continue;
        };
        match parse_manifest(&text) {
            Ok(manifest) => {
                let enabled = folder.join(ENABLED_MARKER).exists();
                installed.push(Installed { manifest, directory: folder, enabled });
            }
            Err(error) => errors.push(error),
        }
    }
    installed.sort_by(|left, right| left.manifest.name.cmp(&right.manifest.name));
    (installed, errors)
}

pub fn read_manifest(folder: &Path) -> Result<Manifest, String> {
    let text = fs::read_to_string(folder.join(MANIFEST)).map_err(|error| format!("No {MANIFEST} in this folder: {error}"))?;
    parse_manifest(&text)
}

/// Copies an add-on folder into `directory`, in place of an older version.
/// It stays turned off until the user turns it on.
pub fn install_from(source: &Path, directory: &Path) -> Result<Manifest, String> {
    let manifest = read_manifest(source)?;
    let target = directory.join(&manifest.id);
    if source.canonicalize().map(crate::platform::plain_path).ok() == target.canonicalize().map(crate::platform::plain_path).ok() {
        return Err(format!("{} is already installed from this folder.", manifest.name));
    }
    let describe = |error: std::io::Error| format!("Cannot install {}: {error}", manifest.name);
    let bytes = installed_size(source).map_err(describe)?;
    if bytes > MAX_INSTALL_BYTES {
        let (size, limit) = (crate::storage::human(bytes), crate::storage::human(MAX_INSTALL_BYTES));
        return Err(format!("{} holds {size}, more than the {limit} an add-on may take.", manifest.name));
    }
    fs::create_dir_all(directory).map_err(describe)?;
    let staging = directory.join(format!(".installing-{}", manifest.id));
    if staging.exists() {
        fs::remove_dir_all(&staging).map_err(describe)?;
    }
    if let Err(error) = copy_directory(source, &staging) {
        fs::remove_dir_all(&staging).ok();
        return Err(describe(error));
    }
    if target.exists() {
        fs::remove_dir_all(&target).map_err(describe)?;
    }
    fs::rename(&staging, &target).map_err(describe)?;
    Ok(manifest)
}

/// The bytes `copy_directory` would copy.
fn installed_size(source: &Path) -> std::io::Result<u64> {
    let mut bytes = 0;
    for entry in fs::read_dir(source)?.flatten() {
        let kind = entry.file_type()?;
        if kind.is_dir() && !NOT_INSTALLED.iter().any(|name| entry.file_name() == *name) {
            bytes += installed_size(&entry.path())?;
        } else if kind.is_file() {
            bytes += entry.metadata()?.len();
        }
    }
    Ok(bytes)
}

/// Links stay behind, so that an add-on holds no reference to files outside.
fn copy_directory(source: &Path, target: &Path) -> std::io::Result<()> {
    fs::create_dir_all(target)?;
    for entry in fs::read_dir(source)?.flatten() {
        let kind = entry.file_type()?;
        let to = target.join(entry.file_name());
        if kind.is_dir() && NOT_INSTALLED.iter().any(|name| entry.file_name() == *name) {
            continue;
        }
        if kind.is_dir() {
            copy_directory(&entry.path(), &to)?;
        } else if kind.is_file() && entry.file_name() != ENABLED_MARKER {
            fs::copy(entry.path(), to)?;
        }
    }
    Ok(())
}

pub fn set_enabled(add_on: &Installed, enabled: bool) -> std::io::Result<()> {
    let marker = add_on.directory.join(ENABLED_MARKER);
    if enabled { fs::write(marker, "") } else { fs::remove_file(marker) }
}

pub fn remove(add_on: &Installed) -> std::io::Result<()> {
    fs::remove_dir_all(&add_on.directory)
}

/// A path from an add-on, as a file inside the project. Links that lead out and
/// the insides of `.git` are refused. The file need not exist.
pub fn resolve_path(root: &Path, relative: &str) -> Result<PathBuf, String> {
    let path = Path::new(relative);
    let refused = || format!("{relative} is outside the project.");
    if !stays_inside(path) {
        return Err(refused());
    }
    if path.components().any(|part| part.as_os_str() == ".git") {
        return Err(format!("{relative} belongs to Git."));
    }
    let root = root.canonicalize().map(crate::platform::plain_path).map_err(|error| format!("Cannot read the project: {error}"))?;
    let full = root.join(path);
    // The deepest part that exists shows where a link really leads.
    let existing = full.ancestors().find(|ancestor| ancestor.symlink_metadata().is_ok()).ok_or_else(refused)?;
    let real = existing.canonicalize().map(crate::platform::plain_path).map_err(|_| refused())?;
    if real.starts_with(&root) { Ok(full) } else { Err(refused()) }
}

pub fn read_file_limited(path: &Path) -> Result<String, String> {
    let size = fs::metadata(path).map_err(|error| format!("Cannot read {}: {error}", path.display()))?.len();
    if size > MAX_READ_BYTES {
        return Err(format!("{} is too large to read.", path.display()));
    }
    fs::read_to_string(path).map_err(|error| format!("Cannot read {}: {error}", path.display()))
}

/// The byte range of `old`, which must occur exactly once.
pub fn find_unique(text: &str, old: &str) -> Result<std::ops::Range<usize>, String> {
    if old.is_empty() {
        return Err("The text to replace is empty.".to_string());
    }
    let mut hits = text.match_indices(old);
    let Some((start, _)) = hits.next() else {
        return Err("The text to replace was not found.".to_string());
    };
    let more = hits.count();
    if more > 0 {
        return Err(format!("The text to replace occurs {} times. Include more around it.", more + 1));
    }
    Ok(start..start + old.len())
}

pub enum ProcessEvent {
    Message(AddonMessage),
    /// A line that is not a valid message.
    Invalid(String),
    Exited(String),
}

/// A running add-on. Dropping it ends the program.
pub struct AddonProcess {
    sender: Option<std_mpsc::Sender<HostMessage>>,
    /// The add-on and the programs it started, which end with it.
    group: Option<std::sync::Arc<build::Group>>,
}

impl AddonProcess {
    pub fn start(add_on: &Installed) -> Result<(Self, mpsc::Receiver<ProcessEvent>), String> {
        let program = add_on.program();
        let mut command = crate::platform::command(&program);
        command.current_dir(&add_on.directory).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        // The add-on can start programs of its own, which should end with it.
        let (mut child, group) = build::spawn_group(&mut command).map_err(|error| format!("Cannot start {}: {error}", program.display()))?;
        let group = group.map(std::sync::Arc::new);
        let (Some(mut stdin), Some(stdout), Some(stderr)) = (child.stdin.take(), child.stdout.take(), child.stderr.take()) else {
            return Err("The add-on has no pipes.".to_string());
        };

        let (sender, outgoing) = std_mpsc::channel::<HostMessage>();
        thread::spawn(move || {
            for message in outgoing {
                let Ok(line) = serde_json::to_string(&message) else {
                    continue;
                };
                if writeln!(stdin, "{line}").and_then(|()| stdin.flush()).is_err() {
                    break;
                }
            }
        });

        // A full channel makes the add-on wait when it talks faster than the panel reads.
        let (events, receiver) = mpsc::channel(EVENT_CAPACITY);
        let error_tail = thread::spawn(move || {
            let mut reader = BufReader::new(stderr);
            let mut last = String::new();
            let mut bytes = Vec::new();
            while let Ok(Some(_)) = read_line(&mut reader, &mut bytes, MAX_ERROR_LINE_BYTES) {
                last = String::from_utf8_lossy(&bytes).trim_end().to_string();
            }
            last
        });
        thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            let mut bytes = Vec::new();
            while let Ok(Some(complete)) = read_line(&mut reader, &mut bytes, MAX_MESSAGE_BYTES) {
                let event = if complete {
                    match serde_json::from_slice(&bytes) {
                        Ok(message) => ProcessEvent::Message(message),
                        Err(error) => ProcessEvent::Invalid(error.to_string()),
                    }
                } else {
                    ProcessEvent::Invalid(format!("A message is larger than {} MB.", MAX_MESSAGE_BYTES / 1024 / 1024))
                };
                if events.blocking_send(event).is_err() {
                    break;
                }
            }
            let status = child.wait();
            let last = error_tail.join().unwrap_or_default();
            let code = status.map_or_else(|error| error.to_string(), |status| status.to_string());
            let reason = if last.is_empty() { code } else { format!("{code}: {last}") };
            events.blocking_send(ProcessEvent::Exited(reason)).ok();
        });
        Ok((Self { sender: Some(sender), group }, receiver))
    }

    pub fn send(&self, message: HostMessage) {
        if let Some(sender) = &self.sender {
            sender.send(message).ok();
        }
    }

    /// Ends the add-on before the app exits, since no thread outlives the app to do it.
    pub fn shut_down(&mut self) {
        self.sender = None;
        let Some(group) = &self.group else {
            return;
        };
        // On Windows terminating is killing, so the add-on first gets the time
        // that closing its input gives it to save and exit.
        if !cfg!(windows) {
            group.terminate();
        }
        let started = Instant::now();
        while started.elapsed() < QUIT_GRACE && group.alive() {
            thread::sleep(Duration::from_millis(10));
        }
        group.kill();
    }
}

/// Reads one line, with its end, into `bytes`. `None` means the stream ended, and
/// `Some(false)` means the line was too long, so what it held is gone.
fn read_line(reader: &mut impl BufRead, bytes: &mut Vec<u8>, limit: u64) -> std::io::Result<Option<bool>> {
    bytes.clear();
    if reader.by_ref().take(limit).read_until(b'\n', bytes)? == 0 {
        return Ok(None);
    }
    if bytes.len() as u64 == limit && bytes.last() != Some(&b'\n') {
        bytes.clear();
        reader.skip_until(b'\n')?;
        return Ok(Some(false));
    }
    Ok(Some(true))
}

impl Drop for AddonProcess {
    fn drop(&mut self) {
        // Closing stdin asks it to exit. What is left after that gets a signal.
        self.sender = None;
        let Some(group) = self.group.take() else {
            return;
        };
        thread::spawn(move || {
            thread::sleep(EXIT_GRACE);
            group.terminate();
            thread::sleep(EXIT_GRACE);
            group.kill();
        });
    }
}

const KEYCHAIN_SERVICE: &str = "editor-addon";

/// Reads a secret of an add-on from the system keychain. Blocks.
pub fn get_secret(add_on: &str, key: &str) -> Result<Option<String>, String> {
    crate::secrets::get(KEYCHAIN_SERVICE, &format!("{add_on}/{key}"))
}

/// Stores a secret of an add-on. Blocks.
pub fn set_secret(add_on: &str, key: &str, value: &str) -> Result<(), String> {
    crate::secrets::set(KEYCHAIN_SERVICE, "Editor add-on", &format!("{add_on}/{key}"), value)
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::{PermissionsExt, symlink};

    use tempfile::tempdir;

    use super::*;

    fn manifest_text(id: &str, program: &str) -> String {
        format!("id = \"{id}\"\nname = \"Test\"\nversion = \"1\"\nauthor = \"Ada\"\napi = {API_VERSION}\nprogram = \"{program}\"\npermissions = [\"read_project\"]\n")
    }

    #[test]
    fn manifest_rejects_bad_values() {
        assert!(parse_manifest(&manifest_text("a-1", "bin/run")).is_ok());
        assert!(parse_manifest(&manifest_text("../up", "run")).is_err());
        assert!(parse_manifest(&manifest_text("a", "../run")).is_err());
        assert!(parse_manifest(&manifest_text("a", "/bin/sh")).is_err());
        assert!(parse_manifest(&manifest_text("a", "run").replace(&format!("api = {API_VERSION}"), "api = 999")).is_err());
        assert!(parse_manifest(&format!("{}surprise = 1\n", manifest_text("a", "run"))).is_err());
    }

    #[test]
    fn install_starts_turned_off_and_replaces_old_versions() {
        let directory = tempdir().unwrap();
        let (source, addons) = (directory.path().join("source"), directory.path().join("addons"));
        fs::create_dir_all(source.join("bin")).unwrap();
        fs::write(source.join(MANIFEST), manifest_text("a", "bin/run")).unwrap();
        fs::write(source.join(ENABLED_MARKER), "").unwrap();
        fs::write(source.join("bin/run"), "1").unwrap();
        symlink("/etc/hosts", source.join("link")).unwrap();

        install_from(&source, &addons).unwrap();
        let (installed, errors) = load_all(&addons);
        assert!(errors.is_empty());
        assert!(!installed[0].enabled);
        assert!(!addons.join("a/link").exists());

        set_enabled(&installed[0], true).unwrap();
        assert!(load_all(&addons).0[0].enabled);

        fs::write(source.join("bin/run"), "2").unwrap();
        install_from(&source, &addons).unwrap();
        assert_eq!(fs::read_to_string(addons.join("a/bin/run")).unwrap(), "2");

        remove(&load_all(&addons).0[0]).unwrap();
        assert!(load_all(&addons).0.is_empty());
    }

    #[test]
    fn install_leaves_out_development_folders_and_refuses_large_ones() {
        let directory = tempdir().unwrap();
        let (source, addons) = (directory.path().join("source"), directory.path().join("addons"));
        for dir in [".git/objects", "target/debug", "node_modules/x", "bin"] {
            fs::create_dir_all(source.join(dir)).unwrap();
        }
        fs::write(source.join(MANIFEST), manifest_text("a", "bin/run")).unwrap();
        fs::write(source.join("bin/run"), "1").unwrap();
        // Sparse, so the test writes nothing. Left out, so it does not count.
        fs::File::create(source.join("target/debug/big")).unwrap().set_len(MAX_INSTALL_BYTES + 1).unwrap();
        install_from(&source, &addons).unwrap();
        assert!(addons.join("a/bin/run").is_file());
        for left_out in [".git", "target", "node_modules"] {
            assert!(!addons.join("a").join(left_out).exists(), "{left_out}");
        }

        fs::File::create(source.join("bin/big")).unwrap().set_len(MAX_INSTALL_BYTES + 1).unwrap();
        let error = install_from(&source, &addons).unwrap_err();
        assert!(error.contains("more than the 200.0 MB"), "{error}");
        assert_eq!(fs::read_to_string(addons.join("a/bin/run")).unwrap(), "1");
    }

    #[test]
    fn a_failed_install_leaves_no_half_copy() {
        let directory = tempdir().unwrap();
        let (source, addons) = (directory.path().join("source"), directory.path().join("addons"));
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join(MANIFEST), manifest_text("a", "run")).unwrap();
        fs::write(source.join("run"), "1").unwrap();
        fs::set_permissions(source.join("run"), fs::Permissions::from_mode(0o000)).unwrap();
        assert!(install_from(&source, &addons).is_err());
        assert_eq!(fs::read_dir(&addons).unwrap().count(), 0);
    }

    #[test]
    fn broken_manifest_gives_an_error() {
        let directory = tempdir().unwrap();
        fs::create_dir_all(directory.path().join("x")).unwrap();
        fs::write(directory.path().join("x").join(MANIFEST), "nonsense").unwrap();
        let (installed, errors) = load_all(directory.path());
        assert!(installed.is_empty());
        assert_eq!(errors.len(), 1);
    }

    #[test]
    fn paths_stay_inside_the_project() {
        let directory = tempdir().unwrap();
        let root = directory.path().join("project");
        fs::create_dir_all(root.join("sub")).unwrap();
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::write(root.join("sub/a.tex"), "").unwrap();
        fs::write(directory.path().join("secret"), "").unwrap();
        symlink(directory.path(), root.join("out")).unwrap();
        symlink(directory.path().join("secret"), root.join("linked")).unwrap();

        assert!(resolve_path(&root, "sub/a.tex").is_ok());
        assert!(resolve_path(&root, "sub/new/deep.tex").is_ok());
        assert!(resolve_path(&root, "../secret").is_err());
        assert!(resolve_path(&root, "sub/../../secret").is_err());
        assert!(resolve_path(&root, "/etc/hosts").is_err());
        assert!(resolve_path(&root, "out/secret").is_err());
        assert!(resolve_path(&root, "out/new.tex").is_err());
        assert!(resolve_path(&root, "linked").is_err());
        assert!(resolve_path(&root, ".git/config").is_err());
        assert!(resolve_path(&root, "").is_err());
    }

    #[test]
    fn edits_need_one_exact_match() {
        assert_eq!(find_unique("a bb c", "bb"), Ok(2..4));
        assert!(find_unique("a a", "a").unwrap_err().contains("2 times"));
        assert!(find_unique("a", "x").is_err());
        assert!(find_unique("a", "").is_err());
    }

    fn script_add_on(directory: &Path, script: &str) -> Installed {
        let program = directory.join("run");
        fs::write(&program, format!("#!/bin/sh\n{script}\n")).unwrap();
        fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).unwrap();
        let manifest = parse_manifest(&manifest_text("a", "run")).unwrap();
        Installed { manifest, directory: directory.to_path_buf(), enabled: true }
    }

    #[test]
    fn long_lines_and_broken_text_do_not_stop_reading() {
        let mut input = std::io::Cursor::new([b"short\n".as_slice(), &[b'x'; 40], b"\n\xFF\xFE\nend".as_slice()].concat());
        let mut bytes = Vec::new();
        assert_eq!(read_line(&mut input, &mut bytes, 16).unwrap(), Some(true));
        assert_eq!(bytes, b"short\n");
        assert_eq!(read_line(&mut input, &mut bytes, 16).unwrap(), Some(false));
        assert_eq!(read_line(&mut input, &mut bytes, 16).unwrap(), Some(true));
        assert_eq!(bytes, b"\xFF\xFE\n");
        assert_eq!(read_line(&mut input, &mut bytes, 16).unwrap(), Some(true));
        assert_eq!(bytes, b"end");
        assert_eq!(read_line(&mut input, &mut bytes, 16).unwrap(), None);
    }

    fn next(receiver: &mut mpsc::Receiver<ProcessEvent>) -> ProcessEvent {
        tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap()
            .block_on(async { tokio::time::timeout(Duration::from_secs(5), receiver.recv()).await })
            .expect("the add-on answers in time")
            .expect("the channel stays open")
    }

    #[test]
    fn process_exchanges_lines_and_reports_invalid_ones() {
        let directory = tempdir().unwrap();
        // Echoes the id of the first line back, then writes something broken.
        let add_on = script_add_on(
            directory.path(),
            r#"read line
case "$line" in *init*) echo '{"message":"append","id":"a","text":"hello"}';; esac
echo 'not json'
echo 'last words' >&2"#,
        );
        let (process, mut receiver) = AddonProcess::start(&add_on).unwrap();
        process.send(HostMessage::Init { api: API_VERSION, project: None, host_version: String::new(), features: Vec::new() });
        assert!(matches!(next(&mut receiver), ProcessEvent::Message(AddonMessage::Append { text, .. }) if text == "hello"));
        assert!(matches!(next(&mut receiver), ProcessEvent::Invalid(_)));
        assert!(matches!(next(&mut receiver), ProcessEvent::Exited(reason) if reason.contains("last words")));
    }

    #[test]
    fn missing_program_is_an_error() {
        let directory = tempdir().unwrap();
        let add_on = Installed {
            manifest: parse_manifest(&manifest_text("a", "nothing")).unwrap(),
            directory: directory.path().to_path_buf(),
            enabled: true,
        };
        assert!(AddonProcess::start(&add_on).is_err());
    }

    #[test]
    fn dropping_ends_a_program_that_ignores_stdin() {
        let directory = tempdir().unwrap();
        let add_on = script_add_on(directory.path(), "echo '{\"message\":\"append\",\"id\":\"a\",\"text\":\"up\"}'\nexec sleep 60");
        let (process, mut receiver) = AddonProcess::start(&add_on).unwrap();
        assert!(matches!(next(&mut receiver), ProcessEvent::Message(_)));
        drop(process);
        assert!(matches!(next(&mut receiver), ProcessEvent::Exited(_)));
    }
}
