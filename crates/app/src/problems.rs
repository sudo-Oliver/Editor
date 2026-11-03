use std::path::Path;

use build::{Diagnostic, Severity};

/// Characters of source shown around the error.
const EXCERPT_WIDTH: usize = 56;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExcerptLine {
    pub number: usize,
    pub text: String,
    pub target: bool,
}

/// What the error panel shows for one diagnostic.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problem {
    pub title: String,
    pub hint: Option<String>,
    /// The compiler's own words, when `title` is an explanation of them.
    pub compiler_message: Option<String>,
    pub location: Option<String>,
    pub excerpt: Vec<ExcerptLine>,
    /// Characters before the marker and the marker's width, within the target line.
    pub marker: Option<(usize, usize)>,
}

/// Errors come first, in the order the compiler found them. Only the first
/// error gets a source excerpt, because the panel lists the others compactly.
pub fn problems(diagnostics: &[Diagnostic]) -> Vec<Problem> {
    diagnostics
        .iter()
        .enumerate()
        .map(|(index, diagnostic)| {
            let explanation = build::explain(diagnostic);
            let hint = [explanation.as_ref().map(|explanation| explanation.hint.clone()), diagnostic.hint.clone()]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join("\n");
            let mut problem = Problem {
                compiler_message: explanation.as_ref().map(|_| diagnostic.message.clone()),
                title: explanation.map_or_else(|| diagnostic.message.clone(), |explanation| explanation.title),
                hint: (!hint.is_empty()).then_some(hint),
                location: location(diagnostic),
                excerpt: Vec::new(),
                marker: None,
            };
            if index == 0
                && diagnostic.severity == Severity::Error
                && let (Some(file), Some(line)) = (&diagnostic.file, diagnostic.line)
                && let Some(source) = read(file)
            {
                (problem.excerpt, problem.marker) = excerpt(&source, line, diagnostic.column, diagnostic.length);
            }
            problem
        })
        .collect()
}

fn read(file: &Path) -> Option<String> {
    std::fs::read(file).ok().map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
}

fn location(diagnostic: &Diagnostic) -> Option<String> {
    let name = diagnostic.file.as_ref()?.file_name()?.to_string_lossy().into_owned();
    Some(match diagnostic.line {
        Some(line) => format!("{name}, line {line}"),
        None => name,
    })
}

/// Cuts the lines around `line` to one window, so that long paragraphs still
/// show the error. `line` and `column` are one-based.
fn excerpt(
    source: &str,
    line: usize,
    column: Option<usize>,
    length: Option<usize>,
) -> (Vec<ExcerptLine>, Option<(usize, usize)>) {
    let start = column.map_or(0, |column| column.saturating_sub(1 + EXCERPT_WIDTH / 2));
    let mut lines = Vec::new();
    for (index, text) in source.lines().enumerate().skip(line.saturating_sub(2)).take(3) {
        let characters: Vec<char> = text.chars().map(|ch| if ch == '\t' { ' ' } else { ch }).collect();
        let from = start.min(characters.len());
        let to = (start + EXCERPT_WIDTH).min(characters.len());
        lines.push(ExcerptLine {
            number: index + 1,
            text: characters[from..to].iter().collect(),
            target: index + 1 == line,
        });
    }
    let marker = column.map(|column| {
        let before = column.saturating_sub(1) - start;
        (before, length.unwrap_or(1).clamp(1, EXCERPT_WIDTH - before.min(EXCERPT_WIDTH - 1)))
    });
    (lines, marker)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn excerpt_shows_neighbours_and_marker() {
        let (lines, marker) = excerpt("one\ntwo words\nthree\n", 2, Some(5), Some(5));
        assert_eq!(lines.iter().map(|line| line.number).collect::<Vec<_>>(), [1, 2, 3]);
        assert!(lines[1].target);
        assert_eq!(marker, Some((4, 5)));
    }

    #[test]
    fn excerpt_follows_the_error_in_a_long_line() {
        let long = format!("{}ERROR{}", "a".repeat(300), "b".repeat(300));
        let (lines, marker) = excerpt(&long, 1, Some(301), Some(5));
        let (before, width) = marker.unwrap();
        let text: String = lines[0].text.chars().skip(before).take(width).collect();
        assert_eq!(text, "ERROR");
    }

    #[test]
    fn first_error_has_title_hint_and_location() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("main.tex");
        std::fs::write(&file, "\\fracc\n").unwrap();
        let diagnostic = Diagnostic {
            file: Some(file),
            line: Some(1),
            message: "Undefined control sequence.".into(),
            context: Some("\\fracc".into()),
            ..Default::default()
        };
        let problems = problems(&[diagnostic.clone(), diagnostic]);
        assert_eq!(problems[0].location.as_deref(), Some("main.tex, line 1"));
        assert!(problems[0].title.contains("\\fracc"));
        assert!(problems[0].hint.as_deref().unwrap().contains("\\frac"));
        assert_eq!(problems[0].excerpt.len(), 1);
        assert!(problems[1].excerpt.is_empty());
    }
}
