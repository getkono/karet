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
mod completion;
mod desktop;
mod doctor;
mod editing;
mod keymap;
mod links;
mod logging;
mod notify;
mod outline;
mod overlay;
mod remote;
mod render;
mod tab;
mod term_caps;
mod ui;
mod workspace;

use std::path::Path;
use std::path::PathBuf;

use clap::Parser;

fn main() -> color_eyre::Result<()> {
    if karet_session::lsp_broker::requested() {
        std::process::exit(karet_session::lsp_broker::run_from_env());
    }
    if karet_session::process_supervisor::requested() {
        std::process::exit(karet_session::process_supervisor::run_from_env());
    }
    color_eyre::install()?;
    let cli = cli::Cli::parse();
    if cli.log {
        logging::report_paths()?;
        return Ok(());
    }
    let _logging_guard = match logging::init() {
        Ok(guard) => Some(guard),
        Err(error) => {
            eprintln!("karet: logging disabled: {error}");
            None
        },
    };

    // `--install-desktop` / `--uninstall-desktop` act like subcommands: manage the
    // per-user desktop entry and exit — no config load, never enter the TUI. (clap
    // rejects passing both together.)
    if cli.install_desktop {
        std::process::exit(desktop::run_install());
    }
    if cli.uninstall_desktop {
        std::process::exit(desktop::run_uninstall());
    }

    let path = cli.path.clone().unwrap_or_else(|| PathBuf::from("."));
    let syntax = !cli.no_syntax && std::env::var_os("NO_COLOR").is_none();

    // Resolve the workspace root and an optional initial file.
    let (root, initial_file) = startup_target(path);

    // Load the layered JSONC configuration for this workspace (project/user/system,
    // over sane defaults). Diagnostics are handed to the app to surface as startup
    // notifications; loading itself never fails.
    let mut loaded_config = karet_session::config::load_report(std::slice::from_ref(&root));

    // `--doctor` acts like a subcommand: run the terminal diagnostics against the
    // loaded settings and exit — never enter the alternate screen or the app loop.
    if cli.doctor {
        std::process::exit(doctor::run(&loaded_config.settings));
    }

    // Resolve `--capture*` up front for the same fail-fast reason as `--command`
    // below: a malformed grid must not silently capture a size nobody asked for.
    let capture = if cli.capture {
        match cli.capture_spec() {
            Ok(spec) => Some(spec),
            Err(error) => {
                eprintln!("karet: --capture-size: {error}");
                std::process::exit(2);
            },
        }
    } else {
        None
    };

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

    // Read every `--diff` pair up front for the same reason: an unreadable file
    // fails fast on stderr instead of surfacing inside a TUI an automation run
    // cannot see. `None` content marks a side whose bytes are not UTF-8; the app
    // renders that pair as a binary change.
    let mut startup_diffs: Vec<(PathBuf, PathBuf, Option<String>, Option<String>)> = Vec::new();
    for pair in cli.diff.chunks(2) {
        let [old, new] = pair else {
            // Unreachable: clap's `num_args = 2` accepts only whole pairs.
            continue;
        };
        let (old, new) = (
            resolve_under_root(&root, old),
            resolve_under_root(&root, new),
        );
        let read = |path: &Path| match std::fs::read(path) {
            Ok(bytes) => Some(String::from_utf8(bytes).ok()),
            Err(error) => {
                eprintln!("karet: --diff: cannot read {}: {error}", path.display());
                None
            },
        };
        match (read(&old), read(&new)) {
            (Some(old_text), Some(new_text)) => startup_diffs.push((old, new, old_text, new_text)),
            _ => std::process::exit(2),
        }
    }

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
    for (old, new, old_text, new_text) in startup_diffs {
        app.open_startup_diff(&old, &new, old_text, new_text);
    }
    if !cli.split.is_empty() {
        // The editor rectangle is only computed on the first draw; seed it with the
        // terminal size so the split-room guard has a real budget now. The draw loop
        // recomputes the exact rectangle every frame, so an approximation is fine. A
        // capture has no terminal to measure (`crossterm::terminal::size` fails
        // without a tty), so its requested grid is the budget instead.
        if let Some((w, h)) = capture
            .map(|spec| (spec.cols, spec.rows))
            .or_else(|| crossterm::terminal::size().ok())
        {
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
    // `--capture` acts like the other automation flags: render the shell off-screen,
    // write the frame to stdout, and return — never enter the alternate screen.
    match capture {
        Some(spec) => app::capture(app, spec),
        None => app::run(app),
    }
}

fn resolve_under_root(root: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    }
}

fn startup_target(path: PathBuf) -> (PathBuf, Option<PathBuf>) {
    if path.is_dir() || has_trailing_separator(&path) {
        return (path, None);
    }
    let root = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    (root, Some(path))
}

fn has_trailing_separator(path: &Path) -> bool {
    let last = path.as_os_str().as_encoded_bytes().last();
    last == Some(&(std::path::MAIN_SEPARATOR as u8)) || (cfg!(windows) && last == Some(&b'/'))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positional_path_distinguishes_files_directories_and_missing_targets()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let existing = dir.path().join("README.md");
        std::fs::write(&existing, "read me\n")?;

        assert_eq!(
            startup_target(existing.clone()),
            (dir.path().to_path_buf(), Some(existing))
        );
        assert_eq!(
            startup_target(dir.path().to_path_buf()),
            (dir.path().to_path_buf(), None)
        );
        let missing = dir.path().join("NEW.md");
        assert_eq!(
            startup_target(missing.clone()),
            (dir.path().to_path_buf(), Some(missing))
        );
        Ok(())
    }

    #[test]
    fn trailing_separator_keeps_a_missing_path_as_a_workspace() {
        let raw = format!("missing{}", std::path::MAIN_SEPARATOR);
        assert_eq!(
            startup_target(PathBuf::from(&raw)),
            (PathBuf::from(raw), None)
        );
    }
}
