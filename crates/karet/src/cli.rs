//! Command-line interface.

use std::path::PathBuf;

use clap::Parser;
use clap::ValueEnum;
use karet_filetype::IconStyle;

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

impl From<IconChoice> for IconStyle {
    fn from(choice: IconChoice) -> Self {
        match choice {
            IconChoice::Nerd => Self::NerdFont,
            IconChoice::Unicode => Self::Unicode,
            IconChoice::Ascii => Self::Ascii,
        }
    }
}

impl Cli {
    /// Resolve the icon style: an explicit `--icons` flag wins, then the
    /// `KARET_ICONS` env var, else the default (Nerd Font).
    #[must_use]
    pub fn icon_style(&self) -> IconStyle {
        self.icons
            .map(IconStyle::from)
            .or_else(|| {
                std::env::var("KARET_ICONS")
                    .ok()
                    .and_then(|v| IconStyle::from_name(&v))
            })
            .unwrap_or_default()
    }
}
