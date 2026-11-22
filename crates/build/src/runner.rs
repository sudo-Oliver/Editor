use std::{
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        Arc, Mutex as StdMutex,
        atomic::{AtomicI32, Ordering},
    },
    time::{Duration, Instant, SystemTime},
};

use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, BufReader},
    process::Command,
    runtime::Handle,
    sync::{Mutex, mpsc, oneshot},
};

use crate::{
    config::Engine,
    explain, fontmap,
    parse::{self, Diagnostic, Severity},
    preamble::{self, Preamble, State},
    tidy,
    tools::{child_path, find_tool},
};

pub(crate) const OUTPUT_DIRECTORY: &str = ".build";
const MAX_LATEX_RUNS: usize = 3;
/// zlib's own default. pdfTeX's 9 spends a quarter of a build on compressing the
/// PDF and its images, for a file only about 4% smaller.
const PDF_COMPRESSION: &str = "\\pdfcompresslevel=6 \\input mylatex.ltx";
const KILL_GRACE: Duration = Duration::from_millis(500);
/// How long the pipes may stay open after the compiler has ended.
const DRAIN_GRACE: Duration = Duration::from_secs(2);
const RUN_TIME_LIMIT: Duration = Duration::from_secs(15 * 60);
const EVENT_CAPACITY: usize = 1024;
const MAX_LINE_BYTES: u64 = 64 * 1024;
const OUTPUT_TAIL_BYTES: usize = 1024 * 1024;
/// Files LaTeX carries between runs. A failed or stopped run can leave them
/// broken, and the next run then fails on them. The PDF is not in this list.
pub(crate) const LATEX_STATE_EXTENSIONS: &[&str] = &[
    "aux", "bbl", "bcf", "blg", "fdb_latexmk", "fls", "idx", "ilg", "ind", "lof", "log", "lot", "nav", "out",
    "run.xml", "snm", "toc", "vrb", "xdv",
];

#[derive(Clone, Debug)]
pub struct BuildRequest {
    pub root: PathBuf,
    pub main: PathBuf,
    pub engine: Engine,
    /// Discards LaTeX's state from earlier runs first.
    pub clean: bool,
}

#[derive(Clone, Debug)]
pub struct BuildOutcome {
    pub success: bool,
    pub pdf: Option<PathBuf>,
    pub synctex: Option<PathBuf>,
    pub diagnostics: Vec<Diagnostic>,
    pub duration: Duration,
    /// The PDF of an earlier build, when this build produced none.
    pub last_good_pdf: Option<PathBuf>,
}

#[derive(Clone, Debug)]
pub enum BuildEvent {
    Started,
    Output(String),
    Finished(BuildOutcome),
    Cancelled,
}

/// Runs one build at a time. Starting a build cancels the running one.
pub struct BuildManager {
    runtime: Handle,
    serial: Arc<Mutex<()>>,
    cancel: StdMutex<Option<oneshot::Sender<()>>>,
    /// The process group of the running program, or 0.
    group: Arc<AtomicI32>,
}

impl Default for BuildManager {
    fn default() -> Self {
        Self::new()
    }
}

impl BuildManager {
    pub fn new() -> Self {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("the build runtime starts");
        let handle = runtime.handle().clone();
        std::thread::Builder::new()
            .name("build".into())
            .spawn(move || runtime.block_on(std::future::pending::<()>()))
            .expect("the build thread starts");
        Self {
            runtime: handle,
            serial: Arc::new(Mutex::new(())),
            cancel: StdMutex::new(None),
            group: Arc::new(AtomicI32::new(0)),
        }
    }

    pub fn start(&self, request: BuildRequest) -> mpsc::Receiver<BuildEvent> {
        self.cancel();
        // A full channel makes the compiler wait, so that a runaway one cannot flood the app.
        let (events, receiver) = mpsc::channel(EVENT_CAPACITY);
        let (cancel, mut cancelled) = oneshot::channel();
        *self.cancel.lock().expect("the cancel lock is not poisoned") = Some(cancel);
        let serial = self.serial.clone();
        let group = self.group.clone();
        self.runtime.spawn(async move {
            let _guard = tokio::select! {
                guard = serial.lock() => guard,
                _ = &mut cancelled => {
                    events.send(BuildEvent::Cancelled).await.ok();
                    return;
                }
            };
            events.send(BuildEvent::Started).await.ok();
            let event = match build(&request, &events, &mut cancelled, &group).await {
                Some(outcome) => BuildEvent::Finished(outcome),
                None => BuildEvent::Cancelled,
            };
            events.send(event).await.ok();
        });
        receiver
    }

    pub fn cancel(&self) {
        let cancel = self.cancel.lock().expect("the cancel lock is not poisoned").take();
        if let Some(cancel) = cancel {
            cancel.send(()).ok();
        }
    }

    /// Ends the running program at once, because the app is about to exit and the
    /// build thread with it.
    pub fn shut_down(&self) {
        self.cancel();
        let group = self.group.swap(0, Ordering::SeqCst);
        if group > 0 {
            signal_group(Some(group), libc::SIGKILL);
        }
    }
}

struct Invocation {
    tool: &'static str,
    args: Vec<String>,
    env: Vec<(&'static str, String)>,
}

struct RunResult {
    success: bool,
    stderr: String,
}

async fn build(
    request: &BuildRequest,
    events: &mpsc::Sender<BuildEvent>,
    cancelled: &mut oneshot::Receiver<()>,
    group: &AtomicI32,
) -> Option<BuildOutcome> {
    let started = Instant::now();
    let started_at = SystemTime::now();
    // Typst resolves imports against the project root, the others against the main file.
    let cwd = match request.engine {
        Engine::Typst => request.root.clone(),
        _ => request.main.parent().unwrap_or(&request.root).to_path_buf(),
    };
    let output_directory = cwd.join(OUTPUT_DIRECTORY);
    if let Err(error) = std::fs::create_dir_all(&output_directory) {
        return Some(failure(format!("Cannot create {}: {error}", output_directory.display()), started));
    }
    tidy::keep_out_of_backups(&output_directory);
    let stem = request.main.file_stem().map_or_else(|| "main".into(), |stem| stem.to_string_lossy().into_owned());
    tidy::remove_orphans(&cwd, &output_directory, &stem);
    let input = match request.engine {
        Engine::Typst => request.main.strip_prefix(&request.root).unwrap_or(&request.main),
        _ => Path::new(request.main.file_name().unwrap_or_default()),
    }
    .to_string_lossy()
    .into_owned();

    let plain = invocation(request.engine, &input, &stem, None, false, false);
    if find_tool(plain.tool).is_none() {
        return Some(failure(format!("{} is not installed.", plain.tool), started));
    }
    if matches!(request.engine, Engine::PdfLatex | Engine::LuaLatex) {
        // `\include{dir/file}` writes its .aux below the output directory.
        mirror_directories(&cwd, &output_directory);
    }

    let is_latex = matches!(request.engine, Engine::PdfLatex | Engine::LuaLatex);
    let log_path = output_directory.join(format!("{stem}.log"));
    let mut cleaned = request.clean;
    // Only pdfLaTeX uses a format, so after a switch to another LaTeX the old one is of no use.
    if request.clean || matches!(request.engine, Engine::LuaLatex | Engine::Tectonic) {
        preamble::remove_formats(&output_directory, None);
    }
    let mut format = if request.engine == Engine::PdfLatex && !request.clean {
        prepare_format(&cwd, &output_directory, &request.main, &input, events, cancelled, group).await?
    } else {
        None
    };
    let mut format_dropped = None;
    let (run, mut diagnostics) = loop {
        if is_latex && cleaned {
            remove_latex_state(&output_directory);
        }
        let name = format.as_ref().map(Preamble::name);
        // LaTeX runs by itself, which spares latexmk's start and its checks on
        // every edit. latexmk takes over only for what LaTeX cannot do alone,
        // such as a bibliography or an index, and for a clean build.
        let font_map = request.engine == Engine::PdfLatex && fontmap::current(&output_directory).is_some();
        // The first build of a project goes to latexmk at once, since it may need
        // a bibliography, which a run of LaTeX alone would only find out.
        let first = !output_directory.join(format!("{stem}.aux")).is_file();
        let direct = invocation(request.engine, &input, &stem, name.as_deref(), cleaned || first, font_map);
        let reruns_by_hand = direct.tool == "pdflatex" || direct.tool == "lualatex";
        let mut runs = 0;
        let mut map_grown = false;
        let mut run = loop {
            runs += 1;
            let run = run_process(&direct, &cwd, events, cancelled, group).await?;
            let log = std::fs::read(&log_path).map(|log| String::from_utf8_lossy(&log).into_owned()).unwrap_or_default();
            // A font outside the project's map came out as a bitmap: the map
            // grows by its family, and LaTeX runs again once.
            if font_map && !map_grown && fontmap::missed_a_font(&log) {
                map_grown = true;
                if fontmap::update(&output_directory, &stem) {
                    continue;
                }
                fontmap::remove(&output_directory);
            }
            let rerun = reruns_by_hand && runs < MAX_LATEX_RUNS && log.contains("Rerun to get");
            if !rerun {
                break run;
            }
        };
        let mut by_latexmk = direct.tool == "latexmk";
        if reruns_by_hand && find_tool("latexmk").is_some() && needs_latexmk(&cwd, &output_directory, &stem) {
            // The full font map, since latexmk's runs would make bitmaps for a missing font.
            let full = invocation(request.engine, &input, &stem, name.as_deref(), true, false);
            run = run_process(&full, &cwd, events, cancelled, group).await?;
            by_latexmk = true;
        }
        if run.success && request.engine == Engine::PdfLatex && !font_map {
            fontmap::update(&output_directory, &stem);
        }
        let diagnostics = match request.engine {
            Engine::Typst => parse::parse_typst(&run.stderr, &cwd),
            Engine::Tectonic => parse::parse_tectonic(&run.stderr, &cwd),
            Engine::Pandoc => parse::parse_pandoc(&run.stderr, &cwd),
            Engine::PdfLatex | Engine::LuaLatex => {
                let log = std::fs::read(&log_path).unwrap_or_default();
                parse::parse_latex_log(&String::from_utf8_lossy(&log), &cwd)
            }
        };
        let mut diagnostics = diagnostics;
        // latexmk fails when BibTeX complains, even when LaTeX made the whole
        // document. A fresh PDF and a log without errors mean it worked.
        if by_latexmk && !run.success && !diagnostics.iter().any(|diagnostic| diagnostic.severity == Severity::Error) && written_since(&output_directory.join(format!("{stem}.pdf")), started_at) {
            run.success = true;
            diagnostics.extend(bibliography_problems(&output_directory, &stem));
        }
        // An untested format could be what is wrong, and one normal run decides that.
        if let Some(used) = format.take_if(|used| {
            let log = std::fs::read_to_string(&log_path).ok();
            !run.success && (used.state() != State::Trusted || preamble::format_failed(log.as_deref()))
        }) {
            format_dropped = Some(used);
            continue;
        }
        // LaTeX reads its own files at \\begin and \\end{document}, so an earlier run
        // that left them broken shows up there. One run without them decides.
        let state_is_broken = diagnostics.iter().any(|diagnostic| {
            diagnostic.severity == Severity::Error
                && (diagnostic.file.as_deref().is_some_and(|file| file.starts_with(&output_directory))
                    || diagnostic.context.as_deref().is_some_and(|context| context.contains("{document}")))
        });
        if is_latex && !run.success && !cleaned && state_is_broken {
            cleaned = true;
            continue;
        }
        break (run, diagnostics);
    };
    if run.success {
        if let Some(format) = &format {
            format.mark_trusted();
        }
        if let Some(format) = &format_dropped {
            format.mark_unsuitable();
        }
    }
    if is_latex || request.engine == Engine::Tectonic {
        explain::locate(&mut diagnostics, &request.main);
    }
    if !run.success && !diagnostics.iter().any(|diagnostic| diagnostic.severity == Severity::Error) {
        // No error was parsed, so the first line of stderr is the best explanation.
        let message = failure_summary(&run.stderr);
        diagnostics.push(Diagnostic {
            file: None,
            line: None,
            column: None,
            severity: Severity::Error,
            message: message.to_string(),
            ..Default::default()
        });
    }

    // A failed LaTeX run can still leave a usable PDF. Only a fresh one counts.
    let pdf = output_directory.join(format!("{stem}.pdf"));
    let is_fresh = std::fs::metadata(&pdf)
        .and_then(|metadata| metadata.modified())
        .is_ok_and(|modified| modified >= started_at);
    let synctex = output_directory.join(format!("{stem}.synctex.gz"));
    let usable = pdf.is_file() && (run.success || is_fresh);
    Some(BuildOutcome {
        success: run.success,
        last_good_pdf: (pdf.is_file() && !usable).then(|| pdf.clone()),
        pdf: usable.then_some(pdf),
        synctex: (request.engine.has_synctex() && synctex.is_file()).then_some(synctex),
        diagnostics,
        duration: started.elapsed(),
    })
}

fn written_since(path: &Path, since: SystemTime) -> bool {
    std::fs::metadata(path).and_then(|metadata| metadata.modified()).is_ok_and(|modified| modified >= since)
}

/// A warning when BibTeX found problems in the bibliography that it worked around.
fn bibliography_problems(output_directory: &Path, stem: &str) -> Option<Diagnostic> {
    let blg = std::fs::read(output_directory.join(format!("{stem}.blg"))).ok()?;
    let blg = String::from_utf8_lossy(&blg);
    let count = blg.lines().find_map(|line| line.trim().strip_prefix("(There were ")?.strip_suffix(" error messages)")?.parse::<usize>().ok()).or_else(|| blg.contains("(There was 1 error message)").then_some(1))?;
    let example = blg.lines().find(|line| line.contains("is a missing field") || line.starts_with("I was expecting") || line.starts_with("Repeated entry")).map(str::trim);
    let mut message = format!("BibTeX found {count} problems in the bibliography and skipped over them, so the PDF is complete.");
    if let Some(example) = example {
        message.push_str(&format!(" The first: {example}"));
    }
    Some(Diagnostic {
        file: Some(output_directory.join(format!("{stem}.blg"))),
        line: None,
        column: None,
        severity: Severity::Warning,
        message,
        ..Default::default()
    })
}

/// Whether the last LaTeX run left work that only latexmk does: BibTeX or
/// Biber for the bibliography, or makeindex for an index.
fn needs_latexmk(cwd: &Path, output_directory: &Path, stem: &str) -> bool {
    let log = std::fs::read(output_directory.join(format!("{stem}.log"))).unwrap_or_default();
    let log = String::from_utf8_lossy(&log);
    let asks = ["Please (re)run Biber", "Please (re)run BibTeX", "Please rerun BibTeX", "Please rerun Biber"];
    // A new \cite that BibTeX has not seen yet shows up as undefined.
    let new_citation = log.contains("Citation `") && log.contains("undefined") && log.contains(&format!("{stem}.bbl"));
    if asks.iter().any(|ask| log.contains(ask)) || log.contains(&format!("No file {stem}.bbl")) || new_citation {
        return true;
    }
    if output_directory.join(format!("{stem}.idx")).is_file() || output_directory.join(format!("{stem}.glo")).is_file() {
        return true;
    }
    // An edited .bib file changes the bibliography without LaTeX noticing.
    let modified = |path: &Path| std::fs::metadata(path).and_then(|metadata| metadata.modified()).ok();
    let Some(bbl) = modified(&output_directory.join(format!("{stem}.bbl"))) else {
        return false;
    };
    std::fs::read_dir(cwd)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "bib"))
        .any(|bib| modified(&bib).is_some_and(|bib| bib > bbl))
}

/// Picks the most useful line. latexmk prints a summary after its other output.
fn failure_summary(stderr: &str) -> &str {
    let mut lines = stderr.lines();
    let summary = stderr
        .lines()
        .position(|line| line.starts_with("Collected error summary"))
        .and_then(|position| stderr.lines().nth(position + 1));
    summary
        .map(str::trim)
        .or_else(|| lines.find(|line| !line.trim().is_empty()))
        .unwrap_or("The build failed. See the log.")
}

fn failure(message: String, started: Instant) -> BuildOutcome {
    BuildOutcome {
        success: false,
        pdf: None,
        synctex: None,
        last_good_pdf: None,
        diagnostics: vec![Diagnostic {
            file: None,
            line: None,
            column: None,
            severity: Severity::Error,
            message,
            ..Default::default()
        }],
        duration: started.elapsed(),
    }
}

/// `latexmk` lets latexmk run LaTeX, when it is there, instead of LaTeX itself.
fn invocation(engine: Engine, input: &str, stem: &str, format: Option<&str>, latexmk: bool, font_map: bool) -> Invocation {
    let strings = |args: &[&str]| args.iter().map(ToString::to_string).collect::<Vec<_>>();
    let pdf = format!("{OUTPUT_DIRECTORY}/{stem}.pdf");
    match engine {
        Engine::Typst => Invocation {
            tool: "typst",
            args: strings(&["compile", "--root", ".", input, &pdf]),
            env: Vec::new(),
        },
        Engine::Tectonic => Invocation {
            tool: "tectonic",
            args: strings(&["--untrusted", "--synctex", "--keep-logs", "--outdir", OUTPUT_DIRECTORY, input]),
            env: Vec::new(),
        },
        Engine::Pandoc => Invocation {
            tool: "pandoc",
            args: strings(&[input, "-o", &pdf, "--pdf-engine=typst"]),
            env: Vec::new(),
        },
        Engine::PdfLatex | Engine::LuaLatex => latex_invocation(engine, input, format, latexmk, font_map),
    }
}

/// Builds the format if the preamble has earned one. `None` inside means a normal run,
/// and only a cancelled build ends up as `None` outside.
async fn prepare_format(
    cwd: &Path,
    output_directory: &Path,
    main: &Path,
    input: &str,
    events: &mpsc::Sender<BuildEvent>,
    cancelled: &mut oneshot::Receiver<()>,
    group: &AtomicI32,
) -> Option<Option<Preamble>> {
    let (Some(preamble), Some(compiler)) = (Preamble::of(main, output_directory), find_tool("pdflatex")) else {
        return Some(None);
    };
    let state = preamble.state();
    // Only this preamble's format is of use. A new preamble may still be undone,
    // so the old format stays until the new one settles.
    if state != State::New {
        preamble.remove_others();
    }
    match state {
        State::New => {
            preamble.mark_seen();
            Some(None)
        }
        State::Unsuitable => Some(None),
        State::Untested | State::Trusted => Some(Some(preamble)),
        State::Wanted => {
            let name = preamble.name();
            let args = [
                "-ini",
                "-interaction=nonstopmode",
                "-no-shell-escape",
                "-recorder",
                &format!("-output-directory={OUTPUT_DIRECTORY}"),
                &format!("-jobname={name}"),
                "&pdflatex",
                // Kept in the format, so every build with it compresses less.
                PDF_COMPRESSION,
                input,
            ];
            let build = Invocation {
                tool: "pdflatex",
                args: args.iter().map(ToString::to_string).collect(),
                env: Vec::new(),
            };
            let started = SystemTime::now();
            let run = run_process(&build, cwd, events, cancelled, group).await?;
            if run.success && preamble.finish_format(cwd, main, &compiler, started) {
                Some(Some(preamble))
            } else {
                preamble.mark_unsuitable();
                Some(None)
            }
        }
    }
}

fn latex_invocation(engine: Engine, input: &str, format: Option<&str>, latexmk: bool, font_map: bool) -> Invocation {
    // A long line limit keeps file names and messages in the log on one line.
    // The limit is set per program, because BibTeX aborts on a value this high.
    let mut env = vec![("max_print_line_pdflatex", "100000".to_string()), ("max_print_line_lualatex", "100000".to_string())];
    if format.is_some() {
        // The trailing colon keeps the normal search path.
        env.push(("TEXFORMATS", format!("{OUTPUT_DIRECTORY}:")));
    }
    let font_map = font_map && engine == Engine::PdfLatex;
    if font_map {
        env.push(("TEXFONTMAPS", format!("{OUTPUT_DIRECTORY}:")));
    }
    let common = ["-interaction=nonstopmode", "-file-line-error", "-synctex=1", "-no-shell-escape"];
    if latexmk && find_tool("latexmk").is_some() {
        // -norc ignores latexmkrc files, which can run arbitrary commands from a project.
        let mode = if engine == Engine::LuaLatex { "-lualatex" } else { "-pdf" };
        // -f carries on past BibTeX's complaints, such as a missing field in
        // one entry, which otherwise stop latexmk before LaTeX reads the
        // bibliography. LaTeX's own log still decides whether the build worked.
        let mut args = vec!["-norc".to_string(), "-f".to_string(), mode.to_string()];
        args.extend(common.iter().map(ToString::to_string));
        if let Some(format) = format {
            args.push(format!("-pdflatex=pdflatex %O \"&{format}\" %S"));
        }
        args.push(format!("-outdir={OUTPUT_DIRECTORY}"));
        args.push(input.to_string());
        return Invocation { tool: "latexmk", args, env };
    }
    let mut args: Vec<String> = common.iter().map(ToString::to_string).collect();
    // The list of files read, for the fonts of the project's own font map.
    args.push("-recorder".to_string());
    if font_map {
        // A font the map lacks ends the run, which then repeats with a
        // larger map, instead of becoming a slow and blurry bitmap.
        args.push("-no-mktex=pk".to_string());
    }
    args.push(format!("-output-directory={OUTPUT_DIRECTORY}"));
    if let Some(format) = format {
        args.push(format!("&{format}"));
    }
    args.push(input.to_string());
    Invocation {
        tool: if engine == Engine::LuaLatex { "lualatex" } else { "pdflatex" },
        args,
        env,
    }
}

/// Walks the output directory, because `\\include` mirrors the source folders there.
fn remove_latex_state(directory: &Path) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            remove_latex_state(&path);
        } else {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if LATEX_STATE_EXTENSIONS.iter().any(|extension| name.ends_with(&format!(".{extension}"))) {
                std::fs::remove_file(path).ok();
            }
        }
    }
}

fn mirror_directories(source: &Path, target: &Path) {
    let Ok(entries) = std::fs::read_dir(source) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        if name.to_string_lossy().starts_with('.') {
            continue;
        }
        if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            let child = target.join(&name);
            if std::fs::create_dir_all(&child).is_ok() {
                mirror_directories(&entry.path(), &child);
            }
        }
    }
}

/// Runs the tool in its own process group, so that cancelling also ends the
/// programs it started. Returns `None` when cancelled.
async fn run_process(
    invocation: &Invocation,
    cwd: &Path,
    events: &mpsc::Sender<BuildEvent>,
    cancelled: &mut oneshot::Receiver<()>,
    running_group: &AtomicI32,
) -> Option<RunResult> {
    let program = find_tool(invocation.tool)?;
    let mut command = Command::new(program);
    command
        .args(&invocation.args)
        .current_dir(cwd)
        .env("PATH", child_path())
        .envs(invocation.env.iter().map(|(name, value)| (*name, value.as_str())))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .kill_on_drop(true);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            return Some(RunResult {
                success: false,
                stderr: format!("Cannot start {}: {error}", invocation.tool),
            });
        }
    };
    let group = child.id().map(|id| id as libc::pid_t);
    running_group.store(group.unwrap_or(0), Ordering::SeqCst);
    let stdout = child.stdout.take().expect("stdout is piped");
    let stderr = child.stderr.take().expect("stderr is piped");
    let out_task = tokio::spawn(forward_lines(stdout, events.clone()));
    let err_task = tokio::spawn(forward_lines(stderr, events.clone()));

    let (out_abort, err_abort) = (out_task.abort_handle(), err_task.abort_handle());
    let status = tokio::select! {
        status = child.wait() => status,
        _ = &mut *cancelled => {
            stop_group(group, &mut child).await;
            out_abort.abort();
            err_abort.abort();
            running_group.store(0, Ordering::SeqCst);
            return None;
        }
        _ = tokio::time::sleep(RUN_TIME_LIMIT) => {
            stop_group(group, &mut child).await;
            out_abort.abort();
            err_abort.abort();
            running_group.store(0, Ordering::SeqCst);
            return Some(RunResult {
                success: false,
                stderr: format!("{} ran for {} minutes and was stopped.", invocation.tool, RUN_TIME_LIMIT.as_secs() / 60),
            });
        }
    };
    // A program left behind can hold the pipes open for as long as it likes, and
    // waiting for it would block every later build.
    let drained = async {
        out_task.await.ok();
        err_task.await.unwrap_or_default()
    };
    let stderr = match tokio::time::timeout(DRAIN_GRACE, drained).await {
        Ok(stderr) => stderr,
        Err(_) => {
            signal_group(group, libc::SIGKILL);
            out_abort.abort();
            err_abort.abort();
            String::new()
        }
    };
    running_group.store(0, Ordering::SeqCst);
    Some(RunResult {
        success: status.is_ok_and(|status| status.success()),
        stderr,
    })
}

async fn stop_group(group: Option<libc::pid_t>, child: &mut tokio::process::Child) {
    signal_group(group, libc::SIGTERM);
    if tokio::time::timeout(KILL_GRACE, child.wait()).await.is_err() {
        signal_group(group, libc::SIGKILL);
        child.wait().await.ok();
    }
}

fn signal_group(group: Option<libc::pid_t>, signal: libc::c_int) {
    if let Some(group) = group {
        // SAFETY: killpg only reads its two integer arguments.
        unsafe {
            libc::killpg(group, signal);
        }
    }
}

async fn forward_lines(
    stream: impl AsyncRead + Unpin,
    events: mpsc::Sender<BuildEvent>,
) -> String {
    let mut reader = BufReader::new(stream);
    let mut collected = String::new();
    let mut bytes = Vec::new();
    loop {
        bytes.clear();
        match (&mut reader).take(MAX_LINE_BYTES).read_until(b'\n', &mut bytes).await {
            Ok(0) | Err(_) => return collected,
            Ok(_) => {
                // TeX can print bytes that are not UTF-8.
                let line = String::from_utf8_lossy(&bytes);
                collected.push_str(&line);
                keep_tail(&mut collected);
                events.send(BuildEvent::Output(line.into_owned())).await.ok();
            }
        }
    }
}

/// Keeps the end of the output. Cutting only at twice the size keeps this cheap.
fn keep_tail(text: &mut String) {
    if text.len() > OUTPUT_TAIL_BYTES * 2 {
        let cut = text.ceil_char_boundary(text.len() - OUTPUT_TAIL_BYTES);
        text.drain(..cut);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn finish(mut receiver: mpsc::Receiver<BuildEvent>) -> BuildEvent {
        let mut last = None;
        while let Some(event) = receiver.recv().await {
            last = Some(event);
        }
        last.expect("a build sends events")
    }

    fn block_on<T>(future: impl Future<Output = T>) -> T {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(future)
    }

    #[test]
    fn a_program_left_behind_does_not_block_the_build() {
        let invocation = Invocation {
            tool: "sh",
            args: vec!["-c".to_string(), "sleep 60 & echo done; exit 0".to_string()],
            env: Vec::new(),
        };
        let (events, mut receiver) = mpsc::channel(EVENT_CAPACITY);
        let (_cancel, mut cancelled) = oneshot::channel();
        let group = AtomicI32::new(0);
        let directory = tempfile::tempdir().unwrap();
        let started = std::time::Instant::now();
        let result = block_on(run_process(&invocation, directory.path(), &events, &mut cancelled, &group)).unwrap();
        assert!(result.success);
        assert!(started.elapsed() < Duration::from_secs(20));
        assert_eq!(group.load(Ordering::SeqCst), 0);
        receiver.close();
    }

    #[test]
    fn output_keeps_only_its_end() {
        let mut text = "ä".repeat(OUTPUT_TAIL_BYTES * 2);
        text.push('z');
        keep_tail(&mut text);
        assert!(text.len() <= OUTPUT_TAIL_BYTES + 1);
        assert!(text.ends_with('z'));
    }

    #[test]
    fn typst_build_produces_pdf_then_reports_errors() {
        if find_tool("typst").is_none() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let main = root.join("main.typ");
        std::fs::write(&main, "= Hello\n").unwrap();
        let manager = BuildManager::new();
        let request = BuildRequest { root: root.clone(), main: main.clone(), engine: Engine::Typst, clean: false };

        let BuildEvent::Finished(outcome) = block_on(finish(manager.start(request.clone()))) else {
            panic!("the build did not finish");
        };
        assert!(outcome.success, "{:?}", outcome.diagnostics);
        assert!(outcome.pdf.is_some());

        std::fs::write(&main, "= Hello\n#foo(\n").unwrap();
        let BuildEvent::Finished(outcome) = block_on(finish(manager.start(request))) else {
            panic!("the build did not finish");
        };
        assert!(!outcome.success);
        assert_eq!(outcome.diagnostics[0].file.as_deref(), Some(main.as_path()));
    }

    #[test]
    fn bibliography_problems_become_one_warning() {
        let directory = tempfile::tempdir().unwrap();
        assert!(bibliography_problems(directory.path(), "main").is_none());
        let blg = "Database file #1: references.bib\n`journal' is a missing field, not a string, for entry a2020\nwhile executing---line 1733 of file style.bst\n(There were 44 error messages)\n";
        std::fs::write(directory.path().join("main.blg"), blg).unwrap();
        let warning = bibliography_problems(directory.path(), "main").unwrap();
        assert_eq!(warning.severity, Severity::Warning);
        assert!(warning.message.starts_with("BibTeX found 44 problems"), "{}", warning.message);
        assert!(warning.message.contains("for entry a2020"), "{}", warning.message);
        std::fs::write(directory.path().join("main.blg"), "Database file #1: references.bib\n").unwrap();
        assert!(bibliography_problems(directory.path(), "main").is_none());
    }

    #[test]
    fn failure_summary_prefers_latexmk_summary() {
        let stderr = "Rc files read:\nCollected error summary (may duplicate other messages):\n  bibtex main: failed\n";
        assert_eq!(failure_summary(stderr), "bibtex main: failed");
        assert_eq!(failure_summary("\nboom\n"), "boom");
        assert_eq!(failure_summary(""), "The build failed. See the log.");
    }

    #[test]
    fn missing_tool_is_a_diagnostic() {
        let outcome = failure("typst is not installed.".into(), Instant::now());
        assert_eq!(outcome.diagnostics[0].severity, Severity::Error);
        assert!(outcome.pdf.is_none());
    }

    fn build_file(name: &str, source: &str, engine: Engine, clean: bool, prepare: impl Fn(&Path)) -> BuildOutcome {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let main = root.join(name);
        std::fs::write(&main, source).unwrap();
        prepare(&root);
        let request = BuildRequest { root, main, engine, clean };
        let BuildEvent::Finished(outcome) = block_on(finish(BuildManager::new().start(request))) else {
            panic!("the build did not finish");
        };
        outcome
    }

    /// Runs real compilers, so that the explanations follow what they print.
    #[test]
    fn real_latex_errors_are_explained_and_located() {
        if find_tool("pdflatex").is_none() {
            return;
        }
        let head = "\\documentclass{article}\n\\begin{document}\n";
        let cases: &[(&str, &str, usize, Option<usize>)] = &[
            ("Hello \\fracc{1}{2}\n\\end{document}\n", "\\fracc", 3, Some(7)),
            ("$\\mathbb{R}$\n\\end{document}\n", "\\mathbb", 3, Some(2)),
            ("a_b\n\\end{document}\n", "math mode", 3, Some(2)),
            ("oops}\n\\end{document}\n", "closing }", 3, Some(5)),
            ("a & b\n\\end{document}\n", "&", 3, Some(3)),
            ("\\textbf{oops\n\\end{document}\n", "\\textbf", 3, None),
            ("\\begin{itemize}\n\\item a\n\\end{enumerate}\n\\end{document}\n", "\\begin{itemize}", 5, Some(15)),
            ("\\begin{foo}\na\n\\end{foo}\n\\end{document}\n", "environment foo", 3, Some(11)),
            ("hello\n", "\\end{document}", 3, None),
        ];
        for (body, title, line, column) in cases {
            let outcome = build_file("main.tex", &format!("{head}{body}"), Engine::PdfLatex, false, |_| {});
            assert!(!outcome.success, "{body}");
            let first = &outcome.diagnostics[0];
            let explanation = explain::explain(first).unwrap_or_else(|| panic!("no explanation: {first:?}"));
            assert!(explanation.title.contains(title), "{body}: {explanation:?}");
            assert_eq!(first.line, Some(*line), "{body}: {first:?}");
            assert_eq!(first.column, *column, "{body}: {first:?}");
            assert!(first.file.as_deref().is_some_and(|file| file.ends_with("main.tex")), "{first:?}");
        }
    }

    #[test]
    fn real_missing_package_points_at_usepackage() {
        if find_tool("pdflatex").is_none() {
            return;
        }
        let source = "\\documentclass{article}\n\\usepackage{nosuchpkg}\n\\begin{document}\nx\n\\end{document}\n";
        let outcome = build_file("main.tex", source, Engine::PdfLatex, false, |_| {});
        assert_eq!(outcome.diagnostics.iter().filter(|diagnostic| diagnostic.severity == Severity::Error).count(), 1);
        let first = &outcome.diagnostics[0];
        assert_eq!(first.line, Some(2), "{first:?}");
        assert!(explain::explain(first).unwrap().title.contains("package nosuchpkg"));
    }

    #[test]
    fn real_typst_errors_are_explained_and_located() {
        if find_tool("typst").is_none() {
            return;
        }
        let outcome = build_file("My Note.typ", "= Hi\n$foo$\n", Engine::Typst, false, |_| {});
        let first = &outcome.diagnostics[0];
        assert!(first.file.as_deref().is_some_and(|file| file.ends_with("My Note.typ")), "{first:?}");
        assert_eq!((first.line, first.column, first.length), (Some(2), Some(2), Some(3)));
        assert!(first.hint.is_some());
        assert!(explain::explain(first).unwrap().title.contains("foo"));
    }

    #[test]
    fn names_with_spaces_compile() {
        if find_tool("typst").is_some() {
            let outcome = build_file("My Note.typ", "= Hi\n", Engine::Typst, false, |_| {});
            assert!(outcome.success && outcome.pdf.is_some(), "{:?}", outcome.diagnostics);
        }
        if find_tool("pdflatex").is_some() {
            let source = "\\documentclass{article}\n\\begin{document}\nx\n\\end{document}\n";
            let outcome = build_file("my file.tex", source, Engine::PdfLatex, false, |_| {});
            assert!(outcome.success && outcome.pdf.is_some(), "{:?}", outcome.diagnostics);
        }
    }

    /// A broken .aux from an earlier run would stop a correct document for good.
    #[test]
    fn broken_latex_state_recovers_without_help() {
        if find_tool("pdflatex").is_none() {
            return;
        }
        let source = "\\documentclass{article}\n\\begin{document}\nx\n\\end{document}\n";
        let corrupt = |root: &Path| {
            std::fs::create_dir_all(root.join(".build")).unwrap();
            std::fs::write(root.join(".build/main.aux"), "\\relax\n\\brokencommand{\n").unwrap();
        };
        let outcome = build_file("main.tex", source, Engine::PdfLatex, false, corrupt);
        assert!(outcome.success, "{:?}", outcome.diagnostics);
    }

    fn latex_project_builds(root: &Path, main: &Path, times: usize) -> Vec<BuildOutcome> {
        let manager = BuildManager::new();
        let request = BuildRequest { root: root.to_path_buf(), main: main.to_path_buf(), engine: Engine::PdfLatex, clean: false };
        (0..times)
            .map(|_| match block_on(finish(manager.start(request.clone()))) {
                BuildEvent::Finished(outcome) => outcome,
                _ => panic!("the build did not finish"),
            })
            .collect()
    }

    fn format_files(root: &Path) -> usize {
        std::fs::read_dir(root.join(".build"))
            .unwrap()
            .flatten()
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".fmt"))
            .count()
    }

    #[test]
    fn a_repeated_preamble_gets_a_format_and_errors_stay_real() {
        if find_tool("pdflatex").is_none() || find_tool("kpsewhich").is_none() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let main = root.join("main.tex");
        std::fs::write(&main, "\\documentclass{article}\n\\begin{document}\nx\n\\end{document}\n").unwrap();

        let outcomes = latex_project_builds(&root, &main, 3);
        assert!(outcomes.iter().all(|outcome| outcome.success && outcome.pdf.is_some()));
        assert_eq!(format_files(&root), 1);

        // A typo in the text is reported as such and does not cost the format.
        std::fs::write(&main, "\\documentclass{article}\n\\begin{document}\n\\undefinedthing\n\\end{document}\n").unwrap();
        let outcome = latex_project_builds(&root, &main, 1).remove(0);
        assert!(!outcome.success);
        assert!(outcome.diagnostics.iter().any(|diagnostic| diagnostic.line == Some(3)), "{:?}", outcome.diagnostics);
        assert_eq!(format_files(&root), 1);
    }

    #[test]
    fn an_index_keeps_its_entries_with_a_format() {
        if find_tool("pdflatex").is_none() || find_tool("kpsewhich").is_none() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let main = root.join("main.tex");
        std::fs::write(&main, "\\documentclass{article}\n\\makeindex\n\\begin{document}\nx\\index{x}\n\\end{document}\n").unwrap();
        let outcomes = latex_project_builds(&root, &main, 3);
        assert!(outcomes.iter().all(|outcome| outcome.success));
        assert!(std::fs::read_to_string(root.join(".build/main.idx")).unwrap().contains("indexentry{x}"));
    }

    #[test]
    fn a_package_that_opens_a_file_keeps_it_with_a_format() {
        if find_tool("pdflatex").is_none() || find_tool("kpsewhich").is_none() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let main = root.join("main.tex");
        std::fs::write(root.join("local.sty"), "\\newwrite\\extra\n\\immediate\\openout\\extra=extra.txt\n").unwrap();
        std::fs::write(&main, "\\documentclass{article}\n\\usepackage{local}\n\\begin{document}\nx\\immediate\\write\\extra{hello}\n\\end{document}\n").unwrap();
        let outcomes = latex_project_builds(&root, &main, 3);
        assert!(outcomes.iter().all(|outcome| outcome.success));
        assert_eq!(format_files(&root), 1);
        assert!(std::fs::read_to_string(root.join(".build/extra.txt")).unwrap().contains("hello"));
    }

    #[test]
    fn failed_build_keeps_the_last_good_pdf() {
        if find_tool("typst").is_none() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let main = root.join("main.typ");
        std::fs::write(&main, "#foo(\n").unwrap();
        std::fs::create_dir_all(root.join(".build")).unwrap();
        std::fs::write(root.join(".build/main.pdf"), "old").unwrap();
        let request = BuildRequest { root, main, engine: Engine::Typst, clean: false };
        let BuildEvent::Finished(outcome) = block_on(finish(BuildManager::new().start(request))) else {
            panic!("the build did not finish");
        };
        assert!(outcome.pdf.is_none());
        let last_good = outcome.last_good_pdf.expect("the old PDF stays");
        assert_eq!(std::fs::read(last_good).unwrap(), b"old");
    }
}

#[cfg(test)]
mod speed {
    use super::*;

    const DOCUMENT: &str = "\\documentclass{article}\n\\usepackage{amsmath}\n\\usepackage{hyperref}\n\\title{Welcome}\n\\author{You}\n\\begin{document}\n\\maketitle\n\\section{How}\\label{s}\nText, see \\ref{s}. EDIT\n\\begin{equation} e^{i\\pi} + 1 = 0 \\end{equation}\n\\end{document}\n";

    /// Times a run of builds as when typing, for a look by hand:
    /// cargo test -p build speed -- --ignored --nocapture
    #[test]
    #[ignore]
    fn typing_with_pdflatex() {
        let directory = tempfile::tempdir().unwrap();
        let main = directory.path().join("main.tex");
        let manager = BuildManager::new();
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        for edit in 0..8 {
            std::fs::write(&main, DOCUMENT.replace("EDIT", &format!("edit {edit}"))).unwrap();
            let request = BuildRequest { root: directory.path().to_path_buf(), main: main.clone(), engine: Engine::PdfLatex, clean: false };
            let mut events = manager.start(request);
            let outcome = runtime.block_on(async {
                let mut outcome = None;
                while let Some(event) = events.recv().await {
                    if let BuildEvent::Finished(finished) = event {
                        outcome = Some(finished);
                    }
                }
                outcome.unwrap()
            });
            assert!(outcome.success, "{:?}", outcome.diagnostics);
            println!("edit {edit}: {:?}", outcome.duration);
        }
    }
}

#[cfg(test)]
mod direct_latex {
    use super::*;

    fn compile(manager: &BuildManager, main: &Path) -> BuildOutcome {
        let request = BuildRequest { root: main.parent().unwrap().to_path_buf(), main: main.to_path_buf(), engine: Engine::PdfLatex, clean: false };
        let mut events = manager.start(request);
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        runtime.block_on(async {
            let mut outcome = None;
            while let Some(event) = events.recv().await {
                if let BuildEvent::Finished(finished) = event {
                    outcome = Some(finished);
                }
            }
            outcome.unwrap()
        })
    }

    fn pdf_text(pdf: &Path) -> String {
        let output = std::process::Command::new("pdftotext").arg(pdf).arg("-").output().unwrap();
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    fn bitmap_fonts(pdf: &Path) -> bool {
        let output = std::process::Command::new("pdffonts").arg(pdf).output().unwrap();
        String::from_utf8_lossy(&output.stdout).contains("Type 3")
    }

    /// Needs MacTeX and poppler: cargo test -p build direct_latex -- --ignored
    #[test]
    #[ignore]
    fn a_new_font_family_while_typing_stays_a_real_font() {
        let directory = tempfile::tempdir().unwrap();
        let main = directory.path().join("main.tex");
        let manager = BuildManager::new();
        let body = |extra: &str| format!("\\documentclass{{article}}\n\\begin{{document}}\nText {extra}\n\\end{{document}}\n");
        for edit in 0..3 {
            std::fs::write(&main, body(&format!("edit {edit}"))).unwrap();
            assert!(compile(&manager, &main).success);
        }
        assert!(directory.path().join(".build/pdftex.map").is_file());
        // A family outside the map, without a change of the preamble.
        std::fs::write(&main, body("{\\usefont{U}{msb}{m}{n}AB}")).unwrap();
        let outcome = compile(&manager, &main);
        assert!(outcome.success, "{:?}", outcome.diagnostics);
        let pdf = outcome.pdf.unwrap();
        assert!(!bitmap_fonts(&pdf));
        assert!(std::fs::read_to_string(directory.path().join(".build/pdftex.map")).unwrap().contains("msbm10"));
    }

    #[test]
    #[ignore]
    fn citations_still_go_through_bibtex() {
        let directory = tempfile::tempdir().unwrap();
        let main = directory.path().join("main.tex");
        let bib = directory.path().join("refs.bib");
        std::fs::write(&bib, "@book{knuth, title={The TeXbook}, author={Knuth, Donald}, year={1984}}\n@book{lamport, title={LaTeX}, author={Lamport, Leslie}, year={1994}}\n").unwrap();
        let manager = BuildManager::new();
        let body = |cites: &str, text: &str| format!("\\documentclass{{article}}\n\\begin{{document}}\n{text} {cites}\n\\bibliographystyle{{plain}}\n\\bibliography{{refs}}\n\\end{{document}}\n");
        std::fs::write(&main, body("\\cite{knuth}", "First")).unwrap();
        let outcome = compile(&manager, &main);
        assert!(outcome.success);
        let text = pdf_text(outcome.pdf.as_ref().unwrap());
        assert!(text.contains("[1]") && text.contains("TeXbook"), "{text}");
        // Typing without a new citation keeps it.
        for edit in 0..3 {
            std::fs::write(&main, body("\\cite{knuth}", &format!("Edit {edit}"))).unwrap();
            let outcome = compile(&manager, &main);
            assert!(pdf_text(outcome.pdf.as_ref().unwrap()).contains("TeXbook"));
        }
        // A new citation runs BibTeX again.
        std::fs::write(&main, body("\\cite{knuth} \\cite{lamport}", "More")).unwrap();
        let text = pdf_text(compile(&manager, &main).pdf.as_ref().unwrap());
        assert!(text.contains("Lamport") && !text.contains("[?]"), "{text}");
        // So does an edited .bib file.
        std::thread::sleep(Duration::from_millis(1100));
        std::fs::write(&bib, "@book{knuth, title={The METAFONTbook}, author={Knuth, Donald}, year={1986}}\n@book{lamport, title={LaTeX}, author={Lamport, Leslie}, year={1994}}\n").unwrap();
        let text = pdf_text(compile(&manager, &main).pdf.as_ref().unwrap());
        assert!(text.contains("METAFONTbook"), "{text}");
    }

    #[test]
    #[ignore]
    fn biblatex_goes_through_biber() {
        let directory = tempfile::tempdir().unwrap();
        let main = directory.path().join("main.tex");
        std::fs::write(directory.path().join("refs.bib"), "@book{knuth, title={The TeXbook}, author={Knuth, Donald}, year={1984}}\n@book{lamport, title={LaTeX}, author={Lamport, Leslie}, year={1994}}\n").unwrap();
        let manager = BuildManager::new();
        let body = |cites: &str| format!("\\documentclass{{article}}\n\\usepackage{{biblatex}}\n\\addbibresource{{refs.bib}}\n\\begin{{document}}\n{cites}\n\\printbibliography\n\\end{{document}}\n");
        std::fs::write(&main, body("\\cite{knuth}")).unwrap();
        assert!(pdf_text(compile(&manager, &main).pdf.as_ref().unwrap()).contains("TeXbook"));
        std::fs::write(&main, body("\\cite{knuth} \\cite{lamport}")).unwrap();
        let text = pdf_text(compile(&manager, &main).pdf.as_ref().unwrap());
        assert!(text.contains("Lamport"), "{text}");
    }
}
