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
use unicode_width::UnicodeWidthChar;

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
    ///
    /// Every one is `East_Asian_Width=Neutral` and carries no emoji presentation, so it
    /// measures one cell in every terminal. That is a hard requirement rather than a
    /// preference: these are read as a group, and one glyph two cells wide pushes
    /// everything after it out of line with the row above.
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
            Self::SeamApi => '\u{25c9}',          // ◉ fisheye (a filled eye)
            Self::SeamSubstitution => '\u{25ca}', // ◊ lozenge
            Self::SeamVariation => '\u{2325}',    // ⌥ option key
            Self::SeamBoundary => '\u{21e5}',     // ⇥ rightwards arrow to bar
            Self::SeamHazard => '\u{2621}',       // ☡ caution sign
            Self::SeamHasChildren => '\u{25b8}',  // ▸ small right triangle
            Self::SeamInactive => '\u{2219}',     // ∙ bullet operator
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

/// The cells one marker glyph is given in `style`, whatever it measures.
///
/// Marker glyphs are read as a group — five lenses side by side on a spine row, the same
/// five in the legend above. One glyph measuring two cells where its neighbour measures
/// one stops the group being a column of aligned marks, and the count on the row below no
/// longer lines up with the one above it.
///
/// Two cells for the Nerd tier, because its codepoints sit in the Private Use Area, which
/// `East_Asian_Width` calls *ambiguous*: `unicode-width` answers one, and a terminal
/// configured for double-width ambiguous characters paints two. No measurement settles
/// that, so the slot is reserved wide and the glyph padded into it — a glyph the font
/// draws wide then overruns its own padding rather than the row. One cell for the Unicode
/// and ASCII tiers, whose glyphs are chosen to be unambiguously narrow.
#[must_use]
pub const fn glyph_slot(style: IconStyle) -> usize {
    match style {
        IconStyle::NerdFont => 2,
        IconStyle::Unicode | IconStyle::Ascii => 1,
    }
}

/// `glyph` padded with trailing spaces to exactly [`glyph_slot`] cells.
///
/// Trailing, never leading: the glyph stays on its slot's left edge, so a run of them
/// reads as a column rather than as marks drifting rightwards.
#[must_use]
pub fn slot(glyph: char, style: IconStyle) -> String {
    let width = UnicodeWidthChar::width(glyph).unwrap_or(0);
    let mut out = String::with_capacity(glyph.len_utf8() + 1);
    out.push(glyph);
    for _ in 0..glyph_slot(style).saturating_sub(width) {
        out.push(' ');
    }
    out
}

/// A run of glyphs, each in its own slot: `n` of them always occupy `n * glyph_slot` cells.
#[must_use]
pub fn slots(glyphs: impl IntoIterator<Item = char>, style: IconStyle) -> String {
    glyphs.into_iter().map(|glyph| slot(glyph, style)).collect()
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
    fn every_seam_glyph_fills_exactly_one_slot_in_every_tier() {
        // The whole point: n glyphs occupy n slots, so a row of markers is a column of
        // aligned marks and the count beside them never drifts.
        for style in [IconStyle::NerdFont, IconStyle::Unicode, IconStyle::Ascii] {
            for icon in SEAM_ICONS {
                let painted = slot(icon.glyph(style), style);
                assert_eq!(
                    crate::text::width(&painted),
                    glyph_slot(style),
                    "{icon:?} in {style:?}"
                );
            }
        }
    }

    #[test]
    fn no_seam_glyph_measures_wider_than_its_slot() {
        // What makes `slot`'s saturating pad safe: over-wide would silently under-pad.
        for style in [IconStyle::NerdFont, IconStyle::Unicode, IconStyle::Ascii] {
            for icon in SEAM_ICONS {
                let glyph = icon.glyph(style);
                let width = unicode_width::UnicodeWidthChar::width(glyph).unwrap_or(0);
                assert!(width <= glyph_slot(style), "{icon:?} in {style:?}: {width}");
            }
        }
    }

    #[test]
    fn a_run_of_glyphs_occupies_one_slot_each() {
        for style in [IconStyle::NerdFont, IconStyle::Unicode, IconStyle::Ascii] {
            let run = slots(SEAM_ICONS.iter().map(|icon| icon.glyph(style)), style);
            assert_eq!(
                crate::text::width(&run),
                SEAM_ICONS.len() * glyph_slot(style),
                "{style:?}"
            );
        }
    }

    #[test]
    fn the_nerd_tier_reserves_two_cells_because_its_codepoints_are_ambiguous() {
        // The reservation is only justified while the glyphs really are Private Use.
        assert_eq!(glyph_slot(IconStyle::NerdFont), 2);
        for icon in SEAM_ICONS {
            let glyph = icon.glyph(IconStyle::NerdFont);
            assert!(
                ('\u{e000}'..='\u{f8ff}').contains(&glyph),
                "{icon:?} is not a Private Use codepoint: {glyph:?}"
            );
        }
    }

    #[test]
    fn the_unicode_tier_is_pinned_to_narrow_non_emoji_codepoints() {
        // Each is East_Asian_Width=Neutral with no emoji presentation. Pinned by
        // codepoint so swapping one forces re-checking that class.
        assert_eq!(UiIcon::SeamApi.glyph(IconStyle::Unicode), '\u{25c9}');
        assert_eq!(
            UiIcon::SeamSubstitution.glyph(IconStyle::Unicode),
            '\u{25ca}'
        );
        assert_eq!(UiIcon::SeamVariation.glyph(IconStyle::Unicode), '\u{2325}');
        assert_eq!(UiIcon::SeamBoundary.glyph(IconStyle::Unicode), '\u{21e5}');
        assert_eq!(UiIcon::SeamHazard.glyph(IconStyle::Unicode), '\u{2621}');
        assert_eq!(
            UiIcon::SeamHasChildren.glyph(IconStyle::Unicode),
            '\u{25b8}'
        );
        assert_eq!(UiIcon::SeamInactive.glyph(IconStyle::Unicode), '\u{2219}');
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
