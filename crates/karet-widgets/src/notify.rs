//! A toast-stack overlay: renders active [`Notification`]s as small cards stacked
//! in a corner, newest nearest the corner.
//!
//! The application owns notification lifetime (see its notification center); this
//! widget is a pure renderer. Because ratatui's `Widget::render` can't hand back the
//! per-card geometry the app needs for click hit-testing, [`Toasts::layout`] is a
//! pure function that both the renderer and the app's mouse handler call — it maps a
//! render area to one [`ToastSlot`] per shown notification.

use karet_core::Notification;
use karet_core::ThemeRole;
use karet_core::severity_role;
use karet_theme::Theme;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::widgets::Block;
use ratatui::widgets::Clear;
use ratatui::widgets::Widget;
use unicode_width::UnicodeWidthStr;

/// Which screen corner the stack grows from.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Corner {
    /// Anchor bottom-right; stack upward (VS Code style).
    BottomRight,
    /// Anchor top-right; stack downward.
    TopRight,
}

/// The rendered position of one toast, returned by [`Toasts::layout`] so the
/// application can hit-test clicks (dismiss on the `×` or anywhere on the card).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ToastSlot {
    /// The full card rectangle (including its border).
    pub rect: Rect,
    /// The `(column, row)` of the `×` close glyph.
    pub close: (u16, u16),
    /// The notification this card shows.
    pub id: karet_core::NotificationId,
}

/// The `×` glyph used to close a toast (matches the editor's tab-close glyph).
const CLOSE_GLYPH: &str = "\u{00d7}";
/// The leading severity bullet.
const BULLET: &str = "\u{25cf}";
const MAX_WIDTH: u16 = 44;
const MIN_WIDTH: u16 = 16;
const MARGIN_X: u16 = 1;
const MARGIN_Y: u16 = 1;
const GAP: u16 = 1;
/// The most cards drawn at once; extras are summarized by a `+N more` line.
const MAX_ACTIVE: usize = 5;

/// A toast-stack overlay over a slice of notifications (newest first).
pub struct Toasts<'a> {
    /// The notifications to show, newest first.
    pub notifications: &'a [&'a Notification],
    /// The active theme (for severity colors).
    pub theme: &'a Theme,
    /// Which corner the stack grows from.
    pub corner: Corner,
}

impl Toasts<'_> {
    /// The card height in rows for `note` at the given card `width` (title row +
    /// optional single body row + top/bottom border).
    fn card_height(note: &Notification, width: u16) -> u16 {
        let inner_w = width.saturating_sub(2);
        let has_body = note.body.as_ref().is_some_and(|b| !b.is_empty()) && inner_w > 0;
        3 + u16::from(has_body)
    }

    /// Compute the on-screen slot for each shown notification. Pure: no clock, no
    /// theme lookup. Cards tile without overlap inside `area`, capped at
    /// [`MAX_ACTIVE`] and at whatever fits vertically.
    #[must_use]
    pub fn layout(&self, area: Rect) -> Vec<ToastSlot> {
        let mut slots = Vec::new();
        if area.width < MIN_WIDTH || area.height < 3 {
            return slots;
        }
        let width = MAX_WIDTH
            .min(area.width.saturating_sub(MARGIN_X.saturating_mul(2)))
            .max(MIN_WIDTH);
        let x = area.right().saturating_sub(MARGIN_X).saturating_sub(width);

        match self.corner {
            Corner::BottomRight => {
                let mut bottom = area.bottom().saturating_sub(MARGIN_Y);
                for note in self.notifications.iter().take(MAX_ACTIVE) {
                    let h = Self::card_height(note, width);
                    if bottom < area.y.saturating_add(h) {
                        break;
                    }
                    let top = bottom - h;
                    slots.push(Self::slot(note, x, top, width));
                    if top <= area.y.saturating_add(GAP) {
                        break;
                    }
                    bottom = top - GAP;
                }
            },
            Corner::TopRight => {
                let mut top = area.y.saturating_add(MARGIN_Y);
                for note in self.notifications.iter().take(MAX_ACTIVE) {
                    let h = Self::card_height(note, width);
                    if top.saturating_add(h) > area.bottom() {
                        break;
                    }
                    slots.push(Self::slot(note, x, top, width));
                    top = top.saturating_add(h).saturating_add(GAP);
                }
            },
        }
        slots
    }

    fn slot(note: &Notification, x: u16, top: u16, width: u16) -> ToastSlot {
        let h = Self::card_height(note, width);
        let rect = Rect {
            x,
            y: top,
            width,
            height: h,
        };
        // The `×` sits on the title row (inner top), in the last inner column.
        let close = (rect.x + width.saturating_sub(2), top + 1);
        ToastSlot {
            rect,
            close,
            id: note.id,
        }
    }
}

/// Truncate `s` to `max` display columns, appending `…` when it overflows.
fn fit(s: &str, max: usize) -> String {
    if s.width() <= max {
        return s.to_string();
    }
    if max == 0 {
        return String::new();
    }
    let budget = max.saturating_sub(1);
    let mut out = String::new();
    let mut used = 0usize;
    for ch in s.chars() {
        let w = UnicodeWidthStr::width(ch.to_string().as_str());
        if used + w > budget {
            break;
        }
        out.push(ch);
        used += w;
    }
    out.push('\u{2026}');
    out
}

impl Widget for Toasts<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let slots = self.layout(area);
        let bg = self.theme.role(ThemeRole::StatusBarBackground).to_ratatui();
        for (note, slot) in self.notifications.iter().zip(slots.iter()) {
            let color = self.theme.role(severity_role(note.severity)).to_ratatui();
            let rect = slot.rect;
            Clear.render(rect, buf);
            let block = Block::bordered()
                .border_style(Style::default().fg(color))
                .style(Style::default().bg(bg));
            let inner = block.inner(rect);
            block.render(rect, buf);
            if inner.width == 0 || inner.height == 0 {
                continue;
            }
            let inner_w = inner.width as usize;

            // Title row: a severity bullet, the title, and a right-aligned `×`.
            let title = fit(&note.title, inner_w.saturating_sub(4));
            let title_w = title.width() + BULLET.width() + 1;
            let pad = inner_w.saturating_sub(title_w + CLOSE_GLYPH.width()).max(1);
            let title_line = Line::from(vec![
                Span::styled(format!("{BULLET} "), Style::default().fg(color)),
                Span::styled(
                    title,
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                ),
                Span::raw(" ".repeat(pad)),
                Span::styled(CLOSE_GLYPH.to_string(), Style::default().fg(color)),
            ]);
            buf.set_line(inner.x, inner.y, &title_line, inner.width);

            // Optional single-line body.
            if inner.height > 1
                && let Some(body) = note.body.as_ref().filter(|b| !b.is_empty())
            {
                let dim = self.theme.role(ThemeRole::LineNumber).to_ratatui();
                let body_line = Line::styled(fit(body, inner_w), Style::default().fg(dim));
                buf.set_line(inner.x, inner.y + 1, &body_line, inner.width);
            }
        }

        // A `+N more` hint above the topmost card when the stack overflowed.
        let hidden = self.notifications.len().saturating_sub(slots.len());
        if hidden > 0
            && let Some(top) = slots.iter().map(|s| s.rect.y).min()
            && top > area.y
        {
            let dim = self.theme.role(ThemeRole::LineNumber).to_ratatui();
            let x = slots.first().map_or(area.x, |s| s.rect.x);
            let more = format!("+{hidden} more");
            buf.set_line(
                x,
                top - 1,
                &Line::styled(more, Style::default().fg(dim)),
                area.width,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use karet_core::NotificationId;
    use karet_core::NotificationKind;
    use karet_core::Severity;

    use super::*;

    fn note(id: u64, title: &str, body: Option<&str>) -> Notification {
        Notification {
            id: NotificationId(id),
            severity: Severity::Error,
            kind: NotificationKind::Io,
            title: title.to_string(),
            body: body.map(str::to_string),
            tag: None,
            timeout: None,
            dismissable: true,
        }
    }

    fn area() -> Rect {
        Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        }
    }

    #[test]
    fn layout_places_non_overlapping_cards_inside_area() {
        let a = area();
        let theme = Theme::dark();
        let notes = [note(1, "one", None), note(2, "two", Some("body"))];
        let refs: Vec<&Notification> = notes.iter().collect();
        let toasts = Toasts {
            notifications: &refs,
            theme: &theme,
            corner: Corner::BottomRight,
        };
        let slots = toasts.layout(a);
        assert_eq!(slots.len(), 2);
        for s in &slots {
            assert!(s.rect.right() <= a.right());
            assert!(s.rect.bottom() <= a.bottom());
            assert!(s.rect.x >= a.x);
        }
        // Newest (index 0) sits below the second card, and they do not overlap.
        assert!(slots[0].rect.y >= slots[1].rect.bottom());
    }

    #[test]
    fn close_glyph_is_on_the_title_row_at_the_right_edge() {
        let a = area();
        let theme = Theme::dark();
        let notes = [note(1, "hello", None)];
        let refs: Vec<&Notification> = notes.iter().collect();
        let slots = Toasts {
            notifications: &refs,
            theme: &theme,
            corner: Corner::BottomRight,
        }
        .layout(a);
        let s = slots[0];
        assert_eq!(s.close.1, s.rect.y + 1); // title row (inner top)
        assert_eq!(s.close.0, s.rect.x + s.rect.width - 2); // last inner column
    }

    #[test]
    fn caps_at_max_active() {
        let a = area();
        let theme = Theme::dark();
        let notes: Vec<Notification> = (0..8).map(|i| note(i, "n", None)).collect();
        let refs: Vec<&Notification> = notes.iter().collect();
        let slots = Toasts {
            notifications: &refs,
            theme: &theme,
            corner: Corner::BottomRight,
        }
        .layout(a);
        assert!(slots.len() <= MAX_ACTIVE);
    }

    #[test]
    fn tiny_area_yields_no_slots() {
        let theme = Theme::dark();
        let notes = [note(1, "x", None)];
        let refs: Vec<&Notification> = notes.iter().collect();
        let tiny = Rect {
            x: 0,
            y: 0,
            width: 4,
            height: 2,
        };
        let slots = Toasts {
            notifications: &refs,
            theme: &theme,
            corner: Corner::BottomRight,
        }
        .layout(tiny);
        assert!(slots.is_empty());
    }

    #[test]
    fn fit_truncates_with_ellipsis() {
        assert_eq!(fit("hello", 10), "hello");
        assert_eq!(fit("hello world", 6), "hello\u{2026}");
        assert_eq!(fit("hello", 0), "");
    }
}
