//! The row model shared by the [menu](crate::menu) and [dialog](crate::dialog)
//! widgets: a list of selectable choices, the navigation that skips the ones
//! that cannot run, and the painting of the rows themselves.
//!
//! The seam is the same in both widgets: this module owns the *model* (rows,
//! enabled/disabled state, selection, hover) and the *row painting* (dimmed
//! disabled rows, right-aligned hints, the selection accent). Resolving what a
//! row *says* (labels, key hints) and what accepting it *does* stays with the
//! consumer — which is why every paint call takes the resolved `labels` and
//! `hints` rather than reaching for them itself.
//!
//! Placement is *not* shared: a menu is anchored at a point and clamped into
//! view, a dialog is centered. Each widget keeps its own geometry and hit-tests
//! through [`row_at`], the one implementation of "which row is this point on".

use karet_core::ThemeRole;
use karet_theme::Theme;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::widgets::List;
use ratatui::widgets::ListItem;
use ratatui::widgets::ListState;

use crate::text::width as display_width;

/// One selectable row: its action, whether it can run right now, and an
/// optional note explaining why not.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Choice<A> {
    /// The action this row yields when accepted.
    pub action: A,
    /// An action-specific label. `None` lets the consumer resolve a default.
    pub label: Option<String>,
    /// Whether the row can be activated. A disabled row renders dimmed, is
    /// skipped by keyboard navigation, and should refuse Accept.
    pub enabled: bool,
    /// Why the row is disabled, surfaced when the user tries to activate it
    /// anyway (e.g. by clicking it).
    pub note: Option<String>,
}

impl<A> Choice<A> {
    /// An enabled entry yielding `action` (labeled by the consumer's default).
    pub fn enabled(action: impl Into<A>) -> Self {
        Self {
            action: action.into(),
            label: None,
            enabled: true,
            note: None,
        }
    }

    /// A disabled entry for `action`, greyed out with an explanatory `note`.
    pub fn disabled(action: impl Into<A>, note: impl Into<String>) -> Self {
        Self {
            action: action.into(),
            label: None,
            enabled: false,
            note: Some(note.into()),
        }
    }

    /// An enabled contextual action with a label supplied by its producer.
    pub fn custom(label: impl Into<String>, action: impl Into<A>) -> Self {
        Self {
            action: action.into(),
            label: Some(label.into()),
            enabled: true,
            note: None,
        }
    }

    /// A disabled contextual row carrying an explanatory note.
    pub fn disabled_custom(
        label: impl Into<String>,
        action: impl Into<A>,
        note: impl Into<String>,
    ) -> Self {
        Self {
            action: action.into(),
            label: Some(label.into()),
            enabled: false,
            note: Some(note.into()),
        }
    }
}

/// The rows of a menu or dialog, plus the cursor moving over them.
///
/// Navigation counts only activatable rows and never wraps: overshooting lands
/// on the last enabled row in that direction, so holding a key cannot teleport
/// the cursor to the opposite end of a permission prompt.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChoiceList<A> {
    /// The rows, in display order.
    pub entries: Vec<Choice<A>>,
    /// The selected row index.
    pub selected: usize,
    /// The activatable row under the pointer, painted with a secondary accent so
    /// the mouse gets the same live feedback the keyboard cursor has.
    pub hover: Option<usize>,
}

impl<A> ChoiceList<A> {
    /// A list over `entries`, with the initial selection on the first
    /// activatable row (or `0` when none is).
    #[must_use]
    pub fn new(entries: Vec<Choice<A>>) -> Self {
        let selected = entries.iter().position(|e| e.enabled).unwrap_or(0);
        Self {
            entries,
            selected,
            hover: None,
        }
    }

    /// The number of rows.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the list has no rows at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Move the selection by `delta` rows, skipping disabled entries. When
    /// fewer enabled rows exist in that direction, the selection lands on the
    /// last one found (or stays put).
    pub fn select_by(&mut self, delta: i32) {
        if self.entries.is_empty() || delta == 0 {
            return;
        }
        let step: i64 = if delta > 0 { 1 } else { -1 };
        let mut remaining = i64::from(delta).abs();
        let mut idx = self.selected as i64;
        let mut landed = self.selected as i64;
        loop {
            idx += step;
            if idx < 0 || idx >= self.entries.len() as i64 {
                break;
            }
            if self.entries[idx as usize].enabled {
                landed = idx;
                remaining -= 1;
                if remaining == 0 {
                    break;
                }
            }
        }
        self.selected = landed as usize;
    }

    /// The currently selected row, if any.
    #[must_use]
    pub fn selected_entry(&self) -> Option<&Choice<A>> {
        self.entries.get(self.selected)
    }

    /// Highlight `row` under the pointer. `None` — or a row that cannot be
    /// activated — clears the highlight: feedback promising a click that would
    /// be refused is worse than none.
    pub fn set_hover_row(&mut self, row: Option<usize>) {
        self.hover =
            row.filter(|&index| self.entries.get(index).is_some_and(|entry| entry.enabled));
    }

    /// The rows as list items `width` cells wide: disabled rows fully dimmed,
    /// hints right-aligned against the far edge, the hovered row accented.
    ///
    /// `labels` and `hints` are the consumer-resolved row texts, index-aligned
    /// with [`entries`](Self::entries); rows past either run are not painted.
    #[must_use]
    pub fn items(
        &self,
        theme: &Theme,
        width: u16,
        labels: &[String],
        hints: &[Option<String>],
    ) -> Vec<ListItem<'static>> {
        let dim = theme.style(ThemeRole::LineNumber);
        // The hovered row carries a secondary accent; the selected row keeps the
        // primary one, so a pointer resting elsewhere never hides the cursor.
        let hover = theme.role(ThemeRole::HoverHighlight).to_ratatui();
        let cell_width = |s: &str| u16::try_from(display_width(s)).unwrap_or(u16::MAX);
        labels
            .iter()
            .zip(hints.iter())
            .zip(self.entries.iter())
            .enumerate()
            .map(|(index, ((label, hint), entry))| {
                // Disabled rows render fully dimmed (label and hint alike).
                let label_style = if entry.enabled { Style::default() } else { dim };
                let item = match hint {
                    Some(hint) => {
                        let used = cell_width(label) + cell_width(hint);
                        let pad = width.saturating_sub(used).max(1);
                        ListItem::new(Line::from(vec![
                            Span::styled(label.clone(), label_style),
                            Span::raw(" ".repeat(pad as usize)),
                            Span::styled(hint.clone(), dim),
                        ]))
                    },
                    None => ListItem::new(Line::from(Span::styled(label.clone(), label_style))),
                };
                if self.hover == Some(index) && index != self.selected {
                    item.style(Style::default().bg(hover))
                } else {
                    item
                }
            })
            .collect()
    }

    /// Paint the rows into `rows`, one row per entry, with the selected row
    /// carrying the primary accent.
    pub fn render(
        &self,
        f: &mut Frame,
        theme: &Theme,
        rows: Rect,
        labels: &[String],
        hints: &[Option<String>],
    ) {
        let items = self.items(theme, rows.width, labels, hints);
        let mut state = ListState::default();
        state.select(Some(self.selected));
        let list = List::new(items).highlight_style(
            Style::default()
                .bg(theme.role(ThemeRole::Selection).to_ratatui())
                .add_modifier(Modifier::BOLD),
        );
        f.render_stateful_widget(list, rows, &mut state);
    }
}

/// The index of the row at terminal point `(x, y)`, given the `rows` rect a
/// `len`-row list was painted into (one row per entry, no borders).
///
/// Disabled rows answer too — refusing them is the consumer's business, and a
/// click on one still owes the user its note. Both the click and the hover path
/// resolve rows through here, so what lights up under the pointer is the row a
/// click addresses.
#[must_use]
pub fn row_at(rows: Rect, len: usize, x: u16, y: u16) -> Option<usize> {
    if rows.width == 0
        || rows.height == 0
        || x < rows.x
        || x >= rows.right()
        || y < rows.y
        || y >= rows.bottom()
    {
        return None;
    }
    let index = usize::from(y - rows.y);
    (index < len).then_some(index)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(enabled: &[bool]) -> ChoiceList<u8> {
        ChoiceList::new(
            enabled
                .iter()
                .enumerate()
                .map(|(index, on)| Choice {
                    enabled: *on,
                    ..Choice::<u8>::enabled(index as u8)
                })
                .collect(),
        )
    }

    #[test]
    fn a_new_list_selects_the_first_activatable_row() {
        assert_eq!(list(&[false, true, true]).selected, 1);
        assert_eq!(list(&[false, false]).selected, 0, "nothing to land on");
        assert!(ChoiceList::<u8>::new(Vec::new()).is_empty());
        assert_eq!(list(&[true, true]).len(), 2);
    }

    #[test]
    fn navigation_skips_disabled_rows_and_saturates() {
        let mut l = list(&[true, false, true]);
        l.select_by(9);
        assert_eq!(l.selected, 2, "overshoot lands on the last enabled row");
        l.select_by(-9);
        assert_eq!(l.selected, 0, "and never wraps around");
        l.select_by(0);
        assert_eq!(l.selected, 0, "a zero step is a no-op");
    }

    #[test]
    fn the_selected_entry_is_the_row_under_the_cursor() {
        let l = list(&[false, true]);
        assert_eq!(l.selected_entry().map(|e| e.action), Some(1));
        assert!(ChoiceList::<u8>::new(Vec::new()).selected_entry().is_none());
    }

    #[test]
    fn hover_only_lands_on_activatable_rows() {
        let mut l = list(&[true, false]);
        l.set_hover_row(Some(0));
        assert_eq!(l.hover, Some(0));
        l.set_hover_row(Some(1));
        assert_eq!(l.hover, None, "a disabled row clears the highlight");
        l.set_hover_row(Some(0));
        l.set_hover_row(Some(7));
        assert_eq!(l.hover, None, "and so does a row that does not exist");
    }

    #[test]
    fn a_point_resolves_to_the_row_it_rests_on() {
        let rows = Rect::new(2, 3, 10, 3);
        assert_eq!(row_at(rows, 3, 2, 3), Some(0));
        assert_eq!(row_at(rows, 3, 11, 5), Some(2));
        assert_eq!(row_at(rows, 3, 2, 2), None, "above the rows");
        assert_eq!(row_at(rows, 3, 2, 6), None, "below the rows");
        assert_eq!(row_at(rows, 3, 1, 3), None, "left of the rows");
        assert_eq!(row_at(rows, 3, 12, 3), None, "right of the rows");
        assert_eq!(row_at(rows, 2, 2, 5), None, "past the last entry");
        assert_eq!(
            row_at(Rect::default(), 3, 0, 0),
            None,
            "nothing painted yet"
        );
    }

    #[test]
    fn the_constructors_carry_labels_notes_and_enabled_state() {
        let enabled = Choice::<u8>::enabled(1);
        assert!(enabled.enabled && enabled.label.is_none() && enabled.note.is_none());
        let disabled = Choice::<u8>::disabled(2, "no repo");
        assert!(!disabled.enabled);
        assert_eq!(disabled.note.as_deref(), Some("no repo"));
        let custom = Choice::<u8>::custom("Allow", 3);
        assert_eq!(custom.label.as_deref(), Some("Allow"));
        assert!(custom.enabled);
        let both = Choice::<u8>::disabled_custom("Allow", 4, "read-only");
        assert_eq!(both.label.as_deref(), Some("Allow"));
        assert_eq!(both.note.as_deref(), Some("read-only"));
        assert!(!both.enabled);
    }

    #[test]
    fn a_hint_is_padded_out_to_the_right_edge() {
        let theme = Theme::dark();
        let l = list(&[true]);
        let labels = vec!["Allow".to_owned()];
        let hints = vec![Some("y".to_owned())];
        let items = l.items(&theme, 20, &labels, &hints);
        assert_eq!(items.len(), 1);
        // "Allow" (5) + pad (14) + "y" (1) fills the 20-cell row exactly.
        let width: usize = items.first().map(|item| item.width()).unwrap_or_default();
        assert_eq!(width, 20);
    }
}
