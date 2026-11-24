//! A problem report that someone without any technical knowledge can send:
//! they say in their own words what happened, and the report adds what a
//! developer needs, such as the version, the tools and the last build. It
//! becomes a GitHub issue of the editor, filled in, in the browser. Nothing is
//! sent before they press the button on GitHub themselves.

use std::{
    fmt::Write as _,
    path::{Path, PathBuf},
};

const NEW_ISSUE: &str = "https://github.com/sudo-Oliver/Editor/issues/new";
/// GitHub refuses longer addresses; what does not fit goes by the clipboard.
const MAX_URL: usize = 7000;
const LOG_LINES: usize = 40;
const CRASH_FILE: &str = "last-crash.txt";

/// What the app knows that helps to find the cause.
#[derive(Clone, Debug, Default)]
pub struct Details {
    /// Programs and whether they are there, such as `("LaTeX", "found at …")`.
    pub tools: Vec<(String, String)>,
    /// Installs that failed, with what they said.
    pub install_errors: Vec<(String, String)>,
    /// The kind of the open file, such as `LaTeX`, and the compiler.
    pub document: Option<String>,
    pub build: Option<String>,
    /// The first errors of the last build, as the editor showed them.
    pub build_errors: Vec<String>,
    pub build_log: String,
    /// The last messages of the status bar, oldest first.
    pub messages: Vec<String>,
    pub collaboration: Option<String>,
    /// What the app wrote when it quit unexpectedly last time.
    pub crash: Option<String>,
}

pub struct Report {
    pub title: String,
    pub body: String,
}

impl Report {
    pub fn new(description: &str, details: Option<&Details>) -> Self {
        let description = description.trim();
        let first_line = description.lines().next().unwrap_or_default();
        let title = if first_line.is_empty() { "Problem report".to_string() } else { shorten(first_line, 80) };
        let mut body = String::new();
        body.push_str("### What happened\n\n");
        body.push_str(if description.is_empty() { "(No description.)" } else { description });
        body.push_str("\n\n");
        if let Some(details) = details {
            body.push_str(&details_section(details));
        }
        Self { title, body: redact_home(&body) }
    }

    /// The page that opens the issue, filled in. A body too long for an
    /// address is cut, with a note to paste the whole of it from the clipboard.
    pub fn url(&self) -> String {
        let base = format!("{NEW_ISSUE}?template=problem-report.md&title={}&body=", encode(&self.title));
        let full = encode(&self.body);
        if base.len() + full.len() <= MAX_URL {
            return base + &full;
        }
        let note = if cfg!(windows) { "\n\n**The whole report is on your clipboard. Click here, select this text and press Ctrl+V to put it in place.**\n" } else { "\n\n**The whole report is on your clipboard. Click here, select this text and press Cmd+V to put it in place.**\n" };
        let room = MAX_URL - base.len() - encode(note).len();
        // Each byte takes one char encoded, or three: cut at a char boundary.
        let mut used = 0;
        let end = self
            .body
            .char_indices()
            .find(|(_, ch)| {
                let mut buffer = [0; 4];
                used += encode(ch.encode_utf8(&mut buffer)).len();
                used > room
            })
            .map_or(self.body.len(), |(at, _)| at);
        base + &encode(&self.body[..end]) + &encode(note)
    }
}

fn details_section(details: &Details) -> String {
    let mut out = String::from("### Details\n\n");
    let _ = writeln!(out, "- Editor {} on {}", env!("CARGO_PKG_VERSION"), crate::platform::system());
    if let Some(document) = &details.document {
        let _ = writeln!(out, "- Open file: {document}");
    }
    if let Some(build) = &details.build {
        let _ = writeln!(out, "- Last build: {build}");
    }
    if let Some(collaboration) = &details.collaboration {
        let _ = writeln!(out, "- Live session: {collaboration}");
    }
    for (tool, state) in &details.tools {
        let _ = writeln!(out, "- {tool}: {state}");
    }
    for (tool, error) in &details.install_errors {
        let _ = writeln!(out, "- Installing {tool} failed: {error}");
    }
    let block = |out: &mut String, title: &str, text: &str| {
        let _ = write!(out, "\n<details><summary>{title}</summary>\n\n```\n{}\n```\n</details>\n", text.trim_end().replace("```", "'''"));
    };
    if let Some(crash) = &details.crash {
        block(&mut out, "Crash", crash);
    }
    if !details.build_errors.is_empty() {
        block(&mut out, "Build errors", &details.build_errors.join("\n"));
    }
    let log: Vec<&str> = details.build_log.lines().collect();
    if !log.is_empty() {
        block(&mut out, "End of the build log", &log[log.len().saturating_sub(LOG_LINES)..].join("\n"));
    }
    if !details.messages.is_empty() {
        block(&mut out, "Recent messages", &details.messages.join("\n"));
    }
    out
}

/// The user's name stays private: `/Users/ada/Thesis` becomes `~/Thesis`.
/// On Windows also as TeX writes it, `C:/Users/Ada` or `c:/users/ada`.
fn redact_home(text: &str) -> String {
    let Some(home) = crate::platform::home().and_then(|home| home.to_str().map(str::to_string)).filter(|home| home.len() > 1) else {
        return text.to_string();
    };
    if !cfg!(windows) {
        return text.replace(&home, "~");
    }
    let mut out = text.to_string();
    for form in [home.clone(), home.replace('\\', "/")] {
        out = replace_ignoring_case(&out, &form, "~");
    }
    out
}

/// Replaces `needle` in `text`, with ASCII letters of any case, as Windows
/// names paths.
fn replace_ignoring_case(text: &str, needle: &str, with: &str) -> String {
    let (lower, needle) = (text.to_ascii_lowercase(), needle.to_ascii_lowercase());
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for (at, _) in lower.match_indices(&needle) {
        if at >= last {
            out.push_str(&text[last..at]);
            out.push_str(with);
            last = at + needle.len();
        }
    }
    out.push_str(&text[last..]);
    out
}

fn shorten(text: &str, chars: usize) -> String {
    if text.chars().count() <= chars {
        return text.to_string();
    }
    let mut short: String = text.chars().take(chars - 1).collect();
    short.push('…');
    short
}

/// Percent-encoding for a query value.
fn encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(byte as char);
        } else {
            let _ = write!(out, "%{byte:02X}");
        }
    }
    out
}

fn crash_path() -> Option<PathBuf> {
    crate::storage::data_dir().map(|directory| directory.join(CRASH_FILE))
}

/// Keeps what a panic says, with a backtrace, for a report on the next start.
pub fn record_crashes() {
    let Some(path) = crash_path() else {
        return;
    };
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // A worker thread that panics leaves the app running, so it is no crash to report.
        if std::thread::current().name() != Some("main") {
            previous(info);
            return;
        }
        let backtrace = std::backtrace::Backtrace::force_capture();
        let text = format!("Editor {}\n{info}\n\n{backtrace}", env!("CARGO_PKG_VERSION"));
        if let Some(directory) = path.parent() {
            let _ = std::fs::create_dir_all(directory);
        }
        let _ = std::fs::write(&path, text);
        previous(info);
    }));
}

/// The crash of the last run, taken away so that it is offered once.
pub fn take_crash() -> Option<String> {
    let path = crash_path()?;
    let text = std::fs::read_to_string(&path).ok()?;
    let _ = std::fs::remove_file(&path);
    // A backtrace of thousands of frames helps no more than its start.
    Some(text.lines().take(80).collect::<Vec<_>>().join("\n"))
}

/// A path as a report shows it.
pub fn shown_path(path: &Path) -> String {
    redact_home(&path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_carry_the_details_and_hide_the_home_folder() {
        let home = crate::platform::home().unwrap().to_string_lossy().into_owned();
        let details = Details {
            tools: vec![("LaTeX".into(), format!("found at {home}/tex/pdflatex"))],
            build: Some("failed".into()),
            build_log: (0..100).map(|line| format!("line {line}\n")).collect(),
            ..Details::default()
        };
        let report = Report::new("The PDF stays empty\nafter I press Recompile", Some(&details));
        assert_eq!(report.title, "The PDF stays empty");
        assert!(report.body.contains("found at ~/tex/pdflatex"));
        assert!(!report.body.contains(&home));
        assert!(report.body.contains("line 99") && !report.body.contains("line 59\n"));
        assert!(Report::new("", None).title == "Problem report");
    }

    #[test]
    fn paths_match_in_any_case() {
        assert_eq!(replace_ignoring_case("C:/Users/Ada/x and c:/users/ada/y", "c:/users/ada", "~"), "~/x and ~/y");
    }

    #[test]
    fn long_reports_fit_an_address() {
        let details = Details { build_log: "x\n".repeat(LOG_LINES), messages: vec!["ä ".repeat(4000)], ..Details::default() };
        let url = Report::new("Ä problem", Some(&details)).url();
        assert!(url.len() <= MAX_URL);
        assert!(url.starts_with("https://github.com/sudo-Oliver/Editor/issues/new?template=problem-report.md&title=%C3%84%20problem&body="));
        assert!(url.contains("clipboard"));
        let short = Report::new("a&b", None).url();
        assert!(short.contains("title=a%26b") && !short.contains("clipboard"));
    }
}
