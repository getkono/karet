//! Command-line interface.

use std::path::PathBuf;

use clap::Parser;
use clap::ValueEnum;
use karet_filetype::IconStyle;
use karet_session::config::schema::StartupPanel;

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

    /// Print terminal-capability diagnostics and exit instead of starting the
    /// editor. Probes the same features karet checks at startup (kitty keyboard
    /// protocol, kitty graphics protocol, OSC 22 pointer shapes) and reports one
    /// line per check; exits non-zero when a required capability is missing. The
    /// other flags are ignored.
    #[arg(long)]
    pub doctor: bool,

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
    // Peel the rightmost `:<digits>` group. `None` when there is no colon, the tail
    // is empty or non-numeric, the value overflows `u32`, or peeling would leave an
    // empty path (`:5`) — in every such case the colon belongs to the path.
    fn peel(s: &str) -> Option<(&str, u32)> {
        let idx = s.rfind(':')?;
        let head = &s[..idx];
        let tail = &s[idx + 1..];
        if head.is_empty() || tail.is_empty() || !tail.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        Some((head, tail.parse::<u32>().ok()?))
    }

    // The first peel yields the rightmost number: the column when a second number
    // precedes it (`PATH:LINE:COL`), otherwise the line (`PATH:LINE`).
    match peel(spec) {
        None => GotoSpec {
            path: PathBuf::from(spec),
            line: 1,
            col: 1,
        },
        Some((rest, first)) => match peel(rest) {
            Some((rest2, second)) => GotoSpec {
                path: PathBuf::from(rest2),
                line: second.max(1),
                col: first.max(1),
            },
            None => GotoSpec {
                path: PathBuf::from(rest),
                line: first.max(1),
                col: 1,
            },
        },
    }
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
}
