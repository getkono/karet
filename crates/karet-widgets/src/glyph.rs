//! Semantic UI-chrome glyphs (activity-bar entries, close buttons, separators)
//! resolved per [`IconStyle`].
//!
//! These are distinct from file-type icons (which live in the
//! [`karet_filetype`] registry): they label UI *actions* and chrome. Centralizing
//! them keeps glyph choices consistent and testable, and lets the sidebar/activity
//! bar pick a single style at runtime. The Nerd Font tier uses rich glyphs; the
//! Unicode tier uses widely-supported BMP symbols; the ASCII tier uses mnemonic
//! letters so the bar is never a row of ambiguous digits.

use karet_filetype::IconStyle;

/// A semantic UI icon, rendered to a glyph by [`UiIcon::glyph`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum UiIcon {
    /// The file-explorer activity-bar entry.
    Explorer,
    /// The search activity-bar entry.
    Search,
    /// The source-control activity-bar entry.
    SourceControl,
    /// The spelling activity-bar entry.
    Spelling,
    /// The codetag (TODO) activity-bar entry.
    Todos,
    /// The debugger activity-bar entry.
    Debug,
    /// A close ("×") affordance, e.g. on a tab.
    Close,
    /// A right-pointing chevron, e.g. a breadcrumb separator.
    ChevronRight,
    /// The explorer "new file" toolbar action.
    NewFile,
    /// The explorer "new folder" toolbar action.
    NewFolder,
    /// The explorer "refresh" toolbar action.
    Refresh,
    /// The explorer "collapse all folders" toolbar action.
    CollapseAll,
    /// A filesystem symbolic-link marker.
    Symlink,
    /// Show or hide a rendered preview for the active document.
    Preview,
    /// Format a source table.
    FormatTable,
    /// Seam lens: what is visible from outside.
    SeamApi,
    /// Seam lens: what behavior can be swapped.
    SeamSubstitution,
    /// Seam lens: what changes shape before compiling.
    SeamVariation,
    /// Seam lens: what crosses the package line.
    SeamBoundary,
    /// Seam lens: where substitution is dangerous.
    SeamHazard,
    /// A node with something beneath it.
    SeamHasChildren,
    /// A node excluded by the active configuration — present, but not built.
    SeamInactive,
}

impl UiIcon {
    /// The glyph for this icon in the given [`IconStyle`].
    #[must_use]
    pub fn glyph(self, style: IconStyle) -> char {
        match style {
            IconStyle::NerdFont => self.nerd(),
            IconStyle::Unicode => self.unicode(),
            IconStyle::Ascii => self.ascii(),
        }
    }

    /// Nerd Font glyph (FontAwesome codepoints present in every Nerd Font build).
    fn nerd(self) -> char {
        match self {
            Self::Explorer => '\u{f0c5}',      // files
            Self::Search => '\u{f002}',        // magnifier
            Self::SourceControl => '\u{f126}', // code-fork (branch)
            Self::Spelling => '\u{f02d}',      // book (dictionary)
            Self::Todos => '\u{f00c}',         // check mark (tasks)
            Self::Debug => '\u{f188}',         // bug
            Self::Close => '\u{f00d}',         // times
            Self::ChevronRight => '\u{f054}',  // chevron-right
            Self::NewFile => '\u{f15b}',       // file
            Self::NewFolder => '\u{f07b}',     // folder
            Self::Refresh => '\u{f021}',       // refresh
            Self::CollapseAll => '\u{f066}',   // compress
            Self::Symlink => '\u{f0c1}',       // link
            Self::Preview => '\u{f06e}',       // eye
            Self::FormatTable => '\u{f0ce}',   // table
            // Seam lenses. Distinct silhouettes matter more than literal depiction —
            // these are read side by side on one row.
            Self::SeamApi => '\u{f06e}', // eye — visible from outside
            Self::SeamSubstitution => '\u{f0ec}', // exchange — swappable
            Self::SeamVariation => '\u{f126}', // code-branch — varies before compiling
            Self::SeamBoundary => '\u{f08e}', // external-link — crosses the line
            Self::SeamHazard => '\u{f071}', // warning — dangerous to substitute
            Self::SeamHasChildren => '\u{f054}', // chevron-right
            Self::SeamInactive => '\u{f111}', // small circle
        }
    }

    /// Widely-supported 1-cell BMP symbol for the Unicode tier.
    fn unicode(self) -> char {
        match self {
            Self::Explorer => '\u{2630}',         // ☰ trigram (list)
            Self::Search => '\u{2315}',           // ⌕ telephone recorder (magnifier-ish)
            Self::SourceControl => '\u{2387}',    // ⎇ alternative key (branch-ish)
            Self::Spelling => '\u{00b6}',         // ¶ pilcrow (prose)
            Self::Todos => '\u{2713}',            // ✓ check mark (tasks)
            Self::Debug => '\u{25f4}',            // ◴ (dial: run state)
            Self::Close => '\u{00d7}',            // ×
            Self::ChevronRight => '\u{203a}',     // ›
            Self::NewFile => '\u{25A4}',          // ▤ (file-ish lines)
            Self::NewFolder => '\u{25B0}',        // ▰ (folder-ish block)
            Self::Refresh => '\u{21BB}',          // ↻
            Self::CollapseAll => '\u{2212}',      // − (minus / collapse)
            Self::Symlink => '\u{2197}',          // ↗ (redirect / link)
            Self::Preview => '\u{25c9}',          // ◉ (preview)
            Self::FormatTable => '\u{25a6}',      // ▦ (grid)
            Self::SeamApi => '\u{2b24}',          // ⬤ filled circle
            Self::SeamSubstitution => '\u{25c7}', // ◇ open diamond
            Self::SeamVariation => '\u{2325}',    // ⌥ option key
            Self::SeamBoundary => '\u{21e5}',     // ⇥ rightwards arrow to bar
            Self::SeamHazard => '\u{26a1}',       // ⚡ high voltage
            Self::SeamHasChildren => '\u{25b8}',  // ▸ small right triangle
            Self::SeamInactive => '\u{00b7}',     // · middle dot
        }
    }

    /// Mnemonic ASCII letter for the most portable tier.
    fn ascii(self) -> char {
        match self {
            Self::Explorer => 'E',
            Self::Search => 'S',
            Self::SourceControl => 'B', // branch
            Self::Spelling => 'W',      // words
            Self::Todos => 'T',         // todos
            Self::Debug => 'G',         // debuG (D is taken by new directory)
            Self::Close => 'x',
            Self::ChevronRight => '>',
            Self::NewFile => '+',
            Self::NewFolder => 'D', // new directory
            Self::Refresh => 'R',
            Self::CollapseAll => '-',
            Self::Symlink => '@',
            Self::Preview => 'P',
            Self::FormatTable => 'T',
            // Mnemonic and unambiguous side by side; no digits, which read as counts.
            Self::SeamApi => '*',
            Self::SeamSubstitution => '#',
            Self::SeamVariation => '%',
            Self::SeamBoundary => '+',
            Self::SeamHazard => '!',
            Self::SeamHasChildren => '>',
            Self::SeamInactive => '.',
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glyph_varies_by_style() {
        assert_eq!(UiIcon::Search.glyph(IconStyle::NerdFont), '\u{f002}');
        assert_eq!(UiIcon::Search.glyph(IconStyle::Unicode), '\u{2315}');
        assert_eq!(UiIcon::Search.glyph(IconStyle::Ascii), 'S');
    }

    #[test]
    fn ascii_is_printable_single_width() {
        for icon in [
            UiIcon::Explorer,
            UiIcon::Search,
            UiIcon::SourceControl,
            UiIcon::Spelling,
            UiIcon::Close,
            UiIcon::ChevronRight,
            UiIcon::NewFile,
            UiIcon::NewFolder,
            UiIcon::Refresh,
            UiIcon::CollapseAll,
            UiIcon::Symlink,
            UiIcon::Preview,
            UiIcon::FormatTable,
        ] {
            assert!(icon.glyph(IconStyle::Ascii).is_ascii_graphic());
        }
        for icon in SEAM_ICONS {
            assert!(icon.glyph(IconStyle::Ascii).is_ascii_graphic());
        }
    }

    /// The seam legend, which is read as a group on a single row.
    const SEAM_ICONS: [UiIcon; 7] = [
        UiIcon::SeamApi,
        UiIcon::SeamSubstitution,
        UiIcon::SeamVariation,
        UiIcon::SeamBoundary,
        UiIcon::SeamHazard,
        UiIcon::SeamHasChildren,
        UiIcon::SeamInactive,
    ];

    #[test]
    fn seam_glyphs_stay_distinct_within_every_style() {
        // These appear side by side on one row, so two lenses sharing a glyph would make
        // the row unreadable rather than merely imprecise.
        for style in [IconStyle::NerdFont, IconStyle::Unicode, IconStyle::Ascii] {
            let mut seen: Vec<char> = SEAM_ICONS.iter().map(|i| i.glyph(style)).collect();
            let total = seen.len();
            seen.sort_unstable();
            seen.dedup();
            assert_eq!(seen.len(), total, "duplicate seam glyph in {style:?}");
        }
    }

    #[test]
    fn seam_ascii_glyphs_avoid_digits() {
        // A digit beside a rollup count would read as part of the number.
        for icon in SEAM_ICONS {
            let glyph = icon.glyph(IconStyle::Ascii);
            assert!(!glyph.is_ascii_digit(), "{icon:?} uses a digit: {glyph}");
        }
    }
}
