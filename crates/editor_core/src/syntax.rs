use std::{
    ops::Range,
    path::Path,
    sync::{Arc, OnceLock},
    time::{Duration, Instant},
};

use ropey::Rope;
use streaming_iterator::StreamingIterator;
use tree_sitter::{InputEdit, Language as TsLanguage, Node, Parser, Query, QueryCursor, TextProvider, Tree, TreeCursor};

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

/// What an entry of the outline stands for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HeadingKind {
    #[default]
    Section,
    /// A figure with its caption as the title.
    Figure,
    Table,
    /// Where the bibliography goes, without a title of its own.
    References,
}

/// A document heading as written in the source, markup included.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Heading {
    /// For a figure, table or the references, the level is not used.
    pub level: usize,
    pub title: String,
    pub line: usize,
    /// False for `\section*` and its kin, which LaTeX leaves without a number.
    pub numbered: bool,
    pub kind: HeadingKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SymbolKind {
    Label,
    Command,
    /// A name from `#let` in Typst.
    Binding,
    /// The names in a `\usepackage`, as written between its braces.
    Package,
}

/// A name that the document defines, for suggestions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Symbol {
    pub kind: SymbolKind,
    pub name: String,
}

/// The source a node points into: the rope of an open file, or the text of one
/// read from disk.
pub(crate) trait SourceText {
    /// Compares without copying, as this runs for many nodes on every parse.
    fn is(&self, range: Range<usize>, expected: &str) -> bool;
    fn read(&self, range: Range<usize>) -> String;
}

impl SourceText for Rope {
    fn is(&self, range: Range<usize>, expected: &str) -> bool {
        range.len() == expected.len() && range.end <= self.len_bytes() && self.byte_slice(range) == expected
    }

    fn read(&self, range: Range<usize>) -> String {
        if range.end > self.len_bytes() {
            return String::new();
        }
        self.byte_slice(range).to_string()
    }
}

impl SourceText for &str {
    fn is(&self, range: Range<usize>, expected: &str) -> bool {
        self.get(range) == Some(expected)
    }

    fn read(&self, range: Range<usize>) -> String {
        self.get(range).unwrap_or_default().to_string()
    }
}

/// An entry of the outline found at a node: its kind, its level and the bytes
/// of its title, which a bibliography has none of.
type Found = (HeadingKind, usize, Option<Range<usize>>);

/// Finds headings in the parse tree of one grammar.
#[derive(Clone, Copy)]
struct HeadingRules {
    /// Returns what a node of one of `kinds` adds to the outline.
    find: fn(Node, &dyn SourceText) -> Option<Found>,
    /// The nodes `find` answers for.
    kinds: &'static [&'static str],
    /// Nodes that cannot hold a real heading, which also saves walking them.
    skip: &'static [&'static str],
}

const LATEX_HEADINGS: HeadingRules = HeadingRules {
    find: latex_heading,
    kinds: &[
        "part",
        "chapter",
        "section",
        "subsection",
        "subsubsection",
        "caption",
        "bibtex_include",
        "generic_command",
        "generic_environment",
    ],
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
    kinds: &["heading", "call"],
    skip: &["text", "math", "raw_blck", "heading"],
};
const MARKDOWN_HEADINGS: HeadingRules = HeadingRules {
    find: markdown_heading,
    kinds: &["atx_heading", "setext_heading"],
    skip: &["atx_heading", "setext_heading", "paragraph", "fenced_code_block", "indented_code_block"],
};

fn latex_heading(node: Node, text: &dyn SourceText) -> Option<Found> {
    let level = match node.kind() {
        "part" => 0,
        "chapter" => 1,
        "section" => 2,
        "subsection" => 3,
        "subsubsection" => 4,
        "caption" => {
            let title = node.child_by_field_name("long")?.byte_range();
            return Some((caption_kind(node, text), 0, Some(title)));
        }
        "bibtex_include" => return Some((HeadingKind::References, 0, None)),
        "generic_command" => {
            let command = node.child_by_field_name("command")?.byte_range();
            return text.is(command, "\\printbibliography").then_some((HeadingKind::References, 0, None));
        }
        "generic_environment" => {
            let name = node.child_by_field_name("begin")?.child_by_field_name("name")?.byte_range();
            return text.is(name, "{thebibliography}").then_some((HeadingKind::References, 0, None));
        }
        _ => return None,
    };
    Some((HeadingKind::Section, level, Some(node.child_by_field_name("text")?.byte_range())))
}

/// A caption names a table inside a table environment, and a figure otherwise.
fn caption_kind(caption: Node, text: &dyn SourceText) -> HeadingKind {
    let mut parent = caption.parent();
    while let Some(node) = parent {
        if node.kind() == "generic_environment"
            && let Some(name) = node.child_by_field_name("begin").and_then(|begin| begin.child_by_field_name("name"))
        {
            let name = text.read(name.byte_range());
            if name.contains("table") {
                return HeadingKind::Table;
            }
            if name.contains("figure") {
                return HeadingKind::Figure;
            }
        }
        parent = node.parent();
    }
    HeadingKind::Figure
}

fn typst_heading(node: Node, text: &dyn SourceText) -> Option<Found> {
    if node.kind() == "call" {
        return typst_figure(node, text);
    }
    if node.kind() != "heading" {
        return None;
    }
    let marker = node.child(0)?;
    Some((HeadingKind::Section, marker.byte_range().len(), Some(marker.end_byte()..node.end_byte())))
}

/// `#figure(table(..), caption: [..])` and `#bibliography("refs.bib")`.
fn typst_figure(call: Node, text: &dyn SourceText) -> Option<Found> {
    let function = call.child_by_field_name("item")?.byte_range();
    if text.is(function.clone(), "bibliography") {
        return Some((HeadingKind::References, 0, None));
    }
    if !text.is(function, "figure") {
        return None;
    }
    let (mut kind, mut caption) = (HeadingKind::Figure, None);
    let mut cursor = call.walk();
    let arguments = call.named_children(&mut cursor).find(|child| child.kind() == "group")?;
    let mut cursor = arguments.walk();
    for argument in arguments.named_children(&mut cursor) {
        match argument.kind() {
            "tagged" => {
                let (Some(name), Some(value)) = (argument.named_child(0), argument.named_child(1)) else {
                    continue;
                };
                if text.is(name.byte_range(), "caption") {
                    caption = Some(value.byte_range());
                } else if text.is(name.byte_range(), "kind") && text.read(value.byte_range()).contains("table") {
                    kind = HeadingKind::Table;
                }
            }
            "call" if argument.child_by_field_name("item").is_some_and(|item| text.is(item.byte_range(), "table")) => {
                kind = HeadingKind::Table;
            }
            _ => {}
        }
    }
    Some((kind, 0, caption))
}

fn markdown_heading(node: Node, _: &dyn SourceText) -> Option<Found> {
    match node.kind() {
        "atx_heading" => {
            let level = node.child(0)?.kind().strip_prefix("atx_h")?.strip_suffix("_marker")?.parse().ok()?;
            Some((HeadingKind::Section, level, Some(node.child(1)?.byte_range())))
        }
        "setext_heading" => {
            let level = match node.child(1)?.kind() {
                "setext_h1_underline" => 1,
                _ => 2,
            };
            Some((HeadingKind::Section, level, Some(node.child(0)?.byte_range())))
        }
        _ => None,
    }
}

/// The heading at `node` of a LaTeX or Typst file, as the outline of an open file
/// has it, for reading the headings of files that are not open.
pub(crate) fn heading_at(node: Node, text: &str, language: Language) -> Option<Heading> {
    let rules = match language {
        Language::Latex => LATEX_HEADINGS,
        Language::Typst => TYPST_HEADINGS,
        _ => return None,
    };
    if !rules.kinds.contains(&node.kind()) {
        return None;
    }
    Some(heading(node, (rules.find)(node, &text)?, &text))
}

fn heading(node: Node, (kind, level, title): Found, text: &dyn SourceText) -> Heading {
    Heading {
        level,
        title: title.map(|title| text.read(title)).unwrap_or_default(),
        line: node.start_position().row,
        numbered: kind == HeadingKind::Section
            && !node.child_by_field_name("command").is_some_and(|command| command.kind().ends_with('*')),
        kind,
    }
}

/// Returns the kind of a defining node and the bytes of its name.
#[derive(Clone, Copy)]
struct SymbolRules {
    find: fn(Node) -> Option<(SymbolKind, Range<usize>)>,
    /// The nodes `find` answers for.
    kinds: &'static [&'static str],
}

const LATEX_SYMBOLS: SymbolRules = SymbolRules {
    find: latex_symbol,
    kinds: &["package_include", "label_definition", "new_command_definition", "old_command_definition"],
};
const TYPST_SYMBOLS: SymbolRules = SymbolRules {
    find: typst_symbol,
    kinds: &["label", "let"],
};

fn latex_symbol(node: Node) -> Option<(SymbolKind, Range<usize>)> {
    if node.kind() == "package_include" {
        let range = node.child_by_field_name("paths")?.byte_range();
        return (range.len() >= 2).then(|| (SymbolKind::Package, range.start + 1..range.end - 1));
    }
    let (kind, field) = match node.kind() {
        "label_definition" => (SymbolKind::Label, "name"),
        "new_command_definition" | "old_command_definition" => (SymbolKind::Command, "declaration"),
        _ => return None,
    };
    let name = node.child_by_field_name(field)?;
    // A name in braces has the name as its one named child.
    let inner = name.named_child(0).filter(|_| name.named_child_count() == 1).unwrap_or(name);
    Some((kind, inner.byte_range()))
}

fn typst_symbol(node: Node) -> Option<(SymbolKind, Range<usize>)> {
    match node.kind() {
        "label" => {
            let range = node.byte_range();
            // A recovering parser may hand out a label without its brackets.
            (range.len() >= 2).then(|| (SymbolKind::Label, range.start + 1..range.end - 1))
        }
        "let" => {
            let pattern = node.child_by_field_name("pattern")?;
            // `#let name(args) = ...` has the name as first child of the call.
            let name = if pattern.kind() == "call" { pattern.named_child(0)? } else { pattern };
            (name.kind() == "ident").then(|| (SymbolKind::Binding, name.byte_range()))
        }
        _ => None,
    }
}

/// One table entry per kind of node, because this runs over every node of the
/// document after every parse, and comparing names is what would take the time.
struct KindTable(Vec<bool>);

impl KindTable {
    fn new(language: &TsLanguage, names: &[&str]) -> Self {
        // Several ids can share a name, so the ids decide and not the names.
        let ids = 0..language.node_kind_count() as u16;
        Self(ids.map(|id| language.node_kind_for_id(id).is_some_and(|name| names.contains(&name))).collect())
    }

    fn has(&self, node: Node) -> bool {
        // Error nodes have an id outside of the table.
        self.0.get(node.kind_id() as usize).copied().unwrap_or(false)
    }
}

/// The outline of a document, with the bytes of the node behind each entry, so
/// that the next parse can keep the entries of the parts that an edit left alone.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Outline {
    headings: Vec<Heading>,
    heading_nodes: Vec<Range<usize>>,
    symbols: Vec<Symbol>,
    symbol_nodes: Vec<Range<usize>>,
}

/// What the previous outline says about the new tree: where the edits went, and
/// the old entries moved to where their nodes are now.
struct Reuse<'a> {
    old_tree: &'a Tree,
    /// The edited bytes, sorted and disjoint. A node that touches one is walked again.
    dirty: Vec<Range<usize>>,
    headings: Vec<(Range<usize>, Heading)>,
    symbols: Vec<(Range<usize>, Symbol)>,
}

/// How much of a node the previous outline still knows.
#[derive(PartialEq)]
enum Kept {
    /// The node and its old entries are as they were.
    Yes,
    /// The node is new or edited, though parts of it may be kept.
    No,
    /// The node is kept but sits elsewhere now, so all of it needs a walk.
    Moved,
}

/// What an entry depends on outside of its node: whether headings count there,
/// and the environment that gives a caption its kind.
#[derive(PartialEq)]
struct Context {
    heading_free: bool,
    caption: Option<HeadingKind>,
}

impl<'a> Reuse<'a> {
    fn new(old: &Outline, edits: &[InputEdit], old_tree: &'a Tree) -> Self {
        let mut dirty: Vec<Range<usize>> = Vec::with_capacity(edits.len());
        for (index, edit) in edits.iter().enumerate() {
            let mut range = edit.start_byte..edit.new_end_byte;
            for later in &edits[index + 1..] {
                range = edited_range(range, later);
            }
            dirty.push(range);
        }
        dirty.sort_by_key(|range| range.start);
        let mut merged: Vec<Range<usize>> = Vec::with_capacity(dirty.len());
        for range in dirty {
            match merged.last_mut() {
                Some(last) if range.start <= last.end => last.end = last.end.max(range.end),
                _ => merged.push(range),
            }
        }
        let headings = old
            .heading_nodes
            .iter()
            .zip(&old.headings)
            .filter_map(|(node, heading)| {
                let (node, line) = moved(node.clone(), heading.line, edits)?;
                Some((node, Heading { line, ..heading.clone() }))
            })
            .collect();
        let symbols = old
            .symbol_nodes
            .iter()
            .zip(&old.symbols)
            .filter_map(|(node, symbol)| Some((moved(node.clone(), 0, edits)?.0, symbol.clone())))
            .collect();
        Self { old_tree, dirty: merged, headings, symbols }
    }

    /// Whether the node under `cursor` is a subtree of the old tree that the parser
    /// kept, away from the edits and in the same context, so its old entries hold.
    fn check(&self, config: &LanguageConfig, cursor: &TreeCursor, rope: &Rope) -> Kept {
        let node = cursor.node();
        let range = node.byte_range();
        let index = self.dirty.partition_point(|dirty| dirty.end < range.start);
        if self.dirty.get(index).is_some_and(|dirty| touches(dirty, &range)) {
            return Kept::No;
        }
        // Old entries are told apart by their bytes, so a parent with the same bytes
        // that the walk saw already would come twice.
        let mut parent = cursor.clone();
        if parent.goto_parent() && parent.node().byte_range() == range {
            return Kept::No;
        }
        // The parser hands kept subtrees to the new tree as they are, so their ids match.
        let mut old = self.old_tree.walk();
        while old.node().id() != node.id() {
            let current = old.node();
            if current.start_byte() > range.start
                || current.end_byte() < range.end
                || old.goto_first_child_for_byte(range.start).is_none()
            {
                return Kept::No;
            }
        }
        match context(config, cursor.clone(), rope, false) {
            Some(new) if context(config, old, rope, true).as_ref() == Some(&new) => Kept::Yes,
            _ => Kept::Moved,
        }
    }

    /// Copies the old entries of the nodes inside `node`, which did not change.
    fn copy(&self, node: &Range<usize>, outline: &mut Outline) {
        fn inside<T: Clone>(entries: &[(Range<usize>, T)], node: &Range<usize>, nodes: &mut Vec<Range<usize>>, out: &mut Vec<T>) {
            let first = entries.partition_point(|(range, _)| range.start < node.start);
            for (range, entry) in &entries[first..] {
                if range.start > node.end || (range.start == node.end && !node.is_empty()) {
                    break;
                }
                if range.end <= node.end {
                    nodes.push(range.clone());
                    out.push(entry.clone());
                }
            }
        }
        inside(&self.headings, node, &mut outline.heading_nodes, &mut outline.headings);
        inside(&self.symbols, node, &mut outline.symbol_nodes, &mut outline.symbols);
    }
}

/// The context of the node under `cursor`, from its ancestors. In the old tree an
/// environment whose name an edit touched has no answer, as its text is gone.
fn context(config: &LanguageConfig, mut cursor: TreeCursor, rope: &Rope, old: bool) -> Option<Context> {
    let mut context = Context { heading_free: false, caption: None };
    while cursor.goto_parent() {
        let node = cursor.node();
        context.heading_free |= config.heading_skip.has(node);
        if context.caption.is_none()
            && node.kind() == "generic_environment"
            && let Some(begin) = node.child_by_field_name("begin")
        {
            if old && begin.has_changes() {
                return None;
            }
            if let Some(name) = begin.child_by_field_name("name") {
                let name = rope.read(name.byte_range());
                if name.contains("table") {
                    context.caption = Some(HeadingKind::Table);
                } else if name.contains("figure") {
                    context.caption = Some(HeadingKind::Figure);
                }
            }
        }
    }
    Some(context)
}

/// Whether two ranges overlap or meet, which also counts an empty one at either end.
fn touches(a: &Range<usize>, b: &Range<usize>) -> bool {
    a.start <= b.end && b.start <= a.end
}

/// Where `range` of the text before `edit` ends up, grown to hold the edit when they touch.
fn edited_range(range: Range<usize>, edit: &InputEdit) -> Range<usize> {
    let shift = |offset: usize| offset - edit.old_end_byte + edit.new_end_byte;
    if range.end < edit.start_byte {
        range
    } else if range.start > edit.old_end_byte {
        shift(range.start)..shift(range.end)
    } else {
        let end = if range.end > edit.old_end_byte { shift(range.end) } else { edit.new_end_byte };
        range.start.min(edit.start_byte)..end
    }
}

/// Moves the node of an old entry and its line past `edits`. A node that an edit
/// touches is gone, as the walk visits it again.
fn moved(mut node: Range<usize>, mut line: usize, edits: &[InputEdit]) -> Option<(Range<usize>, usize)> {
    for edit in edits {
        if node.end < edit.start_byte {
            continue;
        }
        if node.start <= edit.old_end_byte {
            return None;
        }
        node = node.start - edit.old_end_byte + edit.new_end_byte..node.end - edit.old_end_byte + edit.new_end_byte;
        // Symbols have no line and pass 0, which must not underflow.
        line = (line + edit.new_end_position.row).saturating_sub(edit.old_end_position.row);
    }
    Some((node, line))
}

/// Walks with one cursor instead of recursing, because deeply nested groups in a
/// document must not overflow the stack of the parse thread. With `reuse`, nodes
/// away from the changes keep their old entries, so typing costs a short walk.
fn collect_outline(config: &LanguageConfig, tree: &Tree, rope: &Rope, reuse: Option<&Reuse>) -> Outline {
    let mut outline = Outline::default();
    let mut cursor = tree.walk();
    let mut depth = 0;
    // Below this depth headings don't count. Symbols still do, like a label in a heading.
    let mut heading_free_from: Option<usize> = None;
    // Below this depth nothing is kept, which saves asking every node.
    let mut moved_from: Option<usize> = None;
    loop {
        let node = cursor.node();
        let range = node.byte_range();
        let mut walk_below = false;
        let kept = match reuse {
            Some(reuse) if moved_from.is_none() => reuse.check(config, &cursor, rope),
            _ => Kept::No,
        };
        if kept == Kept::Moved {
            moved_from = Some(depth);
        }
        if let (Kept::Yes, Some(reuse)) = (&kept, reuse) {
            reuse.copy(&range, &mut outline);
        } else {
            if let Some(rules) = config.headings
                && heading_free_from.is_none()
                && config.heading_kinds.has(node)
                && let Some(found) = (rules.find)(node, rope)
            {
                outline.headings.push(heading(node, found, rope));
                outline.heading_nodes.push(range.clone());
            }
            if let Some(rules) = config.symbols
                && config.symbol_kinds.has(node)
                && let Some((kind, name)) = (rules.find)(node)
            {
                outline.symbols.push(Symbol { kind, name: rope.byte_slice(name).to_string() });
                outline.symbol_nodes.push(range);
            }
            if heading_free_from.is_none() && config.heading_skip.has(node) {
                heading_free_from = Some(depth);
            }
            walk_below = config.symbols.is_some() || heading_free_from.is_none();
        }
        if walk_below && cursor.goto_first_child() {
            depth += 1;
            continue;
        }
        loop {
            if heading_free_from == Some(depth) {
                heading_free_from = None;
            }
            if moved_from == Some(depth) {
                moved_from = None;
            }
            if cursor.goto_next_sibling() {
                break;
            }
            if !cursor.goto_parent() {
                return outline;
            }
            depth -= 1;
        }
    }
}

struct LanguageConfig {
    language: TsLanguage,
    query: Query,
    kinds: Vec<Option<HighlightKind>>,
    headings: Option<HeadingRules>,
    symbols: Option<SymbolRules>,
    heading_kinds: KindTable,
    heading_skip: KindTable,
    symbol_kinds: KindTable,
    /// Second grammar that parses the ranges of `inline` nodes in Markdown.
    inline: Option<Box<LanguageConfig>>,
}

impl LanguageConfig {
    fn new(
        language: TsLanguage,
        source: &str,
        headings: Option<HeadingRules>,
        symbols: Option<SymbolRules>,
        inline: Option<Box<LanguageConfig>>,
    ) -> Self {
        let query = Query::new(&language, source).expect("bundled highlight query is valid");
        let kinds = query
            .capture_names()
            .iter()
            .map(|name| kind_for_capture(name))
            .collect();
        let heading_kinds = KindTable::new(&language, headings.map_or(&[], |rules| rules.kinds));
        let heading_skip = KindTable::new(&language, headings.map_or(&[], |rules| rules.skip));
        let symbol_kinds = KindTable::new(&language, symbols.map_or(&[], |rules| rules.kinds));
        Self {
            language,
            query,
            kinds,
            headings,
            symbols,
            heading_kinds,
            heading_skip,
            symbol_kinds,
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
                    Some(LATEX_SYMBOLS),
                    None,
                )
            })),
            Language::Typst => Some(TYPST.get_or_init(|| {
                Self::new(
                    codebook_tree_sitter_typst::LANGUAGE.into(),
                    include_str!("../queries/typst.scm"),
                    Some(TYPST_HEADINGS),
                    Some(TYPST_SYMBOLS),
                    None,
                )
            })),
            Language::Markdown => Some(MARKDOWN.get_or_init(|| {
                Self::new(
                    tree_sitter_md::LANGUAGE.into(),
                    tree_sitter_md::HIGHLIGHT_QUERY_BLOCK,
                    Some(MARKDOWN_HEADINGS),
                    None,
                    Some(Box::new(Self::new(
                        tree_sitter_md::INLINE_LANGUAGE.into(),
                        tree_sitter_md::HIGHLIGHT_QUERY_INLINE,
                        None,
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
    outline: Arc<Outline>,
    /// Edits since the text that `outline` belongs to, which `tree` has seen.
    edits: Vec<InputEdit>,
    applied_results: u64,
    applied_version: u64,
}

/// A parse that holds only owned data and can run on any thread.
pub struct ParseJob {
    config: &'static LanguageConfig,
    rope: Rope,
    old_tree: Option<Tree>,
    /// The outline before `edits`, both of which go with `old_tree`.
    old_outline: Arc<Outline>,
    edits: Vec<InputEdit>,
    version: u64,
}

pub struct ParseResult {
    tree: Option<Tree>,
    outline: Outline,
    version: u64,
    timings: ParseTimings,
}

/// Where the time of a parse went, for benchmarks.
#[derive(Clone, Copy, Debug, Default)]
pub struct ParseTimings {
    pub parse: Duration,
    pub outline: Duration,
}

impl ParseResult {
    pub fn timings(&self) -> ParseTimings {
        self.timings
    }
}

impl Syntax {
    pub fn new(language: Language) -> Self {
        Self {
            config: LanguageConfig::get(language),
            tree: None,
            outline: Arc::default(),
            edits: Vec::new(),
            applied_results: 0,
            applied_version: 0,
        }
    }

    pub fn edit(&mut self, edit: &InputEdit) {
        if let Some(tree) = &mut self.tree {
            tree.edit(edit);
            self.edits.push(*edit);
        }
    }

    pub fn parse_job(&self, rope: &Rope, version: u64) -> Option<ParseJob> {
        Some(ParseJob {
            config: self.config?,
            rope: rope.clone(),
            old_tree: self.tree.clone(),
            old_outline: self.outline.clone(),
            edits: self.edits.clone(),
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
        self.outline = Arc::new(result.outline);
        self.edits.clear();
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
        &self.outline.headings
    }

    /// Names that the document defines, as of the last finished parse.
    pub fn symbols(&self) -> &[Symbol] {
        &self.outline.symbols
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
            if let Some(inline) = &config.inline {
                collect_inline_spans(inline, tree, rope, &range, &mut spans);
            }
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

/// Parses the inline text that touches `range`, one block at a time. Text outside of
/// the view costs nothing this way, and with many ranges in one parse tree-sitter takes
/// quadratic time: 57 s for 100,000 paragraphs. Inline markup cannot cross blocks anyway.
fn collect_inline_spans(
    config: &LanguageConfig,
    blocks: &Tree,
    rope: &Rope,
    range: &Range<usize>,
    spans: &mut Vec<HighlightSpan>,
) {
    let mut parser = Parser::new();
    parser
        .set_language(&config.language)
        .expect("grammar ABI matches tree-sitter");
    let mut cursor = blocks.walk();
    loop {
        let node = cursor.node();
        if node.start_byte() >= range.end {
            return;
        }
        if node.end_byte() > range.start {
            if node.kind() == "inline" {
                if parser.set_included_ranges(&[node.range()]).is_ok()
                    && let Some(tree) = parse_rope(&mut parser, rope, None)
                {
                    collect_spans(config, &tree, rope, range, spans);
                }
            } else if cursor.goto_first_child_for_byte(range.start).is_some() {
                continue;
            }
        }
        while !cursor.goto_next_sibling() {
            if !cursor.goto_parent() {
                return;
            }
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
        let started = Instant::now();
        let tree = parse_rope(&mut parser, &self.rope, self.old_tree.as_ref());
        let parsed = Instant::now();

        let outline = match (&tree, &self.old_tree) {
            (Some(tree), Some(old_tree)) => {
                let reuse = Reuse::new(&self.old_outline, &self.edits, old_tree);
                collect_outline(self.config, tree, &self.rope, Some(&reuse))
            }
            (Some(tree), None) => collect_outline(self.config, tree, &self.rope, None),
            (None, _) => Outline::default(),
        };

        ParseResult {
            tree,
            outline,
            version: self.version,
            timings: ParseTimings {
                parse: parsed - started,
                outline: parsed.elapsed(),
            },
        }
    }
}

/// Hands tree-sitter the rope's own chunks of about 1 KB, without copying. With an
/// old tree the parser keeps all but the edited part: 1 ms per key on 2 MB of LaTeX,
/// against 1.1 s for the first parse, so the chunks are not what costs time.
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

    fn symbols_of(language: Language, text: &str) -> Vec<(SymbolKind, String)> {
        let rope = Rope::from_str(text);
        let mut syntax = Syntax::new(language);
        let result = syntax.parse_job(&rope, 1).unwrap().run();
        assert!(syntax.apply(result, &[]));
        syntax.symbols().iter().map(|symbol| (symbol.kind, symbol.name.clone())).collect()
    }

    #[test]
    fn latex_symbols_are_labels_and_own_commands() {
        let symbols = symbols_of(
            Language::Latex,
            "\\usepackage[round]{natbib, amsmath}\n\\newcommand{\\foo}{x}\n\\newcommand\\bar{y}\n\\def\\baz{z}\n\\section{A}\\label{sec:a}\n",
        );
        assert_eq!(
            symbols,
            [
                (SymbolKind::Package, "natbib, amsmath".to_string()),
                (SymbolKind::Command, "\\foo".to_string()),
                (SymbolKind::Command, "\\bar".to_string()),
                (SymbolKind::Command, "\\baz".to_string()),
                (SymbolKind::Label, "sec:a".to_string()),
            ]
        );
    }

    #[test]
    fn typst_symbols_are_labels_and_bindings() {
        let symbols = symbols_of(Language::Typst, "#let width = 3\n#let add(a, b) = a + b\n= Intro <intro>\n");
        assert_eq!(
            symbols,
            [
                (SymbolKind::Binding, "width".to_string()),
                (SymbolKind::Binding, "add".to_string()),
                (SymbolKind::Label, "intro".to_string()),
            ]
        );
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
    fn markdown_inlines_do_not_depend_on_the_view() {
        let text = "Plain *em* one\n\nOpen **never closed\n\nAnother **strong** one\n\n- item `code`\n";
        let rope = Rope::from_str(text);
        let mut syntax = Syntax::new(Language::Markdown);
        let result = syntax.parse_job(&rope, 1).unwrap().run();
        assert!(syntax.apply(result, &[]));
        let sorted = |range: Range<usize>| {
            let mut spans: Vec<_> = syntax
                .highlights(&rope, range)
                .into_iter()
                .map(|span| (span.range.start, span.range.end, span.kind))
                .collect();
            spans.sort_by_key(|span| (span.0, span.1));
            spans.dedup();
            spans
        };
        let all = sorted(0..text.len());
        let of = |word: &str, kind| {
            let start = text.find(word).unwrap();
            all.iter().any(|span| span.0 == start && span.1 == start + word.len() && span.2 == kind)
        };
        assert!(of("*em*", HighlightKind::Emphasis));
        assert!(of("**strong**", HighlightKind::Strong));
        assert!(of("`code`", HighlightKind::Raw));
        // The open marker stays in its own paragraph.
        assert!(!all.iter().any(|span| span.2 == HighlightKind::Strong && span.0 == text.find("**never").unwrap()));
        for start in (0..text.len()).step_by(3) {
            let range = start..(start + 8).min(text.len());
            let touching: Vec<_> = all.iter().copied().filter(|span| span.0 < range.end && range.start < span.1).collect();
            assert_eq!(sorted(range), touching);
        }
        assert!(syntax.highlights(&rope, text.len()..text.len()).is_empty());
    }

    #[test]
    fn outline_sees_what_broken_and_skipped_nodes_hold() {
        // The label sits in a heading, whose inside holds no further headings.
        let typst = "= Title <first>\n#let x = 1\n== Next\n";
        let rope = Rope::from_str(typst);
        let mut syntax = Syntax::new(Language::Typst);
        let result = syntax.parse_job(&rope, 1).unwrap().run();
        assert!(syntax.apply(result, &[]));
        assert_eq!(syntax.headings().iter().map(|heading| heading.level).collect::<Vec<_>>(), [1, 2]);
        let names: Vec<_> = syntax.symbols().iter().map(|symbol| symbol.name.as_str()).collect();
        assert_eq!(names, ["first", "x"]);

        let latex = "\\section{A}\\label{a}\n}}{ \\section{B}\\label{b}\n$\\section{no}$\n";
        let rope = Rope::from_str(latex);
        let mut syntax = Syntax::new(Language::Latex);
        let result = syntax.parse_job(&rope, 1).unwrap().run();
        assert!(syntax.apply(result, &[]));
        let titles: Vec<_> = syntax.headings().iter().map(|heading| heading.title.as_str()).collect();
        assert_eq!(titles.first(), Some(&"{A}"));
        assert!(!titles.contains(&"{no}"), "{titles:?}");
        assert!(syntax.symbols().iter().any(|symbol| symbol.name == "a"));
    }

    #[test]
    fn starred_sections_have_no_number() {
        let rope = Rope::from_str("\\section*{A}\n\\section{B}\n\\subsection*{C}\n");
        let mut syntax = Syntax::new(Language::Latex);
        let result = syntax.parse_job(&rope, 1).unwrap().run();
        assert!(syntax.apply(result, &[]));
        let numbered: Vec<_> = syntax.headings().iter().map(|heading| heading.numbered).collect();
        assert_eq!(numbered, [false, true, false]);
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

    /// Replaces `range` of `rope` and returns the edit for the trees.
    fn replace(rope: &mut Rope, range: Range<usize>, text: &str) -> InputEdit {
        let point = |rope: &Rope, offset: usize| {
            let row = rope.byte_to_line(offset);
            tree_sitter::Point::new(row, offset - rope.line_to_byte(row))
        };
        let (start_position, old_end_position) = (point(rope, range.start), point(rope, range.end));
        let (start, end) = (rope.byte_to_char(range.start), rope.byte_to_char(range.end));
        rope.remove(start..end);
        rope.insert(start, text);
        let new_end_byte = range.start + text.len();
        InputEdit {
            start_byte: range.start,
            old_end_byte: range.end,
            new_end_byte,
            start_position,
            old_end_position,
            new_end_position: point(rope, new_end_byte),
        }
    }

    /// Edits generated documents at random places, a few edits per parse, and
    /// checks the outline of each parse against a walk of the whole tree.
    fn incremental_outline_matches_a_full_walk(language: Language, pieces: &[&str], seed: u64) {
        let mut state = seed;
        let mut next = |below: usize| {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (state >> 33) as usize % below
        };
        let mut text = String::new();
        while text.len() < 4_000 {
            text.push_str(pieces[next(pieces.len())]);
        }
        let mut rope = Rope::from_str(&text);
        let mut syntax = Syntax::new(language);
        let config = syntax.config.unwrap();
        let mut version = 0;
        for round in 0..300 {
            for _ in 0..1 + next(3) {
                // The pieces are ASCII, so every byte starts a character.
                let at = next(rope.len_bytes() + 1);
                let mut end = (at + next(12)).min(rope.len_bytes());
                if next(3) > 0 {
                    end = at;
                }
                let inserted = if next(4) == 0 { "" } else { pieces[next(pieces.len())] };
                let edit = replace(&mut rope, at..end, inserted);
                syntax.edit(&edit);
            }
            version += 1;
            let result = syntax.parse_job(&rope, version).unwrap().run();
            let full = collect_outline(config, result.tree.as_ref().unwrap(), &rope, None);
            assert!(result.outline == full, "{language:?}, seed {seed}, round {round}:\n{rope}");
            assert!(syntax.apply(result, &[]));
        }
    }

    #[test]
    fn incremental_outline_matches_a_full_walk_in_latex() {
        let pieces = [
            "\\section{Intro}\\label{sec:a}\n",
            "\\subsection*{B}\n",
            "\\chapter{C}\n",
            "Some words here. ",
            "\n\n",
            "$x^2$ ",
            "\\[ y \\]",
            "$",
            "{",
            "}",
            "% comment \\section{no}\n",
            "\\begin{figure}\n\\caption{Fig}\\label{fig:a}\n\\end{figure}\n",
            "\\begin{table}\\caption{Tab}\\end{table}\n",
            "\\begin{table}",
            "\\end{table}",
            "\\begin{figure}",
            "table",
            "figure",
            "\\caption{Loose}",
            "\\newcommand{\\foo}{\\section{macro}}\n",
            "\\def\\bar{x}\n",
            "\\usepackage{amsmath}\n",
            "\\begin{verbatim}\n\\section{raw}\n\\end{verbatim}\n",
            "\\printbibliography\n",
            "\\begin{equation}\\label{eq:a} a \\end{equation}\n",
            "\\section{",
            "\\label{",
            "\\",
        ];
        for seed in 1..=4 {
            incremental_outline_matches_a_full_walk(Language::Latex, &pieces, seed);
        }
    }

    #[test]
    fn incremental_outline_matches_a_full_walk_in_typst_and_markdown() {
        let typst = [
            "= Title <a>\n",
            "== Next\n",
            "text and words ",
            "\n",
            "#let x = 1\n",
            "#let f(a) = a\n",
            "$ x $",
            "$",
            "[",
            "]",
            "```\n= raw\n```\n",
            "#figure(table(columns: 2), caption: [Tab])\n",
            "#figure(image(\"a.png\"), caption: [Fig], kind: table)\n",
            "#bibliography(\"refs.bib\")\n",
            "<label>",
            "*bold* ",
            "// note\n",
        ];
        let markdown = [
            "# One\n",
            "Two\n===\n",
            "Three\n---\n",
            "### Four\n",
            "text and words ",
            "\n",
            "\n\n",
            "```\n# code\n```\n",
            "```",
            "    indented\n",
            "- item\n",
            "> quote\n",
            "*em* ",
        ];
        for seed in 1..=2 {
            incremental_outline_matches_a_full_walk(Language::Typst, &typst, seed);
            incremental_outline_matches_a_full_walk(Language::Markdown, &markdown, seed);
        }
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

