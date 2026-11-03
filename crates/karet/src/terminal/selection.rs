//! Mouse selection over the terminal grid: which kind of selection a click
//! starts, and where a viewport cell sits in the scrolled grid.

use std::time::Instant;

use alacritty_terminal::index::Column;
use alacritty_terminal::index::Line;
use alacritty_terminal::index::Point;
use alacritty_terminal::index::Side;
use alacritty_terminal::selection::Selection;
use alacritty_terminal::selection::SelectionType;
use alacritty_terminal::term::Term;

use super::TerminalPane;
use crate::app::MULTI_CLICK_INTERVAL;

/// Counts consecutive presses on one cell, cycling single → double → triple.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct ClickTracker {
    last: Option<(Instant, u16, u16)>,
    count: u8,
}

impl ClickTracker {
    /// Register a press at viewport cell `(column, row)` at `now`, returning
    /// which press of a run it is (1, 2, or 3; a fourth starts over).
    pub(crate) fn press(&mut self, column: u16, row: u16, now: Instant) -> u8 {
        let repeat = self.last.is_some_and(|(at, c, r)| {
            c == column && r == row && now.saturating_duration_since(at) < MULTI_CLICK_INTERVAL
        });
        self.count = if repeat && self.count < 3 {
            self.count + 1
        } else {
            1
        };
        self.last = Some((now, column, row));
        self.count
    }
}

/// The selection kind a run of `clicks` starts: a character range, a word,
/// or whole lines.
#[must_use]
pub(crate) fn selection_type(clicks: u8) -> SelectionType {
    match clicks {
        2 => SelectionType::Semantic,
        3 => SelectionType::Lines,
        _ => SelectionType::Simple,
    }
}

/// The grid point under viewport cell `(column, row)` when the view is scrolled
/// `display_offset` lines into the history.
#[must_use]
pub(crate) fn grid_point(column: u16, row: u16, display_offset: usize) -> Point {
    let offset = i32::try_from(display_offset).unwrap_or(i32::MAX);
    Point::new(
        Line(i32::from(row).saturating_sub(offset)),
        Column(usize::from(column)),
    )
}

/// The clamped grid point under viewport cell `(column, row)` of `term`.
fn point_in<T>(term: &Term<T>, column: u16, row: u16) -> Point {
    use alacritty_terminal::grid::Dimensions;
    let columns = u16::try_from(term.columns()).unwrap_or(u16::MAX);
    let rows = u16::try_from(term.screen_lines()).unwrap_or(u16::MAX);
    grid_point(
        column.min(columns.saturating_sub(1)),
        row.min(rows.saturating_sub(1)),
        term.grid().display_offset(),
    )
}

impl TerminalPane {
    /// Start a selection of `ty` at viewport cell `(column, row)`.
    pub(crate) fn start_selection(&mut self, ty: SelectionType, column: u16, row: u16) {
        {
            let mut term = self.term();
            let point = point_in(&term, column, row);
            term.selection = Some(Selection::new(ty, point, Side::Left));
        }
        self.selecting = true;
    }

    /// Extend the selection being dragged to viewport cell `(column, row)`.
    pub(crate) fn extend_selection(&self, column: u16, row: u16) {
        let mut term = self.term();
        let point = point_in(&term, column, row);
        if let Some(selection) = term.selection.as_mut() {
            selection.update(point, Side::Right);
        }
    }

    /// Finish a drag; a simple selection that covers nothing is dropped.
    pub(crate) fn finish_selection(&mut self) {
        self.selecting = false;
        let mut term = self.term();
        if term
            .selection
            .as_ref()
            .is_some_and(|selection| selection.ty == SelectionType::Simple && selection.is_empty())
        {
            term.selection = None;
        }
    }

    /// The OSC 8 hyperlink under viewport cell `(column, row)`, if any.
    pub(crate) fn hyperlink_at(&self, column: u16, row: u16) -> Option<String> {
        let term = self.term();
        let point = point_in(&term, column, row);
        term.grid()[point]
            .hyperlink()
            .map(|link| link.uri().to_owned())
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn presses_on_one_cell_cycle_through_three_kinds() {
        let mut clicks = ClickTracker::default();
        let t0 = Instant::now();
        let step = Duration::from_millis(100);
        assert_eq!(clicks.press(3, 2, t0), 1);
        assert_eq!(clicks.press(3, 2, t0 + step), 2);
        assert_eq!(clicks.press(3, 2, t0 + step * 2), 3);
        assert_eq!(clicks.press(3, 2, t0 + step * 3), 1, "a fourth starts over");
    }

    #[test]
    fn a_slow_or_moved_press_starts_a_new_run() {
        let mut clicks = ClickTracker::default();
        let t0 = Instant::now();
        assert_eq!(clicks.press(0, 0, t0), 1);
        assert_eq!(clicks.press(1, 0, t0 + Duration::from_millis(50)), 1);
        assert_eq!(clicks.press(1, 0, t0 + Duration::from_secs(2)), 1);
    }

    #[test]
    fn a_press_exactly_one_interval_later_starts_a_new_run() {
        let mut clicks = ClickTracker::default();
        let t0 = Instant::now();
        let just_inside = MULTI_CLICK_INTERVAL - Duration::from_millis(1);
        assert_eq!(clicks.press(4, 1, t0), 1);
        assert_eq!(clicks.press(4, 1, t0 + just_inside), 2);
        let t1 = t0 + just_inside;
        assert_eq!(
            clicks.press(4, 1, t1 + MULTI_CLICK_INTERVAL),
            1,
            "the interval is exclusive, as in the editor"
        );
    }

    #[test]
    fn click_runs_select_characters_words_then_lines() {
        assert_eq!(selection_type(1), SelectionType::Simple);
        assert_eq!(selection_type(2), SelectionType::Semantic);
        assert_eq!(selection_type(3), SelectionType::Lines);
    }

    #[test]
    fn a_scrolled_view_maps_rows_into_the_history() {
        assert_eq!(grid_point(4, 2, 0), Point::new(Line(2), Column(4)));
        assert_eq!(
            grid_point(0, 0, 5),
            Point::new(Line(-5), Column(0)),
            "the top row of a view scrolled 5 lines up is 5 lines into the history"
        );
    }
}
