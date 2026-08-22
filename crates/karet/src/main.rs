//! karet — a terminal IDE skeleton built from the karet-* toolkit.
//!
//! `karet [PATH]` opens an Explorer-first IDE shell rooted at `PATH` (default `.`):
//! a file explorer, a code window that dispatches on file type (text/code, image,
//! PDF, binary), in-file search, and workspace search. When `PATH` is a file it is
//! opened directly; when it is inside a git repository, the Source Control panel
//! lists the staged and working-tree changes (each opens as a diff tab).
//!
//! Editing, language intelligence, and source control route through the headless
//! `karet-session` backend; the remaining direct engine calls (diff rendering,
//! workspace search, remote-URL reads) are being migrated behind the same seam.

mod app;
mod cli;
mod clipboard;
mod command;
mod compat;
mod completion;
mod desktop;
mod doctor;
mod hover;
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
    if karet_supervisor::broker::requested() {
        std::process::exit(karet_supervisor::broker::run_from_env());
    }
    if karet_supervisor::supervisor::requested() {
        std::process::exit(karet_supervisor::supervisor::run_from_env());
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

/// The window/process title's workspace path, resolved against the real
/// environment.
///
/// Falls back to the root as given when the current directory cannot be read (a
/// deleted cwd), so a cosmetic string never fails startup.
pub(crate) fn window_title_path(root: &Path) -> String {
    let Ok(cwd) = std::env::current_dir() else {
        return root.display().to_string();
    };
    let base_dirs = directories::BaseDirs::new();
    title_path(
        root,
        &cwd,
        base_dirs.as_ref().map(directories::BaseDirs::home_dir),
    )
}

/// Render `root` for the window title: absolute, lexically normalized, with `home`
/// abbreviated to `~`.
///
/// The root reaching this point is whatever the user typed — most often the default
/// `.`, which is useless as a title. `home` is a parameter rather than an ambient
/// lookup so tests never depend on the machine's real home directory.
///
/// The displayed path stays lexical on purpose: `fs::canonicalize` resolves symlinks,
/// which would title the window with a path the user never typed, and fails outright
/// on a path that does not exist yet (`karet new-project/`).
fn title_path(root: &Path, cwd: &Path, home: Option<&Path>) -> String {
    // `join` returns `root` unchanged when it is already absolute.
    let absolute = lexically_normalize(&cwd.join(root));
    // An unusable `$HOME` abbreviates nothing: a relative one would be resolved
    // against the current directory, and `/` (some containers) would turn every
    // absolute path into a misleading `~/...`.
    let home = home
        .map(lexically_normalize)
        .filter(|home| home.is_absolute() && home.parent().is_some());
    match home.as_deref().and_then(|home| strip_home(&absolute, home)) {
        Some(rest) if rest.as_os_str().is_empty() => "~".to_owned(),
        Some(rest) => format!("~{}{}", std::path::MAIN_SEPARATOR, rest.display()),
        None => absolute.display().to_string(),
    }
}

/// The part of `absolute` below `home`, or `None` when it lies outside `home`.
///
/// Matching is component-wise, so a sibling like `/home/adaline` is never mistaken
/// for a child of `/home/ada`. When the lexical comparison fails, both sides are
/// resolved and compared again: `$HOME` and the current directory routinely disagree
/// about symlinks (`getcwd` reports the resolved path, `$HOME` whatever the login
/// record says), so `HOME=/mnt/scratch/ada` must still match a cwd of
/// `/var/mnt/scratch/ada/dev` when `/mnt` links to `/var/mnt`.
///
/// The returned remainder always comes from the comparison that matched, keeping the
/// title free of the resolved prefix.
fn strip_home(absolute: &Path, home: &Path) -> Option<PathBuf> {
    if let Ok(rest) = absolute.strip_prefix(home) {
        return Some(rest.to_path_buf());
    }
    let real_home = std::fs::canonicalize(home).ok()?;
    let (real, unborn) = canonicalize_existing_ancestor(absolute)?;
    let mut rest = real.strip_prefix(real_home).ok()?.to_path_buf();
    // A path that does not exist yet contributes its tail verbatim; `extend` (not
    // `join`) so an empty tail cannot append a trailing separator.
    rest.extend(unborn.iter().rev());
    Some(rest)
}

/// Resolve the longest existing ancestor of `path`, plus the components below it in
/// reverse order.
///
/// `fs::canonicalize` needs every component to exist, but a workspace root may be a
/// directory the user is about to create. Returns `None` only when nothing along the
/// path resolves — a path with no readable ancestor cannot be compared through
/// symlinks at all.
fn canonicalize_existing_ancestor(path: &Path) -> Option<(PathBuf, Vec<&std::ffi::OsStr>)> {
    let mut unborn = Vec::new();
    let mut probe = path;
    loop {
        if let Ok(real) = std::fs::canonicalize(probe) {
            return Some((real, unborn));
        }
        // `file_name` is `None` at a root or bare prefix, ending the walk.
        unborn.push(probe.file_name()?);
        probe = probe.parent()?;
    }
}

/// Resolve `.` and `..` components without touching the filesystem.
fn lexically_normalize(path: &Path) -> PathBuf {
    use std::path::Component;

    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {},
            Component::ParentDir => {
                // Only a real segment can be popped. With nothing to pop, a relative
                // path keeps the `..` (it still means something) while a filesystem
                // root swallows it — `/..` is `/`.
                let poppable = !matches!(
                    out.components().next_back(),
                    None | Some(Component::RootDir | Component::Prefix(_) | Component::ParentDir)
                );
                if poppable {
                    out.pop();
                } else if !out.has_root() {
                    out.push("..");
                }
            },
            other => out.push(other.as_os_str()),
        }
    }
    out
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
    fn the_default_root_titles_the_window_with_the_current_directory() {
        let home = Path::new("/home/ada");
        let cwd = home.join("dev").join("karet");

        assert_eq!(
            title_path(Path::new("."), &cwd, Some(home)),
            format!("~{sep}dev{sep}karet", sep = std::path::MAIN_SEPARATOR)
        );
    }

    #[test]
    fn a_root_outside_the_home_directory_stays_absolute() {
        let absolute = Path::new("/etc").join("nginx");

        assert_eq!(
            title_path(&absolute, Path::new("/tmp"), Some(Path::new("/home/ada"))),
            absolute.display().to_string()
        );
    }

    #[test]
    fn the_home_directory_itself_is_just_a_tilde() {
        let home = Path::new("/home/ada");

        assert_eq!(title_path(Path::new("."), home, Some(home)), "~");
        assert_eq!(title_path(home, Path::new("/tmp"), Some(home)), "~");
    }

    #[test]
    fn dot_and_dot_dot_components_are_resolved_without_touching_the_filesystem() {
        let home = Path::new("/home/ada");
        let cwd = home.join("dev").join("karet").join("crates");

        assert_eq!(
            title_path(Path::new("../.."), &cwd, Some(home)),
            format!("~{sep}dev", sep = std::path::MAIN_SEPARATOR)
        );
        // A `..` at the filesystem root has nothing to pop and is dropped.
        assert_eq!(
            title_path(Path::new("/../.."), &cwd, Some(home)),
            Path::new("/").display().to_string()
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_home_directory_reached_through_a_symlink_is_still_abbreviated()
    -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::symlink;

        // `HOME=/mnt/scratch/ada` with `/mnt` -> `/var/mnt` leaves `$HOME` and the
        // current directory (which the kernel reports resolved) spelling the same
        // directory differently.
        let dir = tempfile::tempdir()?;
        let real_home = dir.path().join("var").join("ada");
        std::fs::create_dir_all(real_home.join("dev"))?;
        symlink(dir.path().join("var"), dir.path().join("link"))?;
        let home = dir.path().join("link").join("ada");

        let sep = std::path::MAIN_SEPARATOR;
        assert_eq!(
            title_path(Path::new("."), &real_home.join("dev"), Some(&home)),
            format!("~{sep}dev")
        );
        assert_eq!(title_path(&real_home, Path::new("/tmp"), Some(&home)), "~");
        // The other direction too: a root typed through the symlink under a resolved
        // `$HOME`.
        assert_eq!(
            title_path(&home.join("dev"), Path::new("/tmp"), Some(&real_home)),
            format!("~{sep}dev")
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn a_directory_that_does_not_exist_yet_is_abbreviated_through_a_symlink()
    -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::symlink;

        // `karet new/project` must title correctly before the directory exists, so the
        // resolved comparison walks up to the deepest ancestor that does.
        let dir = tempfile::tempdir()?;
        let real_home = dir.path().join("var").join("ada");
        std::fs::create_dir_all(&real_home)?;
        symlink(dir.path().join("var"), dir.path().join("link"))?;
        let home = dir.path().join("link").join("ada");

        assert_eq!(
            title_path(Path::new("new/project"), &real_home, Some(&home)),
            format!("~{sep}new{sep}project", sep = std::path::MAIN_SEPARATOR)
        );
        Ok(())
    }

    #[test]
    fn a_sibling_of_the_home_directory_is_never_abbreviated()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let home = dir.path().join("ada");
        let sibling = dir.path().join("adaline");
        std::fs::create_dir_all(&home)?;
        std::fs::create_dir_all(&sibling)?;

        assert_eq!(
            title_path(Path::new("."), &sibling, Some(&home)),
            sibling.display().to_string()
        );
        Ok(())
    }

    #[test]
    fn an_unusable_home_directory_abbreviates_nothing() {
        let cwd = Path::new("/etc").join("nginx");
        let absolute = cwd.display().to_string();

        // `HOME=/` would make every absolute path a misleading `~/...`.
        assert_eq!(
            title_path(Path::new("."), &cwd, Some(Path::new("/"))),
            absolute
        );
        // A relative `$HOME` would otherwise be resolved against the current directory.
        assert_eq!(
            title_path(Path::new("."), &cwd, Some(Path::new("etc"))),
            absolute
        );
        assert_eq!(
            title_path(Path::new("."), &cwd, Some(Path::new(""))),
            absolute
        );
    }

    #[test]
    fn an_unnormalized_home_directory_still_abbreviates() {
        let cwd = Path::new("/home/ada/dev");

        for home in ["/home/ada/", "/home/ada/.", "/home/ada/dev/.."] {
            assert_eq!(
                title_path(Path::new("."), cwd, Some(Path::new(home))),
                format!("~{sep}dev", sep = std::path::MAIN_SEPARATOR),
                "home: {home}"
            );
        }
    }

    #[test]
    fn a_machine_without_a_home_directory_still_gets_an_absolute_title() {
        let cwd = Path::new("/srv/work");

        assert_eq!(
            title_path(Path::new("."), cwd, None),
            cwd.display().to_string()
        );
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
