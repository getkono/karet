//! Command-line interface.

use std::path::PathBuf;

use clap::Parser;

/// karet — a fast terminal viewer for your git diff.
///
/// With no flags it shows staged changes if any are staged, otherwise the unstaged
/// (working-tree) changes — like VS Code's default.
#[derive(Debug, Parser)]
#[command(name = "karet", version, about)]
pub struct Cli {
    /// File or directory to inspect (defaults to the current directory).
    pub path: Option<PathBuf>,

    /// Show staged changes (HEAD vs the index).
    #[arg(long, conflicts_with = "unstaged")]
    pub staged: bool,

    /// Show unstaged changes (the index vs the working tree, plus untracked files).
    #[arg(long)]
    pub unstaged: bool,

    /// Disable syntax highlighting (also respects the NO_COLOR environment variable).
    #[arg(long)]
    pub no_syntax: bool,
}
