//! Command-line interface.

use std::path::PathBuf;
use std::time::Duration;

use clap::Parser;
use clap::ValueEnum;
use karet_filetype::IconStyle;
use karet_session::config::schema::StartupPanel;

use crate::view::View;

/// Full multi-line text shown by `karet -V` / `--version`, assembled at compile
/// time from the build-script env vars (see `build.rs`). clap prefixes it with the
/// binary name, so the first line renders as `karet <version>`, followed by the
/// commit (with a `(dirty)` marker when built from a modified tree), build profile,
/// `rustc` version, and build timestamp.
const LONG_VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    "\ncommit:  ",
    env!("KARET_GIT_SHA"),
    env!("KARET_GIT_DIRTY"),
    " (",
    env!("KARET_GIT_COMMIT_TIMESTAMP"),
    ")\nprofile: ",
    env!("KARET_BUILD_PROFILE"),
    "\nrustc:   ",
    env!("KARET_RUSTC"),
    "\nbuilt:   ",
    env!("KARET_BUILD_TIMESTAMP"),
);

/// One-line build identity for the `--doctor` report: `<version> (<commit><dirty>)`,
/// assembled from the same build-script provenance as [`LONG_VERSION`].
pub const VERSION_LINE: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    " (",
    env!("KARET_GIT_SHA"),
    env!("KARET_GIT_DIRTY"),
    ")",
);

/// karet — a terminal IDE: file explorer, code window, and search.
///
/// Opens an Explorer-first shell rooted at the given path. A file opens directly; a
/// git repository's changes appear in the Source Control panel.
#[derive(Debug, Parser)]
#[command(name = "karet", version = LONG_VERSION, about)]
#[command(group(
    clap::ArgGroup::new("seam_surface")
        .args(["seam_query", "seam_report", "seam_call_tree"])
        .multiple(false)
))]
pub struct Cli {
    /// File or directory to open (defaults to the current directory).
    pub path: Option<PathBuf>,

    /// Disable syntax highlighting (also respects the NO_COLOR environment variable).
    #[arg(long)]
    pub no_syntax: bool,

    /// File-tree / activity-bar icon style. Defaults to Nerd Font (needs a patched
    /// font); falls back to the `KARET_ICONS` env var, then Nerd Font. Use
    /// `--icons unicode` or `--icons ascii` if your font lacks Nerd Font glyphs.
    #[arg(long, value_enum)]
    pub icons: Option<IconChoice>,

    /// Sidebar panel to show at startup, overriding `workbench.startupPanel`.
    #[arg(long, value_enum)]
    pub startup_panel: Option<StartupPanelChoice>,

    /// Top-level view to show at startup. The Agents view owns the full terminal
    /// width, so it starts with the sidebar hidden whatever `--startup-panel` says.
    #[arg(long, value_enum)]
    pub view: Option<ViewChoice>,

    /// Area to focus after startup views are opened.
    #[arg(long, value_enum)]
    pub focus: Option<FocusChoice>,

    /// Open an additional file at startup. Relative paths resolve under the root.
    #[arg(long = "open")]
    pub open: Vec<PathBuf>,

    /// Open one startup preview tab without moving focus from the startup panel.
    #[arg(long)]
    pub preview: Option<PathBuf>,

    /// Open a file in a new pane split to the right of the focused pane, after any
    /// `--open` tabs are placed (repeatable; splits chain left-to-right). Relative
    /// paths resolve under the root. When the layout has no room for another pane,
    /// the file opens as a tab in the current pane and a notification says so.
    ///
    /// Unstable automation surface: intended for scripting and view capture, this
    /// flag's behaviour may change between major versions without notice.
    #[arg(long = "split", value_name = "PATH")]
    pub split: Vec<PathBuf>,

    /// Open a file and place the caret, as `PATH[:LINE[:COL]]` (1-based, both
    /// default to 1), then focus the editor. Relative paths resolve under the root.
    ///
    /// Unstable automation surface: intended for scripting and view capture, this
    /// flag's spec and behaviour may change between major versions without notice.
    /// The trailing `:LINE[:COL]` is peeled from the right, so a path that itself
    /// contains colons still works; the target is clamped into the buffer.
    #[arg(long, value_name = "PATH[:LINE[:COL]]")]
    pub goto: Option<String>,

    /// Open a diff of two files as a startup tab: OLD renders as the "before" side
    /// and NEW as the "after" (repeatable; each occurrence takes exactly two paths
    /// and opens one diff tab). Relative paths resolve under the root. A file that
    /// cannot be read is a fatal error before the TUI starts; non-UTF-8 content on
    /// either side renders as a binary-change placeholder.
    ///
    /// Unstable automation surface: intended for scripting and view capture, this
    /// flag's behaviour may change between major versions without notice.
    #[arg(long = "diff", num_args = 2, value_names = ["OLD", "NEW"])]
    pub diff: Vec<PathBuf>,

    /// Run a command-palette command after every other startup flag is applied
    /// (repeatable; runs in the given order). NAME matches a palette entry's title
    /// (e.g. "Source Control: Commit Graph") or its short slug (e.g. "graph"),
    /// case-insensitively and exactly. An unknown or ambiguous name prints the
    /// closest matches to stderr and exits non-zero before the TUI starts.
    ///
    /// Unstable automation surface: intended for scripting and view capture, the
    /// command set and command names may change between major versions without
    /// notice.
    #[arg(long = "command", value_name = "NAME")]
    pub command: Vec<String>,

    /// Evaluate a seam query against the package at PATH, print the matching nodes as
    /// JSON to stdout, and exit — never enter the TUI.
    ///
    /// Takes the same query language as the Seam view's filter box, so a narrowing a
    /// person reached by pressing keys and one a program asks for are the same string.
    /// Results carry node identities, locations, facets, and rollup counts: enough to
    /// cite a finding and to navigate back to it.
    ///
    /// Unstable automation surface: intended for scripting and analysis, the output
    /// shape may change between major versions without notice.
    #[arg(long, value_name = "QUERY")]
    pub seam_query: Option<String>,

    /// Evaluate `--seam-query`, `--seam-report` or `--seam-call-tree` under a named
    /// configuration rather than the default.
    ///
    /// Unstable automation surface: intended for scripting and analysis, this flag's
    /// behaviour may change between major versions without notice.
    #[arg(long, value_name = "NAME", requires = "seam_surface")]
    pub seam_config: Option<String>,

    /// Write a call-structure report for the package at PATH to stdout, and exit.
    ///
    /// Measures every function by the code it pulls in through its calls, then ranks
    /// where structure most likely needs attention — call cycles, hubs, heavy entry
    /// points, large leaves, unreachable code, module coupling — each with its evidence,
    /// and a ledger of how certain the numbers are. Written to be read by a person or
    /// handed whole to a language model. An optional QUERY, in the seam query language
    /// and written `--seam-report='QUERY'`, narrows which functions are listed;
    /// everything is still measured. The `=` is required so that `--seam-report .`
    /// reports on `.` rather than reading it as a query.
    ///
    /// Unstable automation surface: the report's shape may change between major
    /// versions without notice.
    #[arg(
        long,
        value_name = "QUERY",
        num_args = 0..=1,
        require_equals = true,
        default_missing_value = ""
    )]
    pub seam_report: Option<String>,

    /// Print the call tree of the function at the seam path FUNCTION, and exit.
    ///
    /// Each row gives the function's own and inclusive lines; each function is
    /// expanded once, and later appearances are marked. FUNCTION is a seam path such
    /// as `my-crate::net::Client::send`, as `--seam-query` reports it.
    ///
    /// Unstable automation surface: the output shape may change between major versions
    /// without notice.
    #[arg(long, value_name = "FUNCTION")]
    pub seam_call_tree: Option<String>,

    /// Output format for `--seam-report` and `--seam-call-tree`.
    #[arg(
        long,
        value_enum,
        value_name = "FORMAT",
        default_value_t = SeamFormat::Markdown,
        requires = "seam_surface",
        conflicts_with = "seam_query"
    )]
    pub seam_format: SeamFormat,

    /// Read and measure with test code included, rather than leaving it out — for
    /// `--seam-query`, `--seam-report` and `--seam-call-tree` alike.
    #[arg(long, requires = "seam_surface")]
    pub include_tests: bool,

    /// Render the shell into an off-screen grid and write it to stdout as truecolor
    /// ANSI, then exit — never enter the alternate screen and never read the
    /// terminal. Every other startup flag still applies, so the captured frame is
    /// whatever `--goto` / `--open` / `--diff` / `--command` would have shown. The
    /// session backend runs normally, so the frame carries real syntax highlighting
    /// and Source Control state; karet draws until those settle (see
    /// `--capture-settle`) and exits.
    ///
    /// Unstable automation surface: intended for scripting and view capture, this
    /// flag's behaviour and output format may change between major versions without
    /// notice.
    #[arg(long)]
    pub capture: bool,

    /// Grid to render `--capture` into, as `COLSxROWS` (default `120x34`). Both
    /// dimensions must be positive; karet clamps them to what its layout can use.
    #[arg(long, value_name = "COLSxROWS", requires = "capture")]
    pub capture_size: Option<String>,

    /// How long `--capture` waits for the backend to fall quiet before taking the
    /// final frame, in milliseconds (default 400). Each highlight or Source Control
    /// update restarts the wait, so a slower workspace still captures a settled UI.
    #[arg(long, value_name = "MS", requires = "capture")]
    pub capture_settle: Option<u64>,

    /// Hard ceiling on the whole `--capture` run, in milliseconds (default 10000).
    /// The frame is written and the exit code stays 0 even when the deadline hits
    /// first, so a never-quiet workspace still produces output.
    #[arg(long, value_name = "MS", requires = "capture")]
    pub capture_timeout: Option<u64>,

    /// Print terminal-capability diagnostics and exit instead of starting the
    /// editor. Probes the same features karet checks at startup (kitty keyboard
    /// protocol, kitty graphics protocol, OSC 22 pointer shapes) and reports one
    /// line per check; exits non-zero when a required capability is missing. The
    /// other flags are ignored.
    #[arg(long)]
    pub doctor: bool,

    /// Print the paths of existing application log files and exit instead of
    /// starting the editor. When no logs exist, prints an informational message.
    /// The other flags are ignored.
    #[arg(long)]
    pub log: bool,

    /// Install a per-user desktop entry that opens karet in a terminal, then exit
    /// instead of starting the editor: an XDG .desktop entry + icon on Linux, a
    /// ~/Applications/karet.app bundle on macOS, a Start-Menu launcher on Windows
    /// 10/11. karet requires a modern terminal (kitty keyboard protocol) and offers
    /// no guarantees with the OS default terminal — run `karet --doctor` inside it
    /// to check. Idempotent; prints each created file. The other flags are ignored.
    #[arg(long, conflicts_with = "uninstall_desktop")]
    pub install_desktop: bool,

    /// Remove the desktop entry `--install-desktop` created, then exit instead of
    /// starting the editor. Prints each removed file; an already-absent file is
    /// noted, not an error. The other flags are ignored.
    #[arg(long)]
    pub uninstall_desktop: bool,
}

/// A parsed [`Cli::goto`] argument: a file path with a 1-based caret target.
///
/// Produced by [`parse_goto_spec`]. `line` and `col` are 1-based and default to 1;
/// the app converts them to the editor's 0-based [`karet_core::LineCol`] and lets
/// the editor clamp the target into the buffer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GotoSpec {
    /// The file to open (relative paths resolve under the workspace root).
    pub path: PathBuf,
    /// 1-based line to place the caret on (minimum 1; clamped to the buffer).
    pub line: u32,
    /// 1-based column to place the caret at (minimum 1; clamped to the line).
    pub col: u32,
}

/// Parse a `--goto` spec of the form `PATH[:LINE[:COL]]` into a [`GotoSpec`].
///
/// The optional `LINE`/`COL` are peeled from the **right**, so a path that itself
/// contains colons (a Windows drive prefix, a URL-like name) is preserved: only
/// trailing `:<digits>` groups are read as coordinates. A trailing group that is
/// not a run of ASCII digits (`foo:bar`) stays part of the path; a `0` or
/// out-of-`u32`-range value is clamped up to 1. `LINE` and `COL` default to 1 when
/// absent, so a bare path targets line 1, column 1.
#[must_use]
pub fn parse_goto_spec(spec: &str) -> GotoSpec {
    let (path, at) = split_goto_suffix(spec);
    let (line, col) = at.unwrap_or((1, 1));
    GotoSpec {
        path: PathBuf::from(path),
        line,
        col,
    }
}

/// Split a `PATH[:LINE[:COL]]` spec into its path and, when at least `:LINE` was
/// given, its 1-based `(line, col)` target (`col` defaulting to 1).
///
/// The rules are [`parse_goto_spec`]'s: groups are peeled from the right, only a
/// run of ASCII digits that fits a `u32` is a coordinate, `0` is clamped up to 1,
/// and a group whose removal would leave an empty path stays part of the path.
/// Quick-open uses it to read `main.rs:42` as "this file, at line 42".
#[must_use]
pub fn split_goto_suffix(spec: &str) -> (&str, Option<(u32, u32)>) {
    // Peel the rightmost `:<digits>` group. `None` when there is no colon, the tail
    // is empty or non-numeric, the value overflows `u32`, or peeling would leave an
    // empty path (`:5`) — in every such case the colon belongs to the path.
    fn peel(s: &str) -> Option<(&str, u32)> {
        let (head, tail) = s.rsplit_once(':')?;
        if head.is_empty() {
            return None;
        }
        Some((head, coordinate(tail)?))
    }

    // The first peel yields the rightmost number: the column when a second number
    // precedes it (`PATH:LINE:COL`), otherwise the line (`PATH:LINE`).
    match peel(spec) {
        None => (spec, None),
        Some((rest, first)) => match peel(rest) {
            Some((path, second)) => (path, Some((second, first))),
            None => (rest, Some((first, 1))),
        },
    }
}

/// Parse a bare `LINE[:COL]` target (the text after quick-open's `:` sigil)
/// into a 1-based `(line, col)`, `col` defaulting to 1.
///
/// Each part must be a run of ASCII digits that fits a `u32`; `0` is clamped up
/// to 1, as [`parse_goto_spec`] does. Leading and trailing whitespace is
/// ignored; anything else (empty text, a sign, a third group, inner spaces) is
/// `None`.
#[must_use]
pub fn parse_line_col(text: &str) -> Option<(u32, u32)> {
    let text = text.trim();
    match text.split_once(':') {
        Some((line, col)) => Some((coordinate(line)?, coordinate(col)?)),
        None => Some((coordinate(text)?, 1)),
    }
}

/// One 1-based coordinate: a non-empty run of ASCII digits that fits a `u32`,
/// with `0` clamped up to 1.
fn coordinate(digits: &str) -> Option<u32> {
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(digits.parse::<u32>().ok()?.max(1))
}

/// Default `--capture` grid width in columns.
pub const CAPTURE_DEFAULT_COLS: u16 = 120;
/// Default `--capture` grid height in rows.
pub const CAPTURE_DEFAULT_ROWS: u16 = 34;
/// Default quiet period, in milliseconds, before `--capture` takes its final frame.
pub const CAPTURE_DEFAULT_SETTLE_MS: u64 = 400;
/// Default hard ceiling, in milliseconds, on a whole `--capture` run.
pub const CAPTURE_DEFAULT_TIMEOUT_MS: u64 = 10_000;
/// Smallest grid the shell's layout renders meaningfully into.
const CAPTURE_MIN_COLS: u16 = 20;
/// Smallest grid the shell's layout renders meaningfully into.
const CAPTURE_MIN_ROWS: u16 = 8;

/// A resolved `--capture` run: the grid to draw into and the two time bounds.
///
/// Produced by [`Cli::capture_spec`], which applies the `CAPTURE_DEFAULT_*`
/// constants to whatever the flags left unset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CaptureSpec {
    /// Grid width in terminal columns.
    pub cols: u16,
    /// Grid height in terminal rows.
    pub rows: u16,
    /// Quiet period the backend must hold before the final frame is taken.
    pub settle: Duration,
    /// Ceiling on the whole run, after which the frame is written regardless.
    pub timeout: Duration,
}

/// Parse a `--capture-size` argument of the form `COLSxROWS` into a grid.
///
/// The separator is `x` or `X`. Both dimensions must be a run of ASCII digits in
/// `u16` range and non-zero; anything else is an error described for stderr. A grid
/// smaller than the shell can lay out is clamped up rather than rejected, so a
/// too-small request still renders something.
pub fn parse_capture_size(spec: &str) -> Result<(u16, u16), String> {
    let (cols, rows) = spec
        .split_once(['x', 'X'])
        .ok_or_else(|| format!("expected COLSxROWS, got {spec:?}"))?;
    let dimension = |value: &str, name: &str| -> Result<u16, String> {
        if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
            return Err(format!("{name} must be a number, got {value:?}"));
        }
        match value.parse::<u16>() {
            Ok(0) => Err(format!("{name} must be greater than zero")),
            Ok(parsed) => Ok(parsed),
            Err(_) => Err(format!("{name} is out of range: {value}")),
        }
    };
    Ok((
        dimension(cols, "columns")?.max(CAPTURE_MIN_COLS),
        dimension(rows, "rows")?.max(CAPTURE_MIN_ROWS),
    ))
}

/// The `--seam-format` choices.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum SeamFormat {
    /// One Markdown document, for reading.
    #[value(name = "md")]
    Markdown,
    /// JSON, for programs.
    Json,
}

/// The `--icons` choices, mirroring [`IconStyle`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum IconChoice {
    /// Rich Nerd Font glyphs (the default).
    Nerd,
    /// 1-cell Unicode geometric glyphs.
    Unicode,
    /// Plain ASCII (maximally portable).
    Ascii,
}

/// CLI choices for the startup sidebar panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum StartupPanelChoice {
    /// Start with Explorer shown.
    Explorer,
    /// Start with Search shown.
    Search,
    /// Start with Source Control shown.
    SourceControl,
    /// Start with the sidebar collapsed.
    None,
}

/// CLI choices for the startup top-level view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ViewChoice {
    /// Start in the editor shell (panes and tabs).
    Editor,
    /// Start in the GitHub view.
    Github,
    // #211: /// Start in the Agents view.
    // #211: Agents,
}

/// CLI choices for startup focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum FocusChoice {
    /// Focus the sidebar, if it is visible.
    Sidebar,
    /// Focus the editor area.
    Editor,
}

impl From<IconChoice> for IconStyle {
    fn from(choice: IconChoice) -> Self {
        match choice {
            IconChoice::Nerd => Self::NerdFont,
            IconChoice::Unicode => Self::Unicode,
            IconChoice::Ascii => Self::Ascii,
        }
    }
}

impl From<ViewChoice> for View {
    fn from(choice: ViewChoice) -> Self {
        match choice {
            ViewChoice::Editor => Self::Editor,
            ViewChoice::Github => Self::GitHub,
            // #211: ViewChoice::Agents => Self::Agents,
        }
    }
}

impl From<StartupPanelChoice> for StartupPanel {
    fn from(choice: StartupPanelChoice) -> Self {
        match choice {
            StartupPanelChoice::Explorer => Self::Explorer,
            StartupPanelChoice::Search => Self::Search,
            StartupPanelChoice::SourceControl => Self::SourceControl,
            StartupPanelChoice::None => Self::None,
        }
    }
}

impl Cli {
    /// The explicitly-requested icon style, if any: an `--icons` flag wins, then the
    /// `KARET_ICONS` env var. `None` when neither is set, so the configured
    /// `workbench.iconStyle` (else the Nerd Font default) is left in place.
    /// Resolve the `--capture*` flags into a [`CaptureSpec`], filling every unset
    /// flag from the `CAPTURE_DEFAULT_*` constants.
    ///
    /// Returns the parse error from [`parse_capture_size`] when `--capture-size` is
    /// malformed, so the caller can fail fast on stderr instead of capturing a grid
    /// the user did not ask for.
    pub fn capture_spec(&self) -> Result<CaptureSpec, String> {
        let (cols, rows) = match self.capture_size.as_deref() {
            Some(size) => parse_capture_size(size)?,
            None => (CAPTURE_DEFAULT_COLS, CAPTURE_DEFAULT_ROWS),
        };
        Ok(CaptureSpec {
            cols,
            rows,
            settle: Duration::from_millis(self.capture_settle.unwrap_or(CAPTURE_DEFAULT_SETTLE_MS)),
            timeout: Duration::from_millis(
                self.capture_timeout.unwrap_or(CAPTURE_DEFAULT_TIMEOUT_MS),
            ),
        })
    }

    #[must_use]
    pub fn explicit_icon_style(&self) -> Option<IconStyle> {
        self.icons.map(IconStyle::from).or_else(|| {
            std::env::var("KARET_ICONS")
                .ok()
                .and_then(|v| IconStyle::from_name(&v))
        })
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use clap::CommandFactory;
    use clap::Parser;

    use super::Cli;
    use super::SeamFormat;

    #[test]
    fn cli_definition_is_valid() {
        // clap's self-check: panics on conflicting/ill-formed argument definitions.
        Cli::command().debug_assert();
    }

    #[test]
    fn doctor_flag_parses() -> Result<(), clap::Error> {
        let cli = Cli::try_parse_from(["karet", "--doctor"])?;
        assert!(cli.doctor);
        Ok(())
    }

    #[test]
    fn doctor_defaults_to_off() -> Result<(), clap::Error> {
        let cli = Cli::try_parse_from(["karet"])?;
        assert!(!cli.doctor);
        Ok(())
    }

    #[test]
    fn log_flag_parses_and_defaults_to_off() -> Result<(), clap::Error> {
        let cli = Cli::try_parse_from(["karet", "--log"])?;
        assert!(cli.log);
        assert!(!Cli::try_parse_from(["karet"])?.log);
        Ok(())
    }

    #[test]
    fn install_desktop_flag_parses() -> Result<(), clap::Error> {
        let cli = Cli::try_parse_from(["karet", "--install-desktop"])?;
        assert!(cli.install_desktop);
        assert!(!cli.uninstall_desktop);
        Ok(())
    }

    #[test]
    fn uninstall_desktop_flag_parses() -> Result<(), clap::Error> {
        let cli = Cli::try_parse_from(["karet", "--uninstall-desktop"])?;
        assert!(cli.uninstall_desktop);
        assert!(!cli.install_desktop);
        Ok(())
    }

    #[test]
    fn desktop_flags_default_to_off() -> Result<(), clap::Error> {
        let cli = Cli::try_parse_from(["karet"])?;
        assert!(!cli.install_desktop);
        assert!(!cli.uninstall_desktop);
        Ok(())
    }

    #[test]
    fn desktop_flags_conflict() {
        let error = Cli::try_parse_from(["karet", "--install-desktop", "--uninstall-desktop"]);
        assert!(error.is_err(), "the two desktop flags must be exclusive");
    }

    #[test]
    fn the_seam_surfaces_are_one_at_a_time() -> Result<(), clap::Error> {
        // A bare `--seam-report` is the whole report; a query narrows it.
        let bare = Cli::try_parse_from(["karet", "--seam-report"])?;
        assert_eq!(bare.seam_report.as_deref(), Some(""));
        assert_eq!(bare.seam_format, SeamFormat::Markdown);
        let narrowed = Cli::try_parse_from([
            "karet",
            "--seam-report=in:k::net",
            "--seam-format",
            "json",
            "--include-tests",
        ])?;
        assert_eq!(narrowed.seam_report.as_deref(), Some("in:k::net"));
        assert_eq!(narrowed.seam_format, SeamFormat::Json);
        assert!(narrowed.include_tests);
        // A word after a bare `--seam-report` is the path to report on, never a query.
        let pathed = Cli::try_parse_from(["karet", "--seam-report", "crates/core"])?;
        assert_eq!(pathed.seam_report.as_deref(), Some(""));
        assert_eq!(pathed.path, Some(PathBuf::from("crates/core")));
        let tree =
            Cli::try_parse_from(["karet", "--seam-call-tree", "k::run", "--seam-config", "x"])?;
        assert_eq!(tree.seam_call_tree.as_deref(), Some("k::run"));

        for clash in [
            &["karet", "--seam-query", "lens:api", "--seam-report"][..],
            &["karet", "--seam-report", "--seam-call-tree", "k::run"][..],
            &["karet", "--seam-query", "x", "--seam-call-tree", "k::run"][..],
        ] {
            assert!(Cli::try_parse_from(clash).is_err(), "{clash:?}");
        }
        // The modifiers mean nothing without a surface to modify.
        for orphan in [
            &["karet", "--include-tests"][..],
            &["karet", "--seam-format", "json"][..],
            &["karet", "--seam-config", "x"][..],
        ] {
            assert!(Cli::try_parse_from(orphan).is_err(), "{orphan:?}");
        }
        assert!(Cli::try_parse_from(["karet", "--seam-report", "--seam-format", "yaml"]).is_err());
        // A query always answers in JSON, so a format there would be silently ignored.
        assert!(
            Cli::try_parse_from(["karet", "--seam-query", "x", "--seam-format", "md"]).is_err()
        );
        assert!(
            Cli::try_parse_from(["karet", "--seam-query", "x", "--include-tests"])?.include_tests
        );
        Ok(())
    }

    #[test]
    fn goto_flag_parses() -> Result<(), clap::Error> {
        let cli = Cli::try_parse_from(["karet", "--goto", "src/main.rs:12:3"])?;
        assert_eq!(cli.goto.as_deref(), Some("src/main.rs:12:3"));
        Ok(())
    }

    #[test]
    fn goto_defaults_to_none() -> Result<(), clap::Error> {
        let cli = Cli::try_parse_from(["karet"])?;
        assert!(cli.goto.is_none());
        Ok(())
    }

    #[test]
    fn split_flag_is_repeatable() -> Result<(), clap::Error> {
        let cli = Cli::try_parse_from(["karet", "--split", "a.rs", "--split", "b.rs"])?;
        assert_eq!(
            cli.split,
            vec![PathBuf::from("a.rs"), PathBuf::from("b.rs")]
        );
        Ok(())
    }

    #[test]
    fn split_defaults_to_empty() -> Result<(), clap::Error> {
        let cli = Cli::try_parse_from(["karet"])?;
        assert!(cli.split.is_empty());
        Ok(())
    }

    #[test]
    fn command_flag_is_repeatable_and_ordered() -> Result<(), clap::Error> {
        let cli = Cli::try_parse_from([
            "karet",
            "--command",
            "graph",
            "--command",
            "View: Split Editor Right",
        ])?;
        assert_eq!(cli.command, vec!["graph", "View: Split Editor Right"]);
        Ok(())
    }

    #[test]
    fn command_defaults_to_empty() -> Result<(), clap::Error> {
        let cli = Cli::try_parse_from(["karet"])?;
        assert!(cli.command.is_empty());
        Ok(())
    }

    #[test]
    fn diff_flag_takes_exactly_two_paths() -> Result<(), clap::Error> {
        let cli = Cli::try_parse_from(["karet", "--diff", "a.rs", "b.rs"])?;
        assert_eq!(cli.diff, vec![PathBuf::from("a.rs"), PathBuf::from("b.rs")]);
        // One path is a parse error, not a silent half-pair.
        assert!(Cli::try_parse_from(["karet", "--diff", "a.rs"]).is_err());
        Ok(())
    }

    #[test]
    fn diff_flag_is_repeatable_in_pairs() -> Result<(), clap::Error> {
        let cli = Cli::try_parse_from(["karet", "--diff", "a", "b", "--diff", "c", "d"])?;
        assert_eq!(cli.diff.len(), 4);
        assert_eq!(cli.diff[2], PathBuf::from("c"));
        Ok(())
    }

    #[test]
    fn capture_flag_parses_and_defaults_to_off() -> Result<(), clap::Error> {
        let cli = Cli::try_parse_from(["karet", "--capture"])?;
        assert!(cli.capture);
        assert!(!Cli::try_parse_from(["karet"])?.capture);
        Ok(())
    }

    #[test]
    fn capture_tuning_flags_require_capture() {
        // The tuning flags are meaningless on their own; clap rejects them so a
        // scripted typo fails loudly instead of silently starting the TUI.
        for flag in ["--capture-size", "--capture-settle", "--capture-timeout"] {
            assert!(
                Cli::try_parse_from(["karet", flag, "10"]).is_err(),
                "{flag} must require --capture"
            );
        }
    }

    #[test]
    fn capture_spec_uses_defaults_when_unset() -> Result<(), clap::Error> {
        let spec = Cli::try_parse_from(["karet", "--capture"])?
            .capture_spec()
            .map_err(|e| clap::Error::raw(clap::error::ErrorKind::InvalidValue, e))?;
        assert_eq!(
            (spec.cols, spec.rows),
            (super::CAPTURE_DEFAULT_COLS, super::CAPTURE_DEFAULT_ROWS)
        );
        assert_eq!(
            spec.settle.as_millis(),
            u128::from(super::CAPTURE_DEFAULT_SETTLE_MS)
        );
        assert_eq!(
            spec.timeout.as_millis(),
            u128::from(super::CAPTURE_DEFAULT_TIMEOUT_MS)
        );
        Ok(())
    }

    #[test]
    fn capture_spec_reads_every_flag() -> Result<(), clap::Error> {
        let spec = Cli::try_parse_from([
            "karet",
            "--capture",
            "--capture-size",
            "100x30",
            "--capture-settle",
            "50",
            "--capture-timeout",
            "900",
        ])?
        .capture_spec()
        .map_err(|e| clap::Error::raw(clap::error::ErrorKind::InvalidValue, e))?;
        assert_eq!((spec.cols, spec.rows), (100, 30));
        assert_eq!(spec.settle.as_millis(), 50);
        assert_eq!(spec.timeout.as_millis(), 900);
        Ok(())
    }

    #[test]
    fn capture_spec_surfaces_a_bad_size() -> Result<(), clap::Error> {
        let cli = Cli::try_parse_from(["karet", "--capture", "--capture-size", "wide"])?;
        assert!(cli.capture_spec().is_err());
        Ok(())
    }

    #[test]
    fn parse_capture_size_accepts_both_separators() {
        assert_eq!(super::parse_capture_size("120x34"), Ok((120, 34)));
        assert_eq!(super::parse_capture_size("120X34"), Ok((120, 34)));
    }

    #[test]
    fn parse_capture_size_clamps_up_to_the_layout_minimum() {
        // Too small to lay out is clamped, not rejected: a tiny request still renders.
        assert_eq!(
            super::parse_capture_size("1x1"),
            Ok((super::CAPTURE_MIN_COLS, super::CAPTURE_MIN_ROWS))
        );
    }

    #[test]
    fn parse_capture_size_rejects_malformed_input() {
        for spec in ["120", "x34", "120x", "0x34", "120x0", "-1x5", "12.5x30", ""] {
            assert!(
                super::parse_capture_size(spec).is_err(),
                "{spec:?} must be rejected"
            );
        }
        // Out of u16 range is an error, not a wrap-around.
        assert!(super::parse_capture_size("70000x30").is_err());
    }

    #[test]
    fn parse_goto_spec_path_only() {
        let spec = super::parse_goto_spec("src/main.rs");
        assert_eq!(spec.path, PathBuf::from("src/main.rs"));
        assert_eq!((spec.line, spec.col), (1, 1));
    }

    #[test]
    fn parse_goto_spec_path_and_line() {
        let spec = super::parse_goto_spec("src/main.rs:42");
        assert_eq!(spec.path, PathBuf::from("src/main.rs"));
        assert_eq!((spec.line, spec.col), (42, 1));
    }

    #[test]
    fn parse_goto_spec_path_line_col() {
        let spec = super::parse_goto_spec("src/main.rs:42:7");
        assert_eq!(spec.path, PathBuf::from("src/main.rs"));
        assert_eq!((spec.line, spec.col), (42, 7));
    }

    #[test]
    fn parse_goto_spec_preserves_colons_in_path() {
        // A non-numeric trailing segment is part of the path, not a coordinate.
        let spec = super::parse_goto_spec("weird:name.rs");
        assert_eq!(spec.path, PathBuf::from("weird:name.rs"));
        assert_eq!((spec.line, spec.col), (1, 1));

        // Coordinates peel from the right, leaving an interior-colon path intact
        // (e.g. a Windows drive prefix).
        let spec = super::parse_goto_spec("C:/proj/main.rs:12:3");
        assert_eq!(spec.path, PathBuf::from("C:/proj/main.rs"));
        assert_eq!((spec.line, spec.col), (12, 3));
    }

    #[test]
    fn parse_goto_spec_junk_and_zero_semantics() {
        // A non-digit suffix stays part of the path.
        let spec = super::parse_goto_spec("main.rs:x");
        assert_eq!(spec.path, PathBuf::from("main.rs:x"));
        assert_eq!((spec.line, spec.col), (1, 1));

        // Zero is a valid number but clamps up to the 1-based minimum of 1.
        let spec = super::parse_goto_spec("main.rs:0");
        assert_eq!(spec.path, PathBuf::from("main.rs"));
        assert_eq!((spec.line, spec.col), (1, 1));

        let spec = super::parse_goto_spec("main.rs:5:0");
        assert_eq!(spec.path, PathBuf::from("main.rs"));
        assert_eq!((spec.line, spec.col), (5, 1));

        // A leading-colon spec has no path to keep, so the colon belongs to the path.
        let spec = super::parse_goto_spec(":5");
        assert_eq!(spec.path, PathBuf::from(":5"));
        assert_eq!((spec.line, spec.col), (1, 1));

        // An out-of-u32-range number is not a coordinate; it stays in the path.
        let spec = super::parse_goto_spec("main.rs:99999999999999999999");
        assert_eq!(spec.path, PathBuf::from("main.rs:99999999999999999999"));
        assert_eq!((spec.line, spec.col), (1, 1));
    }

    #[test]
    fn split_goto_suffix_reports_whether_a_target_was_given() {
        use super::split_goto_suffix;
        assert_eq!(split_goto_suffix("main.rs"), ("main.rs", None));
        assert_eq!(split_goto_suffix("main.rs:42"), ("main.rs", Some((42, 1))));
        assert_eq!(
            split_goto_suffix("src/a.rs:4:9"),
            ("src/a.rs", Some((4, 9)))
        );
        // `main.rs:1` is a target even though it equals the default.
        assert_eq!(split_goto_suffix("main.rs:1"), ("main.rs", Some((1, 1))));
        // An unfinished suffix is not a target yet.
        assert_eq!(split_goto_suffix("main.rs:"), ("main.rs:", None));
        assert_eq!(split_goto_suffix(":5"), (":5", None));
    }

    #[test]
    fn parse_line_col_reads_a_bare_target() {
        use super::parse_line_col;
        assert_eq!(parse_line_col("42"), Some((42, 1)));
        assert_eq!(parse_line_col("42:7"), Some((42, 7)));
        assert_eq!(parse_line_col(" 42 "), Some((42, 1)));
        assert_eq!(parse_line_col("0:0"), Some((1, 1)), "zero clamps up to 1");
        for junk in [
            "",
            "x",
            "-3",
            "+3",
            "4:",
            ":4",
            "4:5:6",
            "4 5",
            "99999999999",
        ] {
            assert_eq!(parse_line_col(junk), None, "{junk:?}");
        }
    }
}
