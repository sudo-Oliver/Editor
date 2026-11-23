//! What one source file says about citations and references, from a single
//! pass over its text: [`crate::tex`] for LaTeX and [`crate::typst`] for Typst.

use std::ops::Range;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Scan {
    /// Keys in `\cite…{}`, `\nocite{}` or Typst's `#cite(<…>)`.
    pub cites: Vec<Key>,
    /// `\nocite{*}`: every entry counts as cited.
    pub cite_all: bool,
    /// Labels in `\ref{}` and its kind.
    pub refs: Vec<Key>,
    /// Typst's `@name`, which cites an entry or refers to a label, whichever exists.
    pub either: Vec<Key>,
    pub labels: Vec<Label>,
    /// Keys a `thebibliography` environment defines by hand with `\bibitem`.
    pub bibitems: Vec<Key>,
    pub includes: Vec<Include>,
    pub bibliographies: Vec<BibSource>,
    pub packages: Vec<Package>,
    /// `\externaldocument[prefix]{name}` of the xr packages.
    pub external_documents: Vec<External>,
    /// The main file this one names: `% !TEX root = …` or `\documentclass[main.tex]{subfiles}`.
    pub root: Option<String>,
    /// Whether it has `\documentclass`, so it can be compiled on its own.
    pub document: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Key {
    pub name: String,
    pub range: Range<usize>,
    /// The command without its backslash, such as `parencite` or `cref`.
    pub command: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Label {
    pub name: String,
    pub range: Range<usize>,
    /// The environment that numbers it, such as `figure` or `equation`, or
    /// the sectioning command before it, such as `section`. Empty if neither.
    pub env: String,
    /// The caption of its float, or the title of its section.
    pub caption: Option<String>,
    /// `\label` came before `\caption` in a float, so `\ref` gives the section's number.
    pub before_caption: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Include {
    pub path: String,
    pub range: Range<usize>,
    pub kind: IncludeKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IncludeKind {
    /// `\input`, which tries the name with `.tex` first.
    Input,
    /// `\include`, which always adds `.tex`.
    Include,
    Subfile,
    /// `\import{directory}{file}`; `relative` for `\subimport`, which starts
    /// at the folder of the file that says it.
    Import { directory: String, relative: bool },
    /// Typst's `#include "file.typ"`.
    Typst,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BibSource {
    pub name: String,
    pub range: Range<usize>,
    pub kind: BibKind,
    /// `[location=remote]`: a URL, which is not on the disk.
    pub remote: bool,
    /// `[datatype=ris]` and other formats that are not BibTeX.
    pub foreign: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BibKind {
    /// `\bibliography{name}`, which BibTeX reads as `name.bib`.
    Bibliography,
    /// biblatex's `\addbibresource{name.bib}`, whose name is the whole file name.
    Resource,
    /// Typst's `#bibliography("name.bib")`, relative to the file.
    Typst,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Package {
    pub name: String,
    pub options: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct External {
    pub name: String,
    pub prefix: String,
}

/// Splits a comma list of keys. Names with `#` or `\` are macro parameters
/// or commands, not keys, and are left out.
pub(crate) fn push_keys(into: &mut Vec<Key>, text: &str, offset: usize, command: &str) {
    let mut start = 0;
    for piece in text.split(',') {
        let trimmed = piece.trim();
        if !trimmed.is_empty() && !trimmed.contains(['#', '\\', '{', '}']) {
            let at = offset + start + (piece.len() - piece.trim_start().len());
            into.push(Key { name: trimmed.to_string(), range: at..at + trimmed.len(), command: command.to_string() });
        }
        start += piece.len() + 1;
    }
}
