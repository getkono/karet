//! Command-line interface.

use std::path::PathBuf;

use clap::Parser;

/// karet — a terminal IDE: file explorer, code window, and search.
///
/// Opens an Explorer-first shell rooted at the given path. A file opens directly; a
/// git repository's changes appear in the Source Control panel.
#[derive(Debug, Parser)]
#[command(name = "karet", version, about)]
pub struct Cli {
    /// File or directory to open (defaults to the current directory).
    pub path: Option<PathBuf>,

    /// Disable syntax highlighting (also respects the NO_COLOR environment variable).
    #[arg(long)]
    pub no_syntax: bool,
}
