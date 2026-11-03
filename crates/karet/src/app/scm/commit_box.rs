//! How tall the commit box is, and how it scrolls.
//!
//! The box grows downward with its draft and then scrolls, so a one-line subject
//! costs the changes list nothing and a long message is still editable without
//! leaving the panel.
//!
//! # Why the height is computed from the *maximum* rect
//!
//! Reserving a scrollbar column narrows the text by one cell, which changes where
//! the draft wraps, which changes how many rows it needs, which would change the
//! height — and a reservation that depends on the height feeds back on itself.
//! `karet-widgets` states the invariant: a reservation may depend on the area and
//! the axes, never on the content.
//!
//! So the track is reserved once, from the tallest box that could be drawn, and
//! the wrap width that falls out is a pure function of the panel's width. The row
//! count is computed after that and never feeds back. [`MIN_COMMIT_ROWS`] is what
//! closes the loop: `reserve_tracks` reserves a vertical track only above two
//! rows, so a box that never shrinks below three always reserves the same way as
//! the maximum rect it was measured against.

use karet_widgets::scroll::ScrollAxes;
use karet_widgets::scroll::reserve_tracks;
use karet_widgets::textarea::wrap_rows;
use ratatui::layout::Rect;

use super::super::App;
use super::super::MIN_SCM_REGION;

/// Fewest rows of text the box ever shows — the size it had before it could grow,
/// so an empty box looks exactly as it did. Also load-bearing: see the
/// [module docs](self).
pub(crate) const MIN_COMMIT_ROWS: u16 = 3;

/// Most rows of text the box grows to before it starts scrolling. Generous enough
/// for a subject, a blank line and a paragraph of body, and bounded so the changes
/// list is never squeezed out of the panel.
pub(crate) const MAX_COMMIT_ROWS: u16 = 12;

/// The outer height of the commit box drawing `text` at the top of `area`.
///
/// `area` is the whole region the box shares with the changes list; the returned
/// height includes the box's two border rows.
#[must_use]
pub(crate) fn commit_box_height(area: Rect, text: &str) -> u16 {
    let max_rows = area
        .height
        .saturating_sub(MIN_SCM_REGION + 2)
        .clamp(MIN_COMMIT_ROWS, MAX_COMMIT_ROWS);
    let width = wrap_width(area, max_rows);
    let rows = u16::try_from(wrap_rows(text, width).len()).unwrap_or(u16::MAX);
    rows.clamp(MIN_COMMIT_ROWS, max_rows)
        .saturating_add(2)
        .min(area.height)
}

/// The text width inside a box `rows` tall drawn at the top of `area`, after its
/// scrollbar track is reserved.
fn wrap_width(area: Rect, rows: u16) -> u16 {
    let inner = Rect {
        x: area.x.saturating_add(1),
        y: area.y.saturating_add(1),
        width: area.width.saturating_sub(2),
        height: rows,
    };
    reserve_tracks(inner, ScrollAxes::VERTICAL).0.width
}

impl App {
    /// Move the commit box's viewport by `delta` rows, releasing it from the caret.
    ///
    /// Following the caret is what a typist wants and what a reader scrolling back
    /// through a long message does not, so a deliberate scroll wins until the
    /// caret next moves.
    pub(crate) fn scm_scroll_commit_input(&mut self, delta: i32) {
        let position = i64::from(self.commit_input.edit.scroll).saturating_add(i64::from(delta));
        self.scroll_commit_input_to(usize::try_from(position.max(0)).unwrap_or(usize::MAX));
    }

    /// Land the commit box's viewport on an absolute row.
    pub(crate) fn scroll_commit_input_to(&mut self, position: usize) {
        let rect = self.scm_ui.commit_rect;
        self.commit_input.edit.scroll = u16::try_from(position).unwrap_or(u16::MAX);
        self.commit_input
            .edit
            .clamp_scroll(&self.commit_input.text, rect.width, rect.height);
        self.commit_input.scrolled_away = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(width: u16, height: u16) -> Rect {
        Rect::new(0, 0, width, height)
    }

    #[test]
    fn an_empty_box_keeps_the_height_it_always_had() {
        assert_eq!(commit_box_height(area(30, 40), ""), MIN_COMMIT_ROWS + 2);
        assert_eq!(
            commit_box_height(area(30, 40), "one line"),
            MIN_COMMIT_ROWS + 2
        );
    }

    #[test]
    fn the_box_grows_with_the_draft_and_then_stops() {
        let five = "a\nb\nc\nd\ne";
        assert_eq!(commit_box_height(area(30, 40), five), 5 + 2);
        let many = "x\n".repeat(usize::from(MAX_COMMIT_ROWS) * 2);
        assert_eq!(
            commit_box_height(area(30, 40), &many),
            MAX_COMMIT_ROWS + 2,
            "past the cap the box scrolls instead of growing"
        );
    }

    #[test]
    fn the_changes_list_always_keeps_its_floor() {
        let many = "x\n".repeat(60);
        for height in 0..60u16 {
            let box_height = commit_box_height(area(30, height), &many);
            assert!(
                box_height <= height,
                "the box overran its panel at {height}"
            );
            if height >= MIN_COMMIT_ROWS + 2 + MIN_SCM_REGION {
                assert!(
                    height - box_height >= MIN_SCM_REGION,
                    "only {} rows left for the changes list at {height}",
                    height - box_height
                );
            }
        }
    }

    #[test]
    fn the_height_is_stable_under_its_own_reservation() {
        // The feedback loop the module docs describe: feeding the box's own rect
        // back through the reservation must not change the wrap width, or the box
        // would oscillate by a column as the draft is typed.
        let drafts = ["", "short", &"word ".repeat(60), &"x".repeat(500), "a\n\nb"];
        for width in 1..60u16 {
            for draft in drafts {
                let height = commit_box_height(area(width, 40), draft);
                let rows = height.saturating_sub(2);
                assert_eq!(
                    wrap_width(area(width, 40), rows),
                    wrap_width(area(width, 40), MAX_COMMIT_ROWS),
                    "width {width} drifted between the measured and the drawn box"
                );
                assert_eq!(
                    commit_box_height(area(width, 40), draft),
                    height,
                    "width {width} is not idempotent"
                );
            }
        }
    }

    #[test]
    fn a_panel_too_small_for_a_box_gives_it_everything_there_is() {
        for height in 0..=4u16 {
            assert_eq!(commit_box_height(area(30, height), "x"), height);
        }
    }
}
