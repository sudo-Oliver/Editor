//! Installed add-ons and the programs behind them. An add-on is a folder with
//! `addon.toml`. See `addon_api` for how it talks to the editor.

use std::{
    fs,
    io::{BufRead, BufReader, Write},
    os::unix::process::CommandExt,
    path::{Component, Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc as std_mpsc,
    thread,
    time::Duration,
};

use addon_api::{API_VERSION, AddonMessage, HostMessage, Manifest};
use tokio::sync::mpsc;

const MANIFEST: &str = "addon.toml";
/// Marks an add-on as turned on. New ones start turned off.
pub const ENABLED_MARKER: &str = ".enabled";
/// How long a program gets to exit by itself after its stdin closes.
const EXIT_GRACE: Duration = Duration::from_millis(500);
const MAX_READ_BYTES: u64 = 8 << 20;

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

pub fn default_directory() -> Option<PathBuf> {
    Some(crate::settings::default_path()?.parent()?.join("addons"))
}

pub fn parse_manifest(text: &str) -> Result<Manifest, String> {
    let manifest: Manifest = toml::from_str(text).map_err(|error| format!("{MANIFEST}: {error}"))?;
    if manifest.id.is_empty() || !manifest.id.chars().all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-') {
        return Err(format!("\"{}\" is not a valid add-on id. Use a-z, 0-9 and -.", manifest.id));
    }
    if manifest.api != API_VERSION {
        return Err(format!("{} needs add-on API {}, and this editor has {API_VERSION}.", manifest.name, manifest.api));
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
    if source.canonicalize().ok() == target.canonicalize().ok() {
        return Err(format!("{} is already installed from this folder.", manifest.name));
    }
    let describe = |error: std::io::Error| format!("Cannot install {}: {error}", manifest.name);
    fs::create_dir_all(directory).map_err(describe)?;
    let staging = directory.join(format!(".installing-{}", manifest.id));
    if staging.exists() {
        fs::remove_dir_all(&staging).map_err(describe)?;
    }
    copy_directory(source, &staging).map_err(describe)?;
    if target.exists() {
        fs::remove_dir_all(&target).map_err(describe)?;
    }
    fs::rename(&staging, &target).map_err(describe)?;
    Ok(manifest)
}

/// Links stay behind, so that an add-on holds no reference to files outside.
fn copy_directory(source: &Path, target: &Path) -> std::io::Result<()> {
    fs::create_dir_all(target)?;
    for entry in fs::read_dir(source)?.flatten() {
        let kind = entry.file_type()?;
        let to = target.join(entry.file_name());
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
    let root = root.canonicalize().map_err(|error| format!("Cannot read the project: {error}"))?;
    let full = root.join(path);
    // The deepest part that exists shows where a link really leads.
    let existing = full.ancestors().find(|ancestor| ancestor.symlink_metadata().is_ok()).ok_or_else(refused)?;
    let real = existing.canonicalize().map_err(|_| refused())?;
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
    process_group: u32,
}

impl AddonProcess {
    pub fn start(add_on: &Installed) -> Result<(Self, mpsc::UnboundedReceiver<ProcessEvent>), String> {
        let program = add_on.program();
        let mut child = Command::new(&program)
            .current_dir(&add_on.directory)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // The add-on can start programs of its own, which should end with it.
            .process_group(0)
            .spawn()
            .map_err(|error| format!("Cannot start {}: {error}", program.display()))?;
        let process_group = child.id();
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

        let (events, receiver) = mpsc::unbounded_channel();
        let error_tail = thread::spawn(move || {
            let mut last = String::new();
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                last = line;
            }
            last
        });
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                let event = match serde_json::from_str(&line) {
                    Ok(message) => ProcessEvent::Message(message),
                    Err(error) => ProcessEvent::Invalid(error.to_string()),
                };
                if events.send(event).is_err() {
                    break;
                }
            }
            let status = child.wait();
            let last = error_tail.join().unwrap_or_default();
            let code = status.map_or_else(|error| error.to_string(), |status| status.to_string());
            let reason = if last.is_empty() { code } else { format!("{code}: {last}") };
            events.send(ProcessEvent::Exited(reason)).ok();
        });
        Ok((Self { sender: Some(sender), process_group }, receiver))
    }

    pub fn send(&self, message: HostMessage) {
        if let Some(sender) = &self.sender {
            sender.send(message).ok();
        }
    }
}

impl Drop for AddonProcess {
    fn drop(&mut self) {
        // Closing stdin asks it to exit. What is left after that gets a signal.
        self.sender = None;
        let group = self.process_group as libc::pid_t;
        thread::spawn(move || {
            thread::sleep(EXIT_GRACE);
            // SAFETY: `killpg` takes plain values. The group holds only this add-on.
            unsafe { libc::killpg(group, libc::SIGTERM) };
        });
    }
}

const KEYCHAIN_SERVICE: &str = "editor-addon";

fn keychain_account(add_on: &str, key: &str) -> String {
    format!("{add_on}/{key}")
}

/// Reads a secret of an add-on from the system keychain. Blocks.
pub fn get_secret(add_on: &str, key: &str) -> Result<Option<String>, String> {
    let account = keychain_account(add_on, key);
    let output = if cfg!(target_os = "macos") {
        Command::new("security").args(["find-generic-password", "-s", KEYCHAIN_SERVICE, "-a", &account, "-w"]).output()
    } else {
        Command::new("secret-tool").args(["lookup", "service", KEYCHAIN_SERVICE, "account", &account]).output()
    }
    .map_err(|error| format!("The system keychain is unavailable: {error}"))?;
    let value = String::from_utf8_lossy(&output.stdout).trim_end_matches('\n').to_string();
    Ok((output.status.success() && !value.is_empty()).then_some(value))
}

/// Stores a secret. The value goes through stdin, because arguments show in `ps`. Blocks.
pub fn set_secret(add_on: &str, key: &str, value: &str) -> Result<(), String> {
    let account = keychain_account(add_on, key);
    let (mut command, input) = if cfg!(target_os = "macos") {
        let mut command = Command::new("security");
        command.arg("-i");
        let line = format!(
            "add-generic-password -U -s {} -a {} -w {}\n",
            keychain_quote(KEYCHAIN_SERVICE),
            keychain_quote(&account),
            keychain_quote(value),
        );
        (command, line)
    } else {
        let mut command = Command::new("secret-tool");
        command.args(["store", "--label", "Editor add-on", "service", KEYCHAIN_SERVICE, "account", &account]);
        (command, value.to_string())
    };
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("The system keychain is unavailable: {error}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(input.as_bytes()).map_err(|error| format!("Cannot store the secret: {error}"))?;
    }
    let output = child.wait_with_output().map_err(|error| format!("Cannot store the secret: {error}"))?;
    let failure = String::from_utf8_lossy(&output.stderr);
    if output.status.success() && failure.trim().is_empty() {
        Ok(())
    } else {
        // Not the input, which holds the secret.
        Err("The system keychain refused the secret.".to_string())
    }
}

/// Quotes one word for `security -i`.
fn keychain_quote(word: &str) -> String {
    format!("\"{}\"", word.replace('\\', "\\\\").replace('"', "\\\""))
}

#[cfg(test)]
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

    #[test]
    fn keychain_words_are_quoted() {
        assert_eq!(keychain_quote(r#"a"b\c d"#), r#""a\"b\\c d""#);
    }

    fn script_add_on(directory: &Path, script: &str) -> Installed {
        let program = directory.join("run");
        fs::write(&program, format!("#!/bin/sh\n{script}\n")).unwrap();
        fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).unwrap();
        let manifest = parse_manifest(&manifest_text("a", "run")).unwrap();
        Installed { manifest, directory: directory.to_path_buf(), enabled: true }
    }

    fn next(receiver: &mut mpsc::UnboundedReceiver<ProcessEvent>) -> ProcessEvent {
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
        process.send(HostMessage::Init { api: API_VERSION, project: None });
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
