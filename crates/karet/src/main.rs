//! karet — a minimal terminal git-diff viewer.
//!
//! `karet [PATH]` discovers the git repository containing `PATH` (default `.`) and
//! shows its diff like VS Code's default: staged changes if any are staged, else the
//! unstaged (working-tree) changes. It prints a message and exits when `PATH` is not
//! in a repository or there is nothing to show. This is the MVP composition root; the
//! richer editor (via `karet-session`) is future work.

// The merged `clipboard`/`input` modules are scaffolding for the future editor and
// are not yet wired into the diff viewer.
#![allow(dead_code)]

mod app;
mod cli;
mod clipboard;
mod input;
mod render;
mod ui;

use std::path::{Path, PathBuf};

use clap::Parser;
use color_eyre::eyre::eyre;
use karet_vcs::{Repository, Selection, VcsError};

fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;
    let cli = cli::Cli::parse();
    let path = cli.path.clone().unwrap_or_else(|| PathBuf::from("."));

    let repo = match Repository::discover(&path) {
        Ok(repo) => repo,
        Err(VcsError::NotARepository) => {
            println!("karet: not a git repository: {}", path.display());
            return Ok(());
        }
        Err(e) => return Err(eyre!("{e}")),
    };

    let selection = if cli.staged {
        Some(Selection::Staged)
    } else if cli.unstaged {
        Some(Selection::Unstaged)
    } else {
        repo.default_selection().map_err(|e| eyre!("{e}"))?
    };
    let Some(selection) = selection else {
        println!("karet: no changes");
        return Ok(());
    };

    // Scope the diff to the given path unless it's the current directory.
    let pathspec = (path != Path::new(".")).then_some(path.as_path());
    let changes = repo
        .changes(selection, pathspec)
        .map_err(|e| eyre!("{e}"))?;
    if changes.is_empty() {
        println!("karet: no changes");
        return Ok(());
    }

    let syntax = !cli.no_syntax && std::env::var_os("NO_COLOR").is_none();
    app::run(app::App::new(changes, selection, syntax))
}
