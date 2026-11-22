pub mod vim;
pub mod vim_map;
pub mod buffer;
pub mod conflicts;
pub mod line_diff;
pub mod latex_usage;
pub mod word_count;
pub mod selection;
pub mod snippet;
pub mod state;
pub mod syntax;

pub use buffer::Buffer;
pub use selection::{Selection, SelectionSet};
pub use state::{Decoration, DecorationKind, EditorState, LineChange, Mark, MarkSeverity, Motion, TextChange};
pub use syntax::{Heading, HeadingKind, HighlightKind, HighlightSpan, Language, ParseJob, ParseResult, ParseTimings, Symbol, SymbolKind, Syntax};
