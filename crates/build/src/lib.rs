//! Runs external document compilers and turns their output into diagnostics.

mod blg;
mod config;
mod explain;
mod fontmap;
mod packages;
mod parse;
mod preamble;
mod process;
mod runner;
mod tidy;
mod tools;

pub use config::{Config, Engine, Trigger, find_main, write_main};
pub use explain::{Explanation, explain};
pub use packages::{Fix, Loaded, PackageFinding, ProjectUsage, check, loaded};
pub use parse::{Diagnostic, Severity, parse_latex_log, parse_pandoc, parse_tectonic, parse_typst};
pub use process::{Group, no_window, prepare as prepare_group, running as process_running};
pub use runner::{BuildEvent, BuildManager, BuildOutcome, BuildRequest, OUTPUT_DIRECTORY};
pub use tools::{add_directory, child_env, child_path, find_tool, plain_path, pretend_fresh, remove_directory, set_cache_directories};
