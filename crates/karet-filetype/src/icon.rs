//! Icon styles, the [`Category`] taxonomy, and the per-style fallback glyphs.
//!
//! Three tiers of richness, chosen by [`IconStyle`]:
//! - [`IconStyle::NerdFont`] — per-*type* glyphs from the registry (the default);
//!   directories also get a folder glyph beside the expand chevron.
//! - [`IconStyle::Unicode`] — per-*category* geometric BMP glyphs (1 cell, widely
//!   supported); directories show the chevron only.
//! - [`IconStyle::Ascii`] — maximally portable; files are blank, directories use
//!   `>`/`v` chevrons.

/// The glyph set used to render file/folder icons.
///
/// `NerdFont` is the default, matching karet's "modern terminal" stance; the
/// other two are fallbacks selectable at runtime (Nerd Font support is a property
/// of the configured font and cannot be detected, so it is a deliberate choice).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum IconStyle {
    /// ASCII-only — maximally portable (`>`/`v` for directories, blank files).
    Ascii,
    /// Unicode geometric glyphs — 1-cell, widely supported, per-category.
    Unicode,
    /// Nerd Font glyphs — rich per-file-type icons (requires a patched font).
    #[default]
    NerdFont,
}

impl IconStyle {
    /// Resolve a case-insensitive name (`"nerd"`/`"unicode"`/`"ascii"`), as used
    /// by the `--icons` flag and the `KARET_ICONS` env var. Returns `None` for an
    /// unrecognized name so the caller can fall back to the default.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "nerd" | "nerdfont" | "nerd-font" => Some(Self::NerdFont),
            "unicode" | "uni" => Some(Self::Unicode),
            "ascii" | "plain" => Some(Self::Ascii),
            _ => None,
        }
    }
}

/// A coarse classification of a file type, used to pick fallback glyphs and to
/// hint at how content should be presented.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Category {
    /// Source code in a programming language.
    Code,
    /// Markup / prose (HTML, Markdown, XML, …).
    Markup,
    /// Structured data (JSON, CSV, SQL, …).
    Data,
    /// Configuration (TOML, YAML, INI, dotfiles, build files, …).
    Config,
    /// Shell / scripting glue (sh, bash, …).
    Shell,
    /// A raster or vector image.
    Image,
    /// A rendered document (PDF, plain text, office, …).
    Document,
    /// A compressed archive.
    Archive,
    /// Opaque binary content (executables, fonts, media, …).
    Binary,
    /// An unrecognized file type.
    Unknown,
}

impl Category {
    /// The Nerd Font glyph used when a [`FileType`](crate::FileType) has no more
    /// specific per-type glyph. These are classic FontAwesome codepoints present
    /// in every Nerd Font build.
    pub(crate) fn nerd_icon(self) -> char {
        match self {
            Self::Code => '\u{f1c9}',     // file-code-o
            Self::Markup => '\u{f1c9}',   // file-code-o
            Self::Data => '\u{f1c0}',     // database
            Self::Config => '\u{f013}',   // cog
            Self::Shell => '\u{f489}',    // terminal
            Self::Image => '\u{f1c5}',    // file-image-o
            Self::Document => '\u{f15c}', // file-text
            Self::Archive => '\u{f1c6}',  // file-archive-o
            Self::Binary => '\u{f471}',   // binary
            Self::Unknown => '\u{f15b}',  // file
        }
    }

    /// A 1-cell geometric glyph for the Unicode fallback tier (all in the
    /// U+25xx / U+00B7 ranges, reliably single-width and widely available).
    pub(crate) fn unicode_icon(self) -> char {
        match self {
            Self::Code => '\u{25c6}',     // ◆ black diamond
            Self::Markup => '\u{25c8}',   // ◈ diamond-in-diamond
            Self::Data => '\u{25a6}',     // ▦ squared fill
            Self::Config => '\u{25c9}',   // ◉ fisheye
            Self::Shell => '\u{25b7}',    // ▷ right triangle
            Self::Image => '\u{25a3}',    // ▣ squared square
            Self::Document => '\u{25a4}', // ▤ horizontal fill
            Self::Archive => '\u{25a5}',  // ▥ vertical fill
            Self::Binary => '\u{25aa}',   // ▪ small black square
            Self::Unknown => '\u{00b7}',  // · middle dot
        }
    }

    /// A plain-ASCII marker for the most portable tier. Files are intentionally
    /// blank (a space) so the tree stays clean where no font support is assumed;
    /// directories are still marked by the [`chevron`].
    pub(crate) fn ascii_icon(self) -> char {
        ' '
    }
}

/// The expand/collapse chevron for a directory row, per [`IconStyle`].
#[must_use]
pub fn chevron(open: bool, style: IconStyle) -> char {
    match style {
        IconStyle::Ascii => {
            if open {
                'v'
            } else {
                '>'
            }
        },
        IconStyle::Unicode | IconStyle::NerdFont => {
            if open {
                '\u{25be}' // ▾
            } else {
                '\u{25b8}' // ▸
            }
        },
    }
}

/// The folder glyph drawn beside the [`chevron`], if the style has one.
///
/// `NerdFont` returns an open/closed folder; `Unicode`/`Ascii` return `None`
/// (the chevron alone marks a directory) so fallbacks stay clean.
#[must_use]
pub fn directory_icon(open: bool, style: IconStyle) -> Option<char> {
    match style {
        IconStyle::NerdFont => Some(if open { '\u{f07c}' } else { '\u{f07b}' }),
        IconStyle::Unicode | IconStyle::Ascii => None,
    }
}
