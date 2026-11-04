//! Turns compiler messages into explanations for people who do not know TeX or Typst.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use crate::{
    packages::{Fix, closest_package, package_for_command, package_for_environment},
    parse::Diagnostic,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Explanation {
    pub title: String,
    pub hint: String,
    /// What one click can do about it.
    pub fix: Option<Fix>,
}

fn found(title: impl Into<String>, hint: impl Into<String>) -> Option<Explanation> {
    Some(Explanation { title: title.into(), hint: hint.into(), fix: None })
}

/// Common commands used to spot typing mistakes.
const COMMON_COMMANDS: &[&str] = &[
    "\\section", "\\subsection", "\\subsubsection", "\\chapter", "\\paragraph", "\\textbf", "\\textit",
    "\\emph", "\\underline", "\\item", "\\label", "\\ref", "\\cite", "\\caption", "\\centering", "\\title",
    "\\author", "\\date", "\\maketitle", "\\tableofcontents", "\\usepackage", "\\documentclass", "\\begin",
    "\\end", "\\input", "\\include", "\\footnote", "\\frac", "\\sqrt", "\\sum", "\\int", "\\prod", "\\alpha",
    "\\beta", "\\gamma", "\\delta", "\\lambda", "\\sigma", "\\theta", "\\infty", "\\cdot", "\\times", "\\leq",
    "\\geq", "\\neq", "\\left", "\\right", "\\includegraphics", "\\mathbb", "\\mathbf", "\\newline",
    "\\newpage", "\\hline", "\\bibliography", "\\bibliographystyle",
];

pub(crate) fn edit_distance(left: &str, right: &str) -> usize {
    let right: Vec<char> = right.chars().collect();
    let mut row: Vec<usize> = (0..=right.len()).collect();
    for (i, a) in left.chars().enumerate() {
        let mut previous = row[0];
        row[0] = i + 1;
        for (j, b) in right.iter().enumerate() {
            let current = row[j + 1];
            row[j + 1] = (previous + usize::from(a != *b)).min(current + 1).min(row[j] + 1);
            previous = current;
        }
    }
    row[right.len()]
}

fn closest_command(command: &str) -> Option<&'static str> {
    COMMON_COMMANDS
        .iter()
        .map(|known| (edit_distance(command, known), *known))
        .filter(|(distance, _)| (1..=2).contains(distance))
        .min_by_key(|(distance, _)| *distance)
        .map(|(_, known)| known)
}

/// The last `\command` in TeX's error context, which is the one TeX stopped at.
fn last_command(context: &str) -> Option<&str> {
    let start = context.rfind('\\')?;
    let name = &context[start + 1..];
    let end = name.find(|ch: char| !ch.is_ascii_alphabetic()).unwrap_or(name.len());
    (end > 0).then(|| &context[start..start + 1 + end])
}

fn quoted(message: &str) -> Option<&str> {
    let (_, rest) = message.split_once('`')?;
    Some(rest.split_once('\'')?.0)
}

fn between<'a>(message: &'a str, before: &str, after: &str) -> Option<&'a str> {
    let (_, rest) = message.split_once(before)?;
    Some(rest.split_once(after)?.0)
}

pub fn explain(diagnostic: &Diagnostic) -> Option<Explanation> {
    let message = diagnostic.message.trim();
    explain_tool(message)
        .or_else(|| explain_latex(message, diagnostic.context.as_deref()))
        .or_else(|| explain_typst(message))
}

fn explain_tool(message: &str) -> Option<Explanation> {
    let tool = message.strip_suffix(" is not installed.")?;
    found(
        format!("The compiler {tool} is not installed"),
        "Install it, then click Recompile. The README lists what each file type needs.",
    )
}

fn needs_package(title: String, item: &str, package: &str) -> Option<Explanation> {
    Some(Explanation {
        title,
        hint: format!("{item} needs a package. Add \\usepackage{{{package}}} before \\begin{{document}}."),
        fix: Some(Fix::AddPackage(package.to_string())),
    })
}

fn undefined_command(command: Option<&str>) -> Option<Explanation> {
    let Some(command) = command else {
        return found(
            "LaTeX does not know a command on this line",
            "Check the spelling. If the command comes from a package, add \\usepackage{…} before \\begin{document}.",
        );
    };
    let title = format!("LaTeX does not know the command {command}");
    if let Some(package) = package_for_command(command) {
        return needs_package(title, command, package);
    }
    if let Some(known) = closest_command(command) {
        return found(title, format!("Did you mean {known}? Check the spelling."));
    }
    found(title, "Check the spelling. If the command comes from a package, add \\usepackage{…} before \\begin{document}.")
}

fn explain_latex(message: &str, context: Option<&str>) -> Option<Explanation> {
    if message.starts_with("Undefined control sequence") {
        return undefined_command(context.and_then(last_command));
    }
    if message.starts_with("Missing $ inserted") {
        return found(
            "A math symbol is used outside of math mode",
            "Characters like _ and ^ and most math commands only work in math. Put the formula between $ and $. For a plain underscore, write \\_.",
        );
    }
    if message.starts_with("Extra }, or forgotten") || message.starts_with("Too many }'s") {
        return found("There is a closing } without an opening {", "Remove this } or add the missing { before it.");
    }
    if message.starts_with("Missing } inserted") {
        return found("A { is never closed", "Add the missing } on this line or the line before.");
    }
    if message.starts_with("Missing { inserted") {
        return found("A { is missing here", "This command expects its argument in braces, like \\frac{1}{2}.");
    }
    if let Some(name) = between(message, "File ended while scanning use of ", " .")
        .or_else(|| between(message, "Paragraph ended before ", " was complete"))
    {
        return found(format!("The argument of {name} is never closed"), "A } is missing after the text of this command.");
    }
    if message.starts_with("Emergency stop") && context.is_some_and(|context| context.contains("no legal \\end")) {
        return found("\\end{document} is missing", "Add \\end{document} as the last line of the main file.");
    }
    if message.starts_with("Emergency stop") || message.contains("Fatal error occurred") {
        return found("LaTeX had to stop here", "An earlier problem stopped the build. Fix the error listed first.");
    }
    if let Some((_, rest)) = message.split_once("LaTeX Error: ")
        && let Some(explained) = explain_latex_error(rest)
    {
        return Some(explained);
    }
    if message.contains("Error: File `") && message.contains("not found") {
        return missing_file(quoted(message)?);
    }
    if message.starts_with("Misplaced alignment tab") {
        return found(
            "The character & has a special meaning",
            "& only works inside tables and aligned equations. For a plain ampersand, write \\&.",
        );
    }
    if message.starts_with("Extra alignment tab") {
        return found("This table row has too many columns", "Remove an & or add a column to the column list in \\begin{tabular}{…}.");
    }
    if message.starts_with("Double superscript") || message.starts_with("Double subscript") {
        return found("Two ^ or two _ follow each other", "Group them with braces, like x^{a^b}.");
    }
    if message.starts_with("Illegal unit of measure") || message.starts_with("Missing number") {
        return found("LaTeX expected a number or a length here", "Lengths need a unit, like 2cm or 0.5\\textwidth.");
    }
    if message.starts_with("Illegal parameter number") {
        return found("The character # has a special meaning", "For a plain hash sign, write \\#.");
    }
    if message.contains("Unicode character") {
        return found("This special character is not supported", "Use the LuaLaTeX compiler in the Recompile menu, or replace the character with a LaTeX command.");
    }
    if message.starts_with("Citation `") {
        return found(
            format!("The citation {} is not in your bibliography", quoted(message)?),
            "Check the key against your .bib file. Check that \\bibliography{…} names that file.",
        );
    }
    if message.starts_with("Reference `") {
        return found(
            format!("The reference {} points to no \\label", quoted(message)?),
            "Check the name, or add \\label{…} where the reference should point.",
        );
    }
    if message.starts_with("Overfull") {
        return found("A line sticks out past the margin", "This does not stop the build. Rephrase the text or make the image or table smaller.");
    }
    if message.starts_with("Underfull") {
        return found("A line has too much space", "This does not stop the build, and you can usually ignore it.");
    }
    None
}

fn explain_latex_error(error: &str) -> Option<Explanation> {
    if error.starts_with("File `") {
        return missing_file(quoted(error)?);
    }
    if let Some(name) = between(error, "Environment ", " undefined") {
        let title = format!("LaTeX does not know the environment {name}");
        if let Some(package) = package_for_environment(name) {
            return needs_package(title, name, package);
        }
        return found(
            title,
            "Check the spelling. If it comes from a package, add \\usepackage{…} before \\begin{document}.",
        );
    }
    if let Some(opened) = between(error, "\\begin{", "}") {
        let closed = between(error, "\\end{", "}")?;
        let line = between(error, "on input line ", " ended");
        let opened_at = line.map_or(String::new(), |line| format!(" from line {line}"));
        return found(
            format!("\\begin{{{opened}}}{opened_at} is closed by \\end{{{closed}}}"),
            format!("Every \\begin needs a matching \\end. Add \\end{{{opened}}} or fix the name."),
        );
    }
    if error.starts_with("Missing \\begin{document}") {
        return found(
            "Text appears before \\begin{document}",
            "Move this text below \\begin{document}. Add \\begin{document} if it is missing.",
        );
    }
    if error.starts_with("Can be used only in preamble") {
        return found("This command must come before \\begin{document}", "Move this line above \\begin{document}.");
    }
    if error.starts_with("Something's wrong--perhaps a missing \\item") || error.starts_with("Lonely \\item") {
        return found("\\item is used wrongly", "Text in a list must follow an \\item, and \\item must be inside a list.");
    }
    if error.starts_with("There's no line here to end") {
        return found("\\\\ has no line to end", "Remove this \\\\. Use a blank line to start a new paragraph.");
    }
    if let Some(name) = between(error, "Command ", " already defined") {
        return found(format!("{name} is defined twice"), "Rename your command, or use \\renewcommand to replace the old one.");
    }
    if error.starts_with("Unknown graphics extension") {
        return found("This image format is not supported", "Convert the image to PDF, PNG or JPG.");
    }
    None
}

fn missing_file(name: &str) -> Option<Explanation> {
    if let Some(package) = name.strip_suffix(".sty") {
        if let Some(closest) = closest_package(package) {
            return found(format!("The package {package} is not installed"), format!("Did you mean {closest}? Check the name in \\usepackage."));
        }
        return found(
            format!("The package {package} is not installed"),
            "Check the name in \\usepackage. If the name is right, install the package with your TeX distribution.",
        );
    }
    if let Some(class) = name.strip_suffix(".cls") {
        return found(format!("The document class {class} is not installed"), "Check the name in \\documentclass.");
    }
    found(
        format!("The file {name} was not found"),
        "Check the name and the folder. Paths start at the folder of the main file. Upper and lower case can matter.",
    )
}

fn explain_typst(message: &str) -> Option<Explanation> {
    if let Some(name) = message.strip_prefix("unknown variable: ") {
        return found(format!("Typst does not know the name {name}"), "Check the spelling, or define it first with #let.");
    }
    if message == "unclosed delimiter" {
        return found(
            "A bracket or text marker is never closed",
            "A ( [ { * or _ here has no closing partner. Add it. Errors further down usually come from this one.",
        );
    }
    if message == "unclosed string" {
        return found("A quotation mark is never closed", "Add the closing \" at the end of the text.");
    }
    if message == "unclosed raw text" {
        return found("A backtick is never closed", "Add the closing `.");
    }
    if let Some(found_text) = message.strip_prefix("unexpected ") {
        return found(format!("Typst did not expect {found_text} here"), "Often a bracket or a comma right before this spot is missing or extra.");
    }
    if let Some(rest) = message.strip_prefix("expected ") {
        if let Some((expected, actual)) = rest.split_once(", found ") {
            return found(format!("Typst expected {expected} but got {actual}"), "Check the value here. The documentation of the function lists what it accepts.");
        }
        return found(format!("Typst expected {rest} here"), "Often a bracket or a comma right before this spot is missing or extra.");
    }
    if let Some(character) = between(message, "the character `", "` is not valid in code") {
        return found(format!("{character} is not allowed here"), "You are inside code, probably because a bracket above is still open.");
    }
    if message.starts_with("file not found") {
        return found("A file was not found", "Check the name and the folder. Paths start at the folder of this file.");
    }
    if let Some(label) = between(message, "label `", "` does not exist") {
        return found(format!("The label {label} does not exist"), "Check the name after @, or add the label to the heading or figure it should point to.");
    }
    if let Some(name) = message.strip_prefix("missing argument: ") {
        return found(format!("The argument {name} is missing"), "Add it to this function call.");
    }
    if let Some(name) = message.strip_prefix("unexpected argument: ") {
        return found(format!("This function has no argument {name}"), "Check the spelling, or remove the argument.");
    }
    if let Some(name) = message.strip_prefix("unknown font family: ") {
        return found(format!("The font {name} is not installed"), "Typst uses a fallback font. Install the font or pick another one. Run typst fonts to list them.");
    }
    None
}

/// Fills in positions that the compiler left out, using the source files.
pub fn locate(diagnostics: &mut [Diagnostic], main: &Path) {
    let mut sources: HashMap<PathBuf, Option<String>> = HashMap::new();
    for diagnostic in diagnostics.iter_mut() {
        let needs_search = search_line(diagnostic.message.as_str(), diagnostic.context.as_deref()).is_some();
        if diagnostic.file.is_none() && (needs_search || diagnostic.line.is_some()) {
            diagnostic.file = Some(main.to_path_buf());
        }
        let Some(file) = diagnostic.file.clone() else {
            continue;
        };
        let source = sources
            .entry(file.clone())
            .or_insert_with(|| std::fs::read(&file).ok().map(|bytes| String::from_utf8_lossy(&bytes).into_owned()));
        let Some(source) = source.as_deref() else {
            continue;
        };
        if let Some(search) = search_line(&diagnostic.message, diagnostic.context.as_deref())
            && let Some(line) = search.find(source, diagnostic.line)
        {
            diagnostic.line = Some(line);
        }
        locate_column(diagnostic, source);
    }
}

enum Search<'a> {
    /// The first line that names a file or package.
    Name(&'a str),
    /// The last use of a command whose argument never closes.
    Unclosed(&'a str),
    EndOfFile,
}

fn search_line<'a>(message: &'a str, context: Option<&str>) -> Option<Search<'a>> {
    if message.contains("Error: File `") && message.contains("not found") {
        let name = quoted(message)?;
        return Some(Search::Name(name.rsplit_once('.').map_or(name, |(stem, _)| stem)));
    }
    if let Some(name) = between(message, "File ended while scanning use of ", " .")
        .or_else(|| between(message, "Paragraph ended before ", " was complete"))
    {
        return Some(Search::Unclosed(name));
    }
    (message.starts_with("Emergency stop") && context.is_some_and(|context| context.contains("no legal \\end")))
        .then_some(Search::EndOfFile)
}

impl Search<'_> {
    fn find(&self, source: &str, reported: Option<usize>) -> Option<usize> {
        match self {
            Search::Name(name) => source
                .lines()
                .position(|line| !line.trim_start().starts_with('%') && line.contains(name))
                .map(|index| index + 1),
            Search::EndOfFile => Some(source.lines().count().max(1)),
            Search::Unclosed(command) => {
                let limit = reported.map_or(source.len(), |line| {
                    source.split_inclusive('\n').take(line).map(str::len).sum()
                });
                let scope = &source[..limit.min(source.len())];
                let offset = scope.rmatch_indices(command).find(|(offset, _)| {
                    let mut depth = 0i32;
                    let mut previous = ' ';
                    for ch in scope[*offset..].chars() {
                        match ch {
                            '{' if previous != '\\' => depth += 1,
                            '}' if previous != '\\' => depth -= 1,
                            _ => {}
                        }
                        previous = ch;
                    }
                    depth > 0
                })?.0;
                Some(source[..offset].matches('\n').count() + 1)
            }
        }
    }
}

/// TeX prints the line up to the point where it stopped, so the end of that
/// text is the column of the error.
fn locate_column(diagnostic: &mut Diagnostic, source: &str) {
    let (None, Some(line), Some(context)) = (diagnostic.column, diagnostic.line, diagnostic.context.as_deref()) else {
        return;
    };
    let Some(text) = source.lines().nth(line.saturating_sub(1)) else {
        return;
    };
    let context = context.trim_start_matches("...").trim_end();
    if context.is_empty() {
        return;
    }
    let Some(position) = text.find(context) else {
        return;
    };
    let end = position + context.len();
    let start = match last_command(context) {
        Some(command) if context.ends_with(command) => end - command.len(),
        _ => context.char_indices().next_back().map_or(end, |(index, _)| position + index),
    };
    diagnostic.column = Some(text[..start].chars().count() + 1);
    diagnostic.length = Some(text[start..end].chars().count());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_context(message: &str, context: &str) -> Diagnostic {
        Diagnostic { message: message.into(), context: Some(context.into()), ..Default::default() }
    }

    #[test]
    fn undefined_command_suggests_the_package() {
        let explained = explain(&with_context("Undefined control sequence.", "$\\mathbb")).unwrap();
        assert!(explained.title.contains("\\mathbb"));
        assert!(explained.hint.contains("\\usepackage{amssymb}"));
        assert_eq!(explained.fix, Some(Fix::AddPackage("amssymb".to_string())));
    }

    #[test]
    fn undefined_environment_suggests_the_package() {
        let diagnostic = Diagnostic { message: "LaTeX Error: Environment align undefined.".into(), ..Default::default() };
        let explained = explain(&diagnostic).unwrap();
        assert!(explained.title.contains("align"), "{explained:?}");
        assert_eq!(explained.fix, Some(Fix::AddPackage("amsmath".to_string())));
    }

    #[test]
    fn unknown_environment_has_no_fix() {
        let diagnostic = Diagnostic { message: "LaTeX Error: Environment foobar undefined.".into(), ..Default::default() };
        assert_eq!(explain(&diagnostic).unwrap().fix, None);
    }

    #[test]
    fn a_mistyped_package_gets_the_closest_name() {
        let diagnostic = Diagnostic { message: "LaTeX Error: File `amsmth.sty' not found.".into(), ..Default::default() };
        assert!(explain(&diagnostic).unwrap().hint.contains("Did you mean amsmath?"));
    }

    #[test]
    fn undefined_command_suggests_the_spelling() {
        let explained = explain(&with_context("Undefined control sequence.", "Hello \\fracc")).unwrap();
        assert!(explained.hint.contains("\\frac"), "{explained:?}");
    }

    #[test]
    fn unknown_message_has_no_explanation() {
        let diagnostic = Diagnostic { message: "something entirely new".into(), ..Default::default() };
        assert_eq!(explain(&diagnostic), None);
    }

    #[test]
    fn locate_finds_the_command_column() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("main.tex");
        std::fs::write(&file, "one\nÄ Hello \\fracc{1}{2}\n").unwrap();
        let mut diagnostics = [Diagnostic {
            file: Some(file.clone()),
            line: Some(2),
            context: Some("Ä Hello \\fracc".into()),
            ..Default::default()
        }];
        locate(&mut diagnostics, &file);
        assert_eq!((diagnostics[0].column, diagnostics[0].length), (Some(9), Some(6)));
    }

    #[test]
    fn locate_finds_the_unclosed_argument() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("main.tex");
        std::fs::write(&file, "\\textbf{fine}\n\\textbf{oops\nmore\n").unwrap();
        let mut diagnostics = [Diagnostic { message: "File ended while scanning use of \\textbf .".into(), ..Default::default() }];
        locate(&mut diagnostics, &file);
        assert_eq!(diagnostics[0].line, Some(2));
        assert_eq!(diagnostics[0].file.as_deref(), Some(file.as_path()));
    }
}
