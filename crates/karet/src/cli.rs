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
