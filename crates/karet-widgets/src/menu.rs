//! A positioned context menu, generic over the consumer's action type.
//!
//! The widget owns the menu model (rows, enabled/disabled state, selection
//! that skips disabled rows) and the painting (anchored placement, clamping,
//! dimmed disabled rows, right-aligned hints). Resolving what a row *says*
//! (labels, key hints) and what accepting it *does* stays with the consumer.
//!
//! The rows themselves are the shared [`ChoiceList`](crate::choice::ChoiceList)
//! model, which the [dialog](crate::dialog) widget uses too; this module adds
//! only what makes a menu a menu — anchoring at a point and clamping into view.

use std::ops::Deref;
use std::ops::DerefMut;

use karet_core::ThemeRole;
use karet_theme::Theme;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::Block;
use ratatui::widgets::Borders;
use ratatui::widgets::Clear;
use unicode_width::UnicodeWidthStr;

use crate::choice::Choice;
use crate::choice::ChoiceList;

/// One row of a positioned context menu: its action, whether it can run right
/// now, and an optional note explaining why not.
///
/// The menu's rows are the shared [`Choice`] model; the name is kept for the
/// consumers that speak in menus.
pub type ContextMenuEntry<A> = Choice<A>;

/// A positioned context menu (opened from a tree row, a pane, a word, …).
///
/// Dereferences to its [`ChoiceList`], so the row model — `entries`,
/// `selected`, `hover`, [`select_by`](ChoiceList::select_by),
/// [`selected_entry`](ChoiceList::selected_entry) — is reached directly on the
/// menu.
pub struct ContextMenu<A> {
    /// The column where the menu should be anchored.
    pub x: u16,
    /// The row where the menu should be anchored.
    pub y: u16,
    /// The menu rect from the last render (for hit-testing).
    pub rect: Rect,
    /// The rows and the cursor over them.
    rows: ChoiceList<A>,
}

impl<A> Deref for ContextMenu<A> {
    type Target = ChoiceList<A>;

    fn deref(&self) -> &Self::Target {
        &self.rows
    }
}

impl<A> DerefMut for ContextMenu<A> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.rows
    }
}

impl<A> ContextMenu<A> {
    /// A menu anchored at `(x, y)`, with the initial selection on the first
    /// activatable row.
    #[must_use]
    pub fn new(x: u16, y: u16, entries: Vec<ContextMenuEntry<A>>) -> Self {
        Self {
            x,
            y,
            rect: Rect::default(),
            rows: ChoiceList::new(entries),
        }
    }

    /// The rect the rows occupy: the borders bracket them, so the first row
    /// sits one line below the menu's top edge.
    fn rows_rect(&self) -> Rect {
        Rect {
            x: self.rect.x,
            y: self.rect.y.saturating_add(1),
            width: self.rect.width,
            height: self.rect.height.saturating_sub(2),
        }
    }

    /// The row at terminal point `(x, y)`, using the rect recorded by the last
    /// [`draw`](Self::draw). Disabled rows are returned too — refusing them is
    /// the consumer's business, and a click on one still owes the user its note.
    ///
    /// Both the click and the hover path resolve rows through here, so what
    /// lights up under the pointer is the row a click addresses.
    #[must_use]
    pub fn row_at(&self, x: u16, y: u16) -> Option<usize> {
        crate::choice::row_at(self.rows_rect(), self.rows.len(), x, y)
    }

    /// Move the selection by `delta` rows, skipping disabled entries. When
    /// fewer enabled rows exist in that direction, the selection lands on the
    /// last one found (or stays put).
    pub fn select_by(&mut self, delta: i32) {
        self.rows.select_by(delta);
    }

    /// The currently selected row, if any.
    #[must_use]
    pub fn selected_entry(&self) -> Option<&ContextMenuEntry<A>> {
        self.rows.selected_entry()
    }

    /// Track the pointer, highlighting the row it rests on. `None` (or a point
    /// outside the menu, or a row that cannot be activated) clears the
    /// highlight — feedback promising a click that would be refused is worse
    /// than none.
    pub fn set_hover(&mut self, point: Option<(u16, u16)>) {
        let row = point.and_then(|(x, y)| self.row_at(x, y));
        self.rows.set_hover_row(row);
    }

    /// Draw the menu into `area`, clamping it inside, and record its rect for
    /// hit-testing. `labels` and `hints` are the consumer-resolved row texts,
    /// index-aligned with [`entries`](ChoiceList::entries).
    pub fn draw(
        &mut self,
        f: &mut Frame,
        theme: &Theme,
        area: Rect,
        labels: &[String],
        hints: &[Option<String>],
    ) {
        if self.rows.is_empty() {
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
        let height = (self.rows.len() as u16 + 2).min(area.height.max(1));
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
        self.rows.render(f, theme, inner, labels, hints);
    }
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;

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

    /// Draw `m` into a test terminal, returning the painted buffer.
    fn draw(m: &mut ContextMenu<u8>) -> Option<Buffer> {
        let theme = Theme::dark();
        let labels: Vec<String> = (0..m.entries.len()).map(|i| format!("row {i}")).collect();
        let hints = vec![None; m.entries.len()];
        let mut terminal = Terminal::new(TestBackend::new(20, 8)).ok()?;
        terminal
            .draw(|f| m.draw(f, &theme, f.area(), &labels, &hints))
            .ok()?;
        Some(terminal.backend().buffer().clone())
    }

    #[test]
    fn the_pointer_resolves_to_the_row_it_rests_on() {
        let mut m = menu(&[true, false, true]);
        m.rect = Rect::new(2, 1, 12, 5); // border + three rows + border

        assert_eq!(
            m.row_at(4, 2),
            Some(0),
            "the first row sits under the border"
        );
        assert_eq!(
            m.row_at(4, 3),
            Some(1),
            "a disabled row still answers, so a click can explain itself"
        );
        assert_eq!(m.row_at(4, 4), Some(2));
        assert_eq!(m.row_at(4, 1), None, "the top border is not a row");
        assert_eq!(m.row_at(4, 9), None, "below the menu");
        assert_eq!(m.row_at(20, 2), None, "right of the menu");

        m.set_hover(Some((4, 4)));
        assert_eq!(m.hover, Some(2));
        m.set_hover(Some((4, 3)));
        assert_eq!(m.hover, None, "a disabled row clears the highlight");
        m.set_hover(Some((4, 4)));
        m.set_hover(None);
        assert_eq!(m.hover, None);
    }

    #[test]
    fn the_hovered_row_paints_the_hover_accent() {
        let theme = Theme::dark();
        let hover = theme.role(ThemeRole::HoverHighlight).to_ratatui();
        let selection = theme.role(ThemeRole::Selection).to_ratatui();
        let mut m = menu(&[true, true, true]);
        let Some(plain) = draw(&mut m) else {
            return;
        };
        // Row 0 is selected by default, so hovering row 1 keeps the two apart.
        assert_eq!(plain[(2u16, 1u16)].bg, selection);
        assert_ne!(plain[(2u16, 2u16)].bg, hover, "nothing is hovered yet");

        m.set_hover(Some((3, 2)));
        let Some(hovered) = draw(&mut m) else {
            return;
        };
        assert_eq!(hovered[(2u16, 2u16)].bg, hover);
        assert_eq!(
            hovered[(2u16, 1u16)].bg,
            selection,
            "the keyboard cursor keeps the primary accent"
        );
    }

    #[test]
    fn a_disabled_row_never_paints_the_hover_accent() {
        let theme = Theme::dark();
        let hover = theme.role(ThemeRole::HoverHighlight).to_ratatui();
        let mut m = menu(&[true, false]);
        if draw(&mut m).is_none() {
            return; // no rect to hit-test against
        }
        m.set_hover(Some((3, 2)));
        let Some(buffer) = draw(&mut m) else {
            return;
        };

        assert_eq!(m.hover, None);
        assert_ne!(buffer[(2u16, 2u16)].bg, hover);
    }
}
