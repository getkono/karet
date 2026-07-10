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
mod doctor;
mod editing;
mod keymap;
mod notify;
mod outline;
mod overlay;
mod render;
mod tab;
mod term_caps;
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
    let mut loaded_config = karet_session::config::load_report(std::slice::from_ref(&root));

    // `--doctor` acts like a subcommand: run the terminal diagnostics against the
    // loaded settings and exit — never enter the alternate screen or the app loop.
    if cli.doctor {
        std::process::exit(doctor::run(&loaded_config.settings));
    }

    // Resolve every `--command` name up front, so a typo fails fast on stderr with
    // a non-zero exit — an automation run must never enter (and wedge) the TUI on a
    // command that can never dispatch.
    let startup_commands: Vec<command::Command> = match cli
        .command
        .iter()
        .map(|name| command::resolve_named(name))
        .collect()
    {
        Ok(commands) => commands,
        Err(error) => {
            eprintln!("karet: --command: {error}");
            std::process::exit(2);
        },
    };

    if let Some(panel) = cli.startup_panel {
        loaded_config.settings.workbench.startup_panel = panel.into();
    }

    // The Source-Control panel is populated by the session's `VcsStatus` event
    // (seeded on startup and refreshed on filesystem changes), so the shell starts
    // with an empty panel rather than computing status here.
    let mut app = app::App::new(root.clone(), Vec::new(), Vec::new(), syntax)
        .with_loaded_config(loaded_config);
    // An explicit `--icons` flag (or `KARET_ICONS`) overrides `workbench.iconStyle`.
    if let Some(style) = cli.explicit_icon_style() {
        app = app.with_icons(style);
    }
    if let Some(file) = initial_file {
        app.open_initial(&file);
    } else if let Some(preview) = cli.preview.as_ref() {
        app.open_initial_preview(&resolve_under_root(&root, preview));
    } else if cli.open.is_empty()
        && let Some(readme) = startup_readme(&root)
    {
        app.open_initial_preview(&readme);
    }
    for file in &cli.open {
        app.open_initial(&resolve_under_root(&root, file));
    }
    if !cli.split.is_empty() {
        // The editor rectangle is only computed on the first draw; seed it with the
        // terminal size so the split-room guard has a real budget now. The draw loop
        // recomputes the exact rectangle every frame, so an approximation is fine.
        if let Ok((w, h)) = crossterm::terminal::size() {
            app.main_rect = ratatui::layout::Rect::new(0, 0, w, h);
        }
        for file in &cli.split {
            app.open_startup_split(&resolve_under_root(&root, file));
        }
    }
    if let Some(spec) = cli.goto.as_deref() {
        let goto = cli::parse_goto_spec(spec);
        app.open_startup_goto(&resolve_under_root(&root, &goto.path), goto.line, goto.col);
    }
    if let Some(focus) = cli.focus {
        app.apply_startup_focus(focus);
    }
    for command in startup_commands {
        app.apply_startup_command(command);
    }
    app::run(app)
}

fn resolve_under_root(root: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    }
}

fn startup_readme(root: &Path) -> Option<PathBuf> {
    if !root.join(".git").is_dir() {
        return None;
    }
    for name in ["README.md", "README.markdown", "README.txt", "README"] {
        let path = root.join(name);
        if path.is_file() {
            return Some(path);
        }
    }
    std::fs::read_dir(root)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_file())
        .find(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.to_ascii_lowercase().starts_with("readme."))
        })
}
