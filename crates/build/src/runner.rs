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
    explain,
    parse::{self, Diagnostic, Severity},
    tools::{child_path, find_tool},
};

const OUTPUT_DIRECTORY: &str = ".build";
const MAX_LATEX_RUNS: usize = 3;
const KILL_GRACE: Duration = Duration::from_millis(500);
/// How long the pipes may stay open after the compiler has ended.
const DRAIN_GRACE: Duration = Duration::from_secs(2);
const RUN_TIME_LIMIT: Duration = Duration::from_secs(15 * 60);
const EVENT_CAPACITY: usize = 1024;
const MAX_LINE_BYTES: u64 = 64 * 1024;
const OUTPUT_TAIL_BYTES: usize = 1024 * 1024;
/// Files LaTeX carries between runs. A failed or stopped run can leave them
/// broken, and the next run then fails on them. The PDF is not in this list.
const LATEX_STATE_EXTENSIONS: &[&str] = &[
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
    env: Vec<(&'static str, &'static str)>,
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
    let stem = request.main.file_stem().map_or_else(|| "main".into(), |stem| stem.to_string_lossy().into_owned());
    let input = match request.engine {
        Engine::Typst => request.main.strip_prefix(&request.root).unwrap_or(&request.main),
        _ => Path::new(request.main.file_name().unwrap_or_default()),
    }
    .to_string_lossy()
    .into_owned();

    let invocation = invocation(request.engine, &input, &stem);
    if find_tool(invocation.tool).is_none() {
        return Some(failure(format!("{} is not installed.", invocation.tool), started));
    }
    if matches!(request.engine, Engine::PdfLatex | Engine::LuaLatex) {
        // `\include{dir/file}` writes its .aux below the output directory.
        mirror_directories(&cwd, &output_directory);
    }

    let is_latex = matches!(request.engine, Engine::PdfLatex | Engine::LuaLatex);
    let log_path = output_directory.join(format!("{stem}.log"));
    let reruns_by_hand = invocation.tool == "pdflatex" || invocation.tool == "lualatex";
    let mut cleaned = request.clean;
    let (run, mut diagnostics) = loop {
        if is_latex && cleaned {
            remove_latex_state(&output_directory);
        }
        let mut runs = 0;
        let run = loop {
            runs += 1;
            let run = run_process(&invocation, &cwd, events, cancelled, group).await?;
            let rerun = reruns_by_hand
                && runs < MAX_LATEX_RUNS
                && std::fs::read_to_string(&log_path).is_ok_and(|log| log.contains("Rerun to get"));
            if !rerun {
                break run;
            }
        };
        let diagnostics = match request.engine {
            Engine::Typst => parse::parse_typst(&run.stderr, &cwd),
            Engine::Tectonic => parse::parse_tectonic(&run.stderr, &cwd),
            Engine::Pandoc => parse::parse_pandoc(&run.stderr, &cwd),
            Engine::PdfLatex | Engine::LuaLatex => {
                let log = std::fs::read(&log_path).unwrap_or_default();
                parse::parse_latex_log(&String::from_utf8_lossy(&log), &cwd)
            }
        };
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

fn invocation(engine: Engine, input: &str, stem: &str) -> Invocation {
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
        Engine::PdfLatex | Engine::LuaLatex => latex_invocation(engine, input),
    }
}

fn latex_invocation(engine: Engine, input: &str) -> Invocation {
    // A long line limit keeps file names and messages in the log on one line.
    // The limit is set per program, because BibTeX aborts on a value this high.
    let env = vec![("max_print_line_pdflatex", "100000"), ("max_print_line_lualatex", "100000")];
    let common = ["-interaction=nonstopmode", "-file-line-error", "-synctex=1", "-no-shell-escape"];
    if find_tool("latexmk").is_some() {
        // -norc ignores latexmkrc files, which can run arbitrary commands from a project.
        let mode = if engine == Engine::LuaLatex { "-lualatex" } else { "-pdf" };
        let mut args = vec!["-norc".to_string(), mode.to_string()];
        args.extend(common.iter().map(ToString::to_string));
        args.push(format!("-outdir={OUTPUT_DIRECTORY}"));
        args.push(input.to_string());
        return Invocation { tool: "latexmk", args, env };
    }
    let mut args: Vec<String> = common.iter().map(ToString::to_string).collect();
    args.push(format!("-output-directory={OUTPUT_DIRECTORY}"));
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
        .envs(invocation.env.iter().copied())
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
