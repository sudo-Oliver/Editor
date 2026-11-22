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
    time::{Duration, Instant},
};

use gpui::{AnyElement, App, Context, Entity, EventEmitter, FontWeight, SharedString, div, prelude::*, px, relative};

use crate::{marketplace, settings, theme};

const DOWNLOAD_SECONDS: &str = "900";
/// MacTeX is 7 GB, which takes hours on a slow line.
const PACKAGE_SECONDS: &str = "21600";
const POLL: Duration = Duration::from_millis(150);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tool {
    /// The full TeX Live for the Mac, installed to `/Library/TeX`.
    MacTex,
    /// LaTeX in one small program that fetches packages as documents need them.
    Tectonic,
    Typst,
    /// Turns documents into Word and other formats. Installed on the first export.
    Pandoc,
    /// Installs the others when the user chose it. Not a compiler itself.
    Homebrew,
}

struct Release {
    version: &'static str,
    url: &'static str,
    sha256: &'static str,
    bytes: u64,
    /// Where the program lies inside the archive.
    program: &'static str,
}

/// An installer package for macOS, installed with an administrator's password.
pub struct Package {
    url: &'static str,
    /// Where the checksum lies, for packages that change too often to pin one.
    sha512_url: Option<&'static str>,
    sha256: Option<&'static str>,
    /// The Developer ID that must have signed it.
    signer: Option<&'static str>,
    bytes: u64,
}

const MACTEX_TEXBIN: &str = "/Library/TeX/texbin";

impl Tool {
    /// The ones listed in the settings, in the order shown.
    pub const ALL: [Tool; 5] = [Tool::MacTex, Tool::Tectonic, Tool::Typst, Tool::Pandoc, Tool::Homebrew];

    pub fn program(self) -> &'static str {
        match self {
            Self::MacTex => "pdflatex",
            Self::Tectonic => "tectonic",
            Self::Typst => "typst",
            Self::Pandoc => "pandoc",
            Self::Homebrew => "brew",
        }
    }

    /// The language a compiler is for, as people know it.
    pub fn language(self) -> &'static str {
        match self {
            Self::MacTex | Self::Tectonic => "LaTeX",
            Self::Typst => "Typst",
            Self::Pandoc => "Pandoc",
            Self::Homebrew => "Homebrew",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::MacTex => "LaTeX, complete (MacTeX)",
            Self::Tectonic => "LaTeX, compact (Tectonic)",
            Self::Typst => "Typst",
            Self::Pandoc => "Pandoc",
            Self::Homebrew => "Homebrew",
        }
    }

    /// LaTeX means the compact one, since it installs in seconds. The setup
    /// offers the complete one.
    pub fn for_language(language: &str) -> Option<Tool> {
        match language {
            "LaTeX" => Some(Self::Tectonic),
            "Typst" => Some(Self::Typst),
            "Pandoc" => Some(Self::Pandoc),
            _ => None,
        }
    }

    pub fn purpose(self) -> &'static str {
        match self {
            Self::MacTex => "Everything LaTeX has, on your Mac. Every package works at once, also without internet.",
            Self::Tectonic => "Turns LaTeX into a PDF. Small, and fetches each package the first time a document uses it.",
            Self::Typst => "Turns Typst into a PDF. Typst is a newer, simpler language for documents.",
            Self::Pandoc => "Exports documents to Word and other formats.",
            Self::Homebrew => "The usual way to install programs on a Mac. Installs and updates the others.",
        }
    }

    pub fn source(self) -> &'static str {
        match self {
            Self::MacTex => "Official MacTeX from tug.org, signed by its author",
            Self::Tectonic => "Official release from github.com/tectonic-typesetting",
            Self::Typst => "Official release from github.com/typst",
            Self::Pandoc => "Official release from github.com/jgm/pandoc",
            Self::Homebrew => "Official installer from github.com/Homebrew",
        }
    }

    /// The page to download it by hand.
    pub fn download_page(self) -> &'static str {
        match self {
            Self::MacTex => "https://tug.org/mactex/mactex-download.html",
            Self::Tectonic => "https://github.com/tectonic-typesetting/tectonic/releases",
            Self::Typst => "https://github.com/typst/typst/releases",
            Self::Pandoc => "https://github.com/jgm/pandoc/releases",
            Self::Homebrew => "https://brew.sh",
        }
    }

    /// What happens later that people should not be surprised by.
    pub fn note(self) -> Option<&'static str> {
        match self {
            Self::Tectonic => Some("The first PDF downloads the LaTeX packages it uses, so it needs the internet and takes a minute."),
            Self::MacTex => Some("macOS asks for your password once the download is done, to install into /Library/TeX."),
            _ => None,
        }
    }

    /// Installed to the system with a password, never into the app's folder.
    pub fn package(self) -> Option<Package> {
        match self {
            Self::MacTex => Some(Package {
                url: "https://mirror.ctan.org/systems/mac/mactex/MacTeX.pkg",
                sha512_url: Some("https://mirror.ctan.org/systems/mac/mactex/MacTeX.pkg.sha512"),
                sha256: None,
                signer: Some("Developer ID Installer: Richard Koch (RBGCY5RJWM)"),
                bytes: 6_865_013_189,
            }),
            Self::Homebrew => Some(Package {
                url: "https://github.com/Homebrew/brew/releases/download/7.0.14/Homebrew.pkg",
                sha512_url: None,
                sha256: Some("8d7adb3cf2cd26a6fcdc3b93570e7e1ce79e832d780a52e5ba672a303dbf3789"),
                signer: None,
                bytes: 155_283_700,
            }),
            _ => None,
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
            (Self::Pandoc, true) => Release {
                version: "3.14",
                url: "https://github.com/jgm/pandoc/releases/download/3.14/pandoc-3.14-arm64-macOS.zip",
                sha256: "58d6593884fec9d4424b641b92503c316d9c36decb2891ff8d73ae74dd3b6304",
                bytes: 43_712_865,
                program: "pandoc-3.14-arm64/bin/pandoc",
            },
            (Self::Pandoc, false) => Release {
                version: "3.14",
                url: "https://github.com/jgm/pandoc/releases/download/3.14/pandoc-3.14-x86_64-macOS.zip",
                sha256: "891708579061e7b1c1a6ce0df30ebb3cb2b6d0ca045ed3c8554ab830c1e0c270",
                bytes: 27_490_795,
                program: "pandoc-3.14-x86_64/bin/pandoc",
            },
            _ => return None,
        })
    }

    /// Whether the app can install it on this computer, itself or through Homebrew.
    pub fn installable(self) -> bool {
        cfg!(target_os = "macos") && (self.release().is_some() || self.package().is_some() || self.formula().is_some())
    }

    /// The Homebrew formula with the same program. MacTeX has none: the app
    /// installs its package directly, which is what Homebrew's cask does too.
    pub fn formula(self) -> Option<&'static str> {
        match self {
            Self::Tectonic => Some("tectonic"),
            Self::Typst => Some("typst"),
            Self::Pandoc => Some("pandoc"),
            Self::MacTex | Self::Homebrew => None,
        }
    }

    /// The download size in whole megabytes.
    pub fn megabytes(self) -> Option<u64> {
        let bytes = self.release().map(|release| release.bytes).or_else(|| self.package().map(|package| package.bytes))?;
        Some(bytes.div_ceil(1_000_000))
    }

    /// What it takes on the disk once installed, in megabytes, roughly.
    pub fn installed_megabytes(self) -> u64 {
        match self {
            Self::MacTex => 9_000,
            Self::Tectonic => 60,
            Self::Typst => 45,
            Self::Pandoc => 200,
            Self::Homebrew => 600,
        }
    }
}

pub fn tools_directory() -> Option<PathBuf> {
    Some(settings::config_dir()?.join("tools"))
}

/// Where a compiler the app starts keeps the packages it downloads. Only
/// Tectonic and Typst download any.
pub fn cache_directory(tool: Tool) -> Option<PathBuf> {
    let name = match tool {
        Tool::Tectonic => "tectonic",
        Tool::Typst => "typst",
        _ => return None,
    };
    Some(crate::storage::cache_dir()?.join(name))
}

/// The folder for programs downloaded by hand. Finder shows it, unlike the
/// app's own folder, so that people can drag files into it.
pub fn manual_directory() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("HOME")?).join("Applications/Editor Tools"))
}

fn folder(tools: &Path, tool: Tool, release: &Release) -> PathBuf {
    tools.join(format!("{}-{}", tool.program(), release.version))
}

/// Makes the compiler search the folders of installed tools, and keep its
/// packages in the app's cache. Called once at start.
pub fn register_installed() {
    if let (Some(tectonic), Some(typst)) = (cache_directory(Tool::Tectonic), cache_directory(Tool::Typst)) {
        build::set_cache_directories(tectonic, typst);
    }
    if let Some(tools) = tools_directory() {
        for tool in Tool::ALL {
            if let Some(release) = tool.release() {
                let folder = folder(&tools, tool, &release);
                if folder.join(tool.program()).is_file() {
                    build::add_directory(folder);
                }
            }
        }
    }
    if let Some(manual) = manual_directory().filter(|manual| manual.is_dir()) {
        for tool in Tool::ALL {
            if let Some(program) = program_in(&manual, tool.program()) {
                if let Some(folder) = program.parent() {
                    build::add_directory(folder.to_path_buf());
                }
            }
        }
    }
}

/// The program named `name` in `folder` or a folder below it, as people leave
/// it after unpacking a download into it.
fn program_in(folder: &Path, name: &str) -> Option<PathBuf> {
    fn search(folder: &Path, name: &str, depth: usize) -> Option<PathBuf> {
        let entries: Vec<_> = std::fs::read_dir(folder).ok()?.flatten().map(|entry| entry.path()).collect();
        if let Some(program) = entries.iter().find(|path| path.file_name().is_some_and(|file| file == name) && path.is_file()) {
            return Some(program.clone());
        }
        if depth == 0 {
            return None;
        }
        entries.iter().filter(|path| path.is_dir()).find_map(|path| search(path, name, depth - 1))
    }
    search(folder, name, 3)
}

/// Makes what the user put into Editor Tools work: a program becomes
/// executable and loses the quarantine of the browser, and a download that is
/// still packed is checked and unpacked into the app's folder. Blocks.
pub fn adopt_manual(tool: Tool) -> Result<bool, String> {
    let Some(manual) = manual_directory().filter(|manual| manual.is_dir()) else {
        return Ok(false);
    };
    if let Some(program) = program_in(&manual, tool.program()) {
        make_executable(&program)?;
        // Downloads from a browser carry a quarantine, and macOS refuses to
        // run an unsigned program that has one.
        Command::new("xattr").args(["-dr", "com.apple.quarantine"]).arg(&manual).stderr(Stdio::null()).status().ok();
        if let Some(folder) = program.parent() {
            build::add_directory(folder.to_path_buf());
        }
        return Ok(true);
    }
    let (Some(release), Some(tools)) = (tool.release(), tools_directory()) else {
        return Ok(false);
    };
    let archive_name = release.url.rsplit('/').next().unwrap_or_default();
    let Some(archive) = std::fs::read_dir(&manual).ok().into_iter().flatten().flatten().map(|entry| entry.path()).find(|path| path.file_name().is_some_and(|file| file == archive_name)) else {
        return Ok(false);
    };
    std::fs::create_dir_all(&tools).map_err(|error| format!("Cannot make the app's folder: {error}"))?;
    let workspace = tempfile::Builder::new().prefix(".unpack-").tempdir_in(&tools).map_err(|error| format!("Cannot unpack {archive_name}: {error}"))?;
    unpack_release(tool, &release, &archive, workspace.path(), &tools)?;
    std::fs::remove_file(&archive).ok();
    Ok(true)
}

/// The file name to download by hand for this Mac, if it is one program.
pub fn manual_file(tool: Tool) -> Option<&'static str> {
    tool.release().and_then(|release| release.url.rsplit('/').next())
}

/// The direct link to that file.
pub fn manual_url(tool: Tool) -> Option<&'static str> {
    tool.release().map(|release| release.url)
}

/// Uses what most documents use: a title, sections, math, a footnote, a
/// table and links, so that Tectonic fetches those fonts and packages now.
const PROBE_LATEX: &str = "\\documentclass{article}
\\usepackage[margin=2cm]{geometry}
\\usepackage{amsmath}
\\usepackage{amssymb}
\\usepackage{graphicx}
\\usepackage{xcolor}
\\usepackage{booktabs}
\\usepackage{hyperref}
\\title{Test}
\\author{Editor}
\\begin{document}
\\maketitle
\\section{Test}
\\subsection{Math}
It works\\footnote{Small text.}: $\\int_0^1 x^2\\,dx = \\tfrac{1}{3}$, $\\mathbb{R}$, \\textbf{bold}, \\emph{italic}, \\texttt{code}.
\\begin{equation}
  e^{i\\pi} + 1 = 0
\\end{equation}
\\begin{tabular}{ll}\\toprule a & b \\\\ \\bottomrule\\end{tabular}
\\end{document}
";

const PROBE_TYPST: &str = "= Test\nIt works: $integral_0^1 x^2 dif x = 1/3$.\n";

/// Compiles a small document with the compiler of `language`, to know that
/// it works before the user relies on it. For Tectonic this also fetches the
/// packages most documents use, so that the first real PDF is quick. Blocks.
pub fn probe(language: Tool) -> Result<(), String> {
    let directory = tempfile::tempdir().map_err(|error| format!("Cannot prepare the test: {error}"))?;
    let (file, mut command) = match language {
        Tool::Typst => {
            let program = build::find_tool("typst").ok_or("Typst is not installed.")?;
            let mut command = Command::new(program);
            command.args(["compile", "probe.typ"]);
            ("probe.typ", command)
        }
        Tool::MacTex | Tool::Tectonic => {
            if let Some(pdflatex) = build::find_tool("pdflatex") {
                let mut command = Command::new(pdflatex);
                command.args(["-interaction=nonstopmode", "-halt-on-error", "probe.tex"]);
                ("probe.tex", command)
            } else {
                let tectonic = build::find_tool("tectonic").ok_or("LaTeX is not installed.")?;
                let mut command = Command::new(tectonic);
                command.args(["--keep-logs", "probe.tex"]);
                ("probe.tex", command)
            }
        }
        _ => return Ok(()),
    };
    let text = if language == Tool::Typst { PROBE_TYPST } else { PROBE_LATEX };
    std::fs::write(directory.path().join(file), text).map_err(|error| format!("Cannot prepare the test: {error}"))?;
    let output = command
        .current_dir(directory.path())
        .envs(build::child_env())
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("Cannot start the compiler: {error}"))?;
    if output.status.success() && directory.path().join("probe.pdf").is_file() {
        return Ok(());
    }
    let said = format!("{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    Err(probe_failure(&said))
}

/// Why a test compile failed, in words that say what to do.
fn probe_failure(said: &str) -> String {
    let lower = said.to_lowercase();
    if ["could not resolve", "failed to connect", "network", "dns", "timed out"].iter().any(|word| lower.contains(word)) {
        return "The compiler needs the internet the first time, to fetch packages. Connect and try again.".to_string();
    }
    if lower.contains("cannot be opened") || lower.contains("operation not permitted") || lower.contains("killed") {
        return "macOS did not let the compiler run. Install it again with the button.".to_string();
    }
    let last = said.lines().map(str::trim).rfind(|line| !line.is_empty()).unwrap_or("It ended without a PDF.");
    format!("The test document did not compile: {last}")
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

pub fn find(tool: Tool) -> Found {
    match tool {
        Tool::MacTex => {
            let pdflatex = Path::new(MACTEX_TEXBIN).join("pdflatex");
            if pdflatex.is_file() && !build::pretend_fresh() { Found::Own(pdflatex) } else { Found::Missing }
        }
        Tool::Homebrew => homebrew().map_or(Found::Missing, Found::Own),
        _ => found_program(tool.program(), tools_directory().as_deref()),
    }
}

/// Whether some LaTeX works: any TeX distribution counts, since the build
/// prefers it, and Tectonic does too.
pub fn latex() -> Found {
    let tools = tools_directory();
    for program in ["latexmk", "pdflatex"] {
        if let Found::Own(path) = found_program(program, tools.as_deref()) {
            return Found::Own(path);
        }
    }
    find(Tool::Tectonic)
}

/// Whether a complete TeX distribution is there, such as MacTeX or TeX Live
/// from Homebrew, rather than only Tectonic.
pub fn complete_latex() -> bool {
    let tools = tools_directory();
    find(Tool::MacTex) != Found::Missing || ["latexmk", "pdflatex"].iter().any(|program| matches!(found_program(program, tools.as_deref()), Found::Own(_)))
}

/// Whether a language works, by any of its compilers.
pub fn language_ready(tool: Tool) -> bool {
    match tool {
        Tool::MacTex | Tool::Tectonic => latex() != Found::Missing,
        _ => find(tool) != Found::Missing,
    }
}

/// Where Homebrew is, if the user has it.
pub fn homebrew() -> Option<PathBuf> {
    if build::pretend_fresh() {
        return None;
    }
    ["/opt/homebrew/bin/brew", "/usr/local/bin/brew"].into_iter().map(PathBuf::from).find(|path| path.is_file())
}

fn from_homebrew(path: &Path) -> bool {
    path.starts_with("/opt/homebrew") || (path.starts_with("/usr/local") && homebrew().is_some_and(|brew| brew.starts_with("/usr/local")))
}

pub fn describe(found: &Found) -> String {
    match found {
        Found::Installed => "Ready".to_string(),
        Found::Own(path) if path.starts_with("/Library/TeX") => "Ready, uses your MacTeX".to_string(),
        Found::Own(path) if path.ends_with("bin/brew") => "Ready".to_string(),
        Found::Own(path) if from_homebrew(path) => "Ready, from Homebrew".to_string(),
        Found::Own(path) if manual_directory().is_some_and(|manual| path.starts_with(manual)) => "Ready, from Editor Tools".to_string(),
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

/// Downloads `url` to `target`, counting the bytes in `received`. With
/// `resume`, a part from an earlier try is continued instead of started over.
pub(crate) fn download(url: &str, target: &Path, resume: bool, seconds: &str, received: &AtomicU64, cancel: &AtomicBool) -> Result<(), String> {
    let mut command = marketplace::curl(seconds);
    if resume {
        command.args(["--continue-at", "-"]);
    }
    let mut child = command
        .arg("--output")
        .arg(target)
        .arg(url)
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|error| format!("Cannot start the download: {error}"))?;
    loop {
        if cancel.load(Ordering::Relaxed) {
            child.kill().ok();
            child.wait().ok();
            return Err("Stopped.".to_string());
        }
        if let Ok(metadata) = std::fs::metadata(target) {
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
        return Err(format!("Check the internet connection and try again. ({})", marketplace::failure(&output)));
    }
    if let Ok(metadata) = std::fs::metadata(target) {
        received.store(metadata.len(), Ordering::Relaxed);
    }
    Ok(())
}

/// The checksum of a file of any size, without reading it into memory at once.
pub(crate) fn file_digest<D: sha2::Digest>(path: &Path) -> Result<String, String> {
    use std::io::Read;
    let mut file = std::fs::File::open(path).map_err(|error| format!("Cannot read the download: {error}"))?;
    let mut digest = D::new();
    let mut buffer = vec![0; 1 << 20];
    loop {
        let read = file.read(&mut buffer).map_err(|error| format!("Cannot read the download: {error}"))?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(digest.finalize().iter().map(|byte| format!("{byte:02x}")).collect())
}

fn install_release(tool: Tool, release: &Release, tools: &Path, received: &AtomicU64, cancel: &AtomicBool) -> Result<PathBuf, String> {
    let name = tool.name();
    std::fs::create_dir_all(tools).map_err(|error| format!("Cannot make the folder for {name}: {error}"))?;
    // Inside `tools`, so that the final rename stays on one disk.
    let workspace = tempfile::Builder::new().prefix(".download-").tempdir_in(tools).map_err(|error| format!("Cannot prepare the download: {error}"))?;
    let archive = workspace.path().join("download");
    download(release.url, &archive, false, DOWNLOAD_SECONDS, received, cancel).map_err(|error| if error == "Stopped." { error } else { format!("Cannot download {name}. {error}") })?;
    unpack_release(tool, release, &archive, workspace.path(), tools)
}

/// Checks a downloaded archive of `release` and moves its program into the
/// app's folder. `workspace` is an empty folder on the same disk as `tools`.
fn unpack_release(tool: Tool, release: &Release, archive: &Path, workspace: &Path, tools: &Path) -> Result<PathBuf, String> {
    let name = tool.name();
    if file_digest::<sha2::Sha256>(archive)? != release.sha256 {
        return Err(format!("The download of {name} is not the expected file, so it was not installed."));
    }
    let listing = marketplace::run_tar(Command::new("tar").arg("-tf").arg(archive))?;
    if !marketplace::archive_paths_stay_inside(&String::from_utf8_lossy(&listing.stdout)) {
        return Err(format!("The download of {name} leads outside its folder, so it was not installed."));
    }
    let unpacked = workspace.join("unpacked");
    std::fs::create_dir(&unpacked).map_err(|error| format!("Cannot unpack the download: {error}"))?;
    marketplace::run_tar(Command::new("tar").arg("-xf").arg(archive).arg("--no-same-owner").arg("-C").arg(&unpacked))?;
    let program = unpacked.join(release.program);
    if !program.is_file() {
        return Err(format!("The download of {name} has no program."));
    }
    let staged = workspace.join("staged");
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

/// Downloads a package, checks it and installs it with macOS's own password
/// dialog. Blocks. `stage` says what it does while it is not downloading. The
/// download stays until it is installed, so that a second try continues it
/// instead of starting over.
fn install_package(tool: Tool, package: &Package, tools: &Path, received: &AtomicU64, stage: &Mutex<String>, cancel: &AtomicBool) -> Result<(), String> {
    let name = tool.name();
    let downloads = crate::storage::downloads_dir().unwrap_or_else(|| tools.join("downloads"));
    std::fs::create_dir_all(&downloads).map_err(|error| format!("Cannot make the folder for the download: {error}"))?;
    // Named as on its website, so that it reads right in Finder.
    let file = downloads.join(package.url.rsplit('/').next().unwrap_or("download.pkg"));
    let set_stage = |text: &str| {
        if let Ok(mut stage) = stage.lock() {
            *stage = text.to_string();
        }
    };
    // A download from an earlier try that is complete is not fetched again.
    let earlier = file.exists() && {
        set_stage("Checking the earlier download…");
        check_package(package, &file).is_ok()
    };
    if !earlier {
        set_stage("");
        download(package.url, &file, true, PACKAGE_SECONDS, received, cancel).map_err(|error| if error == "Stopped." { error } else { format!("Cannot download {name}. {error}") })?;
        set_stage("Checking the download…");
        if let Err(error) = check_package(package, &file) {
            std::fs::remove_file(&file).ok();
            return Err(format!("The download of {name} is not the expected file, so it was not installed. {error}"));
        }
    }
    received.store(package.bytes, Ordering::Relaxed);
    if cancel.load(Ordering::Relaxed) {
        return Err("Stopped.".to_string());
    }
    set_stage("Waiting for your password…");
    let prompt = format!("Editor wants to install {name}.");
    let output = Command::new("osascript")
        .args([
            "-e",
            "on run argv",
            "-e",
            "do shell script \"/usr/sbin/installer -pkg \" & quoted form of item 1 of argv & \" -target /\" with prompt (item 2 of argv) with administrator privileges",
            "-e",
            "end run",
        ])
        .arg(&file)
        .arg(&prompt)
        .output()
        .map_err(|error| format!("Cannot ask for the password: {error}"))?;
    if !output.status.success() {
        let error = marketplace::failure(&output);
        // -128 is the cancel button of the password dialog.
        return Err(if error.contains("-128") { format!("{name} was not installed, since the password dialog was closed.") } else { format!("Cannot install {name}: {error}") });
    }
    std::fs::remove_file(&file).ok();
    Ok(())
}

fn check_package(package: &Package, file: &Path) -> Result<(), String> {
    if let Some(expected) = package.sha256 {
        if file_digest::<sha2::Sha256>(file)? != expected {
            return Err("Its checksum differs.".to_string());
        }
    }
    if let Some(url) = package.sha512_url {
        let output = marketplace::curl("60").arg(url).output().map_err(|error| format!("Cannot run curl: {error}"))?;
        let listed = String::from_utf8_lossy(&output.stdout);
        let expected = listed.split_whitespace().next().filter(|_| output.status.success()).ok_or("Cannot fetch its checksum.")?;
        if file_digest::<sha2::Sha512>(file)? != expected.to_ascii_lowercase() {
            return Err("Its checksum differs.".to_string());
        }
    }
    if let Some(signer) = package.signer {
        let output = Command::new("pkgutil").arg("--check-signature").arg(file).output().map_err(|error| format!("Cannot check its signature: {error}"))?;
        if !output.status.success() || !signed_by(&String::from_utf8_lossy(&output.stdout), signer) {
            return Err("It is not signed by its author.".to_string());
        }
    }
    Ok(())
}

/// Whether `pkgutil --check-signature` names `signer` first, as the one who signed.
fn signed_by(report: &str, signer: &str) -> bool {
    report.lines().any(|line| line.trim().strip_prefix("1. ") == Some(signer))
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
    crate::storage::clear(&folder).map_err(|error| format!("Cannot remove {}: {error}", tool.language()))?;
    // The packages it downloaded are no use without it.
    if let Some(cache) = cache_directory(tool) {
        crate::storage::clear(&cache).map_err(|error| format!("Cannot remove the packages of {}: {error}", tool.language()))?;
    }
    Ok(())
}

/// Removes the folders of versions the app no longer installs, which an update
/// of the pinned version leaves behind. Only folders named after a program the
/// app installs itself go, so downloads and anything else stay.
pub fn remove_old_versions(tools: &Path) {
    let pinned: Vec<(&str, &str)> = Tool::ALL.into_iter().filter_map(|tool| Some((tool.program(), tool.release()?.version))).collect();
    remove_versions_except(tools, &pinned);
}

/// Removes `<program>-<version>` folders whose version is not the one in `pinned`.
fn remove_versions_except(tools: &Path, pinned: &[(&str, &str)]) {
    let Ok(entries) = std::fs::read_dir(tools) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let old = pinned.iter().any(|(program, version)| {
            name.strip_prefix(program).and_then(|rest| rest.strip_prefix('-')).is_some_and(|found| found != *version && found.starts_with(|first: char| first.is_ascii_digit()))
        });
        if old && entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            std::fs::remove_dir_all(entry.path()).ok();
        }
    }
}

/// Runs `brew install` for the tool. Blocks. `line` holds Homebrew's last line
/// of output, to show what it is doing, and `cancel` stops it.
pub fn install_with_homebrew(brew: &Path, tool: Tool, line: &Arc<Mutex<String>>, cancel: &AtomicBool) -> Result<(), String> {
    let formula = tool.formula().ok_or_else(|| format!("Homebrew does not install {} here.", tool.name()))?;
    let mut child = Command::new(brew)
        .args(["install", formula])
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
        Err(format!("Homebrew could not install {formula}: {last}"))
    }
}

/// How a tool gets installed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Method {
    /// A program from its official release, into the app's folder.
    Release,
    /// A macOS installer package, with a password.
    Package,
    Homebrew,
}

pub struct Job {
    pub received: Arc<AtomicU64>,
    /// The bytes expected, or 0 when nothing is downloaded.
    total: u64,
    /// What it does when the bytes say nothing: Homebrew's last line of
    /// output, or the step of a package. Empty while it downloads.
    pub line: Arc<Mutex<String>>,
    started: Instant,
    cancel: Arc<AtomicBool>,
}

impl Job {
    /// From 0 to 1.
    pub fn progress(&self) -> f32 {
        if self.total == 0 {
            return 0.0;
        }
        (self.received.load(Ordering::Relaxed) as f32 / self.total as f32).min(1.0)
    }

    pub fn line(&self) -> String {
        self.line.lock().map(|line| line.clone()).unwrap_or_default()
    }

    /// Whether it is downloading, so that the bytes tell the progress.
    pub fn downloading(&self) -> bool {
        self.total > 0 && self.line().is_empty()
    }

    /// What its row shows, coarse enough to change a few times a second at most.
    fn shown(&self) -> (u32, Option<u64>, String) {
        let left = self.remaining().filter(|_| self.downloading()).map(|left| left.as_secs() / 10);
        ((self.progress() * 1000.0) as u32, left, self.line())
    }

    /// The time left for the download, once its speed is known.
    pub fn remaining(&self) -> Option<Duration> {
        remaining(self.received.load(Ordering::Relaxed), self.total, self.started.elapsed())
    }
}

/// The time left for `total` bytes at the speed so far. Unknown in the first
/// seconds, when the speed says little.
fn remaining(received: u64, total: u64, elapsed: Duration) -> Option<Duration> {
    if elapsed < Duration::from_secs(3) || received == 0 || total <= received {
        return None;
    }
    let per_second = received as f64 / elapsed.as_secs_f64();
    Some(Duration::from_secs_f64((total - received) as f64 / per_second))
}

/// "About 12 minutes", rounded the way people say it.
pub fn spoken(duration: Duration) -> String {
    let minutes = duration.as_secs().div_ceil(60);
    match minutes {
        0 | 1 => "less than a minute".to_string(),
        2..=59 => format!("about {minutes} minutes"),
        _ => {
            let hours = (minutes as f64 / 60.0 * 2.0).round() / 2.0;
            if hours <= 1.0 { "about an hour".to_string() } else { format!("about {hours} hours") }
        }
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
    /// Tools for Homebrew that wait until it is installed or free, since
    /// Homebrew installs one thing at a time.
    pub waiting: Vec<Tool>,
    /// Install through Homebrew. When it is not there yet, the setup installs it first.
    pub use_homebrew: bool,
    /// What `find` said, since looking costs file system calls in every frame.
    found: std::cell::RefCell<HashMap<Tool, Found>>,
    latex: std::cell::RefCell<Option<bool>>,
    complete: std::cell::RefCell<Option<bool>>,
}

impl EventEmitter<ToolchainEvent> for Toolchain {}

impl Toolchain {
    /// `find`, remembered until something installs or the user looks again.
    pub fn found(&self, tool: Tool) -> Found {
        self.found.borrow_mut().entry(tool).or_insert_with(|| find(tool)).clone()
    }

    /// `language_ready`, remembered the same way.
    pub fn ready(&self, tool: Tool) -> bool {
        match tool {
            Tool::MacTex | Tool::Tectonic => *self.latex.borrow_mut().get_or_insert_with(|| latex() != Found::Missing),
            _ => self.found(tool) != Found::Missing,
        }
    }

    /// `complete_latex`, remembered the same way.
    pub fn has_complete_latex(&self) -> bool {
        *self.complete.borrow_mut().get_or_insert_with(complete_latex)
    }

    /// Forgets what was found, after installs or changes from outside.
    pub fn forget_found(&self) {
        self.found.borrow_mut().clear();
        self.latex.borrow_mut().take();
        self.complete.borrow_mut().take();
    }

    pub fn installing(&self, tool: Tool) -> bool {
        self.jobs.contains_key(&tool) || self.waiting.contains(&tool)
    }

    pub fn busy(&self) -> bool {
        !self.jobs.is_empty() || !self.waiting.is_empty()
    }

    pub fn choose_homebrew(&mut self, use_homebrew: bool, cx: &mut Context<Self>) {
        self.use_homebrew = use_homebrew;
        cx.emit(ToolchainEvent::InstallerChosen(use_homebrew));
        cx.notify();
    }

    fn method(&self, tool: Tool) -> Method {
        match tool {
            Tool::MacTex | Tool::Homebrew => Method::Package,
            _ if self.use_homebrew && tool.formula().is_some() => Method::Homebrew,
            _ => Method::Release,
        }
    }

    /// One line for the status bar while something installs: what, and how far.
    pub fn summary(&self) -> Option<String> {
        let (tool, job) = self.jobs.iter().max_by_key(|(tool, _)| tool.installed_megabytes())?;
        let more = self.jobs.len() + self.waiting.len() - 1;
        let mut text = if job.downloading() {
            format!("Installing {}: {}%", tool.name(), (job.progress() * 100.0).round())
        } else {
            format!("Installing {}…", tool.name())
        };
        if let Some(left) = job.remaining().filter(|_| job.downloading()) {
            text.push_str(&format!(", {} left", spoken(left)));
        }
        if more > 0 {
            text.push_str(&format!(" (and {more} more)"));
        }
        Some(text)
    }

    /// Why a document cannot compile yet with `compiler`, while it installs:
    /// the build waits for it instead of failing. A compiler that works is
    /// not held up by another one of its language that installs, such as
    /// Tectonic by a MacTeX download, unless it is itself still being set up.
    pub fn pending(&self, compiler: Tool) -> Option<String> {
        let name = compiler.language();
        let own = self.jobs.get(&compiler);
        let job = if self.ready(compiler) && find(compiler) != Found::Missing {
            own?
        } else {
            if self.waiting.iter().any(|tool| tool.language() == name) && own.is_none() {
                return Some(format!("{name} is still being set up. The PDF follows on its own."));
            }
            own.or_else(|| self.jobs.iter().find(|(tool, _)| tool.language() == name).map(|(_, job)| job))?
        };
        Some(if job.downloading() {
            format!("{name} is still being set up ({}%). The PDF follows on its own.", (job.progress() * 100.0).round())
        } else {
            format!("{name} is still being set up. The PDF follows on its own.")
        })
    }

    pub fn start(&mut self, tool: Tool, cx: &mut Context<Self>) {
        if self.installing(tool) {
            return;
        }
        self.errors.remove(&tool);
        let method = self.method(tool);
        if method == Method::Homebrew {
            // Homebrew comes first, and then one formula at a time.
            self.waiting.push(tool);
            if homebrew().is_none() {
                self.start(Tool::Homebrew, cx);
            }
            return self.next_for_homebrew(cx);
        }
        let tools = tools_directory();
        let Some(tools) = tools.filter(|_| tool.installable()) else {
            self.errors.insert(tool, format!("{} cannot be installed on this computer from here.", tool.name()));
            return cx.notify();
        };
        let total = match method {
            Method::Release => tool.release().map_or(0, |release| release.bytes),
            _ => tool.package().map_or(0, |package| package.bytes),
        };
        self.run(tool, total, cx, move |job| match method {
            Method::Release => install(tool, &tools, &job.received, &job.cancel).map(|_| ()),
            Method::Package if tool == Tool::Homebrew => install_homebrew(&tools, job),
            Method::Package => {
                let package = tool.package().ok_or("Nothing to install.")?;
                install_package(tool, &package, &tools, &job.received, &job.line, &job.cancel)
            }
            Method::Homebrew => unreachable!(),
        });
    }

    /// Starts the next formula once Homebrew is there and idle.
    fn next_for_homebrew(&mut self, cx: &mut Context<Self>) {
        let Some(brew) = homebrew() else {
            if !self.jobs.contains_key(&Tool::Homebrew) && !self.waiting.is_empty() {
                // Homebrew failed or was stopped, so its formulas cannot follow.
                for tool in std::mem::take(&mut self.waiting) {
                    self.errors.insert(tool, "Needs Homebrew, which is not installed.".to_string());
                }
            }
            return cx.notify();
        };
        if self.jobs.keys().any(|tool| self.method(*tool) == Method::Homebrew) || self.waiting.is_empty() {
            return cx.notify();
        }
        let tool = self.waiting.remove(0);
        self.run(tool, 0, cx, move |job| install_with_homebrew(&brew, tool, &job.line, &job.cancel));
    }

    /// Runs `install` in the background as the job of `tool`.
    fn run(&mut self, tool: Tool, total: u64, cx: &mut Context<Self>, install: impl FnOnce(&JobHandle) -> Result<(), String> + Send + 'static) {
        let job = Job { received: Arc::default(), total, line: Arc::default(), started: Instant::now(), cancel: Arc::default() };
        let handle = JobHandle { received: job.received.clone(), line: job.line.clone(), cancel: job.cancel.clone() };
        self.jobs.insert(tool, job);
        let install = cx.background_spawn(async move {
            install(&handle)?;
            if tool == Tool::Tectonic {
                // Its first run builds its formats and fetches the common
                // packages, which takes a minute or two. Done here, the user's
                // first PDF is quick. Offline it fails, which the first real
                // compile makes up for, so the install still counts.
                if let Ok(mut line) = handle.line.lock() {
                    *line = "Preparing the LaTeX packages. Takes a minute or two, only once.".to_string();
                }
                probe(Tool::Tectonic).ok();
            }
            Ok(())
        });
        cx.spawn(async move |this, cx| {
            let mut install = std::pin::pin!(install);
            let mut shown = None;
            let result = loop {
                let tick = cx.background_executor().timer(POLL);
                match futures_select(&mut install, tick).await {
                    Some(result) => break result,
                    None => {
                        // Drawn again only when what the row shows changed, so
                        // that a long download does not redraw the app all the time.
                        let Ok(now) = this.update(cx, |this, _| this.jobs.get(&tool).map(Job::shown)) else {
                            return;
                        };
                        if now != shown {
                            shown = now;
                            this.update(cx, |_, cx| cx.notify()).ok();
                        }
                    }
                }
            };
            this.update(cx, |this, cx| {
                this.jobs.remove(&tool);
                this.forget_found();
                match result {
                    Ok(()) => cx.emit(ToolchainEvent::Installed(tool)),
                    Err(error) if error == "Stopped." => {}
                    Err(error) => {
                        this.errors.insert(tool, error);
                    }
                }
                this.next_for_homebrew(cx);
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    pub fn cancel(&mut self, tool: Tool) {
        self.waiting.retain(|waiting| *waiting != tool);
        if let Some(job) = self.jobs.get(&tool) {
            job.cancel.store(true, Ordering::Relaxed);
        }
    }

    pub fn remove(&mut self, tool: Tool, cx: &mut Context<Self>) {
        self.forget_found();
        if let Some(tools) = tools_directory() {
            if let Err(error) = remove(tool, &tools) {
                self.errors.insert(tool, error);
            }
        }
        cx.notify();
    }
}

/// What the background side of a job may touch.
struct JobHandle {
    received: Arc<AtomicU64>,
    line: Arc<Mutex<String>>,
    cancel: Arc<AtomicBool>,
}

/// Apple's command line tools, which Homebrew needs and which bring git. Looks
/// at files only, without starting xcode-select, since the git panel asks often.
pub fn developer_tools() -> bool {
    if build::pretend_fresh() {
        return false;
    }
    let chosen = std::fs::read_link("/var/db/xcode_select_link").ok();
    chosen
        .into_iter()
        .chain([PathBuf::from("/Library/Developer/CommandLineTools"), PathBuf::from("/Applications/Xcode.app/Contents/Developer")])
        .any(|developer| developer.join("usr/bin/git").is_file())
}

/// Opens Apple's dialog that installs the command line tools, and waits until
/// they are there. Blocks.
pub fn install_developer_tools(cancel: &AtomicBool) -> Result<(), String> {
    if developer_tools() {
        return Ok(());
    }
    let output = Command::new("xcode-select").arg("--install").output().map_err(|error| format!("Cannot ask macOS for its developer tools: {error}"))?;
    // Already there, only not where the check looks: nothing to wait for.
    if !output.status.success() && String::from_utf8_lossy(&output.stderr).contains("already installed") {
        return Ok(());
    }
    while !developer_tools() {
        if cancel.load(Ordering::Relaxed) {
            return Err("Stopped.".to_string());
        }
        std::thread::sleep(Duration::from_secs(2));
    }
    Ok(())
}

/// Apple's tools first, then Homebrew's own installer. Blocks.
fn install_homebrew(tools: &Path, job: &JobHandle) -> Result<(), String> {
    if let Ok(mut line) = job.line.lock() {
        *line = "Click Install in the dialog from Apple. Its tools take a few minutes.".to_string();
    }
    install_developer_tools(&job.cancel)?;
    let package = Tool::Homebrew.package().ok_or("Nothing to install.")?;
    install_package(Tool::Homebrew, &package, tools, &job.received, &job.line, &job.cancel)?;
    homebrew().map(|_| ()).ok_or_else(|| "Homebrew's installer ended, but Homebrew is not there.".to_string())
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
/// `manage` offers Remove, which the settings want and the setup does not.
pub fn tool_rows(toolchain: &Entity<Toolchain>, tools: &[Tool], manage: bool, cx: &App) -> Vec<AnyElement> {
    let palette = theme::palette();
    let muted = theme::color(palette.ui_muted);
    let state = toolchain.read(cx);
    tools
        .iter()
        .enumerate()
        .map(|(index, &tool)| {
            let found = state.found(tool);
            let job = state.jobs.get(&tool);
            let waiting = state.waiting.contains(&tool);
            let error = state.errors.get(&tool).cloned();
            let method = state.method(tool);
            let status: SharedString = match job {
                _ if waiting => "Waiting for Homebrew…".into(),
                Some(job) if job.downloading() => {
                    let total = tool.megabytes().unwrap_or(0);
                    let done = (job.progress() * total as f32).round() as u64;
                    match job.remaining() {
                        Some(left) => format!("Downloading… {done} of {total} MB, {} left", spoken(left)).into(),
                        None => format!("Downloading… {done} of {total} MB").into(),
                    }
                }
                Some(_) if method == Method::Homebrew => "Installing with Homebrew…".into(),
                Some(job) => job.line().into(),
                None => describe(&found).into(),
            };
            let busy = job.is_some() || waiting;
            let ready = !busy && found != Found::Missing;
            let handle = toolchain.clone();
            let control: AnyElement = if busy {
                small_button(("tool-cancel", index), "Stop", false, move |cx| handle.update(cx, |toolchain, cx| {
                    toolchain.cancel(tool);
                    cx.notify();
                }))
                .into_any_element()
            } else if found == Found::Installed && manage {
                small_button(("tool-remove", index), "Remove", false, move |cx| handle.update(cx, |toolchain, cx| toolchain.remove(tool, cx))).into_any_element()
            } else if found == Found::Missing && tool.installable() {
                small_button(("tool-install", index), "Install", true, move |cx| handle.update(cx, |toolchain, cx| toolchain.start(tool, cx))).into_any_element()
            } else {
                div().into_any_element()
            };
            let details = (found == Found::Missing && !busy).then(|| match (method, tool.formula(), tool.megabytes()) {
                (Method::Homebrew, Some(formula), _) if state.found(Tool::Homebrew) != Found::Missing => format!("Runs brew install {formula} for you."),
                (Method::Homebrew, Some(formula), _) => format!("Installs Homebrew first, then runs brew install {formula}."),
                (Method::Package, _, Some(megabytes)) => {
                    format!("{} download, {} on the disk · {}", size(megabytes), size(tool.installed_megabytes()), tool.source())
                }
                (_, _, Some(megabytes)) => format!("{megabytes} MB · {} · Checked with a fingerprint (sha256) before use.", tool.source()),
                _ => "This computer needs it from elsewhere.".to_string(),
            });
            let line = job.filter(|_| method == Method::Homebrew).map(Job::line).filter(|line| !line.is_empty());
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
                                        .child(div().font_weight(FontWeight::SEMIBOLD).child(tool.name()))
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
                .when_some(line, |element, line| element.child(div().text_size(px(11.)).text_color(muted).truncate().font_family(theme::code_font()).child(line)))
                .when_some(job.filter(|job| job.downloading()), |element, job| {
                    element.child(
                        div()
                            .h(px(4.))
                            .w_full()
                            .rounded(px(2.))
                            .bg(theme::color(palette.divider))
                            .child(div().h_full().rounded(px(2.)).w(relative(job.progress())).bg(theme::color(palette.accent))),
                    )
                })
                .when_some(details, |element, details| element.child(div().text_size(px(11.)).text_color(muted).child(details)))
                .when_some(tool.note().filter(|_| found == Found::Missing && !busy && method != Method::Homebrew), |element, note| {
                    element.child(div().text_size(px(11.)).text_color(muted).child(note))
                })
                .when_some(error, |element, error| element.child(div().text_color(theme::color(palette.error)).child(error)))
                .into_any_element()
        })
        .collect()
}

/// "150 MB" or "6.9 GB".
pub fn size(megabytes: u64) -> String {
    if megabytes < 1_000 { format!("{megabytes} MB") } else { format!("{:.1} GB", megabytes as f64 / 1_000.0) }
}

/// Said once under the rows, so that nobody wonders what the app does to the computer.
pub fn promise(toolchain: &Entity<Toolchain>, cx: &App) -> &'static str {
    if toolchain.read(cx).use_homebrew {
        "Homebrew installs these like any other formula. brew uninstall removes them. MacTeX goes to /Library/TeX, as with its own installer."
    } else {
        "The compilers stay in the app's own folder, and you can remove them any time. Only MacTeX goes to /Library/TeX and asks for your password."
    }
}

/// Where to install from: Homebrew or the app's own folder. Shown only to
/// people who have Homebrew: everyone else gets the app's folder without
/// having to know what Homebrew is. The setup still offers to install it.
pub fn installer_choice(toolchain: &Entity<Toolchain>, cx: &App) -> Option<AnyElement> {
    if !cfg!(target_os = "macos") || toolchain.read(cx).found(Tool::Homebrew) == Found::Missing {
        return None;
    }
    let palette = theme::palette();
    let use_homebrew = toolchain.read(cx).use_homebrew;
    let option = |index: usize, label: &'static str, text: String, homebrew: bool| {
        let chosen = use_homebrew == homebrew;
        let handle = toolchain.clone();
        div()
            .id(("tool-installer", index))
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
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
            .child(option(0, "With Homebrew (recommended)", "You have it. brew install, updated with brew upgrade.".to_string(), true))
            .child(option(1, "In the app's folder", "No Homebrew and no password, removed with one click.".to_string(), false))
            .into_any_element(),
    )
}

/// The install's result once it is done, or `None` when `tick` comes first.
pub(crate) async fn futures_select<T>(install: &mut std::pin::Pin<&mut impl std::future::Future<Output = T>>, tick: gpui::Task<()>) -> Option<T> {
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
    fn the_time_left_follows_the_speed_so_far() {
        assert_eq!(remaining(0, 100, Duration::from_secs(10)), None);
        assert_eq!(remaining(50, 100, Duration::from_secs(1)), None);
        assert_eq!(remaining(50, 100, Duration::from_secs(10)), Some(Duration::from_secs(10)));
        assert_eq!(spoken(Duration::from_secs(20)), "less than a minute");
        assert_eq!(spoken(Duration::from_secs(12 * 60)), "about 12 minutes");
        assert_eq!(spoken(Duration::from_secs(150 * 60)), "about 2.5 hours");
        assert_eq!(size(150), "150 MB");
        assert_eq!(size(6_866), "6.9 GB");
    }

    #[test]
    fn only_the_author_signature_counts() {
        let report = "Package \"MacTeX.pkg\":\n   Status: signed by a developer certificate issued by Apple for distribution\n   Certificate Chain:\n    1. Developer ID Installer: Richard Koch (RBGCY5RJWM)\n    2. Developer ID Certification Authority\n";
        assert!(signed_by(report, "Developer ID Installer: Richard Koch (RBGCY5RJWM)"));
        assert!(!signed_by(report, "Developer ID Installer: Someone Else (AAAAAAAAAA)"));
        assert!(!signed_by("Status: no signature", "Developer ID Installer: Richard Koch (RBGCY5RJWM)"));
    }

    #[test]
    fn a_package_must_match_its_checksum() {
        let directory = tempdir().unwrap();
        let file = directory.path().join("x.pkg");
        fs::write(&file, "package").unwrap();
        let sha256 = marketplace::sha256_hex(b"package");
        let leak = |text: String| -> &'static str { Box::leak(text.into_boxed_str()) };
        let package = |sha256: &'static str| Package { url: "", sha512_url: None, sha256: Some(sha256), signer: None, bytes: 7 };
        assert_eq!(check_package(&package(leak(sha256)), &file), Ok(()));
        assert!(check_package(&package(leak("0".repeat(64))), &file).is_err());
    }

    #[test]
    fn programs_are_found_where_people_drop_them() {
        let directory = tempdir().unwrap();
        assert_eq!(program_in(directory.path(), "typst"), None);
        let nested = directory.path().join("typst-aarch64-apple-darwin");
        fs::create_dir_all(&nested).unwrap();
        fs::write(nested.join("typst"), "").unwrap();
        assert_eq!(program_in(directory.path(), "typst"), Some(nested.join("typst")));
        let pandoc = directory.path().join("pandoc-3.14-arm64/bin");
        fs::create_dir_all(&pandoc).unwrap();
        fs::write(pandoc.join("pandoc"), "").unwrap();
        assert_eq!(program_in(directory.path(), "pandoc"), Some(pandoc.join("pandoc")));
    }

    #[test]
    fn a_failed_test_compile_says_what_helps() {
        assert!(probe_failure("error: failed to connect to relay.fullyjustified.net").contains("internet"));
        assert!(probe_failure("zsh: killed     tectonic").contains("Install it again"));
        assert_eq!(probe_failure("! LaTeX Error: File `x.sty' not found.\n"), "The test document did not compile: ! LaTeX Error: File `x.sty' not found.");
    }

    #[test]
    fn languages_have_their_compilers() {
        assert_eq!(Tool::for_language("LaTeX"), Some(Tool::Tectonic));
        assert_eq!(Tool::for_language("Typst"), Some(Tool::Typst));
        assert_eq!(Tool::for_language("Pandoc"), Some(Tool::Pandoc));
        assert_eq!(Tool::MacTex.language(), Tool::Tectonic.language());
        assert_eq!(Tool::MacTex.formula(), None);
        assert!(Tool::MacTex.package().is_some_and(|package| package.signer.is_some() && package.sha512_url.is_some()));
        assert!(Tool::Homebrew.package().is_some_and(|package| package.sha256.is_some_and(|sha256| sha256.len() == 64)));
    }

    #[test]
    fn only_old_versions_of_known_programs_go() {
        let directory = tempdir().unwrap();
        let tools = directory.path();
        for name in ["typst-0.14.0", "typst-0.15.1", "tectonic-0.16.0", "pandoc-3.14", "downloads", "typst-notes", "other-1.0", ".unpack-x"] {
            fs::create_dir(tools.join(name)).unwrap();
        }
        fs::write(tools.join("typst-0.13.0"), "a file, not an install").unwrap();
        remove_versions_except(tools, &[("typst", "0.15.1"), ("tectonic", "0.17.0")]);
        let mut left: Vec<_> = fs::read_dir(tools).unwrap().map(|entry| entry.unwrap().file_name().into_string().unwrap()).collect();
        left.sort();
        assert_eq!(left, [".unpack-x", "downloads", "other-1.0", "pandoc-3.14", "typst-0.13.0", "typst-0.15.1", "typst-notes"]);
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

#[cfg(test)]
mod live {
    use super::*;

    /// How long the checks take that the UI makes. Prints, for a look by hand.
    #[test]
    #[ignore]
    fn checks_are_cheap() {
        let time = |name: &str, check: &dyn Fn()| {
            let start = Instant::now();
            for _ in 0..100 {
                check();
            }
            println!("{name}: {:?} each", start.elapsed() / 100);
        };
        time("developer_tools", &|| assert!(developer_tools()));
        time("find(Typst)", &|| drop(find(Tool::Typst)));
        time("latex()", &|| drop(latex()));
        time("homebrew()", &|| drop(homebrew()));
    }

    /// Needs the compilers and the internet: cargo test -p app live -- --ignored
    #[test]
    #[ignore]
    fn probes_and_a_real_pandoc_download() {
        for tool in [Tool::Typst, Tool::MacTex] {
            if language_ready(tool) {
                assert_eq!(probe(tool), Ok(()), "{tool:?}");
            }
        }
        let release = Tool::Pandoc.release().unwrap();
        let directory = tempfile::tempdir().unwrap();
        let (received, cancel) = (AtomicU64::new(0), AtomicBool::new(false));
        let program = install_release(Tool::Pandoc, &release, directory.path(), &received, &cancel).unwrap();
        let output = Command::new(&program).arg("--version").output().unwrap();
        assert!(String::from_utf8_lossy(&output.stdout).starts_with("pandoc 3.14"));
        build::remove_directory(program.parent().unwrap());
    }
}
