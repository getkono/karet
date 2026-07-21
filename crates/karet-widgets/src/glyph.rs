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
            Self::Close => '\u{f00d}',         // times
            Self::ChevronRight => '\u{f054}',  // chevron-right
            Self::NewFile => '\u{f15b}',       // file
            Self::NewFolder => '\u{f07b}',     // folder
            Self::Refresh => '\u{f021}',       // refresh
            Self::CollapseAll => '\u{f066}',   // compress
            Self::Symlink => '\u{f0c1}',       // link
        }
    }

    /// Widely-supported 1-cell BMP symbol for the Unicode tier.
    fn unicode(self) -> char {
        match self {
            Self::Explorer => '\u{2630}',      // ☰ trigram (list)
            Self::Search => '\u{2315}',        // ⌕ telephone recorder (magnifier-ish)
            Self::SourceControl => '\u{2387}', // ⎇ alternative key (branch-ish)
            Self::Close => '\u{00d7}',         // ×
            Self::ChevronRight => '\u{203a}',  // ›
            Self::NewFile => '\u{25A4}',       // ▤ (file-ish lines)
            Self::NewFolder => '\u{25B0}',     // ▰ (folder-ish block)
            Self::Refresh => '\u{21BB}',       // ↻
            Self::CollapseAll => '\u{2212}',   // − (minus / collapse)
            Self::Symlink => '\u{2197}',       // ↗ (redirect / link)
        }
    }

    /// Mnemonic ASCII letter for the most portable tier.
    fn ascii(self) -> char {
        match self {
            Self::Explorer => 'E',
            Self::Search => 'S',
            Self::SourceControl => 'B', // branch
            Self::Close => 'x',
            Self::ChevronRight => '>',
            Self::NewFile => '+',
            Self::NewFolder => 'D', // new directory
            Self::Refresh => 'R',
            Self::CollapseAll => '-',
            Self::Symlink => '@',
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
            UiIcon::Close,
            UiIcon::ChevronRight,
            UiIcon::NewFile,
            UiIcon::NewFolder,
            UiIcon::Refresh,
            UiIcon::CollapseAll,
            UiIcon::Symlink,
        ] {
            assert!(icon.glyph(IconStyle::Ascii).is_ascii_graphic());
        }
    }
}
