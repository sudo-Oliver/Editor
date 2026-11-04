use std::{ops::Range, path::Path, sync::OnceLock};

use ropey::Rope;
use streaming_iterator::StreamingIterator;
use tree_sitter::{InputEdit, Language as TsLanguage, Node, Parser, Query, QueryCursor, TextProvider, Tree};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Language {
    Latex,
    Typst,
    Markdown,
    Plain,
}

impl Language {
    pub fn from_path(path: &Path) -> Self {
        match path.extension().and_then(|extension| extension.to_str()) {
            Some("tex" | "ltx" | "sty" | "cls") => Self::Latex,
            Some("typ") => Self::Typst,
            Some("md" | "markdown") => Self::Markdown,
            _ => Self::Plain,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HighlightKind {
    Comment,
    Keyword,
    Function,
    Type,
    String,
    Number,
    Constant,
    Operator,
    Punctuation,
    Variable,
    Tag,
    Heading,
    Emphasis,
    Strong,
    Raw,
    Math,
    Link,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HighlightSpan {
    pub range: Range<usize>,
    pub kind: HighlightKind,
}

fn kind_for_capture(name: &str) -> Option<HighlightKind> {
    use HighlightKind::*;
    let kind = match name {
        "none" => return None,
        _ if name.starts_with("comment") => Comment,
        _ if name.starts_with("keyword") => Keyword,
        _ if name.starts_with("function") => Function,
        _ if name.starts_with("type") => Type,
        _ if name.starts_with("string.escape") => Constant,
        _ if name.starts_with("string") => String,
        _ if name.starts_with("constant.numeric") => Number,
        _ if name.starts_with("constant") => Constant,
        _ if name.starts_with("operator") => Operator,
        _ if name.starts_with("punctuation") => Punctuation,
        _ if name.starts_with("variable") => Variable,
        _ if name.starts_with("tag") => Tag,
        _ if name.starts_with("link") => Link,
        _ if name.starts_with("markup.heading") => Heading,
        _ if name.starts_with("markup.italic") => Emphasis,
        _ if name.starts_with("markup.bold") => Strong,
        _ if name.starts_with("markup.raw") => Raw,
        _ if name.starts_with("markup.math") => Math,
        _ if name.starts_with("markup.list") => Punctuation,
        _ if name.starts_with("markup.quote") => Comment,
        "text.title" => Heading,
        "text.emphasis" => Emphasis,
        "text.strong" => Strong,
        "text.literal" => Raw,
        "text.uri" | "text.reference" => Link,
        _ => return None,
    };
    Some(kind)
}

/// A document heading as written in the source, markup included.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Heading {
    pub level: usize,
    pub title: String,
    pub line: usize,
}

/// Finds headings in the parse tree of one grammar.
#[derive(Clone, Copy)]
struct HeadingRules {
    /// Returns the level of a heading node and the bytes of its title.
    find: fn(Node) -> Option<(usize, Range<usize>)>,
    /// Nodes that cannot hold a real heading, which also saves walking them.
    skip: &'static [&'static str],
}

const LATEX_HEADINGS: HeadingRules = HeadingRules {
    find: latex_heading,
    skip: &[
        "text",
        "inline_formula",
        "displayed_equation",
        "math_environment",
        "new_command_definition",
        "old_command_definition",
        "environment_definition",
    ],
};
const TYPST_HEADINGS: HeadingRules = HeadingRules {
    find: typst_heading,
    skip: &["text", "math", "raw_blck", "heading"],
};
const MARKDOWN_HEADINGS: HeadingRules = HeadingRules {
    find: markdown_heading,
    skip: &["atx_heading", "setext_heading", "paragraph", "fenced_code_block", "indented_code_block"],
};

fn latex_heading(node: Node) -> Option<(usize, Range<usize>)> {
    let level = match node.kind() {
        "part" => 0,
        "chapter" => 1,
        "section" => 2,
        "subsection" => 3,
        "subsubsection" => 4,
        _ => return None,
    };
    Some((level, node.child_by_field_name("text")?.byte_range()))
}

fn typst_heading(node: Node) -> Option<(usize, Range<usize>)> {
    if node.kind() != "heading" {
        return None;
    }
    let marker = node.child(0)?;
    Some((marker.byte_range().len(), marker.end_byte()..node.end_byte()))
}

fn markdown_heading(node: Node) -> Option<(usize, Range<usize>)> {
    match node.kind() {
        "atx_heading" => {
            let level = node.child(0)?.kind().strip_prefix("atx_h")?.strip_suffix("_marker")?.parse().ok()?;
            Some((level, node.child(1)?.byte_range()))
        }
        "setext_heading" => {
            let level = match node.child(1)?.kind() {
                "setext_h1_underline" => 1,
                _ => 2,
            };
            Some((level, node.child(0)?.byte_range()))
        }
        _ => None,
    }
}

/// Walks with one cursor instead of recursing, because deeply nested groups in a
/// document must not overflow the stack of the parse thread.
fn collect_headings(rules: HeadingRules, tree: &Tree, rope: &Rope) -> Vec<Heading> {
    let mut headings = Vec::new();
    let mut cursor = tree.walk();
    loop {
        let node = cursor.node();
        if let Some((level, title)) = (rules.find)(node) {
            headings.push(Heading {
                level,
                title: rope.byte_slice(title).to_string(),
                line: rope.byte_to_line(node.start_byte()),
            });
        }
        if !rules.skip.contains(&node.kind()) && cursor.goto_first_child() {
            continue;
        }
        while !cursor.goto_next_sibling() {
            if !cursor.goto_parent() {
                return headings;
            }
        }
    }
}

struct LanguageConfig {
    language: TsLanguage,
    query: Query,
    kinds: Vec<Option<HighlightKind>>,
    headings: Option<HeadingRules>,
    /// Second grammar that parses the ranges of `inline` nodes in Markdown.
    inline: Option<Box<LanguageConfig>>,
}

impl LanguageConfig {
    fn new(
        language: TsLanguage,
        source: &str,
        headings: Option<HeadingRules>,
        inline: Option<Box<LanguageConfig>>,
    ) -> Self {
        let query = Query::new(&language, source).expect("bundled highlight query is valid");
        let kinds = query
            .capture_names()
            .iter()
            .map(|name| kind_for_capture(name))
            .collect();
        Self {
            language,
            query,
            kinds,
            headings,
            inline,
        }
    }

    fn get(language: Language) -> Option<&'static Self> {
        static LATEX: OnceLock<LanguageConfig> = OnceLock::new();
        static TYPST: OnceLock<LanguageConfig> = OnceLock::new();
        static MARKDOWN: OnceLock<LanguageConfig> = OnceLock::new();
        match language {
            Language::Latex => Some(LATEX.get_or_init(|| {
                Self::new(
                    codebook_tree_sitter_latex::LANGUAGE.into(),
                    include_str!("../queries/latex.scm"),
                    Some(LATEX_HEADINGS),
                    None,
                )
            })),
            Language::Typst => Some(TYPST.get_or_init(|| {
                Self::new(
                    codebook_tree_sitter_typst::LANGUAGE.into(),
                    include_str!("../queries/typst.scm"),
                    Some(TYPST_HEADINGS),
                    None,
                )
            })),
            Language::Markdown => Some(MARKDOWN.get_or_init(|| {
                Self::new(
                    tree_sitter_md::LANGUAGE.into(),
                    tree_sitter_md::HIGHLIGHT_QUERY_BLOCK,
                    Some(MARKDOWN_HEADINGS),
                    Some(Box::new(Self::new(
                        tree_sitter_md::INLINE_LANGUAGE.into(),
                        tree_sitter_md::HIGHLIGHT_QUERY_INLINE,
                        None,
                        None,
                    ))),
                )
            })),
            Language::Plain => None,
        }
    }
}

/// Parse trees of one document. The trees follow edits right away, so that
/// highlights keep their position until a background parse delivers new trees.
pub struct Syntax {
    config: Option<&'static LanguageConfig>,
    tree: Option<Tree>,
    inline_tree: Option<Tree>,
    headings: Vec<Heading>,
    applied_results: u64,
    applied_version: u64,
}

/// A parse that holds only owned data and can run on any thread.
pub struct ParseJob {
    config: &'static LanguageConfig,
    rope: Rope,
    old_tree: Option<Tree>,
    version: u64,
}

pub struct ParseResult {
    tree: Option<Tree>,
    inline_tree: Option<Tree>,
    headings: Vec<Heading>,
    version: u64,
}

impl Syntax {
    pub fn new(language: Language) -> Self {
        Self {
            config: LanguageConfig::get(language),
            tree: None,
            inline_tree: None,
            headings: Vec::new(),
            applied_results: 0,
            applied_version: 0,
        }
    }

    pub fn edit(&mut self, edit: &InputEdit) {
        if let Some(tree) = &mut self.tree {
            tree.edit(edit);
        }
        if let Some(tree) = &mut self.inline_tree {
            tree.edit(edit);
        }
    }

    pub fn parse_job(&self, rope: &Rope, version: u64) -> Option<ParseJob> {
        Some(ParseJob {
            config: self.config?,
            rope: rope.clone(),
            old_tree: self.tree.clone(),
            version,
        })
    }

    /// Stores the trees of a finished job. A job that an edit overtook still
    /// counts when it is newer than the current trees. Later jobs replace it.
    pub fn apply(&mut self, result: ParseResult, edits_since: &[InputEdit]) -> bool {
        if result.version < self.applied_version {
            return false;
        }
        self.applied_version = result.version;
        self.tree = result.tree;
        self.inline_tree = result.inline_tree;
        self.headings = result.headings;
        self.applied_results += 1;
        for edit in edits_since {
            self.edit(edit);
        }
        true
    }

    /// Whether `offset` lies inside a formula. Without a tree the answer is no.
    pub fn in_math(&self, offset: usize) -> bool {
        let Some(tree) = &self.tree else {
            return false;
        };
        let mut node = tree.root_node().descendant_for_byte_range(offset, offset);
        while let Some(current) = node {
            if matches!(current.kind(), "inline_formula" | "displayed_equation" | "math_environment" | "math")
                && current.start_byte() < offset
                && offset < current.end_byte()
            {
                return true;
            }
            node = current.parent();
        }
        false
    }

    pub fn has_tree(&self) -> bool {
        self.tree.is_some()
    }

    /// Headings of the last finished parse, so they can lag behind the latest edit.
    pub fn headings(&self) -> &[Heading] {
        &self.headings
    }

    /// Counts finished parses, which tells when `headings` may have changed.
    pub fn applied_results(&self) -> u64 {
        self.applied_results
    }

    /// Returns spans that touch `range`, in document order of their matches.
    pub fn highlights(&self, rope: &Rope, range: Range<usize>) -> Vec<HighlightSpan> {
        let mut spans = Vec::new();
        let Some(config) = self.config else {
            return spans;
        };
        if let Some(tree) = &self.tree {
            collect_spans(config, tree, rope, &range, &mut spans);
        }
        if let (Some(inline), Some(tree)) = (&config.inline, &self.inline_tree) {
            collect_spans(inline, tree, rope, &range, &mut spans);
        }
        spans
    }
}

fn collect_spans(
    config: &LanguageConfig,
    tree: &Tree,
    rope: &Rope,
    range: &Range<usize>,
    spans: &mut Vec<HighlightSpan>,
) {
    let mut cursor = QueryCursor::new();
    cursor.set_byte_range(range.clone());
    let mut captures = cursor.captures(&config.query, tree.root_node(), RopeProvider(rope));
    while let Some((query_match, index)) = captures.next() {
        let capture = query_match.captures()[*index];
        let Some(kind) = config.kinds[capture.index as usize] else {
            continue;
        };
        let node_range = capture.node.byte_range();
        if node_range.start < node_range.end {
            spans.push(HighlightSpan {
                range: node_range,
                kind,
            });
        }
    }
}

impl ParseJob {
    pub fn version(&self) -> u64 {
        self.version
    }

    pub fn run(self) -> ParseResult {
        let mut parser = Parser::new();
        parser
            .set_language(&self.config.language)
            .expect("grammar ABI matches tree-sitter");
        let tree = parse_rope(&mut parser, &self.rope, self.old_tree.as_ref());

        let inline_tree = match (&self.config.inline, &tree) {
            (Some(inline), Some(tree)) => {
                let mut ranges = Vec::new();
                collect_inline_ranges(tree.root_node(), &mut ranges);
                parser
                    .set_language(&inline.language)
                    .expect("grammar ABI matches tree-sitter");
                if ranges.is_empty() || parser.set_included_ranges(&ranges).is_err() {
                    None
                } else {
                    parse_rope(&mut parser, &self.rope, None)
                }
            }
            _ => None,
        };

        let headings = match (self.config.headings, &tree) {
            (Some(rules), Some(tree)) => collect_headings(rules, tree, &self.rope),
            _ => Vec::new(),
        };

        ParseResult {
            tree,
            inline_tree,
            headings,
            version: self.version,
        }
    }
}

fn parse_rope(parser: &mut Parser, rope: &Rope, old_tree: Option<&Tree>) -> Option<Tree> {
    parser.parse_with_options(
        &mut |byte, _| {
            if byte >= rope.len_bytes() {
                return &b""[..];
            }
            let (chunk, chunk_start, _, _) = rope.chunk_at_byte(byte);
            &chunk.as_bytes()[byte - chunk_start..]
        },
        old_tree,
        None,
    )
}

fn collect_inline_ranges(node: Node, ranges: &mut Vec<tree_sitter::Range>) {
    if node.kind() == "inline" {
        ranges.push(node.range());
        return;
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_inline_ranges(child, ranges);
    }
}

struct RopeProvider<'a>(&'a Rope);

impl<'a> TextProvider<&'a [u8]> for RopeProvider<'a> {
    type I = std::vec::IntoIter<&'a [u8]>;

    fn text(&mut self, node: Node) -> Self::I {
        let range = node.byte_range();
        let end = range.end.min(self.0.len_bytes());
        let start = range.start.min(end);
        let chunks: Vec<&'a [u8]> = self
            .0
            .byte_slice(start..end)
            .chunks()
            .map(str::as_bytes)
            .collect();
        chunks.into_iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds_for(language: Language, text: &str) -> Vec<(String, HighlightKind)> {
        let rope = Rope::from_str(text);
        let mut syntax = Syntax::new(language);
        let result = syntax.parse_job(&rope, 1).unwrap().run();
        assert!(syntax.apply(result, &[]));
        syntax
            .highlights(&rope, 0..text.len())
            .into_iter()
            .map(|span| (text[span.range].to_string(), span.kind))
            .collect()
    }

    #[test]
    fn latex_highlights_commands_comments_and_math() {
        let spans = kinds_for(
            Language::Latex,
            "% note\n\\section{Intro}\n\\foo{bar} $x$\n\\begin{itemize}\n\\end{itemize}\n",
        );
        assert!(spans.contains(&("% note".into(), HighlightKind::Comment)));
        assert!(spans.contains(&("\\section".into(), HighlightKind::Function)));
        assert!(spans.contains(&("{Intro}".into(), HighlightKind::Heading)));
        assert!(spans.contains(&("\\foo".into(), HighlightKind::Function)));
        assert!(spans.contains(&("$x$".into(), HighlightKind::Math)));
        assert!(spans.contains(&("itemize".into(), HighlightKind::Type)));
    }

    #[test]
    fn latex_highlights_commands_that_are_grammar_tokens() {
        let spans = kinds_for(
            Language::Latex,
            "\\author{Olli}\n\\caption{x}\n\\textcolor{red}{x}\n\\def\\foo{x}\n",
        );
        assert!(spans.contains(&("\\author".into(), HighlightKind::Function)));
        assert!(spans.contains(&("\\caption".into(), HighlightKind::Function)));
        assert!(spans.contains(&("\\textcolor".into(), HighlightKind::Function)));
        assert!(spans.contains(&("\\def".into(), HighlightKind::Keyword)));
    }

    #[test]
    fn typst_highlights_markup_and_code() {
        let spans = kinds_for(
            Language::Typst,
            "= Title\n// note\n#let x = 1\n*bold* `raw`\n",
        );
        assert!(spans.iter().any(|(_, kind)| *kind == HighlightKind::Heading));
        assert!(spans.contains(&("// note".into(), HighlightKind::Comment)));
        assert!(spans.contains(&("let".into(), HighlightKind::Keyword)));
        assert!(spans.contains(&("1".into(), HighlightKind::Number)));
        assert!(spans.iter().any(|(_, kind)| *kind == HighlightKind::Strong));
    }

    #[test]
    fn markdown_highlights_blocks_and_inlines() {
        let spans = kinds_for(
            Language::Markdown,
            "# Title\n\nSome *em* and **strong** text\n\n```\ncode\n```\n",
        );
        assert!(spans.iter().any(|(_, kind)| *kind == HighlightKind::Heading));
        assert!(spans.iter().any(|(_, kind)| *kind == HighlightKind::Emphasis));
        assert!(spans.iter().any(|(_, kind)| *kind == HighlightKind::Strong));
        assert!(spans.iter().any(|(_, kind)| *kind == HighlightKind::Raw));
    }

    #[test]
    fn plain_text_has_no_parser() {
        assert!(Syntax::new(Language::Plain).parse_job(&Rope::new(), 1).is_none());
    }

    fn headings_of(language: Language, text: &str) -> Vec<(usize, String, usize)> {
        let rope = Rope::from_str(text);
        let mut syntax = Syntax::new(language);
        let result = syntax.parse_job(&rope, 1).unwrap().run();
        assert!(syntax.apply(result, &[]));
        syntax.headings().iter().map(|heading| (heading.level, heading.title.clone(), heading.line)).collect()
    }

    #[test]
    fn latex_headings_come_from_the_tree() {
        let text = "\\section[short]{Long\ntitle}\n% \\section{comment}\n\\subsection*{B}\n\\begin{verbatim}\n\\section{raw}\n\\end{verbatim}\n\\newcommand{\\foo}{\\section{macro}}\n\\chapter{C} \\section{D}\n";
        assert_eq!(
            headings_of(Language::Latex, text),
            vec![
                (2, "{Long\ntitle}".into(), 0),
                (3, "{B}".into(), 3),
                (1, "{C}".into(), 8),
                (2, "{D}".into(), 8),
            ]
        );
    }

    #[test]
    fn typst_headings_come_from_the_tree() {
        let text = "= *1.* Eins <a>\ntext\n=== Drei\n```\n= raw\n```\n";
        assert_eq!(
            headings_of(Language::Typst, text),
            vec![(1, " *1.* Eins <a>".into(), 0), (3, " Drei".into(), 2)]
        );
    }

    #[test]
    fn markdown_headings_come_from_the_tree() {
        let text = "# One\n\n```\n# code\n```\n\nTwo\n===\n\nThree\n---\n\n### Four\n";
        assert_eq!(
            headings_of(Language::Markdown, text),
            vec![
                (1, "One".into(), 0),
                (1, "Two\n".into(), 6),
                (2, "Three\n".into(), 9),
                (3, "Four".into(), 12),
            ]
        );
    }

    #[test]
    fn deep_nesting_does_not_overflow_the_stack() {
        let text = format!("\\section{{A}}\n{}{}", "{".repeat(50_000), "}".repeat(50_000));
        assert_eq!(headings_of(Language::Latex, &text)[0].1, "{A}");
    }

    #[test]
    fn stale_results_are_dropped() {
        let rope = Rope::from_str("% a\n");
        let mut syntax = Syntax::new(Language::Latex);
        let old = syntax.parse_job(&rope, 1).unwrap().run();
        let new = syntax.parse_job(&rope, 2).unwrap().run();
        assert!(syntax.apply(new, &[]));
        assert!(!syntax.apply(old, &[]));
    }
}
