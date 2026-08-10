//! A positioned context menu, generic over the consumer's action type.
//!
//! The widget owns the menu model (rows, enabled/disabled state, selection
//! that skips disabled rows) and the painting (anchored placement, clamping,
//! dimmed disabled rows, right-aligned hints). Resolving what a row *says*
//! (labels, key hints) and what accepting it *does* stays with the consumer.

use karet_core::ThemeRole;
use karet_theme::Theme;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::widgets::Block;
use ratatui::widgets::Borders;
use ratatui::widgets::Clear;
use ratatui::widgets::List;
use ratatui::widgets::ListItem;
use ratatui::widgets::ListState;
use unicode_width::UnicodeWidthStr;

/// One row of a positioned context menu: its action, whether it can run right
/// now, and an optional note explaining why not.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContextMenuEntry<A> {
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

impl<A> ContextMenuEntry<A> {
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

/// A positioned context menu (opened from a tree row, a pane, a word, …).
pub struct ContextMenu<A> {
    /// The column where the menu should be anchored.
    pub x: u16,
    /// The row where the menu should be anchored.
    pub y: u16,
    /// The rows shown in the menu, in display order.
    pub entries: Vec<ContextMenuEntry<A>>,
    /// The selected row index.
    pub selected: usize,
    /// The menu rect from the last render (for hit-testing).
    pub rect: Rect,
}

impl<A> ContextMenu<A> {
    /// A menu anchored at `(x, y)`, with the initial selection on the first
    /// activatable row.
    #[must_use]
    pub fn new(x: u16, y: u16, entries: Vec<ContextMenuEntry<A>>) -> Self {
        let selected = entries.iter().position(|e| e.enabled).unwrap_or(0);
        Self {
            x,
            y,
            entries,
            selected,
            rect: Rect::default(),
        }
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
    pub fn selected_entry(&self) -> Option<&ContextMenuEntry<A>> {
        self.entries.get(self.selected)
    }

    /// Draw the menu into `area`, clamping it inside, and record its rect for
    /// hit-testing. `labels` and `hints` are the consumer-resolved row texts,
    /// index-aligned with [`entries`](Self::entries).
    pub fn draw(
        &mut self,
        f: &mut Frame,
        theme: &Theme,
        area: Rect,
        labels: &[String],
        hints: &[Option<String>],
    ) {
        if self.entries.is_empty() {
            self.rect = Rect::default();
            return;
        }
        let width_of = |s: &str| u16::try_from(UnicodeWidthStr::width(s)).unwrap_or(u16::MAX);
        let label_w = labels
            .iter()
            .map(|label| width_of(label))
            .max()
            .unwrap_or(0);
        let hint_w = hints
            .iter()
            .flatten()
            .map(|hint| width_of(hint))
            .max()
            .unwrap_or(0);
        let width = (label_w + hint_w + 6).clamp(18, 46).min(area.width.max(1));
        let height = (self.entries.len() as u16 + 2).min(area.height.max(1));
        let x = self.x.min(area.right().saturating_sub(width));
        let y = self.y.min(area.bottom().saturating_sub(height));
        let rect = Rect {
            x,
            y,
            width,
            height,
        };
        self.rect = rect;
        f.render_widget(Clear, rect);
        let style = Style::default()
            .bg(theme.role(ThemeRole::Background).to_ratatui())
            .fg(theme.role(ThemeRole::Foreground).to_ratatui());
        let block = Block::default()
            .borders(Borders::ALL)
            .style(style)
            .border_style(theme.style(ThemeRole::IndentGuide));
        let inner = block.inner(rect);
        f.render_widget(block, rect);
        let dim = theme.style(ThemeRole::LineNumber);
        let items: Vec<ListItem> = labels
            .iter()
            .zip(hints.iter())
            .zip(self.entries.iter())
            .map(|((label, hint), entry)| {
                // Disabled rows render fully dimmed (label and hint alike).
                let label_style = if entry.enabled { Style::default() } else { dim };
                match hint {
                    Some(hint) => {
                        let used = width_of(label) + width_of(hint);
                        let pad = inner.width.saturating_sub(used).max(1);
                        ListItem::new(Line::from(vec![
                            Span::styled(label.clone(), label_style),
                            Span::raw(" ".repeat(pad as usize)),
                            Span::styled(hint.clone(), dim),
                        ]))
                    },
                    None => ListItem::new(Line::from(Span::styled(label.clone(), label_style))),
                }
            })
            .collect();
        let mut state = ListState::default();
        state.select(Some(self.selected));
        let list = List::new(items).highlight_style(
            Style::default()
                .bg(theme.role(ThemeRole::Selection).to_ratatui())
                .add_modifier(Modifier::BOLD),
        );
        f.render_stateful_widget(list, inner, &mut state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn menu(enabled: &[bool]) -> ContextMenu<u8> {
        ContextMenu::new(
            0,
            0,
            enabled
                .iter()
                .enumerate()
                .map(|(index, on)| {
                    let entry = ContextMenuEntry::<u8>::enabled(index as u8);
                    ContextMenuEntry {
                        enabled: *on,
                        ..entry
                    }
                })
                .collect(),
        )
    }

    #[test]
    fn initial_selection_lands_on_the_first_enabled_row() {
        assert_eq!(menu(&[false, false, true, true]).selected, 2);
        assert_eq!(menu(&[true, false]).selected, 0);
    }

    #[test]
    fn selection_skips_disabled_rows_in_both_directions() {
        let mut m = menu(&[true, false, true, false, true]);
        m.select_by(1);
        assert_eq!(m.selected, 2);
        m.select_by(1);
        assert_eq!(m.selected, 4);
        m.select_by(-2);
        assert_eq!(m.selected, 0);
    }

    #[test]
    fn selection_stops_at_the_last_enabled_row() {
        let mut m = menu(&[true, true, false]);
        m.select_by(5);
        assert_eq!(m.selected, 1, "overshoot lands on the last enabled row");
        m.select_by(-5);
        assert_eq!(m.selected, 0);
    }
}
