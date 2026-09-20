//! Painting the language-server badge: its glyph, its words, and its colour.
//!
//! Two surfaces read the same model. A pane's breadcrumb gets the compact form
//! — a glyph, the letters `LSP`, and a count only when providers disagree — so
//! every visible pane can carry its own without crowding the path. The status
//! bar gets the same state spelled out, because there is room for it and one
//! long label is easier to learn the glyphs from than a legend would be.
//!
//! This lives in the app, not in `karet-widgets`: a widget crate that knew what
//! `NotInstalled` meant would be a renderer depending on a producer's
//! vocabulary, which is exactly the coupling the workspace forbids.

use karet_core::ThemeRole;
use karet_filetype::IconStyle;

use crate::app::LanguageServerBadge;
use crate::app::LanguageServerBadgeSummary;

/// The glyph for a badge state.
///
/// Every Unicode-tier glyph is one cell wide. That is a requirement, not a
/// preference: the badge is right-aligned against the breadcrumb, so a glyph
/// two cells wide would shift the path under it out of line with the tab strip
/// above.
fn glyph(state: LanguageServerBadge, style: IconStyle) -> char {
    match style {
        IconStyle::NerdFont => match state {
            LanguageServerBadge::Off => '\u{f111}',          // circle
            LanguageServerBadge::Idle => '\u{f111}',         // circle
            LanguageServerBadge::Ready => '\u{f00c}',        // check
            LanguageServerBadge::Starting => '\u{f021}',     // refresh
            LanguageServerBadge::Retrying => '\u{f021}',     // refresh
            LanguageServerBadge::NeedsSetup => '\u{f071}',   // warning
            LanguageServerBadge::NotInstalled => '\u{f019}', // download
            LanguageServerBadge::Failed => '\u{f00d}',       // times
        },
        IconStyle::Unicode => match state {
            LanguageServerBadge::Off => '\u{2219}',   // ∙ bullet operator
            LanguageServerBadge::Idle => '\u{25e6}',  // ◦ white bullet
            LanguageServerBadge::Ready => '\u{25c6}', // ◆ black diamond
            LanguageServerBadge::Starting => '\u{25cc}', // ◌ dotted circle
            LanguageServerBadge::Retrying => '\u{21bb}', // ↻ clockwise arrow
            LanguageServerBadge::NeedsSetup => '\u{2621}', // ☡ caution sign
            LanguageServerBadge::NotInstalled => '\u{2193}', // ↓ downwards arrow
            LanguageServerBadge::Failed => '\u{25b2}', // ▲ black up triangle
        },
        // Mnemonic and unambiguous side by side; no digits, which read as counts
        // next to a badge that really does show one.
        IconStyle::Ascii => match state {
            LanguageServerBadge::Off => '-',
            LanguageServerBadge::Idle => 'o',
            LanguageServerBadge::Ready => '=',
            LanguageServerBadge::Starting => '~',
            LanguageServerBadge::Retrying => '~',
            LanguageServerBadge::NeedsSetup => '!',
            LanguageServerBadge::NotInstalled => '+',
            LanguageServerBadge::Failed => 'X',
        },
    }
}

/// The state spelled out, for the status bar.
pub(super) fn label(state: LanguageServerBadge) -> &'static str {
    match state {
        LanguageServerBadge::Off => "LSP off",
        LanguageServerBadge::Idle => "LSP idle",
        LanguageServerBadge::Ready => "LSP ready",
        LanguageServerBadge::Starting => "LSP starting",
        LanguageServerBadge::Retrying => "LSP retrying",
        LanguageServerBadge::NeedsSetup => "LSP needs setup",
        LanguageServerBadge::NotInstalled => "LSP not installed",
        LanguageServerBadge::Failed => "LSP failed",
    }
}

/// The semantic colour for a state.
///
/// `Failed` and `NotInstalled` are errors because something the user expected
/// to work does not. `NeedsSetup` is only a warning: karet is reporting a
/// condition it was never going to resolve itself, and the user may well have
/// chosen it.
pub(super) fn role(state: LanguageServerBadge) -> ThemeRole {
    match state {
        LanguageServerBadge::Ready => ThemeRole::DiagnosticHint,
        LanguageServerBadge::Starting | LanguageServerBadge::Retrying => {
            ThemeRole::DiagnosticWarning
        },
        LanguageServerBadge::NeedsSetup => ThemeRole::DiagnosticWarning,
        LanguageServerBadge::NotInstalled | LanguageServerBadge::Failed => {
            ThemeRole::DiagnosticError
        },
        LanguageServerBadge::Off | LanguageServerBadge::Idle => ThemeRole::Muted,
    }
}

/// The `n/m` healthy-provider suffix, when it informs.
pub(super) fn count_suffix(badge: LanguageServerBadgeSummary) -> Option<String> {
    badge
        .shows_count()
        .then(|| format!("{}/{}", badge.healthy, badge.total))
}

/// The compact breadcrumb form: `" ◆ LSP "`, or `" ▲ LSP 1/2 "`.
///
/// Padded on both sides so the glyph never abuts the path on its left or the
/// pane border on its right.
pub(super) fn compact(badge: LanguageServerBadgeSummary, style: IconStyle) -> String {
    match count_suffix(badge) {
        Some(count) => format!(" {} LSP {count} ", glyph(badge.state, style)),
        None => format!(" {} LSP ", glyph(badge.state, style)),
    }
}

/// The status-bar form: the state spelled out, with the count when it informs.
pub(super) fn spelled(badge: LanguageServerBadgeSummary) -> String {
    match count_suffix(badge) {
        Some(count) => format!("{} {count}", label(badge.state)),
        None => label(badge.state).to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use unicode_width::UnicodeWidthChar;

    use super::*;

    const EVERY_STATE: [LanguageServerBadge; 8] = [
        LanguageServerBadge::Off,
        LanguageServerBadge::Idle,
        LanguageServerBadge::Ready,
        LanguageServerBadge::Starting,
        LanguageServerBadge::Retrying,
        LanguageServerBadge::NeedsSetup,
        LanguageServerBadge::NotInstalled,
        LanguageServerBadge::Failed,
    ];

    fn summary(
        state: LanguageServerBadge,
        healthy: usize,
        total: usize,
    ) -> LanguageServerBadgeSummary {
        LanguageServerBadgeSummary {
            state,
            healthy,
            total,
        }
    }

    #[test]
    fn every_unicode_glyph_is_one_cell() {
        for state in EVERY_STATE {
            let glyph = glyph(state, IconStyle::Unicode);
            assert_eq!(
                glyph.width(),
                Some(1),
                "{state:?} renders {glyph:?} at a width other than one cell"
            );
        }
    }

    #[test]
    fn every_ascii_glyph_is_printable_ascii() {
        for state in EVERY_STATE {
            let glyph = glyph(state, IconStyle::Ascii);
            assert!(
                glyph.is_ascii_graphic(),
                "{state:?} renders {glyph:?}, which is not printable ASCII"
            );
        }
    }

    #[test]
    fn distinct_conditions_get_distinct_glyphs() {
        // Off/Idle and Starting/Retrying deliberately share a glyph -- they differ
        // by colour, and the pair reads as one condition at a glance. Everything
        // else must be told apart without reading the words.
        let distinct = [
            LanguageServerBadge::Idle,
            LanguageServerBadge::Ready,
            LanguageServerBadge::Starting,
            LanguageServerBadge::NeedsSetup,
            LanguageServerBadge::NotInstalled,
            LanguageServerBadge::Failed,
        ];
        let mut glyphs: Vec<char> = distinct
            .iter()
            .map(|state| glyph(*state, IconStyle::Unicode))
            .collect();
        glyphs.sort_unstable();
        let before = glyphs.len();
        glyphs.dedup();
        assert_eq!(glyphs.len(), before, "two conditions share a Unicode glyph");
    }

    #[test]
    fn a_single_healthy_provider_shows_no_count() {
        let badge = summary(LanguageServerBadge::Ready, 1, 1);
        assert_eq!(count_suffix(badge), None);
        assert_eq!(spelled(badge), "LSP ready");
        assert_eq!(compact(badge, IconStyle::Ascii), " = LSP ");
    }

    #[test]
    fn a_partial_failure_shows_the_count_on_both_surfaces() {
        let badge = summary(LanguageServerBadge::NotInstalled, 1, 2);
        assert_eq!(count_suffix(badge), Some("1/2".to_owned()));
        assert_eq!(spelled(badge), "LSP not installed 1/2");
        assert_eq!(compact(badge, IconStyle::Ascii), " + LSP 1/2 ");
    }

    #[test]
    fn two_healthy_providers_show_no_count() {
        assert_eq!(
            count_suffix(summary(LanguageServerBadge::Ready, 2, 2)),
            None
        );
    }

    #[test]
    fn needs_setup_warns_while_not_installed_errors() {
        assert_eq!(
            role(LanguageServerBadge::NeedsSetup),
            ThemeRole::DiagnosticWarning
        );
        assert_eq!(
            role(LanguageServerBadge::NotInstalled),
            ThemeRole::DiagnosticError
        );
    }

    #[test]
    fn every_label_names_lsp() {
        for state in EVERY_STATE {
            assert!(label(state).starts_with("LSP "), "{state:?}");
        }
    }
}
