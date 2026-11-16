//! Compilers the app installs for people without a TeX distribution, Homebrew
//! or a terminal. Each is one program from its project's official release,
//! checked against a checksum written here, and kept in the app's own folder.
//! Nothing outside that folder changes, so removing it removes everything.
//!
//! People who have Homebrew get the same tools from Homebrew instead, unless
//! they choose the app's folder.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    io::{BufRead, BufReader},
    os::unix::process::CommandExt,
    process::{Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};

use gpui::{AnyElement, App, Context, Entity, EventEmitter, FontWeight, SharedString, div, prelude::*, px, relative};

use crate::{marketplace, settings, theme};

const DOWNLOAD_SECONDS: &str = "900";
const POLL: Duration = Duration::from_millis(150);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tool {
    Tectonic,
    Typst,
}

struct Release {
    version: &'static str,
    url: &'static str,
    sha256: &'static str,
    bytes: u64,
    /// Where the program lies inside the archive.
    program: &'static str,
}

impl Tool {
    pub const ALL: [Tool; 2] = [Tool::Tectonic, Tool::Typst];

    pub fn program(self) -> &'static str {
        match self {
            Self::Tectonic => "tectonic",
            Self::Typst => "typst",
        }
    }

    /// The name people know, which is the language and not the program.
    pub fn language(self) -> &'static str {
        match self {
            Self::Tectonic => "LaTeX",
            Self::Typst => "Typst",
        }
    }

    pub fn for_language(language: &str) -> Option<Tool> {
        Self::ALL.into_iter().find(|tool| tool.language() == language)
    }

    pub fn purpose(self) -> &'static str {
        match self {
            Self::Tectonic => "Turns LaTeX into a PDF. LaTeX is what Overleaf uses.",
            Self::Typst => "Turns Typst into a PDF. Typst is a newer, simpler language for documents.",
        }
    }

    pub fn source(self) -> &'static str {
        match self {
            Self::Tectonic => "Official release from github.com/tectonic-typesetting",
            Self::Typst => "Official release from github.com/typst",
        }
    }

    /// What happens later that people should not be surprised by.
    pub fn note(self) -> Option<&'static str> {
        match self {
            Self::Tectonic => Some("The first PDF downloads the LaTeX packages it uses, so it needs the internet and takes a minute."),
            Self::Typst => None,
        }
    }

    fn release(self) -> Option<Release> {
        let arm = cfg!(target_arch = "aarch64");
        if !cfg!(target_os = "macos") {
            return None;
        }
        Some(match (self, arm) {
            (Self::Tectonic, true) => Release {
                version: "0.17.0",
                url: "https://github.com/tectonic-typesetting/tectonic/releases/download/tectonic%400.17.0/tectonic-0.17.0-aarch64-apple-darwin.tar.gz",
                sha256: "a3f1cac7c5678f01661a92212f58480ae3b0634115d880dbc59e2953ded45667",
                bytes: 21_704_674,
                program: "tectonic",
            },
            (Self::Tectonic, false) => Release {
                version: "0.17.0",
                url: "https://github.com/tectonic-typesetting/tectonic/releases/download/tectonic%400.17.0/tectonic-0.17.0-x86_64-apple-darwin.tar.gz",
                sha256: "7c90ef5b6ddb1eb1937e4337add5237b79338e4b9676459fa91187d24d6cdf80",
                bytes: 21_790_177,
                program: "tectonic",
            },
            (Self::Typst, true) => Release {
                version: "0.15.1",
                url: "https://github.com/typst/typst/releases/download/v0.15.1/typst-aarch64-apple-darwin.tar.xz",
                sha256: "48f62ed034aa3a7978309579ac6ca00045e2ef0da73114e8af27cfd8e74dc05a",
                bytes: 14_438_168,
                program: "typst-aarch64-apple-darwin/typst",
            },
            (Self::Typst, false) => Release {
                version: "0.15.1",
                url: "https://github.com/typst/typst/releases/download/v0.15.1/typst-x86_64-apple-darwin.tar.xz",
                sha256: "7f9fdd9584866245de9a79e0add8f9236fae6f40a8a45e2c4771ccc14db4e0fa",
                bytes: 15_635_600,
                program: "typst-x86_64-apple-darwin/typst",
            },
        })
    }

    /// Whether the app can install it on this computer, itself or through Homebrew.
    pub fn installable(self) -> bool {
        self.release().is_some() || homebrew().is_some()
    }

    /// The Homebrew formula with the same program.
    pub fn formula(self) -> &'static str {
        match self {
            Tool::Tectonic => "tectonic",
            Tool::Typst => "typst",
        }
    }

    /// The download size in whole megabytes, for the setup screen.
    pub fn megabytes(self) -> Option<u64> {
        self.release().map(|release| release.bytes.div_ceil(1_000_000))
    }
}

pub fn tools_directory() -> Option<PathBuf> {
    Some(settings::config_dir()?.join("tools"))
}

fn folder(tools: &Path, tool: Tool, release: &Release) -> PathBuf {
    tools.join(format!("{}-{}", tool.program(), release.version))
}

/// Makes the compiler search the folders of installed tools. Called once at start.
pub fn register_installed() {
    let Some(tools) = tools_directory() else {
        return;
    };
    for tool in Tool::ALL {
        if let Some(release) = tool.release() {
            let folder = folder(&tools, tool, &release);
            if folder.join(tool.program()).is_file() {
                build::add_directory(folder);
            }
        }
    }
}

/// Where a tool comes from, as people should hear it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Found {
    /// The app installed it.
    Installed,
    /// The user has it from elsewhere, such as MacTeX or Homebrew.
    Own(PathBuf),
    Missing,
}

fn found_program(program: &str, tools: Option<&Path>) -> Found {
    match build::find_tool(program) {
        Some(path) if tools.is_some_and(|tools| path.starts_with(tools)) => Found::Installed,
        Some(path) => Found::Own(path),
        None => Found::Missing,
    }
}

/// For LaTeX, an installed TeX distribution counts, since the build prefers it.
pub fn find(tool: Tool) -> Found {
    let tools = tools_directory();
    if tool == Tool::Tectonic {
        for program in ["latexmk", "pdflatex"] {
            if let Found::Own(path) = found_program(program, tools.as_deref()) {
                return Found::Own(path);
            }
        }
    }
    found_program(tool.program(), tools.as_deref())
}

/// Where Homebrew is, if the user has it.
pub fn homebrew() -> Option<PathBuf> {
    ["/opt/homebrew/bin/brew", "/usr/local/bin/brew"].into_iter().map(PathBuf::from).find(|path| path.is_file())
}

fn from_homebrew(path: &Path) -> bool {
    path.starts_with("/opt/homebrew") || (path.starts_with("/usr/local") && homebrew().is_some_and(|brew| brew.starts_with("/usr/local")))
}

pub fn describe(found: &Found) -> String {
    match found {
        Found::Installed => "Ready".to_string(),
        Found::Own(path) if path.starts_with("/Library/TeX") => "Ready, uses your MacTeX".to_string(),
        Found::Own(path) if from_homebrew(path) => "Ready, from Homebrew".to_string(),
        Found::Own(path) => format!("Ready, uses {}", crate::projects::tilde(path)),
        Found::Missing => "Not installed".to_string(),
    }
}

/// Downloads, checks and installs. Blocks. `received` counts the bytes so far,
/// and `cancel` stops the download.
pub fn install(tool: Tool, tools: &Path, received: &AtomicU64, cancel: &AtomicBool) -> Result<PathBuf, String> {
    let release = tool.release().ok_or_else(|| format!("{} cannot be installed on this computer from here.", tool.language()))?;
    install_release(tool, &release, tools, received, cancel)
}

fn install_release(tool: Tool, release: &Release, tools: &Path, received: &AtomicU64, cancel: &AtomicBool) -> Result<PathBuf, String> {
    let name = tool.language();
    std::fs::create_dir_all(tools).map_err(|error| format!("Cannot make the folder for {name}: {error}"))?;
    // Inside `tools`, so that the final rename stays on one disk.
    let workspace = tempfile::Builder::new().prefix(".download-").tempdir_in(tools).map_err(|error| format!("Cannot prepare the download: {error}"))?;
    let archive = workspace.path().join("download");
    let mut child = marketplace::curl(DOWNLOAD_SECONDS)
        .arg("--output")
        .arg(&archive)
        .arg(release.url)
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|error| format!("Cannot start the download: {error}"))?;
    loop {
        if cancel.load(Ordering::Relaxed) {
            child.kill().ok();
            child.wait().ok();
            return Err("Stopped.".to_string());
        }
        if let Ok(metadata) = std::fs::metadata(&archive) {
            received.store(metadata.len(), Ordering::Relaxed);
        }
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => std::thread::sleep(POLL),
            Err(error) => return Err(format!("The download failed: {error}")),
        }
    }
    let output = child.wait_with_output().map_err(|error| format!("The download failed: {error}"))?;
    if !output.status.success() {
        return Err(format!("Cannot download {name}. Check the internet connection and try again. ({})", marketplace::failure(&output)));
    }
    let bytes = std::fs::read(&archive).map_err(|error| format!("Cannot read the download: {error}"))?;
    received.store(bytes.len() as u64, Ordering::Relaxed);
    if marketplace::sha256_hex(&bytes) != release.sha256 {
        return Err(format!("The download of {name} is not the expected file, so it was not installed."));
    }
    let listing = marketplace::run_tar(Command::new("tar").arg("-tf").arg(&archive))?;
    if !marketplace::archive_paths_stay_inside(&String::from_utf8_lossy(&listing.stdout)) {
        return Err(format!("The download of {name} leads outside its folder, so it was not installed."));
    }
    let unpacked = workspace.path().join("unpacked");
    std::fs::create_dir(&unpacked).map_err(|error| format!("Cannot unpack the download: {error}"))?;
    marketplace::run_tar(Command::new("tar").arg("-xf").arg(&archive).arg("--no-same-owner").arg("-C").arg(&unpacked))?;
    let program = unpacked.join(release.program);
    if !program.is_file() {
        return Err(format!("The download of {name} has no program."));
    }
    let staged = workspace.path().join("staged");
    std::fs::create_dir(&staged).map_err(|error| format!("Cannot unpack the download: {error}"))?;
    let target = staged.join(tool.program());
    std::fs::rename(&program, &target).map_err(|error| format!("Cannot unpack the download: {error}"))?;
    make_executable(&target)?;
    let destination = folder(tools, tool, release);
    if destination.exists() {
        std::fs::remove_dir_all(&destination).map_err(|error| format!("Cannot replace the old {name}: {error}"))?;
    }
    std::fs::rename(&staged, &destination).map_err(|error| format!("Cannot install {name}: {error}"))?;
    build::add_directory(destination.clone());
    Ok(destination.join(tool.program()))
}

#[cfg(unix)]
fn make_executable(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).map_err(|error| format!("Cannot install the program: {error}"))
}

#[cfg(not(unix))]
fn make_executable(_: &Path) -> Result<(), String> {
    Ok(())
}

/// Removes what the app installed. A tool from elsewhere stays.
pub fn remove(tool: Tool, tools: &Path) -> Result<(), String> {
    let Some(release) = tool.release() else {
        return Ok(());
    };
    let folder = folder(tools, tool, &release);
    build::remove_directory(&folder);
    match std::fs::remove_dir_all(&folder) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(format!("Cannot remove {}: {error}", tool.language())),
        _ => Ok(()),
    }
}

/// Runs `brew install` for the tool. Blocks. `line` holds Homebrew's last line
/// of output, to show what it is doing, and `cancel` stops it.
pub fn install_with_homebrew(brew: &Path, tool: Tool, line: &Arc<Mutex<String>>, cancel: &AtomicBool) -> Result<(), String> {
    let mut child = Command::new(brew)
        .args(["install", tool.formula()])
        .env("NONINTERACTIVE", "1")
        .env("HOMEBREW_NO_ENV_HINTS", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // Its own group, so that stopping also stops the downloads it started.
        .process_group(0)
        .spawn()
        .map_err(|error| format!("Cannot start Homebrew: {error}"))?;
    let readers: Vec<_> = [child.stdout.take().map(|out| Box::new(out) as Box<dyn std::io::Read + Send>), child.stderr.take().map(|err| Box::new(err) as Box<dyn std::io::Read + Send>)]
        .into_iter()
        .flatten()
        .map(|stream| {
            let line = line.clone();
            std::thread::spawn(move || {
                for text in BufReader::new(stream).lines().map_while(Result::ok) {
                    let text = text.trim().to_string();
                    if !text.is_empty() {
                        if let Ok(mut last) = line.lock() {
                            *last = text;
                        }
                    }
                }
            })
        })
        .collect();
    let status = loop {
        if cancel.load(Ordering::Relaxed) {
            unsafe {
                libc::kill(-(child.id() as i32), libc::SIGTERM);
            }
            let _ = child.wait();
            return Err("Stopped.".to_string());
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => std::thread::sleep(POLL),
            Err(error) => return Err(format!("Homebrew stopped: {error}")),
        }
    };
    for reader in readers {
        let _ = reader.join();
    }
    if status.success() {
        Ok(())
    } else {
        let last = line.lock().map(|line| line.clone()).unwrap_or_default();
        Err(format!("Homebrew could not install {}: {last}", tool.formula()))
    }
}

pub struct Job {
    pub received: Arc<AtomicU64>,
    /// Set when Homebrew installs: its last line of output.
    pub homebrew: Option<Arc<Mutex<String>>>,
    cancel: Arc<AtomicBool>,
}

impl Job {
    /// From 0 to 1.
    pub fn progress(&self, tool: Tool) -> f32 {
        let total = tool.release().map_or(1, |release| release.bytes.max(1));
        (self.received.load(Ordering::Relaxed) as f32 / total as f32).min(1.0)
    }
}

pub enum ToolchainEvent {
    Installed(Tool),
    /// The user chose between Homebrew (true) and the app's folder.
    InstallerChosen(bool),
}

/// The installs that are running, shared by the setup screen, the hub and the
/// settings, so that each shows the same progress.
#[derive(Default)]
pub struct Toolchain {
    pub jobs: HashMap<Tool, Job>,
    pub errors: HashMap<Tool, String>,
    /// Install through Homebrew when it is there.
    pub use_homebrew: bool,
}

impl EventEmitter<ToolchainEvent> for Toolchain {}

impl Toolchain {
    pub fn installing(&self, tool: Tool) -> bool {
        self.jobs.contains_key(&tool)
    }

    /// The Homebrew to install with, if it is there and chosen.
    pub fn brew(&self) -> Option<PathBuf> {
        homebrew().filter(|_| self.use_homebrew)
    }

    pub fn choose_homebrew(&mut self, use_homebrew: bool, cx: &mut Context<Self>) {
        self.use_homebrew = use_homebrew;
        cx.emit(ToolchainEvent::InstallerChosen(use_homebrew));
        cx.notify();
    }

    pub fn start(&mut self, tool: Tool, cx: &mut Context<Self>) {
        if self.installing(tool) {
            return;
        }
        let brew = self.brew();
        let tools = tools_directory();
        if brew.is_none() && (tools.is_none() || tool.release().is_none()) {
            self.errors.insert(tool, format!("{} cannot be installed on this computer from here.", tool.language()));
            return cx.notify();
        }
        let job = Job { received: Arc::default(), homebrew: brew.as_ref().map(|_| Arc::default()), cancel: Arc::default() };
        let (received, line, cancel) = (job.received.clone(), job.homebrew.clone(), job.cancel.clone());
        self.jobs.insert(tool, job);
        self.errors.remove(&tool);
        let install = cx.background_spawn(async move {
            match (brew, line, tools) {
                (Some(brew), Some(line), _) => install_with_homebrew(&brew, tool, &line, &cancel).map(|_| PathBuf::new()),
                (_, _, Some(tools)) => install(tool, &tools, &received, &cancel),
                _ => Err("Cannot find the home folder.".to_string()),
            }
        });
        cx.spawn(async move |this, cx| {
            let mut install = std::pin::pin!(install);
            let result = loop {
                let tick = cx.background_executor().timer(POLL);
                match futures_select(&mut install, tick).await {
                    Some(result) => break result,
                    None => {
                        if this.update(cx, |_, cx| cx.notify()).is_err() {
                            return;
                        }
                    }
                }
            };
            this.update(cx, |this, cx| {
                this.jobs.remove(&tool);
                match result {
                    Ok(_) => cx.emit(ToolchainEvent::Installed(tool)),
                    Err(error) if error == "Stopped." => {}
                    Err(error) => {
                        this.errors.insert(tool, error);
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    pub fn cancel(&mut self, tool: Tool) {
        if let Some(job) = self.jobs.get(&tool) {
            job.cancel.store(true, Ordering::Relaxed);
        }
    }

    pub fn remove(&mut self, tool: Tool, cx: &mut Context<Self>) {
        if let Some(tools) = tools_directory() {
            if let Err(error) = remove(tool, &tools) {
                self.errors.insert(tool, error);
            }
        }
        cx.notify();
    }
}

fn small_button(id: (&'static str, usize), label: &'static str, primary: bool, on_click: impl Fn(&mut App) + 'static) -> impl IntoElement {
    let palette = theme::palette();
    div()
        .id(id)
        .flex_none()
        .px(px(12.))
        .py(px(5.))
        .rounded(px(6.))
        .cursor_pointer()
        .font_weight(FontWeight::SEMIBOLD)
        .bg(theme::color(if primary { palette.accent } else { palette.divider }))
        .when(primary, |style| style.text_color(theme::color(palette.on_accent)))
        .hover(|style| style.opacity(0.85))
        .on_click(move |_, _, cx| on_click(cx))
        .child(label)
}

/// One row per tool: what it is for, where it comes from, and a button to
/// install or remove it. The hub, the setup and the settings all show these.
pub fn tool_rows(toolchain: &Entity<Toolchain>, tools: &[Tool], cx: &App) -> Vec<AnyElement> {
    let palette = theme::palette();
    let muted = theme::color(palette.ui_muted);
    let state = toolchain.read(cx);
    tools
        .iter()
        .enumerate()
        .map(|(index, &tool)| {
            let found = find(tool);
            let job = state.jobs.get(&tool);
            let error = state.errors.get(&tool).cloned();
            let status: SharedString = match job {
                Some(job) if job.homebrew.is_some() => "Installing with Homebrew…".into(),
                Some(job) => {
                    let total = tool.megabytes().unwrap_or(0);
                    let done = (job.progress(tool) * total as f32).round() as u64;
                    format!("Downloading… {done} of {total} MB").into()
                }
                None => describe(&found).into(),
            };
            let ready = job.is_none() && found != Found::Missing;
            let handle = toolchain.clone();
            let control: AnyElement = if job.is_some() {
                small_button(("tool-cancel", index), "Stop", false, move |cx| handle.update(cx, |toolchain, _| toolchain.cancel(tool))).into_any_element()
            } else if found == Found::Installed {
                small_button(("tool-remove", index), "Remove", false, move |cx| handle.update(cx, |toolchain, cx| toolchain.remove(tool, cx))).into_any_element()
            } else if found == Found::Missing && tool.installable() {
                small_button(("tool-install", index), "Install", true, move |cx| handle.update(cx, |toolchain, cx| toolchain.start(tool, cx))).into_any_element()
            } else {
                div().into_any_element()
            };
            let brew = state.brew();
            let details = match (found == Found::Missing, tool.megabytes()) {
                (true, _) if brew.is_some() => Some(format!("Runs brew install {} for you.", tool.formula())),
                (true, Some(megabytes)) => Some(format!("{megabytes} MB · {} · Checked with a fingerprint (sha256) before use.", tool.source())),
                (true, None) => Some("This computer needs it from elsewhere, such as TeX Live or the Typst website.".to_string()),
                _ => None,
            };
            div()
                .flex()
                .flex_col()
                .flex_none()
                .gap(px(6.))
                .py(px(12.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(12.))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .flex_1()
                                .min_w_0()
                                .gap(px(2.))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(8.))
                                        .child(div().font_weight(FontWeight::SEMIBOLD).child(tool.language()))
                                        .child(
                                            div()
                                                .text_size(px(11.))
                                                .px(px(6.))
                                                .rounded(px(4.))
                                                .when(ready, |style| style.bg(theme::color(palette.accent_soft)))
                                                .when(!ready, |style| style.text_color(muted))
                                                .child(status),
                                        ),
                                )
                                .child(div().text_color(muted).child(tool.purpose())),
                        )
                        .child(control),
                )
                .when_some(job.and_then(|job| job.homebrew.as_ref()), |element, line| {
                    let line = line.lock().map(|line| line.clone()).unwrap_or_default();
                    element.child(div().text_size(px(11.)).text_color(muted).truncate().font_family(theme::code_font()).child(line))
                })
                .when_some(job.filter(|job| job.homebrew.is_none()), |element, job| {
                    element.child(
                        div()
                            .h(px(4.))
                            .w_full()
                            .rounded(px(2.))
                            .bg(theme::color(palette.divider))
                            .child(div().h_full().rounded(px(2.)).w(relative(job.progress(tool))).bg(theme::color(palette.accent))),
                    )
                })
                .when_some(details, |element, details| element.child(div().text_size(px(11.)).text_color(muted).child(details)))
                .when_some(tool.note().filter(|_| found == Found::Missing && brew.is_none()), |element, note| {
                    element.child(div().text_size(px(11.)).text_color(muted).child(note))
                })
                .when(brew.is_some() && tool == Tool::Tectonic && !matches!(&found, Found::Own(path) if path.starts_with("/Library/TeX")), |element| {
                    element.child(div().text_size(px(11.)).text_color(muted).child(
                        "Tectonic fetches LaTeX packages as needed. For a full TeX Live instead, run brew install --cask mactex-no-gui in Terminal; the editor prefers it once it is there.",
                    ))
                })
                .when_some(error, |element, error| element.child(div().text_color(theme::color(palette.error)).child(error)))
                .into_any_element()
        })
        .collect()
}

/// Said once under the rows, so that nobody wonders what the app does to the computer.
pub fn promise(toolchain: &Entity<Toolchain>, cx: &App) -> &'static str {
    if toolchain.read(cx).brew().is_some() {
        "Homebrew installs these like any other formula. brew uninstall removes them."
    } else {
        "Everything stays in the app's own folder. No admin password, nothing else on your Mac changes, and you can remove it any time."
    }
}

/// Where to install from, offered only to people who have Homebrew. The app's
/// own folder stays the default for everyone else, who never hears of Homebrew.
pub fn installer_choice(toolchain: &Entity<Toolchain>, cx: &App) -> Option<AnyElement> {
    homebrew()?;
    let palette = theme::palette();
    let use_homebrew = toolchain.read(cx).use_homebrew;
    let option = |index: usize, label: &'static str, text: &'static str, homebrew: bool| {
        let chosen = use_homebrew == homebrew;
        let handle = toolchain.clone();
        div()
            .id(("tool-installer", index))
            .flex()
            .flex_col()
            .flex_1()
            .gap(px(2.))
            .px(px(12.))
            .py(px(8.))
            .rounded(px(6.))
            .cursor_pointer()
            .border_1()
            .border_color(theme::color(if chosen { palette.accent } else { palette.divider }))
            .when(chosen, |style| style.bg(theme::color(palette.accent_soft)))
            .on_click(move |_, _, cx| handle.update(cx, |toolchain, cx| toolchain.choose_homebrew(homebrew, cx)))
            .child(div().font_weight(FontWeight::SEMIBOLD).child(label))
            .child(div().text_size(px(11.)).text_color(theme::color(palette.ui_muted)).child(text))
    };
    Some(
        div()
            .flex()
            .gap(px(8.))
            .child(option(0, "With Homebrew", "brew install, updated with brew upgrade.", true))
            .child(option(1, "In the app's folder", "No Homebrew needed, removed with one click.", false))
            .into_any_element(),
    )
}

/// The install's result once it is done, or `None` when `tick` comes first.
async fn futures_select<T>(install: &mut std::pin::Pin<&mut impl std::future::Future<Output = T>>, tick: gpui::Task<()>) -> Option<T> {
    use std::task::Poll;
    let mut tick = std::pin::pin!(tick);
    std::future::poll_fn(|context| {
        if let Poll::Ready(result) = install.as_mut().poll(context) {
            return Poll::Ready(Some(result));
        }
        match tick.as_mut().poll(context) {
            Poll::Ready(_) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    })
    .await
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;

    fn fake_brew(directory: &Path, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let brew = directory.join("brew");
        std::fs::write(&brew, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&brew, std::fs::Permissions::from_mode(0o755)).unwrap();
        brew
    }

    #[test]
    fn homebrew_installs_and_reports_its_last_line() {
        let directory = tempfile::tempdir().unwrap();
        let cancel = AtomicBool::new(false);
        let line = Arc::new(Mutex::new(String::new()));
        let brew = fake_brew(directory.path(), "echo \"==> Installing $2\"; echo \"==> Pouring $2\" >&2");
        assert_eq!(install_with_homebrew(&brew, Tool::Typst, &line, &cancel), Ok(()));
        assert!(line.lock().unwrap().contains("typst"));

        let brew = fake_brew(directory.path(), "echo \"Error: No available formula\" >&2; exit 1");
        let error = install_with_homebrew(&brew, Tool::Tectonic, &line, &cancel).unwrap_err();
        assert_eq!(error, "Homebrew could not install tectonic: Error: No available formula");
    }

    #[test]
    fn a_stopped_homebrew_install_ends() {
        let directory = tempfile::tempdir().unwrap();
        let brew = fake_brew(directory.path(), "sleep 30");
        let cancel = AtomicBool::new(true);
        let line = Arc::new(Mutex::new(String::new()));
        assert_eq!(install_with_homebrew(&brew, Tool::Typst, &line, &cancel), Err("Stopped.".to_string()));
    }

    fn packed(directory: &Path) -> (String, String, u64) {
        let source = directory.join("source/inner");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("typst"), "#!/bin/sh\n").unwrap();
        let archive = directory.join("typst.tar.gz");
        let status = Command::new("tar").arg("-czf").arg(&archive).arg("-C").arg(directory.join("source")).arg("inner").status().unwrap();
        assert!(status.success());
        let bytes = fs::read(&archive).unwrap();
        (format!("file://{}", archive.display()), marketplace::sha256_hex(&bytes), bytes.len() as u64)
    }

    fn release(url: String, sha256: String, bytes: u64) -> Release {
        let leak = |text: String| -> &'static str { Box::leak(text.into_boxed_str()) };
        Release { version: "9", url: leak(url), sha256: leak(sha256), bytes, program: "inner/typst" }
    }

    #[test]
    fn a_checked_download_lands_in_its_own_folder() {
        let directory = tempdir().unwrap();
        let tools = directory.path().join("tools");
        let (url, sha256, bytes) = packed(directory.path());
        let (received, cancel) = (AtomicU64::new(0), AtomicBool::new(false));

        let wrong = release(url.clone(), "0".repeat(64), bytes);
        let error = install_release(Tool::Typst, &wrong, &tools, &received, &cancel).unwrap_err();
        assert!(error.contains("not the expected file"), "{error}");
        assert!(!tools.join("typst-9").exists());

        let good = release(url, sha256, bytes);
        let program = install_release(Tool::Typst, &good, &tools, &received, &cancel).unwrap();
        assert_eq!(program, tools.join("typst-9/typst"));
        assert_eq!(received.load(Ordering::Relaxed), bytes);
        let leftovers: Vec<_> = fs::read_dir(&tools).unwrap().map(|entry| entry.unwrap().file_name()).collect();
        assert_eq!(leftovers, ["typst-9"]);
        build::remove_directory(&tools.join("typst-9"));
    }

    #[test]
    fn a_stopped_download_installs_nothing() {
        let directory = tempdir().unwrap();
        let tools = directory.path().join("tools");
        let (url, sha256, bytes) = packed(directory.path());
        let (received, cancel) = (AtomicU64::new(0), AtomicBool::new(true));
        let result = install_release(Tool::Typst, &release(url, sha256, bytes), &tools, &received, &cancel);
        assert_eq!(result.unwrap_err(), "Stopped.");
        assert!(!tools.join("typst-9").exists());
    }

    #[test]
    fn every_release_is_pinned() {
        for tool in Tool::ALL {
            if let Some(release) = tool.release() {
                assert!(release.url.starts_with("https://github.com/"));
                assert_eq!(release.sha256.len(), 64);
                assert!(release.bytes > 1_000_000);
            }
        }
    }
}
