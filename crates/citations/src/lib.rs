//! Everything the editor knows about citations and cross references, from
//! LaTeX and Typst alone: the bibliographies a project uses, their entries,
//! the labels, and what is wrong with them. No network and no external
//! programs; add-ons bring Zotero and the like.

use std::ops::Range;

pub mod aux;
pub mod bib;
pub mod fuzzy;
pub mod hayagriva;
pub mod index;
pub mod lint;
pub mod project;
pub mod scan;
pub mod tex;
pub mod text;
pub mod typst;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    Error,
    Warning,
    Hint,
}

/// Something wrong at a byte range of a file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub range: Range<usize>,
    pub severity: Severity,
    pub message: String,
}
