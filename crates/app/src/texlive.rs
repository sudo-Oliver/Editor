//! Updates for MacTeX, the LaTeX of this app, without Homebrew or a terminal.
//! Once a week the app asks TeX Live's own package manager, tlmgr, which
//! packages have newer versions. Asking needs no password. Updating does,
//! since MacTeX belongs to the system: one click, one password dialog of
//! macOS, and tlmgr updates itself and every package.
//!
//! tlmgr changes packages one at a time, so a TeX Live it is still updating
//! does not compile: hyperref can be new before nameref, which it now needs,
//! is there, and the formats are rebuilt only at the end. An update that stops
//! halfway leaves that for good, as tlmgr then takes the missing packages for
//! removed on purpose. So tlmgr updates a clone of the year, which APFS makes
//! in a minute without taking space, and the clone takes the place of the year
//! only once it is done. LaTeX keeps working meanwhile, and a stopped update
//! changes nothing.
//!
//! Each spring tug.org publishes the TeX Live of the new year, which the old
//! one cannot update to. The weekly question finds out, since tlmgr then names
//! both years. That beats asking the download server: MacTeX.pkg has the same
//! name every year, so only tlmgr can tell. The app then offers the new MacTeX
//! through the same download as the setup, and once it is in use, the removal
//! of the old year. Nothing is ever removed without asking.

use std::{
    ffi::OsStr,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use gpui::{AnyElement, App, Context, Entity, FontWeight, SharedString, Task, div, prelude::*, px};
use serde::{Deserialize, Serialize};

use crate::{
    storage, theme,
    toolchain::{self, AdminError, Tool, Toolchain, small_button},
};

const TEXLIVE_ROOT: &str = "/usr/local/texlive";
const DISTRIBUTIONS: &str = "/Library/TeX/Distributions";
/// A week, in seconds.
const CHECK_EVERY: u64 = 7 * 86_400;
/// The first look waits until the app has settled.
const FIRST_LOOK: Duration = Duration::from_secs(30);
/// How often an app that stays open looks whether a week has passed.
const LOOK_EVERY: Duration = Duration::from_secs(6 * 60 * 60);
/// tlmgr fetches the package list of the server, a few megabytes.
const LIST_TIMEOUT: Duration = Duration::from_secs(300);
const POLL: Duration = Duration::from_millis(250);
const UPDATE_PROMPT: &str = "Editor wants to update the LaTeX packages of MacTeX.";
/// The name the update runs under, so that a later start of the app finds it.
const UPDATE_NAME: &str = "editor-texlive-update";
/// The last line of the log of an update that went through.
const UPDATED: &str = "TeX Live is updated.";
/// Runs as root with the TeX Live root, the year, and the folder of tlmgr
/// within the year. Next to the year it keeps `<year>-update` for the clone,
/// with `.pid` and `.log`. Only one runs at a time, even across starts of the
/// app. Lines it writes before its log starts begin with `editor:`.
const UPDATE_SCRIPT: &str = r#"cd "$1" || exit 1
live=$2
work=$2-update
old=$2-previous
if [ -f "$work.pid" ] && /bin/ps -p "$(cat "$work.pid")" >/dev/null; then
  echo "editor: An update of the LaTeX packages is already running." >&2
  exit 3
fi
echo $$ >"$work.pid"
trap 'rm -f "$work.pid"' EXIT
exec >"$work.log" 2>&1
rm -rf "$work" "$old"
echo "Preparing a copy of TeX Live $live, so that LaTeX keeps working meanwhile"
/bin/cp -cR "$live" "$work" || { rm -rf "$work"; exit 1; }
if ! "$1/$work/$3/tlmgr" update --self --all; then
  echo "Removing the unfinished copy"
  rm -rf "$work"
  exit 1
fi
echo "Switching to the updated copy"
mv "$live" "$old" || exit 1
mv "$work" "$live" || { mv "$old" "$live"; exit 1; }
echo "Removing the old copy"
rm -rf "$old"
echo "TeX Live is updated."
"#;

/// What `tlmgr update --list` said.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Listed {
    /// Packages with a newer version. `tlmgr_first` when tlmgr has to update
    /// itself before it can tell all of them.
    Updates { count: usize, tlmgr_first: bool },
    /// The year gets no more updates, since the next one is being prepared.
    Frozen,
    /// The server already has the TeX Live of a later year.
    NewRelease(u16),
    Failed(String),
}

/// What the last check found, kept between starts.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(default)]
struct Record {
    /// Seconds since 1970 of the last check that reached the server, 0 for never.
    checked: u64,
    /// The year it was for, since a new MacTeX makes it stale.
    year: u16,
    updates: usize,
    tlmgr_first: bool,
    frozen: bool,
    /// The later year the server has, if any.
    newer: Option<u16>,
}

fn record_path() -> Option<PathBuf> {
    Some(storage::data_dir()?.join("texlive.toml"))
}

fn load() -> Record {
    record_path().and_then(|path| std::fs::read_to_string(path).ok()).and_then(|text| toml::from_str(&text).ok()).unwrap_or_default()
}

fn save(record: &Record) {
    let Some(path) = record_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    if let Ok(text) = toml::to_string(record) {
        crate::atomic::write(&path, text.as_bytes()).ok();
    }
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| since.as_secs())
}

/// Whether a week has passed since the last check, or it was for another
/// year. A clock set back counts as due, so that it cannot stop the checks.
pub fn due(checked: u64, checked_year: u16, year: u16, now: u64) -> bool {
    checked == 0 || checked_year != year || now < checked || now - checked >= CHECK_EVERY
}

fn year(text: &str) -> Option<u16> {
    (text.len() == 4 && text.bytes().all(|byte| byte.is_ascii_digit())).then(|| text.parse().ok()).flatten().filter(|year| *year >= 1996)
}

/// The TeX Live year in a path such as `/usr/local/texlive/2025/bin/universal-darwin`
/// or `/Library/TeX/Distributions/TeXLive-2025.texdist`.
pub fn year_in(path: &Path) -> Option<u16> {
    let parts: Vec<String> = path.components().map(|part| part.as_os_str().to_string_lossy().into_owned()).collect();
    parts
        .windows(2)
        .find_map(|pair| if pair[0] == "texlive" { year(&pair[1]) } else { None })
        .or_else(|| parts.iter().find_map(|part| part.strip_prefix("TeXLive-").and_then(|rest| rest.strip_suffix(".texdist")).and_then(year)))
}

/// The years among the folder names of `/usr/local/texlive` before `current`, oldest first.
pub fn older_years(names: impl IntoIterator<Item = String>, current: u16) -> Vec<u16> {
    let mut years: Vec<u16> = names.into_iter().filter_map(|name| year(&name)).filter(|year| *year < current).collect();
    years.sort_unstable();
    years.dedup();
    years
}

/// tlmgr of the MacTeX in use, which `/Library/TeX/texbin` points to.
fn tlmgr() -> Option<PathBuf> {
    let tlmgr = Path::new(toolchain::MACTEX_TEXBIN).join("tlmgr");
    (tlmgr.is_file() && !build::pretend_fresh()).then_some(tlmgr)
}

/// The year of the MacTeX in use. Blocks briefly, as it follows links.
pub fn installed_year() -> Option<u16> {
    tlmgr()?;
    year_in(&std::fs::canonicalize(toolchain::MACTEX_TEXBIN).ok()?)
}

/// The year the server has, from tlmgr's refusal to update across years.
fn newer_release(said: &str) -> Option<u16> {
    let (_, rest) = said.split_once("Local TeX Live (")?;
    let local = year(rest.split_once(')')?.0.trim())?;
    let (_, rest) = rest.split_once("remote repository (")?;
    let remote = year(rest.split_once(')')?.0.trim())?;
    (remote > local).then_some(remote)
}

/// Why a check or an update failed, in words that say what to do.
fn failure(said: &str) -> String {
    let lower = said.to_lowercase();
    if lower.contains("older than local") {
        return "The download server has not caught up with your TeX Live yet. Editor tries again later.".to_string();
    }
    if ["cannot load", "unable to download", "could not resolve", "connection", "timed out", "no route", "network is unreachable"].iter().any(|word| lower.contains(word)) {
        return "No connection to the TeX Live server. Editor tries again later.".to_string();
    }
    let last = said.lines().map(str::trim).rfind(|line| !line.is_empty()).map(|line| line.trim_start_matches("tlmgr: ").to_string());
    last.unwrap_or_else(|| "tlmgr ended without saying why.".to_string())
}

/// Reads what `tlmgr update --list --machine-readable` printed, and also the
/// plain format of older versions.
pub fn parse_list(said: &str, success: bool) -> Listed {
    if let Some(year) = newer_release(said) {
        return Listed::NewRelease(year);
    }
    if said.contains(" is frozen") {
        return Listed::Frozen;
    }
    if !success {
        return Listed::Failed(failure(said));
    }
    let mut tlmgr_first = said.contains("tlmgr itself needs to be updated");
    let mut count = 0;
    let mut body = false;
    for line in said.lines() {
        let line = line.trim();
        match line {
            "end-of-header" => body = true,
            "end-of-updates" => body = false,
            _ if body => {
                let mut fields = line.split_whitespace();
                if let (Some(name), Some(status)) = (fields.next(), fields.next())
                    && matches!(status, "u" | "a" | "I")
                {
                    count += 1;
                    tlmgr_first |= name == "texlive.infra";
                }
            }
            _ => {
                if let Some(rest) = line.strip_prefix("update:").or_else(|| line.strip_prefix("auto-install:")) {
                    count += 1;
                    tlmgr_first |= rest.split_whitespace().next() == Some("texlive.infra");
                }
            }
        }
    }
    Listed::Updates { count, tlmgr_first }
}

/// Runs `command` and returns what it printed on both streams, and whether it
/// succeeded. Stops it after `timeout`. Blocks.
fn run_for(mut command: Command, timeout: Duration) -> Result<(String, bool), String> {
    let mut child = command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|error| format!("Cannot start tlmgr: {error}"))?;
    let readers: Vec<_> = [child.stdout.take().map(|out| Box::new(out) as Box<dyn Read + Send>), child.stderr.take().map(|err| Box::new(err) as Box<dyn Read + Send>)]
        .into_iter()
        .flatten()
        .map(|mut stream| {
            std::thread::spawn(move || {
                let mut text = String::new();
                stream.read_to_string(&mut text).ok();
                text
            })
        })
        .collect();
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() > timeout => {
                child.kill().ok();
                child.wait().ok();
                return Err("No connection to the TeX Live server. Editor tries again later.".to_string());
            }
            Ok(None) => std::thread::sleep(POLL),
            Err(error) => return Err(format!("tlmgr stopped: {error}")),
        }
    };
    let said = readers.into_iter().filter_map(|reader| reader.join().ok()).collect::<Vec<_>>().join("\n");
    Ok((said, status.success()))
}

/// Asks the server which packages have updates. Needs no password. Blocks.
fn list_updates() -> Listed {
    let Some(tlmgr) = tlmgr() else {
        return Listed::Failed("MacTeX is not installed.".to_string());
    };
    let mut command = Command::new(tlmgr);
    command.args(["update", "--list", "--machine-readable"]);
    match run_for(command, LIST_TIMEOUT) {
        Ok((said, success)) => parse_list(&said, success),
        Err(error) => Listed::Failed(error),
    }
}

/// How an update or a removal ended.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Outcome {
    Done,
    /// The password dialog was closed, so nothing changed.
    Closed,
    NewRelease(u16),
    Failed(String),
}

/// The last line tlmgr wrote, to show what it is doing.
fn last_line(log: &str) -> Option<&str> {
    log.lines().map(str::trim).rfind(|line| !line.is_empty())
}

/// The clone of `year` an update works on, or its `.pid` and `.log` with `extension`.
fn update_path(year: u16, extension: &str) -> PathBuf {
    Path::new(TEXLIVE_ROOT).join(format!("{year}-update{extension}"))
}

/// The folder of tlmgr within the year, such as `bin/universal-darwin`.
fn bin_within(year: u16) -> Option<PathBuf> {
    let bin = std::fs::canonicalize(toolchain::MACTEX_TEXBIN).ok()?;
    Some(bin.strip_prefix(Path::new(TEXLIVE_ROOT).join(year.to_string())).ok()?.to_path_buf())
}

/// Whether an update of `year` runs, maybe started before the app was. Blocks briefly.
fn updating(year: u16) -> bool {
    let Ok(pid) = std::fs::read_to_string(update_path(year, ".pid")) else {
        return false;
    };
    let Ok(output) = Command::new("/bin/ps").args(["-p", pid.trim(), "-o", "command="]).stderr(Stdio::null()).output() else {
        return false;
    };
    output.status.success() && String::from_utf8_lossy(&output.stdout).contains(UPDATE_NAME)
}

/// Puts the last line of the update's log into `line`.
fn show_progress(year: u16, line: &Mutex<String>) {
    if let Ok(text) = std::fs::read_to_string(update_path(year, ".log"))
        && let Some(last) = last_line(&text)
        && let Ok(mut line) = line.lock()
    {
        *line = last.to_string();
    }
}

/// How an update ended, from its log.
fn ended(said: &str) -> Outcome {
    if last_line(said) == Some(UPDATED) {
        return Outcome::Done;
    }
    match newer_release(said) {
        Some(year) => Outcome::NewRelease(year),
        None => Outcome::Failed(failure(said)),
    }
}

/// Updates tlmgr and every package of `year` as an administrator, on a clone
/// that takes the place of the year once it is done. `line` follows the log.
/// Blocks.
fn update_packages(year: u16, line: &Mutex<String>) -> Outcome {
    let Some(bin) = bin_within(year) else {
        return Outcome::Failed(format!("MacTeX is not in {TEXLIVE_ROOT}, so Editor cannot update it."));
    };
    if updating(year) {
        return follow_update(year, line);
    }
    let work = std::thread::spawn(move || {
        let year = year.to_string();
        let args = [OsStr::new("-c"), OsStr::new(UPDATE_SCRIPT), OsStr::new(UPDATE_NAME), OsStr::new(TEXLIVE_ROOT), OsStr::new(&year), bin.as_os_str()];
        toolchain::as_administrator(UPDATE_PROMPT, Path::new("/bin/sh"), &args)
    });
    while !work.is_finished() {
        show_progress(year, line);
        std::thread::sleep(POLL);
    }
    let said = std::fs::read_to_string(update_path(year, ".log")).unwrap_or_default();
    match work.join() {
        Ok(Ok(())) => Outcome::Done,
        Ok(Err(AdminError::Closed)) => Outcome::Closed,
        Ok(Err(AdminError::Failed(error))) => match error.split_once("editor: ") {
            Some((_, why)) => Outcome::Failed(why.trim().to_string()),
            None if said.trim().is_empty() => Outcome::Failed(error),
            None => ended(&said),
        },
        Err(_) => Outcome::Failed("The update stopped unexpectedly.".to_string()),
    }
}

/// Follows an update that runs without the app waiting for it, as it was
/// started before the app was, until it ends. Blocks.
fn follow_update(year: u16, line: &Mutex<String>) -> Outcome {
    while updating(year) {
        show_progress(year, line);
        std::thread::sleep(POLL);
    }
    ended(&std::fs::read_to_string(update_path(year, ".log")).unwrap_or_default())
}

/// Removes the TeX Live of `year` as an administrator, never the one in use. Blocks.
fn remove_year(year: u16) -> Outcome {
    if installed_year().is_none_or(|current| current <= year) {
        return Outcome::Failed(format!("TeX Live {year} is still in use, so it stays."));
    }
    let folder = Path::new(TEXLIVE_ROOT).join(year.to_string());
    let distribution = Path::new(DISTRIBUTIONS).join(format!("TeXLive-{year}.texdist"));
    let mut args = vec![OsStr::new("-rf"), OsStr::new("--"), folder.as_os_str()];
    if distribution.exists() {
        args.push(distribution.as_os_str());
    }
    match toolchain::as_administrator(&format!("Editor wants to remove TeX Live {year}."), Path::new("/bin/rm"), &args) {
        Ok(()) => Outcome::Done,
        Err(AdminError::Closed) => Outcome::Closed,
        Err(AdminError::Failed(error)) => Outcome::Failed(format!("Cannot remove TeX Live {year}: {error}")),
    }
}

/// Earlier years of TeX Live, with what each takes in bytes.
type Older = Vec<(u16, Option<u64>)>;

/// Earlier years on the disk, with what each takes, measured once, and
/// whether an update runs.
fn look_around(known: Older) -> (Option<u16>, Older, bool) {
    let Some(current) = installed_year() else {
        return (None, Vec::new(), false);
    };
    let names = std::fs::read_dir(TEXLIVE_ROOT).into_iter().flatten().flatten().map(|entry| entry.file_name().to_string_lossy().into_owned());
    let older = older_years(names, current)
        .into_iter()
        .map(|year| {
            let size = known.iter().find(|(other, _)| *other == year).and_then(|(_, size)| *size);
            (year, size.or_else(|| Some(storage::size_of(&Path::new(TEXLIVE_ROOT).join(year.to_string())))))
        })
        .collect();
    (Some(current), older, updating(current))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ask {
    Update,
    Remove(u16),
}

enum Work {
    Checking,
    Updating(Arc<Mutex<String>>),
    Removing(u16),
}

/// The state of MacTeX's packages, which the settings show next to the compilers.
pub struct TexLive {
    /// The year of the MacTeX in use, or `None` without MacTeX.
    year: Option<u16>,
    /// Earlier years still on the disk, with their size in bytes.
    older: Vec<(u16, Option<u64>)>,
    record: Record,
    work: Option<Work>,
    /// What waits for the user to confirm, after the reason for the password.
    ask: Option<Ask>,
    /// The last thing that happened, and whether it went wrong.
    said: Option<(String, bool)>,
    _looking: Option<Task<()>>,
}

impl TexLive {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let mut texlive = Self { year: None, older: Vec::new(), record: Record::default(), work: None, ask: None, said: None, _looking: None };
        if cfg!(target_os = "macos") {
            texlive._looking = Some(cx.spawn(async move |this, cx| {
                // The year at once, for the settings; the check once the app has settled.
                let (record, year) = cx.background_spawn(async { (load(), installed_year()) }).await;
                let shown = this.update(cx, |this, cx| {
                    (this.record, this.year) = (record, year);
                    cx.notify();
                });
                if shown.is_err() {
                    return;
                }
                cx.background_executor().timer(FIRST_LOOK).await;
                loop {
                    if this.update(cx, |this, cx| this.look(cx)).is_err() {
                        return;
                    }
                    cx.background_executor().timer(LOOK_EVERY).await;
                }
            }));
        }
        texlive
    }

    /// Whether MacTeX is there, as of the last look.
    pub fn installed(&self) -> bool {
        self.year.is_some()
    }

    /// Looks at what is installed, and checks for updates once a week. Off
    /// the main thread, since measuring an old year reads many files.
    pub fn look(&mut self, cx: &mut Context<Self>) {
        let known = self.older.clone();
        cx.spawn(async move |this, cx| {
            let (record, (year, older, updating)) = cx.background_spawn(async move { (load(), look_around(known)) }).await;
            this.update(cx, |this, cx| {
                this.record = record;
                this.year = year;
                this.older = older;
                if updating && this.work.is_none() {
                    // Started before this start of the app, so it is followed.
                    this.update_packages(cx);
                } else if let Some(year) = year
                    && due(this.record.checked, this.record.year, year, now())
                {
                    this.check(cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Asks tlmgr for updates now.
    pub fn check(&mut self, cx: &mut Context<Self>) {
        let Some(year) = self.year.filter(|_| self.work.is_none()) else {
            return;
        };
        self.work = Some(Work::Checking);
        cx.notify();
        cx.spawn(async move |this, cx| {
            let listed = cx.background_spawn(async { list_updates() }).await;
            this.update(cx, |this, cx| {
                this.work = None;
                let checked = Record { checked: now(), year, ..Record::default() };
                let record = match listed {
                    Listed::Updates { count, tlmgr_first } => Record { updates: count, tlmgr_first, ..checked },
                    Listed::Frozen => Record { frozen: true, ..checked },
                    Listed::NewRelease(newer) => Record { newer: Some(newer), ..checked },
                    Listed::Failed(error) => {
                        // Offline is no news: the next look tries again.
                        this.said = Some((error, true));
                        return cx.notify();
                    }
                };
                if matches!(this.said, Some((_, true))) {
                    this.said = None;
                }
                save(&record);
                this.record = record;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Shows why the password is needed, before macOS asks for it.
    pub fn ask_update(&mut self, cx: &mut Context<Self>) {
        if self.year.is_some() && self.work.is_none() {
            self.ask = Some(Ask::Update);
            cx.notify();
        }
    }

    fn confirm(&mut self, cx: &mut Context<Self>) {
        match self.ask.take() {
            Some(Ask::Update) => self.update_packages(cx),
            Some(Ask::Remove(year)) => self.remove(year, cx),
            None => {}
        }
    }

    /// Updates the packages, or follows the update that already runs.
    fn update_packages(&mut self, cx: &mut Context<Self>) {
        let Some(year) = self.year.filter(|_| self.work.is_none()) else {
            return;
        };
        let line = Arc::new(Mutex::new(String::new()));
        self.work = Some(Work::Updating(line.clone()));
        self.said = None;
        cx.notify();
        let work = cx.background_spawn({
            let line = line.clone();
            async move { update_packages(year, &line) }
        });
        cx.spawn(async move |this, cx| {
            let mut work = std::pin::pin!(work);
            let mut shown = String::new();
            let outcome = loop {
                let tick = cx.background_executor().timer(POLL);
                match toolchain::futures_select(&mut work, tick).await {
                    Some(outcome) => break outcome,
                    None => {
                        // Drawn again only when tlmgr says something new.
                        let now = line.lock().map(|line| line.clone()).unwrap_or_default();
                        if now != shown {
                            shown = now;
                            if this.update(cx, |_, cx| cx.notify()).is_err() {
                                return;
                            }
                        }
                    }
                }
            };
            this.update(cx, |this, cx| {
                this.work = None;
                match outcome {
                    Outcome::Done => {
                        this.said = Some(("The LaTeX packages are up to date.".to_string(), false));
                        this.check(cx);
                    }
                    Outcome::Closed => this.said = Some(("Nothing changed, since the password dialog was closed.".to_string(), false)),
                    Outcome::NewRelease(newer) => {
                        this.record.newer = Some(newer);
                        save(&this.record);
                    }
                    Outcome::Failed(error) => this.said = Some((format!("The update did not finish. {error}"), true)),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn remove(&mut self, year: u16, cx: &mut Context<Self>) {
        if self.work.is_some() {
            return;
        }
        self.work = Some(Work::Removing(year));
        self.said = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let outcome = cx.background_spawn(async move { remove_year(year) }).await;
            this.update(cx, |this, cx| {
                this.work = None;
                this.said = match outcome {
                    Outcome::Done => {
                        this.older.retain(|(other, _)| *other != year);
                        Some((format!("TeX Live {year} is removed."), false))
                    }
                    Outcome::Closed => Some(("Nothing changed, since the password dialog was closed.".to_string(), false)),
                    Outcome::NewRelease(_) => None,
                    Outcome::Failed(error) => Some((error, true)),
                };
                this.look(cx);
            })
            .ok();
        })
        .detach();
    }
}

/// What the settings show under the compilers when MacTeX is there: its
/// year, the last check, the updates, and what to do about them.
pub fn section(texlive: &Entity<TexLive>, toolchain: &Entity<Toolchain>, cx: &App) -> Option<AnyElement> {
    let palette = theme::palette();
    let muted = theme::color(palette.ui_muted);
    let state = texlive.read(cx);
    let year = state.year?;
    let record = &state.record;
    let fresh = record.year == year && record.checked > 0;
    let newer = record.newer.filter(|newer| fresh && *newer > year);
    let installing = toolchain.read(cx).installing(Tool::MacTex);
    let status: SharedString = match &state.work {
        Some(Work::Checking) => "Checking for updates…".into(),
        Some(Work::Updating(_)) => "Updating…".into(),
        Some(Work::Removing(old)) => format!("Removing TeX Live {old}…").into(),
        None if newer.is_some() => "A new year is out".into(),
        None if !fresh => "Not checked yet".into(),
        None if record.frozen => "No more updates this year".into(),
        None if record.tlmgr_first => "Updates available".into(),
        None if record.updates == 1 => "1 package update available".into(),
        None if record.updates > 1 => format!("{} package updates available", record.updates).into(),
        None => "Up to date".into(),
    };
    let up_to_date = state.work.is_none() && fresh && newer.is_none() && !record.tlmgr_first && record.updates == 0;
    let checked = if record.checked > 0 && record.year == year { format!("Last checked {}.", crate::projects::ago(now(), record.checked).to_lowercase()) } else { "Editor checks once a week.".to_string() };
    let mut lines: Vec<AnyElement> = Vec::new();
    let mut buttons: Vec<AnyElement> = Vec::new();
    let line = |text: String| div().text_size(px(11.)).text_color(muted).child(text).into_any_element();
    match (&state.work, state.ask) {
        (Some(Work::Updating(progress)), _) => {
            lines.push(line("tlmgr updates a copy of TeX Live, which takes its place once every package is done. With many updates this takes an hour or more. LaTeX keeps working meanwhile.".to_string()));
            let last = progress.lock().map(|line| line.clone()).unwrap_or_default();
            if !last.is_empty() {
                lines.push(div().text_size(px(11.)).text_color(muted).truncate().font_family(theme::code_font()).child(last).into_any_element());
            }
        }
        (Some(_), _) => {}
        (None, Some(ask)) => {
            let text = match ask {
                Ask::Update => "MacTeX belongs to your Mac's system, so macOS asks for your Mac password to update it. Editor never sees the password.".to_string(),
                Ask::Remove(old) => {
                    let frees = state.older.iter().find(|(other, _)| *other == old).and_then(|(_, size)| *size).map_or(String::new(), |size| format!(" and frees {}", storage::human(size)));
                    format!("This deletes {TEXLIVE_ROOT}/{old}{frees}. It belongs to your Mac's system, so macOS asks for your Mac password. Editor never sees the password.")
                }
            };
            lines.push(div().text_color(theme::color(palette.ui_text)).child(text).into_any_element());
            let (go, stop) = (texlive.clone(), texlive.clone());
            buttons.push(small_button(("texlive-confirm", 0), "Continue", true, move |cx| go.update(cx, |texlive, cx| texlive.confirm(cx))).into_any_element());
            buttons.push(
                small_button(("texlive-cancel", 0), "Cancel", false, move |cx| {
                    stop.update(cx, |texlive, cx| {
                        texlive.ask = None;
                        cx.notify();
                    })
                })
                .into_any_element(),
            );
        }
        (None, None) => {
            lines.push(line(match newer {
                Some(newer) => format!("TeX Live {newer} is out, and TeX Live {year} gets no more updates. {checked}"),
                None if fresh && record.frozen => format!("TeX Live {year} gets no more updates, since the next year is being prepared. The new MacTeX comes out in spring. {checked}"),
                None if fresh && record.tlmgr_first => format!("LaTeX's package manager updates itself first, then the packages. {checked}"),
                None => checked,
            }));
            if let Some(newer) = newer {
                let size = toolchain::size(Tool::MacTex.megabytes().unwrap_or(0));
                lines.push(line(if installing {
                    format!("MacTeX {newer} is downloading; the row above shows how long it still takes. Your LaTeX keeps working until it is installed.")
                } else {
                    format!("The new MacTeX is one {size} download, like the first time. Your LaTeX keeps working until it is installed, and then everything works offline again.")
                }));
                if !installing {
                    let start = toolchain.clone();
                    buttons.push(small_button(("texlive-new-year", 0), format!("Install MacTeX {newer}"), true, move |cx| start.update(cx, |toolchain, cx| toolchain.start(Tool::MacTex, cx))).into_any_element());
                }
            } else {
                let (update, check) = (texlive.clone(), texlive.clone());
                buttons.push(small_button(("texlive-check", 0), "Check now", false, move |cx| check.update(cx, |texlive, cx| texlive.check(cx))).into_any_element());
                let worth = fresh && !record.frozen && (record.updates > 0 || record.tlmgr_first);
                buttons.push(small_button(("texlive-update", 0), "Update LaTeX packages", worth, move |cx| update.update(cx, |texlive, cx| texlive.ask_update(cx))).into_any_element());
            }
        }
    }
    // An earlier year can go once a later one is in use.
    let mut older: Vec<AnyElement> = Vec::new();
    if state.work.is_none() && state.ask.is_none() {
        for (index, (old, size)) in state.older.iter().enumerate() {
            let old = *old;
            let space = size.map_or(String::new(), |size| format!(" ({})", storage::human(size)));
            let remove = texlive.clone();
            older.push(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .child(div().flex_1().min_w_0().text_size(px(11.)).text_color(muted).child(format!("TeX Live {old} is still on your Mac{space}. TeX Live {year} works, so the old one can go.")))
                    .child(small_button(("texlive-remove", index), format!("Remove TeX Live {old}"), false, move |cx| {
                        remove.update(cx, |texlive, cx| {
                            texlive.ask = Some(Ask::Remove(old));
                            cx.notify();
                        })
                    }))
                    .into_any_element(),
            );
        }
    }
    Some(
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
                                    .child(div().font_weight(FontWeight::SEMIBOLD).child(format!("LaTeX packages, TeX Live {year}")))
                                    .child(
                                        div()
                                            .text_size(px(11.))
                                            .px(px(6.))
                                            .rounded(px(4.))
                                            .when(up_to_date, |style| style.bg(theme::color(palette.accent_soft)))
                                            .when(!up_to_date, |style| style.text_color(muted))
                                            .child(status),
                                    ),
                            )
                            .child(div().text_color(muted).child("New versions of LaTeX packages come from tug.org, without Homebrew or the Terminal.")),
                    )
                    .child(div().flex().flex_none().gap(px(6.)).children(buttons)),
            )
            .children(lines)
            .children(older)
            .when_some(state.said.clone(), |element, (text, error)| {
                element.child(div().text_color(theme::color(if error { palette.error } else { palette.ui_muted })).child(text))
            })
            .into_any_element(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const MACHINE: &str = "location-url\thttps://mirror.ctan.org/systems/texlive/tlnet\ntotal-bytes\t1234567\nend-of-header\nbabel\tu\t68432\t69000\t1234\t-\t-\t-\t-\t-\t-\nl3kernel\tu\t1\t2\t3\t-\t-\t-\t-\t-\t-\nnewpkg\ta\t-\t5\t6\t-\t-\t-\t-\t-\t-\ngone\td\t4\t-\t-\t-\t-\t-\t-\t-\t-\nmine\tr\t9\t8\t-\t-\t-\t-\t-\t-\t-\nend-of-updates\nother-updates\nend-of-updates\n";

    #[test]
    fn updates_are_counted_from_the_machine_readable_list() {
        assert_eq!(parse_list(MACHINE, true), Listed::Updates { count: 3, tlmgr_first: false });
        let empty = "location-url\thttps://x\ntotal-bytes\t0\nend-of-header\nend-of-updates\n";
        assert_eq!(parse_list(empty, true), Listed::Updates { count: 0, tlmgr_first: false });
    }

    #[test]
    fn the_plain_list_of_older_versions_counts_too() {
        let plain = "tlmgr: package repository https://mirror.ctan.org/systems/texlive/tlnet (verified)\nupdate:   babel  [1234k]: local:    68432, source:    69000\nauto-install: newpkg\n";
        assert_eq!(parse_list(plain, true), Listed::Updates { count: 2, tlmgr_first: false });
    }

    #[test]
    fn tlmgr_that_must_update_itself_is_noticed() {
        let said = "end-of-header\ntexlive.infra\tu\t1\t2\t3\t-\t-\t-\t-\t-\t-\nend-of-updates\n";
        assert_eq!(parse_list(said, true), Listed::Updates { count: 1, tlmgr_first: true });
        let said = "tlmgr itself needs to be updated.\nPlease do this via either\n  tlmgr update --self\n";
        assert_eq!(parse_list(said, true), Listed::Updates { count: 0, tlmgr_first: true });
    }

    #[test]
    fn a_later_year_on_the_server_is_a_new_release() {
        let said = "tlmgr: Local TeX Live (2025) is older than remote repository (2026).\nCross release updates are only supported with\n  update-tlmgr-latest(.sh/.exe) --update\n";
        assert_eq!(parse_list(said, false), Listed::NewRelease(2026));
        assert_eq!(newer_release("Local TeX Live (2026) is older than remote repository (2025)."), None);
    }

    #[test]
    fn a_frozen_year_and_failures_are_told_apart() {
        assert_eq!(parse_list("tlmgr: TeX Live 2025 is frozen\nand will no longer be routinely updated.\n", true), Listed::Frozen);
        let offline = "tlmgr: Cannot load TeX Live database from https://mirror.ctan.org/systems/texlive/tlnet at /usr/local/texlive/2025/bin/universal-darwin/tlmgr line 7003.\n";
        assert!(matches!(parse_list(offline, false), Listed::Failed(error) if error.contains("No connection")));
        assert!(matches!(parse_list("tlmgr: Remote database is older than local\n", false), Listed::Failed(error) if error.contains("not caught up")));
        assert_eq!(parse_list("tlmgr: something new\n", false), Listed::Failed("something new".to_string()));
    }

    #[test]
    fn only_a_log_that_ends_with_the_switch_is_an_update() {
        assert_eq!(ended("Switching to the updated copy\nRemoving the old copy\nTeX Live is updated.\n"), Outcome::Done);
        let stopped = "[ 823/1806, 23:26/49:51] update: match_parens [84k] (76442 -> 79202) ... done\nRemoving the unfinished copy\n";
        assert!(matches!(ended(stopped), Outcome::Failed(_)));
        let later = "tlmgr: Local TeX Live (2025) is older than remote repository (2026).\nRemoving the unfinished copy\n";
        assert_eq!(ended(later), Outcome::NewRelease(2026));
        assert!(UPDATE_SCRIPT.trim_end().ends_with(&format!("echo \"{UPDATED}\"")));
    }

    #[test]
    fn the_year_comes_from_the_path() {
        assert_eq!(year_in(Path::new("/usr/local/texlive/2025/bin/universal-darwin")), Some(2025));
        assert_eq!(year_in(Path::new("/Library/TeX/Distributions/TeXLive-2024.texdist/Contents/Programs/texbin")), Some(2024));
        assert_eq!(year_in(Path::new("/usr/local/texlive/texmf-local")), None);
        assert_eq!(year_in(Path::new("/opt/homebrew/bin")), None);
    }

    #[test]
    fn only_earlier_years_can_go() {
        let names = ["2024", "texmf-local", "2026", "2025", "12345", "0042"].map(String::from);
        assert_eq!(older_years(names, 2026), [2024, 2025]);
        assert_eq!(older_years(Vec::<String>::new(), 2026), Vec::<u16>::new());
    }

    #[test]
    fn a_check_is_due_once_a_week() {
        let day = 86_400;
        assert!(due(0, 0, 2025, 100 * day));
        assert!(!due(100 * day, 2025, 2025, 103 * day));
        assert!(due(100 * day, 2025, 2025, 107 * day));
        // A new MacTeX makes the last check stale, and a clock set back does too.
        assert!(due(100 * day, 2025, 2026, 101 * day));
        assert!(due(100 * day, 2025, 2025, 99 * day));
    }

    #[test]
    fn the_record_survives_a_round_trip() {
        let record = Record { checked: 5, year: 2025, updates: 3, tlmgr_first: false, frozen: false, newer: Some(2026) };
        assert_eq!(toml::from_str::<Record>(&toml::to_string(&record).unwrap()).unwrap(), record);
        assert_eq!(toml::from_str::<Record>("").unwrap(), Record::default());
    }
}
