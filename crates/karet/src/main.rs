//! karet — a terminal IDE skeleton built from the karet-* toolkit.
//!
//! `karet [PATH]` opens an Explorer-first IDE shell rooted at `PATH` (default `.`):
//! a file explorer, a code window that dispatches on file type (text/code, image,
//! PDF, binary), in-file search, and workspace search. When `PATH` is a file it is
//! opened directly; when it is inside a git repository, the Source Control panel
//! lists the staged and working-tree changes (each opens as a diff tab).
//!
//! Routing through the headless `karet-session` backend is a deferred step; for now
//! the shell calls the engines directly.

// `clipboard`/`input` are scaffolding for future editing work and are not yet wired
// into the read-only shell.
#![allow(dead_code)]

mod app;
mod cli;
mod clipboard;
mod command;
mod input;
mod keymap;
mod overlay;
mod render;
mod tab;
mod ui;
mod workspace;

use std::path::{Path, PathBuf};

use clap::Parser;
use color_eyre::eyre::eyre;
use karet_vcs::{Repository, Selection, VcsError};

fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;
    let cli = cli::Cli::parse();
    let path = cli.path.clone().unwrap_or_else(|| PathBuf::from("."));
    let syntax = !cli.no_syntax && std::env::var_os("NO_COLOR").is_none();

    // Resolve the workspace root and an optional initial file.
    let (root, initial_file) = if path.is_file() {
        let root = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
        (root, Some(path.clone()))
    } else {
        (path.clone(), None)
    };

    // Collect VCS changes when inside a repository; the shell still opens otherwise.
    let (staged, working) = match Repository::discover(&path) {
        Ok(repo) => {
            let pathspec = (path != Path::new(".")).then_some(path.as_path());
            let staged = repo
                .changes(Selection::Staged, pathspec)
                .map_err(|e| eyre!("{e}"))?;
            let working = repo
                .changes(Selection::Unstaged, pathspec)
                .map_err(|e| eyre!("{e}"))?;
            (staged, working)
        }
        Err(VcsError::NotARepository) => (Vec::new(), Vec::new()),
        Err(e) => return Err(eyre!("{e}")),
    };

    let mut app = app::App::new(root, staged, working, syntax);
    if let Some(file) = initial_file {
        app.open_initial(&file);
    }
    app::run(app)
}
