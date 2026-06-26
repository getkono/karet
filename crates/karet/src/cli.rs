//! Command-line interface.

use std::path::PathBuf;

use clap::Parser;

/// karet — a fast terminal viewer for your git diff.
///
/// Shows your staged changes and working-tree changes together, like VS Code's Source
/// Control panel.
#[derive(Debug, Parser)]
#[command(name = "karet", version, about)]
pub struct Cli {
    /// File or directory to inspect (defaults to the current directory).
    pub path: Option<PathBuf>,

    /// Disable syntax highlighting (also respects the NO_COLOR environment variable).
    #[arg(long)]
    pub no_syntax: bool,
}
