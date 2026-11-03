//! Runs external document compilers and turns their output into diagnostics.

mod config;
mod explain;
mod parse;
mod runner;
mod tools;

pub use config::{Config, Engine, Trigger, find_main};
pub use explain::{Explanation, explain};
pub use parse::{Diagnostic, Severity, parse_latex_log, parse_pandoc, parse_tectonic, parse_typst};
pub use runner::{BuildEvent, BuildManager, BuildOutcome, BuildRequest};
pub use tools::find_tool;
