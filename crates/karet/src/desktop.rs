//! `karet --install-desktop` / `--uninstall-desktop`: desktop integration.
//!
//! Adds a per-user desktop entry that launches karet in the user's default
//! terminal, on the three desktop platforms karet supports: Linux following the
//! XDG Base Directory / Desktop Entry specs, macOS, and Windows 10/11. Every
//! artifact carries the same [`DISCLAIMER`]: karet needs a *modern* terminal, and
//! the OS default terminal may not be one.
//!
//! The module is layered so the interesting logic is pure and unit-testable on
//! every platform, regardless of the host it is compiled for:
//!
//! - **Content generators** ([`linux_desktop_entry`], [`icon_svg`],
//!   [`macos_info_plist`], [`macos_launcher_script`], [`windows_cmd_launcher`]) are
//!   plain string builders — no I/O, no `cfg`.
//! - **Planners** ([`linux_plan`], [`macos_plan`], [`windows_plan`]) compose a
//!   [`Plan`] from *injected* base directories, the executable path, and the
//!   version — no environment reads, so tests pass synthetic roots.
//! - **Executors** ([`install`], [`uninstall`]) apply a plan to the filesystem;
//!   they take the plan as data, so tests drive them against temp roots.
//! - Only [`current_plan`] (base-directory resolution) is `cfg(target_os)`-gated,
//!   and only [`run_install`]/[`run_uninstall`] read the real environment.
//!
//! On Windows the Start-Menu artifact is a plain-text `.cmd` launcher rather than
//! a binary `.lnk` shortcut: the pure-Rust `.lnk` writer (`mslnk`) is stale (last
//! release 2022) and its binary output cannot be exercised by this workspace's
//! cross-platform tests, whereas the `.cmd` is a fully-tested string with zero
//! added dependencies. The accepted tradeoff: the Windows Start-Menu entry has no
//! custom icon or description field.

use std::path::Path;
use std::path::PathBuf;

/// The compatibility disclaimer stamped into every platform's artifact and printed
/// on install. karet hard-requires the kitty keyboard protocol (see
/// `karet --doctor`), which the user's OS-default terminal may not speak, so the
/// desktop entry can make no promise about the terminal it lands in.
///
/// Kept to a single line with no `<`, `>`, `&`, backslash, or leading whitespace so
/// it drops verbatim into a `.desktop` `Comment`, an XML `<string>`, an `sh`
/// comment, and a `cmd` `rem` without escaping surprises.
pub(crate) const DISCLAIMER: &str = "Requires a modern terminal (kitty keyboard \
     protocol); karet offers no guarantees with your OS default terminal. Run \
     'karet --doctor' to check.";

/// The scalable application icon: a rounded square with a downward caret over a
/// prompt underline, in two colours (a dark slate ground and a single accent).
/// Referenced by the Linux desktop entry as `Icon=karet`.
pub(crate) const ICON_SVG: &str = r##"<?xml version="1.0" encoding="UTF-8"?>
<svg xmlns="http://www.w3.org/2000/svg" width="128" height="128" viewBox="0 0 128 128" role="img" aria-label="karet">
  <rect width="128" height="128" rx="28" fill="#1e2430"/>
  <path d="M40 44 L64 68 L88 44" fill="none" stroke="#8ab4f8" stroke-width="10" stroke-linecap="round" stroke-linejoin="round"/>
  <line x1="46" y1="90" x2="82" y2="90" stroke="#8ab4f8" stroke-width="10" stroke-linecap="round"/>
</svg>
"##;

/// One file an install writes (and an uninstall may remove).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PlannedFile {
    /// Absolute destination path (parent directories are created as needed).
    pub path: PathBuf,
    /// The exact bytes to write (text artifacts are UTF-8).
    pub contents: Vec<u8>,
    /// Whether the file must be made executable (`0755`) on Unix — the macOS
    /// launcher script; ignored on non-Unix hosts.
    pub executable: bool,
}

/// A platform's desktop-integration plan: the files to create on install and the
/// paths to delete on uninstall.
///
/// `remove` is not always `files.map(|f| f.path)`: on macOS the two files live
/// inside a `karet.app` bundle directory, and uninstall removes the whole bundle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Plan {
    /// Files written on install, in order.
    pub files: Vec<PlannedFile>,
    /// Paths removed on uninstall — individual files, or a bundle directory.
    pub remove: Vec<PathBuf>,
}

// ---------------------------------------------------------------------------
// Escaping helpers
// ---------------------------------------------------------------------------

/// Characters that force an `Exec` argument to be quoted, per the XDG Desktop
/// Entry spec's "reserved characters" list.
const EXEC_RESERVED: &[char] = &[
    ' ', '\t', '\n', '"', '\'', '\\', '>', '<', '~', '|', '&', ';', '$', '*', '?', '#', '(', ')',
    '`',
];

/// Escape one `Exec` argument for a `.desktop` file: the spec's argument-quoting
/// pass followed by the general string-value escaping pass — the two passes a
/// reader undoes in reverse order.
///
/// An argument with no reserved character is emitted bare; otherwise it is wrapped
/// in double quotes with `"`, `` ` ``, `$`, and `\` backslash-escaped. Because the
/// general string escaping then doubles every backslash, a literal backslash in the
/// path ends up as four backslashes and a literal `$` as `\\$`, exactly as the spec
/// prescribes.
fn exec_field(exe: &Path) -> String {
    let arg = exe.to_string_lossy();
    if !arg.chars().any(|c| EXEC_RESERVED.contains(&c)) {
        return general_escape(&arg);
    }
    let mut quoted = String::with_capacity(arg.len() + 2);
    quoted.push('"');
    for c in arg.chars() {
        if matches!(c, '"' | '`' | '$' | '\\') {
            quoted.push('\\');
        }
        quoted.push(c);
    }
    quoted.push('"');
    general_escape(&quoted)
}

/// General `.desktop` string-value escaping: a literal backslash is written `\\`.
/// Paths never contain the other escapable control characters (newline, tab,
/// carriage return), but a backslash is plausible (a Windows-style path, a test
/// fixture) and must be doubled so the value round-trips.
fn general_escape(s: &str) -> String {
    s.replace('\\', "\\\\")
}

/// Escape text for insertion into an XML/plist `<string>` body: `&`, `<`, `>`.
/// (`"` and `'` are legal in element text, so they are left alone.)
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Wrap a string in single quotes for `/bin/sh`, escaping embedded single quotes as
/// `'\''` — the only sequence that is unambiguous inside a single-quoted word.
fn sh_single_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.chars() {
        if c == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(c);
        }
    }
    out.push('\'');
    out
}

// ---------------------------------------------------------------------------
// Content generators (pure string builders)
// ---------------------------------------------------------------------------

/// Build the `karet.desktop` entry pointing `TryExec`/`Exec` at `exe`.
///
/// `Terminal=true` makes the DE launch karet inside the user's default terminal.
/// The trailing field code is `%f` (a single file), not `%F`: karet's CLI takes one
/// optional path, so `%f` opens one karet per file a file manager passes — no
/// silently-dropped arguments — whereas `%F` would pass several and karet would keep
/// only the first.
pub(crate) fn linux_desktop_entry(exe: &Path) -> String {
    let exec = exec_field(exe);
    let tryexec = general_escape(&exe.to_string_lossy());
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Version=1.5\n\
         Name=karet\n\
         GenericName=Terminal IDE\n\
         Comment={DISCLAIMER}\n\
         TryExec={tryexec}\n\
         Exec={exec} %f\n\
         Icon=karet\n\
         Terminal=true\n\
         Categories=Development;IDE;TextEditor;\n\
         Keywords=editor;ide;code;text;terminal;\n\
         StartupNotify=false\n"
    )
}

/// The scalable application icon markup (see [`ICON_SVG`]).
#[must_use]
pub(crate) fn icon_svg() -> &'static str {
    ICON_SVG
}

/// Build the macOS bundle `Info.plist`. The executable is the launcher script
/// (`CFBundleExecutable`), the identifier is `dev.getkono.karet` (matching the
/// `getkono`/`karet` qualifier used elsewhere), and the disclaimer rides in
/// `CFBundleGetInfoString` — the field the Finder's Get Info panel surfaces — so no
/// separate readme is needed inside the bundle.
pub(crate) fn macos_info_plist(version: &str) -> String {
    let version = xml_escape(version);
    let disclaimer = xml_escape(DISCLAIMER);
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleName</key>
	<string>karet</string>
	<key>CFBundleDisplayName</key>
	<string>karet</string>
	<key>CFBundleIdentifier</key>
	<string>dev.getkono.karet</string>
	<key>CFBundleVersion</key>
	<string>{version}</string>
	<key>CFBundleShortVersionString</key>
	<string>{version}</string>
	<key>CFBundlePackageType</key>
	<string>APPL</string>
	<key>CFBundleExecutable</key>
	<string>karet-launcher</string>
	<key>CFBundleInfoDictionaryVersion</key>
	<string>6.0</string>
	<key>LSMinimumSystemVersion</key>
	<string>11.0</string>
	<key>CFBundleGetInfoString</key>
	<string>{disclaimer}</string>
</dict>
</plist>
"#
    )
}

/// Build the macOS bundle launcher: a `/bin/sh` script that opens karet in
/// Terminal.app.
///
/// macOS exposes no "user's default terminal" API, so Terminal.app — always present
/// — is the documented target; a user who prefers another terminal is covered by the
/// [`DISCLAIMER`]. `open -a Terminal <exe>` hands the executable to Terminal, which
/// runs it in a new window; the path is single-quoted for the shell.
pub(crate) fn macos_launcher_script(exe: &Path) -> String {
    let quoted = sh_single_quote(&exe.to_string_lossy());
    format!(
        "#!/bin/sh\n\
         # karet launcher — opens karet in Terminal.app. macOS has no default-terminal\n\
         # API, so Terminal.app is the documented target.\n\
         # {DISCLAIMER}\n\
         exec open -a Terminal {quoted}\n"
    )
}

/// Build the Windows Start-Menu `.cmd` launcher that runs `exe` in the default
/// console host (Windows Terminal on Windows 11).
///
/// A `.cmd` is used rather than a binary `.lnk` shortcut: its contents are a pure,
/// fully-testable string, it adds no dependency, and it carries the [`DISCLAIMER`]
/// as a `rem` header. `%*` forwards any arguments; the path is double-quoted (a `"`
/// is illegal in Windows filenames, so stripping any is a safe no-op).
pub(crate) fn windows_cmd_launcher(exe: &Path) -> String {
    let path = exe.to_string_lossy().replace('"', "");
    // Windows text files conventionally use CRLF line endings.
    format!(
        "@echo off\r\n\
         rem karet — Terminal IDE\r\n\
         rem {DISCLAIMER}\r\n\
         \"{path}\" %*\r\n"
    )
}

// ---------------------------------------------------------------------------
// Planners (pure: inputs in, Plan out)
// ---------------------------------------------------------------------------

/// Plan the Linux (XDG) install under `data_home` (`$XDG_DATA_HOME`, default
/// `~/.local/share`): the desktop entry in `applications/` and the scalable icon in
/// the hicolor theme. Uninstall removes exactly those two files (shared parent
/// directories are left in place).
pub(crate) fn linux_plan(data_home: &Path, exe: &Path) -> Plan {
    let desktop = data_home.join("applications").join("karet.desktop");
    let icon = data_home
        .join("icons")
        .join("hicolor")
        .join("scalable")
        .join("apps")
        .join("karet.svg");
    Plan {
        files: vec![
            PlannedFile {
                path: desktop.clone(),
                contents: linux_desktop_entry(exe).into_bytes(),
                executable: false,
            },
            PlannedFile {
                path: icon.clone(),
                contents: icon_svg().as_bytes().to_vec(),
                executable: false,
            },
        ],
        remove: vec![desktop, icon],
    }
}

/// Plan the macOS install: a minimal `karet.app` bundle under `applications_dir`
/// (`~/Applications`), holding `Contents/Info.plist` and an executable
/// `Contents/MacOS/karet-launcher`. Uninstall removes the whole bundle directory.
pub(crate) fn macos_plan(applications_dir: &Path, exe: &Path, version: &str) -> Plan {
    let bundle = applications_dir.join("karet.app");
    let contents = bundle.join("Contents");
    Plan {
        files: vec![
            PlannedFile {
                path: contents.join("Info.plist"),
                contents: macos_info_plist(version).into_bytes(),
                executable: false,
            },
            PlannedFile {
                path: contents.join("MacOS").join("karet-launcher"),
                contents: macos_launcher_script(exe).into_bytes(),
                executable: true,
            },
        ],
        remove: vec![bundle],
    }
}

/// Plan the Windows install: a `karet.cmd` launcher in `start_menu_programs`
/// (`%APPDATA%\Microsoft\Windows\Start Menu\Programs`). Uninstall removes that file.
pub(crate) fn windows_plan(start_menu_programs: &Path, exe: &Path) -> Plan {
    let cmd = start_menu_programs.join("karet.cmd");
    Plan {
        files: vec![PlannedFile {
            path: cmd.clone(),
            contents: windows_cmd_launcher(exe).into_bytes(),
            executable: false,
        }],
        remove: vec![cmd],
    }
}

// ---------------------------------------------------------------------------
// Executors (impure: apply a plan to the filesystem)
// ---------------------------------------------------------------------------

/// A desktop-integration failure, rendered on stderr by the CLI entry points.
#[derive(Debug, thiserror::Error)]
pub(crate) enum DesktopError {
    /// The host is not one of the supported desktop platforms (Linux/XDG, macOS,
    /// Windows 10/11).
    #[error("desktop integration is not supported on this platform")]
    Unsupported,
    /// The per-user base directory could not be determined (no home directory).
    #[error("cannot determine the user's home directory")]
    NoBaseDirs,
    /// The running executable's path could not be resolved.
    #[error("cannot resolve the karet executable path: {0}")]
    CurrentExe(#[source] std::io::Error),
    /// A planned file (or one of its parent directories) could not be written.
    #[error("cannot write {path}: {source}")]
    Write {
        /// The destination that failed.
        path: String,
        /// The underlying I/O error.
        #[source]
        source: std::io::Error,
    },
    /// A planned path could not be removed.
    #[error("cannot remove {path}: {source}")]
    Remove {
        /// The path that failed.
        path: String,
        /// The underlying I/O error.
        #[source]
        source: std::io::Error,
    },
}

/// Execute a plan's install half: create parent directories, write every planned
/// file (overwriting a previous install — idempotent by construction), and mark
/// launcher scripts executable on Unix. Returns the written paths in order.
pub(crate) fn install(plan: &Plan) -> Result<Vec<PathBuf>, DesktopError> {
    let write_err = |path: &Path, source: std::io::Error| DesktopError::Write {
        path: path.display().to_string(),
        source,
    };
    let mut written = Vec::with_capacity(plan.files.len());
    for file in &plan.files {
        if let Some(parent) = file.path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| write_err(parent, e))?;
        }
        std::fs::write(&file.path, &file.contents).map_err(|e| write_err(&file.path, e))?;
        #[cfg(unix)]
        if file.executable {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&file.path, std::fs::Permissions::from_mode(0o755))
                .map_err(|e| write_err(&file.path, e))?;
        }
        written.push(file.path.clone());
    }
    Ok(written)
}

/// Execute a plan's uninstall half: remove every planned path (a file or, for the
/// macOS bundle, a whole directory). Returns `(removed, already_absent)` — an
/// absent path is a success with a note, not an error, so uninstall is idempotent.
pub(crate) fn uninstall(plan: &Plan) -> Result<(Vec<PathBuf>, Vec<PathBuf>), DesktopError> {
    let remove_err = |path: &Path, source: std::io::Error| DesktopError::Remove {
        path: path.display().to_string(),
        source,
    };
    let mut removed = Vec::new();
    let mut absent = Vec::new();
    for path in &plan.remove {
        // `symlink_metadata` treats a dangling symlink as present (it should be
        // removed) while a truly missing path lands in `absent`.
        match std::fs::symlink_metadata(path) {
            Err(_) => absent.push(path.clone()),
            Ok(meta) => {
                let result = if meta.is_dir() {
                    std::fs::remove_dir_all(path)
                } else {
                    std::fs::remove_file(path)
                };
                result.map_err(|e| remove_err(path, e))?;
                removed.push(path.clone());
            },
        }
    }
    Ok((removed, absent))
}

// ---------------------------------------------------------------------------
// Platform resolution + CLI entry points
// ---------------------------------------------------------------------------

/// Compose the current platform's plan for `exe`: XDG data home on Linux,
/// `~/Applications` on macOS, the Start-Menu programs folder on Windows.
#[cfg(target_os = "linux")]
fn current_plan(exe: &Path) -> Result<Plan, DesktopError> {
    let dirs = directories::BaseDirs::new().ok_or(DesktopError::NoBaseDirs)?;
    Ok(linux_plan(dirs.data_dir(), exe))
}

/// Compose the current platform's plan for `exe`: XDG data home on Linux,
/// `~/Applications` on macOS, the Start-Menu programs folder on Windows.
#[cfg(target_os = "macos")]
fn current_plan(exe: &Path) -> Result<Plan, DesktopError> {
    let dirs = directories::BaseDirs::new().ok_or(DesktopError::NoBaseDirs)?;
    Ok(macos_plan(
        &dirs.home_dir().join("Applications"),
        exe,
        env!("CARGO_PKG_VERSION"),
    ))
}

/// Compose the current platform's plan for `exe`: XDG data home on Linux,
/// `~/Applications` on macOS, the Start-Menu programs folder on Windows.
#[cfg(windows)]
fn current_plan(exe: &Path) -> Result<Plan, DesktopError> {
    let dirs = directories::BaseDirs::new().ok_or(DesktopError::NoBaseDirs)?;
    // `data_dir()` is `%APPDATA%` (FOLDERID_RoamingAppData) on Windows.
    let programs = dirs
        .data_dir()
        .join("Microsoft")
        .join("Windows")
        .join("Start Menu")
        .join("Programs");
    Ok(windows_plan(&programs, exe))
}

/// Compose the current platform's plan for `exe` — always [`DesktopError::Unsupported`]
/// on a platform without desktop integration (BSDs, other unix).
#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn current_plan(_exe: &Path) -> Result<Plan, DesktopError> {
    Err(DesktopError::Unsupported)
}

/// The running executable's canonical path — the target every launcher points at.
/// Canonicalization resolves the symlink a `cargo install` or packaging shim may
/// have put on `$PATH`; when it fails (rare), the raw path is still usable.
fn current_exe() -> Result<PathBuf, DesktopError> {
    let exe = std::env::current_exe().map_err(DesktopError::CurrentExe)?;
    Ok(std::fs::canonicalize(&exe).unwrap_or(exe))
}

/// Run `--install-desktop`: plan for this platform and executable, write the
/// files, and report each created path plus the disclaimer on stdout. Returns the
/// process exit code (0 on success; 1 with a stderr message on failure).
pub(crate) fn run_install() -> i32 {
    let result = current_exe().and_then(|exe| {
        let plan = current_plan(&exe)?;
        install(&plan)
    });
    match result {
        Ok(written) => {
            for path in &written {
                println!("installed {}", path.display());
            }
            println!("note: {DISCLAIMER}");
            0
        },
        Err(error) => {
            eprintln!("karet: --install-desktop: {error}");
            1
        },
    }
}

/// Run `--uninstall-desktop`: plan for this platform and executable, remove the
/// planned paths, and report each on stdout (an already-absent path is a note, not
/// a failure). Returns the process exit code (0 on success; 1 with a stderr
/// message on failure).
pub(crate) fn run_uninstall() -> i32 {
    let result = current_exe().and_then(|exe| {
        let plan = current_plan(&exe)?;
        uninstall(&plan)
    });
    match result {
        Ok((removed, absent)) => {
            for path in &removed {
                println!("removed {}", path.display());
            }
            for path in &absent {
                println!("already absent: {}", path.display());
            }
            0
        },
        Err(error) => {
            eprintln!("karet: --uninstall-desktop: {error}");
            1
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- Exec / string escaping -------------------------------------------

    #[test]
    fn exec_field_leaves_plain_paths_bare() {
        assert_eq!(exec_field(Path::new("/usr/bin/karet")), "/usr/bin/karet");
    }

    #[test]
    fn exec_field_quotes_paths_with_spaces() {
        assert_eq!(
            exec_field(Path::new("/opt/my apps/karet")),
            "\"/opt/my apps/karet\""
        );
    }

    #[test]
    fn exec_field_escapes_backslash_as_four() {
        // Spec: a literal backslash in a quoted argument is four backslashes.
        let out = exec_field(Path::new(r"C:\bin\karet"));
        assert!(out.starts_with('"') && out.ends_with('"'));
        assert!(out.contains(r"C:\\\\bin\\\\karet"), "got {out}");
    }

    #[test]
    fn exec_field_escapes_dollar() {
        // Spec: a literal dollar in a quoted argument is `\\$`.
        let out = exec_field(Path::new("/opt/$weird/karet"));
        assert!(out.contains(r"/opt/\\$weird/karet"), "got {out}");
    }

    #[test]
    fn general_escape_doubles_backslashes() {
        assert_eq!(general_escape(r"a\b"), r"a\\b");
        assert_eq!(general_escape("plain"), "plain");
    }

    #[test]
    fn xml_escape_covers_markup_characters() {
        assert_eq!(xml_escape("a<b>&c"), "a&lt;b&gt;&amp;c");
    }

    #[test]
    fn sh_single_quote_escapes_embedded_quote() {
        assert_eq!(sh_single_quote("a'b"), "'a'\\''b'");
        assert_eq!(sh_single_quote("/usr/bin/karet"), "'/usr/bin/karet'");
    }

    // --- Linux desktop entry ----------------------------------------------

    #[test]
    fn desktop_entry_has_all_required_keys() {
        let entry = linux_desktop_entry(Path::new("/usr/bin/karet"));
        for key in [
            "[Desktop Entry]",
            "Type=Application",
            "Name=karet",
            "Exec=/usr/bin/karet %f",
            "TryExec=/usr/bin/karet",
            "Icon=karet",
            "Terminal=true",
            "Categories=Development;IDE;TextEditor;",
        ] {
            assert!(entry.contains(key), "missing {key:?} in:\n{entry}");
        }
    }

    #[test]
    fn desktop_entry_opens_in_a_terminal_and_carries_the_disclaimer() {
        let entry = linux_desktop_entry(Path::new("/usr/bin/karet"));
        assert!(entry.contains("Terminal=true"));
        assert!(entry.contains(&format!("Comment={DISCLAIMER}")));
        assert!(entry.contains("--doctor"));
    }

    #[test]
    fn desktop_entry_quotes_a_spaced_exec_path() {
        let entry = linux_desktop_entry(Path::new("/opt/my apps/karet"));
        assert!(
            entry.contains("Exec=\"/opt/my apps/karet\" %f"),
            "spaced Exec path must be quoted:\n{entry}"
        );
    }

    #[test]
    fn desktop_entry_has_no_trailing_whitespace() {
        let entry = linux_desktop_entry(Path::new("/usr/bin/karet"));
        for line in entry.lines() {
            assert_eq!(line, line.trim_end(), "trailing whitespace in {line:?}");
        }
    }

    // --- Icon --------------------------------------------------------------

    #[test]
    fn icon_is_well_formed_two_colour_svg() {
        let svg = icon_svg();
        assert!(svg.trim_start().starts_with("<?xml"));
        assert!(svg.contains("<svg") && svg.contains("</svg>"));
        assert!(svg.contains("viewBox"));
        // Exactly the two declared colours, and no more.
        assert!(svg.contains("#1e2430") && svg.contains("#8ab4f8"));
        assert_eq!(
            svg.matches('#').count(),
            3,
            "expected 2 fills + 1 stroke ref"
        );
    }

    // --- macOS -------------------------------------------------------------

    #[test]
    fn info_plist_substitutes_version_and_carries_disclaimer() {
        let plist = macos_info_plist("1.2.3");
        for needle in [
            "<key>CFBundleName</key>",
            "<string>karet</string>",
            "<key>CFBundleIdentifier</key>",
            "dev.getkono.karet",
            "<key>CFBundleExecutable</key>",
            "<string>karet-launcher</string>",
            "<key>LSMinimumSystemVersion</key>",
            "<string>1.2.3</string>",
            "<key>CFBundleGetInfoString</key>",
        ] {
            assert!(plist.contains(needle), "missing {needle:?} in:\n{plist}");
        }
        assert!(plist.contains(DISCLAIMER));
    }

    #[test]
    fn launcher_script_opens_terminal_with_quoted_path() {
        let script = macos_launcher_script(Path::new("/Users/me/bin/karet"));
        assert!(script.starts_with("#!/bin/sh\n"));
        assert!(script.contains("open -a Terminal '/Users/me/bin/karet'"));
        assert!(script.contains(DISCLAIMER));
    }

    #[test]
    fn launcher_script_single_quotes_spaced_path() {
        let script = macos_launcher_script(Path::new("/opt/my apps/karet"));
        assert!(script.contains("open -a Terminal '/opt/my apps/karet'"));
    }

    // --- Windows -----------------------------------------------------------

    #[test]
    fn cmd_launcher_runs_quoted_exe_with_crlf_and_disclaimer() {
        let cmd = windows_cmd_launcher(Path::new(r"C:\Program Files\karet\karet.exe"));
        assert!(cmd.starts_with("@echo off\r\n"));
        assert!(cmd.contains(&format!("rem {DISCLAIMER}")));
        assert!(cmd.contains("\"C:\\Program Files\\karet\\karet.exe\" %*"));
        assert!(cmd.contains("\r\n"), "cmd files use CRLF");
    }

    // --- Plan composition (injected roots) --------------------------------

    #[test]
    fn linux_plan_lays_out_desktop_entry_and_icon() {
        let plan = linux_plan(
            Path::new("/home/u/.local/share"),
            Path::new("/usr/bin/karet"),
        );
        let paths: Vec<_> = plan.files.iter().map(|f| f.path.clone()).collect();
        assert_eq!(
            paths,
            vec![
                PathBuf::from("/home/u/.local/share/applications/karet.desktop"),
                PathBuf::from("/home/u/.local/share/icons/hicolor/scalable/apps/karet.svg"),
            ]
        );
        // Nothing is marked executable, and uninstall removes exactly those files.
        assert!(plan.files.iter().all(|f| !f.executable));
        assert_eq!(plan.remove, paths);
    }

    #[test]
    fn macos_plan_builds_a_bundle_with_an_executable_launcher() {
        let plan = macos_plan(
            Path::new("/Users/me/Applications"),
            Path::new("/usr/local/bin/karet"),
            "9.9.9",
        );
        let plist = &plan.files[0];
        let launcher = &plan.files[1];
        assert_eq!(
            plist.path,
            PathBuf::from("/Users/me/Applications/karet.app/Contents/Info.plist")
        );
        assert!(!plist.executable);
        assert_eq!(
            launcher.path,
            PathBuf::from("/Users/me/Applications/karet.app/Contents/MacOS/karet-launcher")
        );
        assert!(launcher.executable, "the launcher must be executable");
        // Uninstall removes the whole bundle directory, not the two files.
        assert_eq!(
            plan.remove,
            vec![PathBuf::from("/Users/me/Applications/karet.app")]
        );
    }

    #[test]
    fn windows_plan_writes_one_cmd_in_the_start_menu() {
        let programs =
            Path::new(r"C:\Users\me\AppData\Roaming\Microsoft\Windows\Start Menu\Programs");
        let plan = windows_plan(programs, Path::new(r"C:\Program Files\karet\karet.exe"));
        assert_eq!(plan.files.len(), 1);
        assert_eq!(plan.files[0].path, programs.join("karet.cmd"));
        assert!(!plan.files[0].executable);
        assert_eq!(plan.remove, vec![programs.join("karet.cmd")]);
    }

    // --- Executors (temp roots, never the real HOME/XDG paths) -------------

    #[test]
    fn install_writes_exactly_the_planned_files() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let plan = linux_plan(root.path(), Path::new("/usr/bin/karet"));
        let written = install(&plan)?;
        assert_eq!(
            written,
            plan.files
                .iter()
                .map(|f| f.path.clone())
                .collect::<Vec<_>>()
        );
        for file in &plan.files {
            assert_eq!(std::fs::read(&file.path)?, file.contents, "{:?}", file.path);
        }
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn install_marks_the_launcher_executable() -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir()?;
        let plan = macos_plan(root.path(), Path::new("/usr/local/bin/karet"), "1.0.0");
        install(&plan)?;
        let launcher = &plan.files[1];
        assert!(launcher.executable, "fixture: files[1] is the launcher");
        let mode = std::fs::metadata(&launcher.path)?.permissions().mode();
        assert_eq!(mode & 0o777, 0o755, "launcher must be 0755");
        let plist_mode = std::fs::metadata(&plan.files[0].path)?.permissions().mode();
        assert_eq!(plist_mode & 0o111, 0, "the plist must not be executable");
        Ok(())
    }

    #[test]
    fn double_install_is_idempotent() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let plan = linux_plan(root.path(), Path::new("/usr/bin/karet"));
        install(&plan)?;
        // A second install (even after the entry pointed elsewhere) overwrites
        // cleanly and converges on the new plan's contents.
        let moved = linux_plan(root.path(), Path::new("/opt/new home/karet"));
        install(&moved)?;
        let entry = std::fs::read_to_string(&moved.files[0].path)?;
        assert!(entry.contains("\"/opt/new home/karet\""), "got:\n{entry}");
        Ok(())
    }

    #[test]
    fn uninstall_removes_installed_files() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let plan = linux_plan(root.path(), Path::new("/usr/bin/karet"));
        install(&plan)?;
        let (removed, absent) = uninstall(&plan)?;
        assert_eq!(removed, plan.remove);
        assert!(absent.is_empty());
        for path in &plan.remove {
            assert!(!path.exists(), "{path:?} must be gone");
        }
        Ok(())
    }

    #[test]
    fn uninstall_removes_the_whole_macos_bundle() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let plan = macos_plan(root.path(), Path::new("/usr/local/bin/karet"), "1.0.0");
        install(&plan)?;
        let bundle = root.path().join("karet.app");
        assert!(bundle.is_dir());
        let (removed, absent) = uninstall(&plan)?;
        assert_eq!(removed, vec![bundle.clone()]);
        assert!(absent.is_empty());
        assert!(!bundle.exists(), "the bundle directory must be gone");
        Ok(())
    }

    #[test]
    fn uninstall_when_absent_succeeds_with_notes() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let plan = linux_plan(root.path(), Path::new("/usr/bin/karet"));
        let (removed, absent) = uninstall(&plan)?;
        assert!(removed.is_empty());
        assert_eq!(absent, plan.remove);
        Ok(())
    }

    #[test]
    fn install_reports_an_unwritable_destination() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        // Occupy the `applications` parent with a *file* so create_dir_all fails.
        std::fs::write(root.path().join("applications"), b"in the way")?;
        let plan = linux_plan(root.path(), Path::new("/usr/bin/karet"));
        let error = install(&plan).err().ok_or("install must fail")?;
        assert!(
            matches!(error, DesktopError::Write { .. }),
            "expected Write error, got {error:?}"
        );
        assert!(error.to_string().contains("cannot write"));
        Ok(())
    }
}
