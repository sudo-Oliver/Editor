//! Updates from the GitHub releases. Now and then the app asks GitHub for the
//! newest release and, when it is newer, shows a bar with one button. That
//! button downloads the dmg, checks it, swaps the app in place and starts it
//! again. Unsaved text is saved first, as on every quit.
//!
//! An app installed with the Homebrew cask is updated through Homebrew
//! instead, so that Homebrew keeps knowing which version is there.
//!
//! Only a real app bundle updates itself: a build run with cargo never asks.

use std::{
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};

use gpui::{AnyElement, App, Context, Entity, FontWeight, SharedString, Task, div, prelude::*, px};
use serde::Deserialize;

use crate::{app_location, marketplace, theme, toolchain};

const LATEST: &str = "https://api.github.com/repos/sudo-Oliver/Editor/releases/latest";
const BUNDLE_ID: &str = "io.github.sudo-oliver.editor";
const CASK: &str = "sudo-oliver/tap/editor";
/// GitHub allows 60 questions an hour without an account; this asks four times a day.
const CHECK_EVERY: Duration = Duration::from_secs(6 * 60 * 60);
/// The first look waits until the app has settled.
const FIRST_CHECK: Duration = Duration::from_secs(10);
const DOWNLOAD_SECONDS: &str = "1800";
const POLL: Duration = Duration::from_millis(250);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub version: String,
    /// The release page, with what changed.
    pub page: String,
    dmg_url: String,
    bytes: u64,
    sha256: String,
}

#[derive(Deserialize)]
struct ApiRelease {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<ApiAsset>,
}

#[derive(Deserialize)]
struct ApiAsset {
    name: String,
    browser_download_url: String,
    #[serde(default)]
    size: u64,
    /// "sha256:…", which GitHub computes itself for every upload.
    #[serde(default)]
    digest: Option<String>,
}

/// The release in GitHub's answer, if it has a dmg with a checksum. A dmg
/// without one is never installed, since nothing could tell it was intact.
fn parse_release(json: &str) -> Result<Option<Release>, String> {
    let release: ApiRelease = serde_json::from_str(json).map_err(|error| format!("GitHub answered in an unknown way: {error}"))?;
    if release.draft || release.prerelease {
        return Ok(None);
    }
    let Some(dmg) = release.assets.iter().find(|asset| asset.name.ends_with(".dmg")) else {
        return Ok(None);
    };
    let Some(sha256) = dmg.digest.as_deref().and_then(|digest| digest.strip_prefix("sha256:")) else {
        return Ok(None);
    };
    Ok(Some(Release {
        version: release.tag_name.trim_start_matches('v').to_string(),
        page: release.html_url,
        dmg_url: dmg.browser_download_url.clone(),
        bytes: dmg.size,
        sha256: sha256.to_ascii_lowercase(),
    }))
}

/// Whether `candidate` is a later version than `current`, part by part, so
/// that 0.10.0 comes after 0.9.0.
pub fn newer(candidate: &str, current: &str) -> bool {
    let parts = |version: &str| -> Vec<u64> {
        version.split('.').map(|part| part.chars().take_while(char::is_ascii_digit).collect::<String>().parse().unwrap_or(0)).collect()
    };
    let (candidate, current) = (parts(candidate), parts(current));
    for index in 0..candidate.len().max(current.len()) {
        let (left, right) = (candidate.get(index).copied().unwrap_or(0), current.get(index).copied().unwrap_or(0));
        if left != right {
            return left > right;
        }
    }
    false
}

/// Asks GitHub for the newest release, and returns it if it is newer than this app. Blocks.
pub fn check() -> Result<Option<Release>, String> {
    let output = marketplace::curl("20")
        .args(["--header", "Accept: application/vnd.github+json"])
        .arg(LATEST)
        .output()
        .map_err(|error| format!("Cannot run curl: {error}"))?;
    if !output.status.success() {
        return Err(marketplace::failure(&output));
    }
    let release = parse_release(&String::from_utf8_lossy(&output.stdout))?;
    Ok(release.filter(|release| newer(&release.version, env!("CARGO_PKG_VERSION"))))
}

/// The Editor.app this program runs from, if it runs from one.
pub fn running_bundle() -> Option<PathBuf> {
    let program = std::env::current_exe().ok()?.canonicalize().ok()?;
    program.ancestors().find(|path| path.extension().is_some_and(|extension| extension == "app")).map(Path::to_path_buf)
}

/// Homebrew, when it installed this app with the cask.
fn cask_homebrew() -> Option<PathBuf> {
    let brew = toolchain::homebrew()?;
    let prefix = brew.parent()?.parent()?;
    prefix.join("Caskroom/editor").is_dir().then_some(brew)
}

fn plist_value(bundle: &Path, key: &str) -> Option<String> {
    let output = Command::new("/usr/libexec/PlistBuddy").args(["-c", &format!("Print :{key}")]).arg(bundle.join("Contents/Info.plist")).output().ok()?;
    output.status.success().then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// The Apple team that signed a bundle, or `None` when it is signed ad hoc.
fn team(bundle: &Path) -> Option<String> {
    let output = Command::new("codesign").args(["-dv", "--verbose=2"]).arg(bundle).output().ok()?;
    let report = String::from_utf8_lossy(&output.stderr);
    report.lines().find_map(|line| line.strip_prefix("TeamIdentifier=")).map(str::trim).filter(|team| *team != "not set").map(str::to_string)
}

/// The new app must be Editor, in the promised version, intact, and signed by
/// the same team as the one running.
fn check_bundle(new: &Path, running: &Path, version: &str) -> Result<(), String> {
    if plist_value(new, "CFBundleIdentifier").as_deref() != Some(BUNDLE_ID) {
        return Err("The download is not Editor, so it was not installed.".to_string());
    }
    if plist_value(new, "CFBundleShortVersionString").as_deref() != Some(version) {
        return Err(format!("The download is not version {version}, so it was not installed."));
    }
    let verify = Command::new("codesign").args(["--verify", "--deep", "--strict"]).arg(new).output().map_err(|error| format!("Cannot check the download: {error}"))?;
    if !verify.status.success() {
        return Err("The download is damaged, so it was not installed.".to_string());
    }
    if let Some(expected) = team(running)
        && team(new).as_deref() != Some(expected.as_str()) {
            return Err("The download is not signed by Editor's developer, so it was not installed.".to_string());
        }
    Ok(())
}

/// Detaches a mounted dmg when it goes out of scope, also after an error.
struct Mounted(PathBuf);

impl Drop for Mounted {
    fn drop(&mut self) {
        Command::new("hdiutil").args(["detach", "-force", "-quiet"]).arg(&self.0).status().ok();
    }
}

pub(crate) fn writable(path: &Path) -> bool {
    let Ok(path) = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()) else {
        return false;
    };
    unsafe { libc::access(path.as_ptr(), libc::W_OK) == 0 }
}

/// Copies `source` to `bundle` through `staged`, keeping the app that is
/// there as `old` until the copy is in place, so that a failure leaves it
/// working. Without the quarantine mark, since the app is not notarized and
/// macOS would else stop it at the next start. Blocks.
pub(crate) fn put_in_place(source: &Path, bundle: &Path, staged: &Path, old: &Path) -> Result<(), String> {
    for leftover in [staged, old] {
        if leftover.exists() {
            std::fs::remove_dir_all(leftover).map_err(|error| format!("Cannot clean up an earlier try: {error}"))?;
        }
    }
    let copy = Command::new("ditto").arg(source).arg(staged).output().map_err(|error| format!("Cannot copy the app: {error}"))?;
    if !copy.status.success() {
        std::fs::remove_dir_all(staged).ok();
        return Err(format!("Cannot copy the app: {}", marketplace::failure(&copy)));
    }
    // Fails when there is no mark, which is fine.
    Command::new("xattr").args(["-dr", "com.apple.quarantine"]).arg(staged).output().ok();
    if bundle.exists() {
        std::fs::rename(bundle, old).map_err(|error| format!("Cannot replace the app: {error}"))?;
    }
    if let Err(error) = std::fs::rename(staged, bundle) {
        std::fs::rename(old, bundle).ok();
        return Err(format!("Cannot replace the app: {error}"));
    }
    std::fs::remove_dir_all(old).ok();
    Ok(())
}

/// Puts the Editor.app from `dmg` in place of `bundle`. The old app stays
/// until the new one is in place, so a failure leaves the old one working. Blocks.
pub fn install_from_dmg(dmg: &Path, bundle: &Path, version: &str) -> Result<(), String> {
    let mount = tempfile::Builder::new().prefix("editor-update-").tempdir().map_err(|error| format!("Cannot open the download: {error}"))?;
    let attach = Command::new("hdiutil")
        .args(["attach", "-nobrowse", "-readonly", "-noautoopen", "-quiet", "-mountpoint"])
        .arg(mount.path())
        .arg(dmg)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("Cannot open the download: {error}"))?;
    if !attach.status.success() {
        return Err(format!("Cannot open the download: {}", marketplace::failure(&attach)));
    }
    let mounted = Mounted(mount.path().to_path_buf());
    let source = mounted.0.join("Editor.app");
    if !source.is_dir() {
        return Err("The download has no Editor.app.".to_string());
    }
    check_bundle(&source, bundle, version)?;
    let parent = bundle.parent().ok_or("Cannot find the app's folder.")?;
    let staged = parent.join(".Editor-update.app");
    let old = parent.join(".Editor-old.app");
    if writable(parent) {
        return put_in_place(&source, bundle, &staged, &old);
    }
    // An app folder that belongs to another user: macOS asks for the password.
    let script = "/bin/rm -rf \"$3\" \"$4\" && /usr/bin/ditto \"$1\" \"$3\" && { /usr/bin/xattr -dr com.apple.quarantine \"$3\" || true; } && /bin/mv \"$2\" \"$4\" && { /bin/mv \"$3\" \"$2\" || { /bin/mv \"$4\" \"$2\"; exit 1; }; } && /bin/rm -rf \"$4\"";
    let output = Command::new("osascript")
        .args([
            "-e",
            "on run argv",
            "-e",
            "do shell script \"/bin/sh -c \" & quoted form of item 1 of argv & \" sh \" & quoted form of item 2 of argv & \" \" & quoted form of item 3 of argv & \" \" & quoted form of item 4 of argv & \" \" & quoted form of item 5 of argv with prompt \"Editor wants to install its update.\" with administrator privileges",
            "-e",
            "end run",
        ])
        .arg(script)
        .arg(&source)
        .arg(bundle)
        .arg(&staged)
        .arg(&old)
        .output()
        .map_err(|error| format!("Cannot ask for the password: {error}"))?;
    if !output.status.success() {
        let error = marketplace::failure(&output);
        return Err(if error.contains("-128") { "The update was not installed, since the password dialog was closed.".to_string() } else { format!("Cannot replace the app: {error}") });
    }
    Ok(())
}

/// Runs `brew upgrade` for the cask. Blocks.
fn upgrade_with_homebrew(brew: &Path, bundle: &Path, version: &str, cancel: &AtomicBool) -> Result<(), String> {
    let mut child = Command::new(brew)
        .args(["upgrade", "--cask", CASK])
        .env("NONINTERACTIVE", "1")
        .env("HOMEBREW_NO_ENV_HINTS", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .map_err(|error| format!("Cannot start Homebrew: {error}"))?;
    loop {
        if cancel.load(Ordering::Relaxed) {
            unsafe {
                libc::kill(-(child.id() as i32), libc::SIGTERM);
            }
            child.wait().ok();
            return Err("Stopped.".to_string());
        }
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => std::thread::sleep(POLL),
            Err(error) => return Err(format!("Homebrew stopped: {error}")),
        }
    }
    let output = child.wait_with_output().map_err(|error| format!("Homebrew stopped: {error}"))?;
    if plist_value(bundle, "CFBundleShortVersionString").as_deref() == Some(version) {
        return Ok(());
    }
    if !output.status.success() {
        return Err(format!("Homebrew could not update Editor: {}", marketplace::failure(&output)));
    }
    Err("Homebrew does not have the new version yet. Try again in an hour.".to_string())
}

/// Starts the app at `bundle` once this process has ended.
pub(crate) fn relaunch(bundle: &Path) {
    Command::new("/bin/sh")
        .args(["-c", "while kill -0 \"$1\" 2>/dev/null; do sleep 0.2; done; exec /usr/bin/open \"$2\"", "sh"])
        .arg(std::process::id().to_string())
        .arg(bundle)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .ok();
}

struct Job {
    received: Arc<AtomicU64>,
    cancel: Arc<AtomicBool>,
    homebrew: bool,
}

enum State {
    Idle,
    Available(Release),
    Installing(Release, Job),
    Failed(Release, String),
}

/// Whether there is an update and how far its install is. The workspace
/// shows it as a bar over the hub and the editor.
pub struct Updater {
    state: State,
    bundle: Option<PathBuf>,
    /// "Later" hides the bar for this version until the app starts again.
    dismissed: Option<String>,
    _checking: Option<Task<()>>,
}

impl Updater {
    pub fn new(cx: &mut Context<Self>) -> Self {
        // From the dmg or a hidden copy it cannot replace itself: the offer
        // to move it to Applications comes first.
        let bundle = running_bundle().filter(|_| !app_location::read_only());
        let mut updater = Self { state: State::Idle, bundle, dismissed: None, _checking: None };
        if updater.bundle.is_some() {
            updater._checking = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(FIRST_CHECK).await;
                loop {
                    let found = cx.background_spawn(async { check() }).await;
                    // Offline or GitHub busy: the next look tries again.
                    if let Ok(Some(release)) = found {
                        let Ok(()) = this.update(cx, |this, cx| {
                            if matches!(this.state, State::Idle | State::Available(_)) {
                                this.state = State::Available(release);
                                cx.notify();
                            }
                        }) else {
                            return;
                        };
                    }
                    cx.background_executor().timer(CHECK_EVERY).await;
                }
            }));
        }
        updater
    }

    pub fn install(&mut self, cx: &mut Context<Self>) {
        let release = match &self.state {
            State::Available(release) | State::Failed(release, _) => release.clone(),
            _ => return,
        };
        let Some(bundle) = self.bundle.clone() else {
            return;
        };
        let brew = cask_homebrew();
        let job = Job { received: Arc::default(), cancel: Arc::default(), homebrew: brew.is_some() };
        let (received, cancel) = (job.received.clone(), job.cancel.clone());
        self.state = State::Installing(release.clone(), job);
        cx.notify();
        let install = cx.background_spawn({
            let (release, bundle) = (release.clone(), bundle.clone());
            async move {
                if let Some(brew) = brew {
                    return upgrade_with_homebrew(&brew, &bundle, &release.version, &cancel);
                }
                let folder = tempfile::Builder::new().prefix("editor-update-").tempdir().map_err(|error| format!("Cannot prepare the download: {error}"))?;
                let dmg = folder.path().join("Editor.dmg");
                toolchain::download(&release.dmg_url, &dmg, false, DOWNLOAD_SECONDS, &received, &cancel)?;
                if toolchain::file_digest::<sha2::Sha256>(&dmg)? != release.sha256 {
                    return Err("The download is not the expected file, so it was not installed.".to_string());
                }
                install_from_dmg(&dmg, &bundle, &release.version)
            }
        });
        cx.spawn(async move |this, cx| {
            let mut install = std::pin::pin!(install);
            let mut shown = 0;
            let result = loop {
                let tick = cx.background_executor().timer(POLL);
                match toolchain::futures_select(&mut install, tick).await {
                    Some(result) => break result,
                    None => {
                        // Drawn again only when the percent changes.
                        let Ok(percent) = this.update(cx, |this, _| this.percent()) else {
                            return;
                        };
                        if percent != shown {
                            shown = percent;
                            this.update(cx, |_, cx| cx.notify()).ok();
                        }
                    }
                }
            };
            this.update(cx, |this, cx| {
                match result {
                    Ok(()) => {
                        relaunch(&bundle);
                        // Quitting saves every open file first.
                        cx.quit();
                    }
                    Err(error) if error == "Stopped." => this.state = State::Available(release),
                    Err(error) => this.state = State::Failed(release, error),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn percent(&self) -> u32 {
        match &self.state {
            State::Installing(release, job) if release.bytes > 0 => (job.received.load(Ordering::Relaxed) * 100 / release.bytes).min(100) as u32,
            _ => 0,
        }
    }

    fn cancel(&mut self) {
        if let State::Installing(_, job) = &self.state {
            job.cancel.store(true, Ordering::Relaxed);
        }
    }

    fn later(&mut self, cx: &mut Context<Self>) {
        if let State::Available(release) | State::Failed(release, _) = &self.state {
            self.dismissed = Some(release.version.clone());
            self.state = State::Available(release.clone());
        }
        cx.notify();
    }
}

pub(crate) fn bar_button(id: &'static str, label: &'static str, primary: bool, on_click: impl Fn(&mut App) + 'static) -> impl IntoElement {
    let palette = theme::palette();
    div()
        .id(id)
        .flex_none()
        .px(px(10.))
        .py(px(3.))
        .rounded(px(theme::RADIUS_SM))
        .cursor_pointer()
        .font_weight(FontWeight::SEMIBOLD)
        .when(primary, |style| style.bg(theme::color(palette.accent)).text_color(theme::color(palette.on_accent)))
        .when(!primary, |style| style.text_color(theme::color(palette.ui_muted)))
        .hover(|style| style.opacity(0.85))
        .on_click(move |_, _, cx| on_click(cx))
        .child(label)
}

/// The bar that offers the update, or `None` when there is nothing to say.
pub fn banner(updater: &Entity<Updater>, cx: &App) -> Option<AnyElement> {
    let state = updater.read(cx);
    let (text, buttons): (SharedString, Vec<AnyElement>) = match &state.state {
        State::Idle => return None,
        State::Available(release) if state.dismissed.as_deref() == Some(release.version.as_str()) => return None,
        State::Available(release) => {
            let (install, later, page) = (updater.clone(), updater.clone(), release.page.clone());
            (
                format!("Editor {} is available.", release.version).into(),
                vec![
                    bar_button("update-notes", "What's new", false, move |cx| cx.open_url(&page)).into_any_element(),
                    bar_button("update-later", "Later", false, move |cx| later.update(cx, |updater, cx| updater.later(cx))).into_any_element(),
                    bar_button("update-install", "Update and restart", true, move |cx| install.update(cx, |updater, cx| updater.install(cx))).into_any_element(),
                ],
            )
        }
        State::Installing(release, job) => {
            let text = if job.homebrew {
                format!("Updating Editor to {} with Homebrew…", release.version)
            } else if state.percent() < 100 {
                format!("Downloading Editor {}… {}%", release.version, state.percent())
            } else {
                format!("Installing Editor {}…", release.version)
            };
            let stop = updater.clone();
            (text.into(), vec![bar_button("update-stop", "Stop", false, move |cx| stop.update(cx, |updater, _| updater.cancel())).into_any_element()])
        }
        State::Failed(release, _) if state.dismissed.as_deref() == Some(release.version.as_str()) => return None,
        State::Failed(_, error) => {
            let (again, later) = (updater.clone(), updater.clone());
            (
                format!("The update did not work. {error}").into(),
                vec![
                    bar_button("update-later", "Later", false, move |cx| later.update(cx, |updater, cx| updater.later(cx))).into_any_element(),
                    bar_button("update-again", "Try again", true, move |cx| again.update(cx, |updater, cx| updater.install(cx))).into_any_element(),
                ],
            )
        }
    };
    Some(bar(text, buttons))
}

/// A bar over the hub and the editor, with a line of text and buttons.
pub(crate) fn bar(text: SharedString, buttons: Vec<AnyElement>) -> AnyElement {
    let palette = theme::palette();
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(8.))
        .h(px(36.))
        .px(px(12.))
        .bg(theme::color(palette.accent_soft))
        .text_color(theme::color(palette.ui_text))
        .text_size(px(theme::UI_FONT_SIZE))
        .child(div().flex_1().min_w_0().truncate().child(text))
        .children(buttons)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_part_by_part() {
        assert!(newer("0.2.0", "0.1.0"));
        assert!(newer("0.10.0", "0.9.3"));
        assert!(newer("1.0", "0.99.99"));
        assert!(!newer("0.1.0", "0.1.0"));
        assert!(!newer("0.1.0", "0.2.0"));
        assert!(!newer("0.1", "0.1.0"));
    }

    #[test]
    fn a_release_needs_a_dmg_with_a_checksum() {
        let json = r#"{"tag_name":"v0.2.0","html_url":"https://github.com/x/releases/v0.2.0","draft":false,"prerelease":false,"assets":[
            {"name":"editor.rb","browser_download_url":"https://x/editor.rb","size":10,"digest":"sha256:aa"},
            {"name":"Editor-0.2.0.dmg","browser_download_url":"https://x/Editor-0.2.0.dmg","size":34000000,"digest":"sha256:ABCD"}]}"#;
        let release = parse_release(json).unwrap().unwrap();
        assert_eq!(release.version, "0.2.0");
        assert_eq!(release.dmg_url, "https://x/Editor-0.2.0.dmg");
        assert_eq!(release.sha256, "abcd");
        let unchecked = json.replace(",\"digest\":\"sha256:ABCD\"", "");
        assert_eq!(parse_release(&unchecked).unwrap(), None);
        let draft = json.replace("\"draft\":false", "\"draft\":true");
        assert_eq!(parse_release(&draft).unwrap(), None);
    }

    /// Needs a dmg from `scripts/bundle-macos.sh`. Installs it over a copy of
    /// its own app in a temporary folder.
    #[test]
    #[ignore]
    fn a_dmg_replaces_the_app() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/bundle");
        let version = env!("CARGO_PKG_VERSION");
        let dmg = root.join(format!("Editor-{version}.dmg"));
        let folder = tempfile::tempdir().unwrap();
        let bundle = folder.path().join("Editor.app");
        assert!(Command::new("ditto").arg(root.join("Editor.app")).arg(&bundle).status().unwrap().success());
        std::fs::write(bundle.join("Contents/marker"), "old").unwrap();
        install_from_dmg(&dmg, &bundle, version).unwrap();
        assert!(!bundle.join("Contents/marker").exists());
        assert_eq!(plist_value(&bundle, "CFBundleShortVersionString").as_deref(), Some(version));
        assert!(!folder.path().join(".Editor-old.app").exists());
        assert!(install_from_dmg(&dmg, &bundle, "9.9.9").is_err());
        assert!(bundle.is_dir());
    }
}
