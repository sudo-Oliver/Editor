//! What a LaTeX file uses and loads, read from the parse tree. Comments and
//! verbatim text do not count, because the tree keeps them apart from commands.

use std::{collections::HashMap, ops::Range};

use tree_sitter::{Node, Parser};

/// One name in a `\usepackage`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadedPackage {
    pub name: String,
    pub name_range: Range<usize>,
    statement: Range<usize>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LatexUsage {
    pub class: Option<String>,
    /// Each command with its first use.
    pub commands: HashMap<String, Range<usize>>,
    /// Each environment with the name in its first `\begin`.
    pub environments: HashMap<String, Range<usize>>,
    /// Commands and environments that the file defines itself.
    pub defined: Vec<String>,
    pub packages: Vec<LoadedPackage>,
    /// The end of the last `\usepackage`, or else of `\documentclass`.
    preamble_end: Option<usize>,
}

/// Parses `text` on the calling thread.
pub fn latex_usage(text: &str) -> LatexUsage {
    let mut usage = LatexUsage::default();
    let mut parser = Parser::new();
    let language = codebook_tree_sitter_latex::LANGUAGE.into();
    parser.set_language(&language).expect("grammar ABI matches tree-sitter");
    let Some(tree) = parser.parse(text, None) else {
        return usage;
    };
    let mut cursor = tree.walk();
    loop {
        let node = cursor.node();
        usage.visit(node, text);
        if node.kind() != "comment_environment" && cursor.goto_first_child() {
            continue;
        }
        while !cursor.goto_next_sibling() {
            if !cursor.goto_parent() {
                return usage;
            }
        }
    }
}

fn node_text<'a>(node: Node, text: &'a str) -> &'a str {
    &text[node.byte_range()]
}

fn trimmed_name(node: Node, text: &str) -> String {
    node_text(node, text).trim_matches(|ch: char| ch == '{' || ch == '}' || ch.is_whitespace()).to_string()
}

impl LatexUsage {
    fn visit(&mut self, node: Node, text: &str) {
        let kind = node.kind();
        if kind == "command_name" || (!node.is_named() && kind.starts_with('\\')) {
            self.commands.entry(node_text(node, text).to_string()).or_insert_with(|| node.byte_range());
            return;
        }
        match kind {
            "begin" => {
                if let Some(name) = node.child_by_field_name("name") {
                    self.environments.entry(trimmed_name(name, text)).or_insert_with(|| name.byte_range());
                }
            }
            "class_include" => {
                if let Some(path) = node.child_by_field_name("path") {
                    self.class = Some(trimmed_name(path, text));
                }
                self.preamble_end.get_or_insert(node.end_byte());
            }
            "package_include" => {
                let mut cursor = node.walk();
                if let Some(paths) = node.child_by_field_name("paths") {
                    for path in paths.children_by_field_name("path", &mut cursor) {
                        // The grammar's path includes the blanks after a comma.
                        let raw = node_text(path, text);
                        let start = path.start_byte() + (raw.len() - raw.trim_start().len());
                        self.packages.push(LoadedPackage {
                            name: raw.trim().to_string(),
                            name_range: start..start + raw.trim().len(),
                            statement: node.byte_range(),
                        });
                    }
                }
                self.preamble_end = Some(node.end_byte());
            }
            "new_command_definition" | "old_command_definition" | "let_command_definition" | "paired_delimiter_definition" => {
                if let Some(declaration) = node.child_by_field_name("declaration") {
                    self.defined.push(trimmed_name(declaration, text));
                }
            }
            "environment_definition" | "theorem_definition" => {
                if let Some(name) = node.child_by_field_name("name") {
                    self.defined.extend(trimmed_name(name, text).split(',').map(|name| name.trim().to_string()));
                }
            }
            _ => {}
        }
    }

    /// Where a new `\usepackage` goes and what to write there: a new line below
    /// the preamble's last package. Without `\documentclass` there is no place.
    pub fn package_insertion(&self, text: &str, package: &str) -> Option<(usize, String)> {
        self.class.as_ref()?;
        let end = self.preamble_end?;
        let line_end = text[end..].find('\n').map_or(text.len(), |offset| end + offset);
        Some((line_end, format!("\n\\usepackage{{{package}}}")))
    }

    /// What to delete to unload `package`: its name in a list of several, or else
    /// the statement, with its line when nothing else is on it.
    pub fn package_removal(&self, text: &str, package: &str) -> Option<Range<usize>> {
        let loaded = self.packages.iter().find(|loaded| loaded.name == package)?;
        let shared = self.packages.iter().filter(|other| other.statement == loaded.statement).count() > 1;
        if shared {
            let is_separator = |ch: char| ch == ',' || ch.is_whitespace();
            let after = text[loaded.name_range.end..].find(|ch| !is_separator(ch))?;
            let next = loaded.name_range.end + after;
            if text[loaded.name_range.end..next].contains(',') {
                return Some(loaded.name_range.start..next);
            }
            let before = text[..loaded.name_range.start].trim_end_matches(is_separator).len();
            return Some(before..loaded.name_range.end);
        }
        let line_start = text[..loaded.statement.start].rfind('\n').map_or(0, |index| index + 1);
        let line_end = text[loaded.statement.end..].find('\n').map_or(text.len(), |index| loaded.statement.end + index);
        let alone = text[line_start..loaded.statement.start].trim().is_empty() && text[loaded.statement.end..line_end].trim().is_empty();
        if !alone {
            return Some(loaded.statement.clone());
        }
        Some(line_start..(line_end + 1).min(text.len()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOCUMENT: &str = "\\documentclass{article}\n\\usepackage{a, b,c}\n\\usepackage[x]{d} % note\n\\begin{document}\nbody\n\\end{document}\n";

    fn applied(text: &str, range: Range<usize>) -> String {
        format!("{}{}", &text[..range.start], &text[range.end..])
    }

    #[test]
    fn finds_commands_also_when_the_grammar_has_tokens_for_them() {
        let text = "\\documentclass{article}\n\\begin{document}\n\\toprule \\caption{x} \\cref{a} \\textcolor{red}{y}\n\\end{document}\n";
        let usage = latex_usage(text);
        for command in ["\\toprule", "\\caption", "\\cref", "\\textcolor"] {
            assert!(usage.commands.contains_key(command), "{command}");
        }
        let toprule = usage.commands["\\toprule"].clone();
        assert_eq!(&text[toprule], "\\toprule");
    }

    #[test]
    fn finds_environments_also_in_math() {
        let usage = latex_usage("\\begin{document}\n\\begin{align}\nx\n\\end{align}\n\\begin{itemize}\n\\item a\n\\end{itemize}\n\\begin{lstlisting}\nx\n\\end{lstlisting}\n\\end{document}\n");
        for environment in ["document", "align", "itemize", "lstlisting"] {
            assert!(usage.environments.contains_key(environment), "{environment}");
        }
    }

    #[test]
    fn comments_and_verbatim_do_not_count() {
        let usage = latex_usage("% \\toprule\n\\begin{verbatim}\n\\midrule\n\\end{verbatim}\n\\begin{comment}\n\\cmidrule\n\\end{comment}\n");
        for command in ["\\toprule", "\\midrule", "\\cmidrule"] {
            assert!(!usage.commands.contains_key(command), "{command}");
        }
    }

    #[test]
    fn reads_class_packages_and_definitions() {
        let usage = latex_usage(DOCUMENT);
        assert_eq!(usage.class.as_deref(), Some("article"));
        let names: Vec<&str> = usage.packages.iter().map(|package| package.name.as_str()).collect();
        assert_eq!(names, ["a", "b", "c", "d"]);
        let usage = latex_usage("\\newcommand{\\foo}{x}\n\\newcommand\\bar{y}\n\\def\\baz{z}\n\\newenvironment{env}{a}{b}\n\\newtheorem{thm}{Theorem}\n");
        for name in ["\\foo", "\\bar", "\\baz", "env", "thm"] {
            assert!(usage.defined.iter().any(|defined| defined == name), "{name}: {:?}", usage.defined);
        }
    }

    #[test]
    fn a_new_package_goes_below_the_last_one_and_its_comment() {
        let usage = latex_usage(DOCUMENT);
        let (offset, line) = usage.package_insertion(DOCUMENT, "booktabs").unwrap();
        let result = format!("{}{}{}", &DOCUMENT[..offset], line, &DOCUMENT[offset..]);
        assert!(result.contains("\\usepackage[x]{d} % note\n\\usepackage{booktabs}\n\\begin{document}"), "{result}");
    }

    #[test]
    fn a_new_package_goes_below_the_class_without_other_packages() {
        let text = "\\documentclass{article}\n\\begin{document}\n\\end{document}\n";
        let (offset, line) = latex_usage(text).package_insertion(text, "booktabs").unwrap();
        assert_eq!(format!("{}{}{}", &text[..offset], line, &text[offset..]), "\\documentclass{article}\n\\usepackage{booktabs}\n\\begin{document}\n\\end{document}\n");
    }

    #[test]
    fn a_file_without_a_class_takes_no_new_package() {
        let text = "\\usepackage{a}\n\\section{Part}\n";
        assert_eq!(latex_usage(text).package_insertion(text, "b"), None);
    }

    #[test]
    fn removal_takes_one_name_from_a_list() {
        let usage = latex_usage(DOCUMENT);
        let removed = |name| applied(DOCUMENT, usage.package_removal(DOCUMENT, name).unwrap());
        assert!(removed("a").contains("\\usepackage{b,c}"));
        assert!(removed("b").contains("\\usepackage{a, c}"));
        assert!(removed("c").contains("\\usepackage{a, b}"));
    }

    #[test]
    fn removal_takes_the_line_of_a_lone_package() {
        let text = "\\documentclass{article}\n\\usepackage{booktabs}\n\\begin{document}\n";
        let range = latex_usage(text).package_removal(text, "booktabs").unwrap();
        assert_eq!(applied(text, range), "\\documentclass{article}\n\\begin{document}\n");
    }

    #[test]
    fn removal_keeps_a_comment_on_the_line() {
        let usage = latex_usage(DOCUMENT);
        let range = usage.package_removal(DOCUMENT, "d").unwrap();
        assert!(applied(DOCUMENT, range).contains("\n % note\n"));
    }
}
