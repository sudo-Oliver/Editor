//! Counts the words of a document from its parse tree, across included files.
//! What only macros or Typst functions produce at compile time does not count.

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use tree_sitter::{Node, Parser};

use crate::Language;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SectionCount {
    pub title: String,
    pub words: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WordCount {
    pub body: usize,
    pub headings: usize,
    pub captions: usize,
    pub inline_math: usize,
    pub display_math: usize,
    pub sections: Vec<SectionCount>,
    /// Included files that could not be read, so that a low count gets noticed.
    pub missing: Vec<String>,
}

impl WordCount {
    pub fn total(&self) -> usize {
        self.body + self.headings + self.captions
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Body,
    Heading,
    Caption,
}

/// LaTeX commands with leading arguments that are not prose, and how many.
const NON_PROSE_COMMANDS: &[(&str, usize)] = &[
    ("\\vspace", usize::MAX),
    ("\\vspace*", usize::MAX),
    ("\\hspace", usize::MAX),
    ("\\hspace*", usize::MAX),
    ("\\setlength", usize::MAX),
    ("\\addtolength", usize::MAX),
    ("\\setcounter", usize::MAX),
    ("\\addtocounter", usize::MAX),
    ("\\pagestyle", usize::MAX),
    ("\\thispagestyle", usize::MAX),
    ("\\pagenumbering", usize::MAX),
    ("\\geometry", usize::MAX),
    ("\\hypersetup", usize::MAX),
    ("\\url", usize::MAX),
    ("\\nolinkurl", usize::MAX),
    ("\\fontsize", usize::MAX),
    ("\\rule", usize::MAX),
    ("\\graphicspath", usize::MAX),
    ("\\setstretch", usize::MAX),
    ("\\lstset", usize::MAX),
    ("\\sisetup", usize::MAX),
    ("\\SI", usize::MAX),
    ("\\si", usize::MAX),
    ("\\num", usize::MAX),
    ("\\qty", usize::MAX),
    ("\\unit", usize::MAX),
    ("\\includepdf", usize::MAX),
    ("\\addcontentsline", usize::MAX),
    ("\\textcolor", 1),
    ("\\colorbox", 1),
    ("\\multicolumn", 2),
    ("\\multirow", 2),
];

/// LaTeX environments with leading arguments that are not prose, and how many.
const NON_PROSE_ARGUMENTS: &[(&str, usize)] = &[
    ("tabular", 1),
    ("tabular*", 2),
    ("tabularx", 2),
    ("tabulary", 2),
    ("longtable", 1),
    ("array", 1),
    ("minipage", 1),
    ("multicols", 1),
    ("wrapfigure", 2),
    ("wraptable", 2),
];

const SKIPPED_ENVIRONMENTS: &[&str] = &["tikzpicture", "thebibliography", "filecontents", "filecontents*"];

const SECTION_KINDS: &[&str] = &["part", "chapter", "section", "subsection", "subsubsection", "paragraph", "subparagraph"];

fn latex_kind_is_skipped(kind: &str) -> bool {
    kind.starts_with("curly_group_")
        || kind.starts_with("brack_group")
        || kind.ends_with("_definition")
        || kind.ends_with("_declaration")
        || kind.ends_with("_reference")
        || kind.ends_with("_range")
        || kind.ends_with("_import")
        || kind.ends_with("_include")
        || (kind.ends_with("_environment") && kind != "generic_environment")
        || matches!(kind, "begin" | "end" | "citation" | "comment" | "line_comment" | "block_comment" | "key_value_pair")
}

struct Counter<'a> {
    count: WordCount,
    root: PathBuf,
    read: &'a mut dyn FnMut(&Path) -> Option<String>,
    seen: HashSet<PathBuf>,
}

/// One file's text, and where the last counted word ended.
struct Source<'a> {
    text: &'a str,
    last_end: usize,
}

fn parse(language: Language, text: &str) -> Option<tree_sitter::Tree> {
    let grammar: tree_sitter::Language = match language {
        Language::Latex => codebook_tree_sitter_latex::LANGUAGE.into(),
        Language::Typst => codebook_tree_sitter_typst::LANGUAGE.into(),
        Language::Markdown | Language::Plain => return None,
    };
    let mut parser = Parser::new();
    parser.set_language(&grammar).ok()?;
    parser.parse(text, None)
}

fn inner_title(node: Node, text: &str) -> String {
    let raw = &text[node.byte_range()];
    raw.trim_matches(|ch: char| ch == '{' || ch == '}' || ch.is_whitespace()).split_whitespace().collect::<Vec<_>>().join(" ")
}

impl Counter<'_> {
    /// Adds the words in `range`. A word split by the grammar, as in Typst's `don't`
    /// or LaTeX's `well-known`, counts once.
    fn add_words(&mut self, source: &mut Source, start: usize, end: usize, kind: Kind) {
        let slice = &source.text[start..end];
        for token in slice.split_whitespace() {
            let token_start = token.as_ptr() as usize - source.text.as_ptr() as usize;
            if !token.chars().any(char::is_alphanumeric) {
                continue;
            }
            let joined = source.last_end > 0
                && source.last_end <= token_start
                && source.text[source.last_end..token_start].chars().all(|ch| matches!(ch, '\'' | '’' | '-' | '–'));
            source.last_end = token_start + token.len();
            if joined {
                continue;
            }
            match kind {
                Kind::Body => self.count.body += 1,
                Kind::Heading => self.count.headings += 1,
                Kind::Caption => self.count.captions += 1,
            }
            if self.count.sections.is_empty() {
                self.count.sections.push(SectionCount { title: "Before the first heading".to_string(), words: 0 });
            }
            if let Some(section) = self.count.sections.last_mut() {
                section.words += 1;
            }
        }
    }

    fn start_section(&mut self, title: String) {
        self.count.sections.push(SectionCount { title, words: 0 });
    }

    fn include(&mut self, path: PathBuf) {
        if !self.seen.insert(path.clone()) {
            return;
        }
        match (self.read)(&path) {
            Some(text) => self.add_file(&path, &text),
            None => {
                let shown = path.strip_prefix(&self.root).unwrap_or(&path).display().to_string();
                self.count.missing.push(shown);
            }
        }
    }

    fn add_file(&mut self, path: &Path, text: &str) {
        let language = Language::from_path(path);
        let mut source = Source { text, last_end: 0 };
        match parse(language, text) {
            Some(tree) if language == Language::Latex => self.add_latex(tree.root_node(), &mut source),
            Some(tree) => self.add_typst(tree.root_node(), path, &mut source),
            None => self.add_words(&mut source, 0, text.len(), Kind::Body),
        }
    }

    fn add_latex(&mut self, root: Node, source: &mut Source) {
        let text = source.text;
        // A whole document counts from `\begin{document}`, and a part counts whole.
        let mut cursor = root.walk();
        let document = root.named_children(&mut cursor).find(|node| {
            node.kind() == "generic_environment" && environment_name(*node, text).as_deref() == Some("document")
        });
        let mut stack = vec![(document.unwrap_or(root), Kind::Body)];
        while let Some((node, kind)) = stack.pop() {
            let node_kind = node.kind();
            match node_kind {
                "word" => {
                    self.add_words(source, node.start_byte(), node.end_byte(), kind);
                    continue;
                }
                "inline_formula" => {
                    self.count.inline_math += 1;
                    continue;
                }
                "displayed_equation" | "math_environment" => {
                    self.count.display_math += 1;
                    continue;
                }
                "latex_include" | "import_include" => {
                    self.add_latex_include(node, text);
                    continue;
                }
                _ if latex_kind_is_skipped(node_kind) => continue,
                _ => {}
            }
            let mut skip_arguments = 0;
            match node_kind {
                "generic_environment" => {
                    let name = environment_name(node, text);
                    let name = name.as_deref().unwrap_or_default();
                    if SKIPPED_ENVIRONMENTS.contains(&name) {
                        continue;
                    }
                    skip_arguments = NON_PROSE_ARGUMENTS.iter().find(|(known, _)| *known == name).map_or(0, |(_, count)| *count);
                }
                "generic_command" => {
                    let name = node.child_by_field_name("command").map(|command| &text[command.byte_range()]);
                    skip_arguments = NON_PROSE_COMMANDS.iter().find(|(known, _)| Some(*known) == name).map_or(0, |(_, count)| *count);
                }
                _ => {}
            }
            let title = SECTION_KINDS.contains(&node_kind).then(|| node.child_by_field_name("text")).flatten();
            if let Some(title) = title {
                self.start_section(inner_title(title, text));
            }
            let mut children = Vec::new();
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                if child.kind() == "curly_group" && skip_arguments > 0 {
                    skip_arguments -= 1;
                    continue;
                }
                let child_kind = if title == Some(child) {
                    Kind::Heading
                } else if node_kind == "caption" {
                    Kind::Caption
                } else {
                    kind
                };
                children.push((child, child_kind));
            }
            stack.extend(children.into_iter().rev());
        }
    }

    fn add_latex_include(&mut self, node: Node, text: &str) {
        let path = if node.kind() == "import_include" {
            let directory = node.child_by_field_name("directory").map(|node| inner_title(node, text));
            let file = node.child_by_field_name("file").map(|node| inner_title(node, text));
            directory.zip(file).map(|(directory, file)| format!("{directory}{file}"))
        } else {
            node.child_by_field_name("path").map(|node| inner_title(node, text))
        };
        let Some(path) = path.filter(|path| !path.is_empty()) else {
            return;
        };
        let mut path = self.root.join(path);
        if path.extension().is_none() {
            path.set_extension("tex");
        }
        self.include(path);
    }

    fn add_typst(&mut self, root: Node, file: &Path, source: &mut Source) {
        let text = source.text;
        let mut stack = vec![(root, Kind::Body)];
        while let Some((node, kind)) = stack.pop() {
            let mut child_kind = kind;
            match node.kind() {
                "text" => {
                    self.add_words(source, node.start_byte(), node.end_byte(), kind);
                    continue;
                }
                "comment" | "raw_span" | "raw_blck" | "label" | "ref" | "url" | "let" | "set" | "show" | "import" | "string" => continue,
                "math" => {
                    let inner = text[node.byte_range()].trim_start_matches('$');
                    if inner.starts_with(char::is_whitespace) {
                        self.count.display_math += 1;
                    } else {
                        self.count.inline_math += 1;
                    }
                    continue;
                }
                "include" => {
                    let mut cursor = node.walk();
                    let target = node.named_children(&mut cursor).find(|child| child.kind() == "string");
                    if let Some(target) = target {
                        let name = text[target.byte_range()].trim_matches('"');
                        let path = match name.strip_prefix('/') {
                            Some(from_root) => self.root.join(from_root),
                            None => file.parent().unwrap_or(&self.root).join(name),
                        };
                        self.include(path);
                    }
                    continue;
                }
                "heading" => {
                    let mut cursor = node.walk();
                    let title: Vec<&str> = node
                        .named_children(&mut cursor)
                        .filter(|child| child.kind() != "label")
                        .map(|child| text[child.byte_range()].trim())
                        .collect();
                    self.start_section(title.join(" "));
                    child_kind = Kind::Heading;
                }
                "tagged" => {
                    let is_caption = node.named_child(0).is_some_and(|name| &text[name.byte_range()] == "caption");
                    if is_caption {
                        child_kind = Kind::Caption;
                    }
                }
                _ => {}
            }
            let mut cursor = node.walk();
            let children: Vec<Node> = node.named_children(&mut cursor).collect();
            stack.extend(children.into_iter().rev().map(|child| (child, child_kind)));
        }
    }
}

fn environment_name(node: Node, text: &str) -> Option<String> {
    let begin = node.child_by_field_name("begin")?;
    Some(inner_title(begin.child_by_field_name("name")?, text))
}

/// Counts `main` and what it includes. LaTeX paths start from `root`, as the
/// compiler runs there. `read` provides each included file's text.
pub fn count_words(main: &Path, text: &str, root: &Path, read: &mut dyn FnMut(&Path) -> Option<String>) -> WordCount {
    let mut counter = Counter { count: WordCount::default(), root: root.to_path_buf(), read, seen: HashSet::from([main.to_path_buf()]) };
    counter.add_file(main, text);
    counter.count
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn count(main: &str, text: &str, others: &[(&str, &str)]) -> WordCount {
        let files: HashMap<PathBuf, String> =
            others.iter().map(|(path, text)| (Path::new("/p").join(path), (*text).to_string())).collect();
        count_words(&Path::new("/p").join(main), text, Path::new("/p"), &mut |path| files.get(path).cloned())
    }

    fn doc(body: &str) -> String {
        format!("\\documentclass{{article}}\n\\title{{Not counted}}\n\\begin{{document}}\n{body}\n\\end{{document}}\n")
    }

    #[test]
    fn latex_counts_only_prose() {
        let counted = count("m.tex", &doc("One two % not this\n\\textbf{a b} $x + y$ \\cite{k} \\ref{l} \\label{m}\n\\begin{verbatim}\nno way\n\\end{verbatim}\n\\vspace{1cm}"), &[]);
        assert_eq!(counted.body, 4);
        assert_eq!(counted.inline_math, 1);
    }

    #[test]
    fn latex_keeps_umlauts_and_hyphens_as_one_word() {
        assert_eq!(count("m.tex", &doc("Äpfel well-known don't"), &[]).body, 3);
    }

    #[test]
    fn latex_separates_headings_captions_and_math() {
        let text = doc("\\section{Big Title}\\label{a}\nBody here\n\\begin{figure}[h]\\includegraphics[width=2cm]{f}\\caption{Cap one}\\end{figure}\n\\begin{align} a \\end{align}\n\\[ b \\]\n\\section{Next}\nmore");
        let counted = count("m.tex", &text, &[]);
        assert_eq!((counted.body, counted.headings, counted.captions), (3, 3, 2));
        assert_eq!(counted.display_math, 2);
        assert_eq!(counted.total(), 8);
        assert_eq!(counted.sections[0], SectionCount { title: "Big Title".to_string(), words: 6 });
        assert_eq!(counted.sections[1], SectionCount { title: "Next".to_string(), words: 2 });
    }

    #[test]
    fn latex_skips_table_layouts_and_link_targets() {
        let text = doc("\\begin{tabular}{lcc} x & y \\\\ \\end{tabular}\n\\href{http://x.org}{link text} \\url{http://y.org}\n\\begin{tikzpicture}\\node {no};\\end{tikzpicture}");
        assert_eq!(count("m.tex", &text, &[]).body, 4);
    }

    #[test]
    fn latex_follows_includes() {
        let text = doc("main \\input{part}\n\\include{ch/two.tex}\n\\input{gone}");
        let counted = count("m.tex", &text, &[("part.tex", "in part"), ("ch/two.tex", "two")]);
        assert_eq!(counted.body, 4);
        assert_eq!(counted.missing, ["gone.tex"]);
    }

    #[test]
    fn a_cycle_of_includes_ends() {
        let counted = count("m.tex", &doc("a \\input{b}"), &[("b.tex", "b \\input{m}\\input{b}")]);
        assert_eq!(counted.body, 2);
        assert_eq!(counted.missing, Vec::<String>::new());
    }

    #[test]
    fn typst_counts_only_prose() {
        let text = "#set page(paper: \"a4\")\n#let f(x) = [X #x]\n= Big Title <l>\nWell-known, don't Äpfel // no\n*bold word* $x$ $ y $ @r `raw`\n#figure(image(\"a.png\"), caption: [Cap one])\n#table(columns: 2, [cell a], [cell])\n- item\n```\nraw block\n```\n";
        let counted = count("m.typ", text, &[]);
        assert_eq!((counted.body, counted.headings, counted.captions), (9, 2, 2), "{counted:?}");
        assert_eq!((counted.inline_math, counted.display_math), (1, 1));
        assert_eq!(counted.sections[0].title, "Big Title");
    }

    #[test]
    fn typst_follows_includes_from_the_current_file() {
        let counted = count("ch/m.typ", "a #include \"b.typ\"\n#include \"/c.typ\"\n#include \"no.typ\"", &[("ch/b.typ", "bb"), ("c.typ", "cc")]);
        assert_eq!(counted.body, 3);
        assert_eq!(counted.missing, ["ch/no.typ"]);
    }

    #[test]
    fn deep_nesting_does_not_overflow() {
        let nested = format!("{}x{}", "{".repeat(100_000), "}".repeat(100_000));
        assert_eq!(count("m.tex", &nested, &[]).body, 1);
    }
}
