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
const MAX_WIDTH: u16 = 72;
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
    fn card_width(note: &Notification, area: Rect) -> u16 {
        let available = area.width.saturating_sub(MARGIN_X.saturating_mul(2));
        let title = note
            .title
            .lines()
            .map(UnicodeWidthStr::width)
            .max()
            .unwrap_or_default()
            .saturating_add(6);
        let body = note
            .body
            .as_deref()
            .unwrap_or_default()
            .lines()
            .map(UnicodeWidthStr::width)
            .max()
            .unwrap_or_default()
            .saturating_add(2);
        u16::try_from(title.max(body))
            .unwrap_or(u16::MAX)
            .clamp(MIN_WIDTH, MAX_WIDTH.min(available))
    }

    /// The card height in rows for `note` at the given card `width`.
    fn card_height(note: &Notification, width: u16) -> u16 {
        let inner_w = width.saturating_sub(2);
        let title_width = usize::from(inner_w.saturating_sub(4)).max(1);
        let title_lines = wrap_display(&note.title, title_width).len().max(1);
        let body_lines = note
            .body
            .as_deref()
            .filter(|body| !body.is_empty())
            .map_or(0, |body| {
                wrap_display(body, usize::from(inner_w).max(1)).len()
            });
        2_u16.saturating_add(
            u16::try_from(title_lines.saturating_add(body_lines)).unwrap_or(u16::MAX),
        )
    }

    /// Compute the on-screen slot for each shown notification. Pure: no clock, no
    /// theme lookup. Cards tile without overlap inside `area`, capped at
    /// [`MAX_ACTIVE`] and at whatever fits vertically.
    #[must_use]
    pub fn layout(&self, area: Rect) -> Vec<ToastSlot> {
        let mut slots = Vec::new();
        if area.width < MIN_WIDTH.saturating_add(MARGIN_X.saturating_mul(2)) || area.height < 3 {
            return slots;
        }
        match self.corner {
            Corner::BottomRight => {
                let mut bottom = area.bottom().saturating_sub(MARGIN_Y);
                for note in self.notifications.iter().take(MAX_ACTIVE) {
                    let width = Self::card_width(note, area);
                    let x = area.right().saturating_sub(MARGIN_X).saturating_sub(width);
                    let desired = Self::card_height(note, width);
                    let available = bottom.saturating_sub(area.y);
                    let h = if slots.is_empty() {
                        desired.min(available)
                    } else {
                        desired
                    };
                    if h < 3 {
                        break;
                    }
                    if bottom < area.y.saturating_add(h) {
                        break;
                    }
                    let top = bottom - h;
                    slots.push(Self::slot(note, x, top, width, h));
                    if top <= area.y.saturating_add(GAP) {
                        break;
                    }
                    bottom = top - GAP;
                }
            },
            Corner::TopRight => {
                let mut top = area.y.saturating_add(MARGIN_Y);
                for note in self.notifications.iter().take(MAX_ACTIVE) {
                    let width = Self::card_width(note, area);
                    let x = area.right().saturating_sub(MARGIN_X).saturating_sub(width);
                    let desired = Self::card_height(note, width);
                    let available = area.bottom().saturating_sub(top);
                    let h = if slots.is_empty() {
                        desired.min(available)
                    } else {
                        desired
                    };
                    if h < 3 {
                        break;
                    }
                    if top.saturating_add(h) > area.bottom() {
                        break;
                    }
                    slots.push(Self::slot(note, x, top, width, h));
                    top = top.saturating_add(h).saturating_add(GAP);
                }
            },
        }
        slots
    }

    fn slot(note: &Notification, x: u16, top: u16, width: u16, h: u16) -> ToastSlot {
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

fn wrap_display(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return Vec::new();
    }
    let mut lines = Vec::new();
    for source in text.lines() {
        if source.is_empty() {
            lines.push(String::new());
            continue;
        }
        let mut current = String::new();
        for word in source.split_whitespace() {
            let separator = usize::from(!current.is_empty());
            if current
                .width()
                .saturating_add(separator)
                .saturating_add(word.width())
                <= width
            {
                if separator == 1 {
                    current.push(' ');
                }
                current.push_str(word);
                continue;
            }
            if !current.is_empty() {
                lines.push(std::mem::take(&mut current));
            }
            for character in word.chars() {
                let character_width = UnicodeWidthStr::width(character.to_string().as_str());
                if !current.is_empty() && current.width().saturating_add(character_width) > width {
                    lines.push(std::mem::take(&mut current));
                }
                current.push(character);
            }
        }
        if !current.is_empty() {
            lines.push(current);
        }
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
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

fn with_ellipsis(s: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    let mut text = fit(s, max.saturating_sub(1));
    if text.ends_with('…') {
        return text;
    }
    while text.width() >= max {
        text.pop();
    }
    text.push('…');
    text
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
            let title_width = inner_w.saturating_sub(4).max(1);
            let title_lines = wrap_display(&note.title, title_width);
            let body_lines = note
                .body
                .as_deref()
                .filter(|body| !body.is_empty())
                .map_or_else(Vec::new, |body| wrap_display(body, inner_w));
            let total_lines = title_lines.len().saturating_add(body_lines.len());
            let visible_lines = usize::from(inner.height).min(total_lines);
            let truncated = visible_lines < total_lines;

            // Title row: a severity bullet, the title, and a right-aligned `×`.
            let mut title = title_lines.first().cloned().unwrap_or_default();
            if truncated && visible_lines == 1 {
                title = with_ellipsis(&title, title_width);
            }
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

            let mut row = 1_usize;
            for continuation in title_lines.iter().skip(1) {
                if row >= visible_lines {
                    break;
                }
                let final_visible = truncated && row + 1 == visible_lines;
                let text = if final_visible {
                    with_ellipsis(continuation, inner_w.saturating_sub(2))
                } else {
                    continuation.clone()
                };
                let line = Line::from(vec![
                    Span::raw("  "),
                    Span::styled(
                        text,
                        Style::default().fg(color).add_modifier(Modifier::BOLD),
                    ),
                ]);
                buf.set_line(
                    inner.x,
                    inner
                        .y
                        .saturating_add(u16::try_from(row).unwrap_or(u16::MAX)),
                    &line,
                    inner.width,
                );
                row += 1;
            }

            let dim = self.theme.role(ThemeRole::LineNumber).to_ratatui();
            for body in &body_lines {
                if row >= visible_lines {
                    break;
                }
                let final_visible = truncated && row + 1 == visible_lines;
                let text = if final_visible {
                    with_ellipsis(body, inner_w)
                } else {
                    body.clone()
                };
                buf.set_line(
                    inner.x,
                    inner
                        .y
                        .saturating_add(u16::try_from(row).unwrap_or(u16::MAX)),
                    &Line::styled(text, Style::default().fg(dim)),
                    inner.width,
                );
                row += 1;
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

    #[test]
    fn long_titles_expand_and_wrap_instead_of_being_cut_off() {
        let a = area();
        let theme = Theme::dark();
        let notes = [note(
            1,
            "language server installation failed because the downloaded archive was invalid",
            None,
        )];
        let refs: Vec<&Notification> = notes.iter().collect();
        let toasts = Toasts {
            notifications: &refs,
            theme: &theme,
            corner: Corner::BottomRight,
        };
        let slots = toasts.layout(a);
        assert_eq!(slots.len(), 1);
        assert!(slots[0].rect.width > MIN_WIDTH);
        assert!(slots[0].rect.width <= MAX_WIDTH);
        assert!(slots[0].rect.height > 3);

        let mut buffer = Buffer::empty(a);
        toasts.render(a, &mut buffer);
        let rendered = (slots[0].rect.y..slots[0].rect.bottom())
            .map(|row| {
                (slots[0].rect.x..slots[0].rect.right())
                    .map(|column| buffer[(column, row)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(rendered.contains("downloaded"), "toast:\n{rendered}");
        assert!(rendered.contains("archive"), "toast:\n{rendered}");
        assert!(rendered.contains("was invalid"), "toast:\n{rendered}");
    }

    #[test]
    fn oversized_toast_is_clamped_and_ellipsized() {
        let a = Rect::new(0, 0, 40, 6);
        let theme = Theme::dark();
        let notes = [note(1, &"long error message ".repeat(30), None)];
        let refs: Vec<&Notification> = notes.iter().collect();
        let toasts = Toasts {
            notifications: &refs,
            theme: &theme,
            corner: Corner::BottomRight,
        };
        let slots = toasts.layout(a);
        assert_eq!(slots.len(), 1);
        assert!(slots[0].rect.bottom() <= a.bottom());
        let mut buffer = Buffer::empty(a);
        toasts.render(a, &mut buffer);
        assert!(buffer.content.iter().any(|cell| cell.symbol() == "…"));
    }
}
