use std::path::{Component, Path, PathBuf};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Severity {
    #[default]
    Error,
    Warning,
}

/// One compiler message. `line` and `column` are one-based, and `column` and
/// `length` count characters.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Diagnostic {
    pub file: Option<PathBuf>,
    pub line: Option<usize>,
    pub column: Option<usize>,
    pub severity: Severity,
    pub message: String,
    pub length: Option<usize>,
    /// Advice from the compiler itself.
    pub hint: Option<String>,
    /// The source text TeX had read when it stopped.
    pub context: Option<String>,
}

/// Joins a compiler path onto the working directory and removes `.` parts.
fn resolve(cwd: &Path, file: &str) -> PathBuf {
    let joined = cwd.join(file);
    let mut result = PathBuf::new();
    for component in joined.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                result.pop();
            }
            other => result.push(other),
        }
    }
    result
}

/// Parses Typst's human format, the only one that keeps its hints and the
/// length of a span. Typst prints zero-based columns in the location line.
pub fn parse_typst(stderr: &str, cwd: &Path) -> Vec<Diagnostic> {
    let mut diagnostics: Vec<Diagnostic> = Vec::new();
    // Lines after `help:` describe the call stack, not the error.
    let mut collecting = false;
    for line in stderr.lines() {
        let severity = if let Some(message) = line.strip_prefix("error: ") {
            Some((Severity::Error, message))
        } else {
            line.strip_prefix("warning: ").map(|message| (Severity::Warning, message))
        };
        if let Some((severity, message)) = severity {
            diagnostics.push(Diagnostic { severity, message: message.to_string(), ..Default::default() });
            collecting = true;
            continue;
        }
        if line.starts_with("help:") {
            collecting = false;
        }
        let (true, Some(diagnostic)) = (collecting, diagnostics.last_mut()) else {
            continue;
        };
        let trimmed = line.trim_start();
        if let Some(location) = trimmed.strip_prefix("┌─ ") {
            let mut parts = location.rsplitn(3, ':');
            let column = parts.next().and_then(|part| part.parse::<usize>().ok());
            let number = parts.next().and_then(|part| part.parse().ok());
            diagnostic.file = parts.next().map(|file| resolve(cwd, file));
            diagnostic.line = number;
            diagnostic.column = column.map(|column| column + 1);
        } else if let Some(hint) = trimmed.strip_prefix("= hint: ") {
            match &mut diagnostic.hint {
                Some(existing) => {
                    existing.push('\n');
                    existing.push_str(hint);
                }
                None => diagnostic.hint = Some(hint.to_string()),
            }
        } else if diagnostic.length.is_none()
            && let Some(marker) = trimmed.strip_prefix('│')
            && marker.contains('^')
            && marker.chars().all(|ch| ch == ' ' || ch == '^')
        {
            diagnostic.length = Some(marker.matches('^').count());
        }
    }
    diagnostics
}

/// Parses Tectonic's `severity: file:line: message`. Summary lines that repeat
/// a failure without a location are dropped.
pub fn parse_tectonic(stderr: &str, cwd: &Path) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for line in stderr.lines() {
        let (severity, rest) = if let Some(rest) = line.strip_prefix("error: ") {
            (Severity::Error, rest)
        } else if let Some(rest) = line.strip_prefix("warning: ") {
            (Severity::Warning, rest)
        } else {
            continue;
        };
        if rest.starts_with("something bad happened") || rest.starts_with("the XeTeX engine") {
            continue;
        }
        let located = rest.split_once(':').and_then(|(file, tail)| {
            let (number, message) = tail.split_once(": ")?;
            Some((file, number.parse::<usize>().ok()?, message))
        });
        diagnostics.push(match located {
            Some((file, number, message)) => Diagnostic {
                file: Some(resolve(cwd, file)),
                line: Some(number),
                column: None,
                severity,
                message: message.to_string(),
                ..Default::default()
            },
            None => Diagnostic {
                file: None,
                line: None,
                column: None,
                severity,
                message: rest.to_string(),
                ..Default::default()
            },
        });
    }
    diagnostics
}

/// Parses a TeX `.log` written with `-file-line-error` and a long
/// `max_print_line`. Warnings do not name their file, so a stack of the opened
/// files tracks it.
pub fn parse_latex_log(log: &str, cwd: &Path) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let mut files: Vec<Option<String>> = Vec::new();
    let lines: Vec<&str> = log.lines().collect();
    let mut index = 0;
    while index < lines.len() {
        let line = lines[index];
        index += 1;

        if let Some((file, number, message)) = file_line_error(line) {
            diagnostics.push(Diagnostic {
                file: Some(resolve(cwd, file)),
                line: Some(number),
                column: None,
                severity: Severity::Error,
                message: message.to_string(),
                context: tex_context(&lines[index..]).map(|(_, context)| context),
                ..Default::default()
            });
            continue;
        }
        if let Some(message) = line.strip_prefix("! ") {
            let context = tex_context(&lines[index..]);
            diagnostics.push(Diagnostic {
                file: current_file(&files).map(|file| resolve(cwd, file)),
                line: context.as_ref().and_then(|(number, _)| *number),
                column: None,
                severity: Severity::Error,
                message: message.to_string(),
                context: context.map(|(_, context)| context),
                ..Default::default()
            });
            continue;
        }
        if line.contains("Warning: ") && (line.starts_with("LaTeX ") || line.starts_with("Package ") || line.starts_with("Class ")) {
            let message = line.split_once("Warning: ").map_or(line, |(_, message)| message);
            let number = number_after(line, "on input line ");
            diagnostics.push(Diagnostic {
                file: number.and(current_file(&files)).map(|file| resolve(cwd, file)),
                line: number,
                column: None,
                severity: Severity::Warning,
                message: message.trim_end_matches('.').to_string(),
                ..Default::default()
            });
            continue;
        }
        if line.starts_with("Overfull \\") || line.starts_with("Underfull \\") {
            let number = number_after(line, "at lines ").or_else(|| number_after(line, "at line "));
            diagnostics.push(Diagnostic {
                file: number.and(current_file(&files)).map(|file| resolve(cwd, file)),
                line: number,
                column: None,
                severity: Severity::Warning,
                message: line.split(" in paragraph").next().unwrap_or(line).split(" detected").next().unwrap_or(line).to_string(),
                ..Default::default()
            });
            continue;
        }
        track_files(line, &mut files);
    }
    drop_consequences(&mut diagnostics);
    diagnostics
}

/// TeX reports that it stopped after the error that stopped it. Only the cause helps.
fn drop_consequences(diagnostics: &mut Vec<Diagnostic>) {
    let is_consequence = |diagnostic: &Diagnostic| {
        diagnostic.message.trim_start().starts_with("==> Fatal error occurred")
            || diagnostic.message.starts_with("Emergency stop")
    };
    let has_cause = diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity == Severity::Error && !is_consequence(diagnostic));
    if has_cause {
        diagnostics.retain(|diagnostic| !is_consequence(diagnostic));
    } else {
        diagnostics.retain(|diagnostic| !diagnostic.message.trim_start().starts_with("==> Fatal error occurred"));
    }
}

/// Reads what TeX prints after an error: `l.12 text it had read`, or a note
/// when the error has no line.
fn tex_context(lines: &[&str]) -> Option<(Option<usize>, String)> {
    for line in lines.iter().take(30) {
        if line.starts_with("! ") || file_line_error(line).is_some() {
            return None;
        }
        if let Some(note) = line.strip_prefix("*** (") {
            return Some((None, note.trim_end_matches(')').to_string()));
        }
        let Some(rest) = line.strip_prefix("l.") else {
            continue;
        };
        let (number, context) = rest.split_once(' ').unwrap_or((rest, ""));
        if let Ok(number) = number.parse() {
            return Some((Some(number), context.to_string()));
        }
    }
    None
}

fn current_file(files: &[Option<String>]) -> Option<&str> {
    files.iter().rev().find_map(|file| file.as_deref())
}

/// Whether `text` starts with a path as TeX prints one: relative, absolute,
/// or with a Windows drive, as in `C:/Users/…` or `C:\Users\…`.
fn starts_path(text: &str) -> bool {
    let bytes = text.as_bytes();
    let drive = bytes.len() > 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && matches!(bytes[2], b'/' | b'\\');
    drive || ["./", "/", "../", ".\\", "..\\"].iter().any(|start| text.starts_with(start))
}

fn file_line_error(line: &str) -> Option<(&str, usize, &str)> {
    if !starts_path(line) {
        return None;
    }
    // The colon after a drive letter is part of the path.
    let skip = if line.as_bytes().get(1) == Some(&b':') { 2 } else { 0 };
    let colon = skip + line[skip..].find(':')?;
    let (file, rest) = (&line[..colon], &line[colon + 1..]);
    let (number, message) = rest.split_once(": ")?;
    Some((file, number.parse().ok()?, message))
}

fn number_after(line: &str, marker: &str) -> Option<usize> {
    let (_, tail) = line.split_once(marker)?;
    let digits: String = tail.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// Follows the `(file` and `)` markers TeX prints while it opens and closes files.
fn track_files(line: &str, files: &mut Vec<Option<String>>) {
    let mut rest = line;
    while let Some(position) = rest.find(['(', ')']) {
        if rest.as_bytes()[position] == b')' {
            files.pop();
            rest = &rest[position + 1..];
            continue;
        }
        let after = &rest[position + 1..];
        let end = after.find([' ', '(', ')', '[', '{', '<']).unwrap_or(after.len());
        let token = &after[..end];
        let is_file = starts_path(token);
        files.push(is_file.then(|| token.to_string()));
        rest = &after[end..];
    }
}

/// Parses Pandoc messages. Pandoc has no fixed format, so this is best effort.
pub fn parse_pandoc(stderr: &str, cwd: &Path) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for line in stderr.lines() {
        if let Some(message) = line.strip_prefix("[WARNING] ") {
            let number = number_after(message, " line ");
            let column = number_after(message, " column ");
            let file = message
                .split_once(" at ")
                .and_then(|(_, tail)| tail.split_once(" line "))
                .map(|(file, _)| resolve(cwd, file));
            diagnostics.push(Diagnostic {
                file: number.and(file),
                line: number,
                column,
                severity: Severity::Warning,
                message: message.to_string(),
                ..Default::default()
            });
        } else if let Some((file, number, column, message)) = pandoc_location(line) {
            diagnostics.push(Diagnostic {
                file: Some(resolve(cwd, file)),
                line: Some(number),
                column: Some(column),
                severity: Severity::Error,
                message: message.to_string(),
                ..Default::default()
            });
        }
    }
    diagnostics
}

fn pandoc_location(line: &str) -> Option<(&str, usize, usize, &str)> {
    let (location, message) = line.split_once(": ")?;
    let mut parts = location.rsplitn(3, ':');
    let column = parts.next()?.parse().ok()?;
    let number = parts.next()?.parse().ok()?;
    Some((parts.next()?, number, column, message))
}

#[cfg(test)]
mod tests {
    #[test]
    fn windows_paths_in_the_log_are_files() {
        assert_eq!(file_line_error("C:/Users/Ada/Thesis/main.tex:12: Undefined control sequence."), Some(("C:/Users/Ada/Thesis/main.tex", 12, "Undefined control sequence.")));
        assert_eq!(file_line_error(r"D:\Arbeit\kapitel.tex:3: Missing $ inserted."), Some((r"D:\Arbeit\kapitel.tex", 3, "Missing $ inserted.")));
        assert_eq!(file_line_error("./main.tex:7: Oops."), Some(("./main.tex", 7, "Oops.")));
        let mut files = Vec::new();
        track_files("(C:/texlive/2026/texmf-dist/tex/latex/base/article.cls", &mut files);
        assert_eq!(files, [Some("C:/texlive/2026/texmf-dist/tex/latex/base/article.cls".to_string())]);
    }

    use super::*;

    fn cwd() -> &'static Path {
        Path::new("/project")
    }

    #[test]
    fn typst_fixture() {
        let diagnostics = parse_typst(include_str!("../tests/fixtures/typst.stderr"), cwd());
        assert_eq!(diagnostics.len(), 3);
        assert_eq!(diagnostics[0].file.as_deref(), Some(Path::new("/project/main.typ")));
        assert_eq!((diagnostics[0].line, diagnostics[0].column), (Some(6), Some(21)));
        assert_eq!(diagnostics[0].length, Some(1));
        assert_eq!(diagnostics[0].message, "unclosed delimiter");
        assert_eq!(diagnostics[0].severity, Severity::Error);

        // The file name has a space, and the `help:` block must not move the location.
        assert_eq!(diagnostics[1].file.as_deref(), Some(Path::new("/project/sub/My Note.typ")));
        assert_eq!(diagnostics[1].line, Some(2));
        assert_eq!(diagnostics[1].length, Some(3));
        assert_eq!(diagnostics[1].hint.as_deref().unwrap().lines().count(), 2);

        assert_eq!(diagnostics[2].severity, Severity::Warning);
    }

    #[test]
    fn tectonic_fixture_keeps_only_the_located_error() {
        let diagnostics = parse_tectonic(include_str!("../tests/fixtures/tectonic.stderr"), cwd());
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].line, Some(3));
        assert_eq!(diagnostics[0].message, "Undefined control sequence");
    }

    #[test]
    fn latex_fixture_finds_errors_warnings_and_included_files() {
        let diagnostics = parse_latex_log(include_str!("../tests/fixtures/latex.log"), cwd());
        let find = |text: &str| {
            diagnostics
                .iter()
                .find(|diagnostic| diagnostic.message.contains(text))
                .unwrap_or_else(|| panic!("missing {text}: {diagnostics:#?}"))
        };
        let undefined = find("Undefined control sequence");
        assert_eq!(undefined.file.as_deref(), Some(Path::new("/project/main.tex")));
        assert_eq!(undefined.line, Some(3));

        let included = diagnostics
            .iter()
            .find(|diagnostic| diagnostic.file.as_deref() == Some(Path::new("/project/sub/part.tex")))
            .expect("error in included file");
        assert_eq!(included.line, Some(1));

        let reference = find("Reference `missing'");
        assert_eq!(reference.severity, Severity::Warning);
        assert_eq!(reference.file.as_deref(), Some(Path::new("/project/main.tex")));
        assert_eq!(reference.line, Some(5));

        let overfull = find("Overfull");
        assert_eq!(overfull.file.as_deref(), Some(Path::new("/project/main.tex")));
        assert_eq!(overfull.line, Some(6));

        assert_eq!(find("There were undefined references").line, None);
    }

    #[test]
    fn pandoc_fixture() {
        let diagnostics = parse_pandoc(include_str!("../tests/fixtures/pandoc.stderr"), cwd());
        assert_eq!(diagnostics.len(), 2);
        assert_eq!(diagnostics[0].severity, Severity::Warning);
        assert_eq!(diagnostics[0].file.as_deref(), Some(Path::new("/project/in.md")));
        assert_eq!((diagnostics[0].line, diagnostics[0].column), (Some(5), Some(1)));
        assert_eq!(diagnostics[1].file.as_deref(), Some(Path::new("/project/three.md")));
        assert_eq!((diagnostics[1].line, diagnostics[1].column), (Some(2), Some(6)));
    }

    #[test]
    fn stopping_is_dropped_when_the_cause_is_known() {
        let log = "! LaTeX Error: File `x.sty' not found.\n./a.tex:3: Emergency stop.\n./a.tex:3:  ==> Fatal error occurred, no output PDF file produced!\n";
        let diagnostics = parse_latex_log(log, cwd());
        assert_eq!(diagnostics.len(), 1);
        assert!(diagnostics[0].message.contains("x.sty"));
    }

    #[test]
    fn tex_context_is_kept() {
        let log = "./a.tex:3: Undefined control sequence.\nl.3 Hello \\fracc\n                {1}\n";
        assert_eq!(parse_latex_log(log, cwd())[0].context.as_deref(), Some("Hello \\fracc"));
    }
}
