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

// Some scaffolding is intentionally not wired into the shell yet: a handful of
// planned commands (scroll/indent), symmetry helpers exercised only by tests, the
// clipboard's read path, and render helpers.
#![allow(dead_code)]

mod app;
mod cli;
mod clipboard;
mod command;
mod compat;
mod editing;
mod keymap;
mod notify;
mod outline;
mod overlay;
mod render;
mod tab;
mod ui;
mod workspace;

use std::path::Path;
use std::path::PathBuf;

use clap::Parser;

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

    // Load the layered JSONC configuration for this workspace (project/user/system,
    // over sane defaults). Diagnostics are handed to the app to surface as startup
    // notifications; loading itself never fails.
    let loaded_config = karet_session::config::load_report(std::slice::from_ref(&root));

    // The Source-Control panel is populated by the session's `VcsStatus` event
    // (seeded on startup and refreshed on filesystem changes), so the shell starts
    // with an empty panel rather than computing status here.
    let mut app =
        app::App::new(root, Vec::new(), Vec::new(), syntax).with_loaded_config(loaded_config);
    // An explicit `--icons` flag (or `KARET_ICONS`) overrides `workbench.iconStyle`.
    if let Some(style) = cli.explicit_icon_style() {
        app = app.with_icons(style);
    }
    if let Some(file) = initial_file {
        app.open_initial(&file);
    }
    app::run(app)
}
