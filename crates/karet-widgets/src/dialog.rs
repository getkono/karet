//! A centered modal dialog: a title, a wrapping body, and a keyboard-navigable
//! list of choices with per-choice hints.
//!
//! This is what a confirmation needs when a status-bar string is not enough —
//! an agent asking permission to run a tool has to show *what* it wants to run
//! and offer several distinct answers (allow once / allow always / reject /
//! reject always), each reachable from the keyboard.
//!
//! The seam is the [menu](crate::menu)'s: the widget owns the model (the shared
//! [`ChoiceList`] rows, the selection that skips disabled ones) and the
//! painting (centering, wrapping, dimmed rows, right-aligned hints). Resolving
//! what a row *says* (labels, key hints) and what accepting it *does* stays
//! with the consumer, which is why [`draw`](Dialog::draw) takes the resolved
//! `labels` and `hints`.
//!
//! The body is plain text by default and always soft-wraps to the dialog's
//! inner width. With the `markdown` feature it may instead be
//! [`DialogBody::Markdown`], parsed and painted through the theme — the same
//! rendering the LSP hover popup uses.

use karet_core::ThemeRole;
use karet_theme::Theme;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::widgets::Block;
use ratatui::widgets::Borders;
use ratatui::widgets::Clear;
use ratatui::widgets::Padding;

use crate::choice::Choice;
use crate::choice::ChoiceList;
use crate::text;

/// The narrowest a dialog is drawn (unless the area itself is narrower).
const MIN_WIDTH: u16 = 24;
/// The widest a dialog is drawn: past this a body is easier to read wrapped.
const MAX_WIDTH: u16 = 72;
/// Blank cells kept either side of the content, so text never touches the border.
const H_PAD: u16 = 1;
/// The cells a border plus its padding costs on both sides together.
const CHROME: u16 = 2 + 2 * H_PAD;

/// A dialog's body text and how it should be rendered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DialogBody {
    /// Plain text, soft-wrapped at whitespace to the dialog's inner width.
    Plain(String),
    /// Markdown, parsed and painted through the theme (headings bold, fenced
    /// code highlighted) — available with the `markdown` feature.
    #[cfg(feature = "markdown")]
    Markdown(String),
}

impl DialogBody {
    /// The body's source text, whatever its rendering.
    #[must_use]
    pub fn source(&self) -> &str {
        match self {
            Self::Plain(text) => text,
            #[cfg(feature = "markdown")]
            Self::Markdown(text) => text,
        }
    }

    /// Whether there is nothing to paint (an empty body reserves no rows).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.source().is_empty()
    }
}

impl From<String> for DialogBody {
    fn from(text: String) -> Self {
        Self::Plain(text)
    }
}

impl From<&str> for DialogBody {
    fn from(text: &str) -> Self {
        Self::Plain(text.to_owned())
    }
}

/// A centered modal dialog over a title, a body, and a list of choices.
///
/// The consumer owns the instance between frames: [`draw`](Self::draw) records
/// the geometry that [`row_at`](Self::row_at) and [`set_hover`](Self::set_hover)
/// hit-test against.
pub struct Dialog<A> {
    /// The title, painted in the top border.
    pub title: String,
    /// The body, wrapped to the dialog's inner width.
    pub body: DialogBody,
    /// The answers, in display order, and the cursor over them.
    pub choices: ChoiceList<A>,
    /// The dialog rect from the last render (for hit-testing and dismissal).
    pub rect: Rect,
    /// The rect the choice rows occupied in the last render.
    pub rows: Rect,
}

impl<A> Dialog<A> {
    /// A dialog titled `title`, explaining itself with `body`, answered by
    /// `choices` — the selection starting on the first activatable one.
    #[must_use]
    pub fn new(
        title: impl Into<String>,
        body: impl Into<DialogBody>,
        choices: Vec<Choice<A>>,
    ) -> Self {
        Self {
            title: title.into(),
            body: body.into(),
            choices: ChoiceList::new(choices),
            rect: Rect::default(),
            rows: Rect::default(),
        }
    }

    /// Move the selection by `delta` choices, skipping the disabled ones and
    /// stopping at the ends rather than wrapping.
    pub fn select_by(&mut self, delta: i32) {
        self.choices.select_by(delta);
    }

    /// The currently selected choice, if any.
    #[must_use]
    pub fn selected_choice(&self) -> Option<&Choice<A>> {
        self.choices.selected_entry()
    }

    /// The choice row at terminal point `(x, y)`, using the rect recorded by
    /// the last [`draw`](Self::draw). Disabled rows answer too, so a click on
    /// one can still explain itself.
    #[must_use]
    pub fn row_at(&self, x: u16, y: u16) -> Option<usize> {
        crate::choice::row_at(self.rows, self.choices.len(), x, y)
    }

    /// Track the pointer, highlighting the choice it rests on. `None` — or a
    /// point off the rows, or a choice that cannot be activated — clears the
    /// highlight.
    pub fn set_hover(&mut self, point: Option<(u16, u16)>) {
        let row = point.and_then(|(x, y)| self.row_at(x, y));
        self.choices.set_hover_row(row);
    }

    /// The body as styled lines, soft-wrapped to `width` cells.
    #[must_use]
    pub fn body_lines(&self, theme: &Theme, width: u16) -> Vec<Line<'static>> {
        // Only a markdown body consults the theme; a plain one inherits the
        // block's style, so the binding is unused without that feature.
        let _ = theme;
        if width == 0 || self.body.is_empty() {
            return Vec::new();
        }
        match &self.body {
            DialogBody::Plain(source) => text::wrap(source, usize::from(width))
                .into_iter()
                .map(Line::from)
                .collect(),
            #[cfg(feature = "markdown")]
            DialogBody::Markdown(source) => {
                let doc = karet_markdown::parse(source).wrap(width);
                karet_markdown::view::to_ratatui(&doc, theme)
            },
        }
    }

    /// The width the dialog wants: wide enough for the title, the widest choice
    /// row and the body's longest paragraph, within `MIN_WIDTH..=MAX_WIDTH` and
    /// never wider than `area`.
    fn width_for(&self, area: Rect, labels: &[String], hints: &[Option<String>]) -> u16 {
        let cells = |s: &str| u16::try_from(text::width(s)).unwrap_or(u16::MAX);
        // The title sits in the top border: the corners, the space either side
        // of the caption, and a little breathing room before the corner.
        let title = cells(&self.title).saturating_add(CHROME).saturating_add(2);
        let rows = labels
            .iter()
            .zip(hints.iter())
            .map(|(label, hint)| {
                cells(label)
                    .saturating_add(hint.as_deref().map_or(0, cells))
                    .saturating_add(CHROME)
                    .saturating_add(2)
            })
            .max()
            .unwrap_or(0);
        let body = self
            .body
            .source()
            .lines()
            .map(cells)
            .max()
            .unwrap_or(0)
            .saturating_add(CHROME);
        let want = title.max(rows).max(body);
        want.clamp(MIN_WIDTH, MAX_WIDTH).min(area.width.max(1))
    }

    /// Draw the dialog centered in `area` and record its geometry.
    ///
    /// `labels` and `hints` are the consumer-resolved choice texts,
    /// index-aligned with [`choices`](Self::choices). When `area` is too small
    /// for everything, the choices win: they are anchored to the bottom of the
    /// box and the body is clipped above them.
    pub fn draw(
        &mut self,
        f: &mut Frame,
        theme: &Theme,
        area: Rect,
        labels: &[String],
        hints: &[Option<String>],
    ) {
        if area.width == 0 || area.height == 0 {
            self.rect = Rect::default();
            self.rows = Rect::default();
            return;
        }
        let width = self.width_for(area, labels, hints);
        let body = self.body_lines(theme, width.saturating_sub(CHROME));
        let body_h = u16::try_from(body.len()).unwrap_or(u16::MAX);
        let rows_h = u16::try_from(self.choices.len()).unwrap_or(u16::MAX);
        // Borders, the body, a blank line between body and choices, the rows.
        let gap = u16::from(body_h > 0 && rows_h > 0);
        let height = body_h
            .saturating_add(rows_h)
            .saturating_add(gap)
            .saturating_add(2)
            .min(area.height.max(1));
        let rect = centered(area, width, height);
        self.rect = rect;
        f.render_widget(Clear, rect);
        let style = theme
            .style(ThemeRole::Foreground)
            .bg(theme.role(ThemeRole::Background).to_ratatui());
        let block = Block::default()
            .borders(Borders::ALL)
            .style(style)
            .border_style(theme.style(ThemeRole::IndentGuide))
            .padding(Padding::horizontal(H_PAD))
            .title(Span::styled(
                format!(" {} ", self.title),
                theme.style(ThemeRole::Foreground),
            ));
        let inner = block.inner(rect);
        f.render_widget(block, rect);
        if inner.width == 0 || inner.height == 0 {
            self.rows = Rect::default();
            return;
        }
        // The choices are the actionable part: they keep their rows when the
        // box is squeezed, and the body gives up lines from the bottom.
        let rows_h = rows_h.min(inner.height);
        let rows = Rect {
            x: inner.x,
            y: inner.bottom().saturating_sub(rows_h),
            width: inner.width,
            height: rows_h,
        };
        self.rows = rows;
        let body_h = body_h.min(inner.height.saturating_sub(rows_h));
        let buf = f.buffer_mut();
        for (row, line) in body.iter().take(usize::from(body_h)).enumerate() {
            let y = inner
                .y
                .saturating_add(u16::try_from(row).unwrap_or(u16::MAX));
            buf.set_line(inner.x, y, line, inner.width);
        }
        if rows_h > 0 {
            self.choices.render(f, theme, rows, labels, hints);
        }
    }
}

/// A `width`×`height` rect centered within `area`, clamped to it.
///
/// An `area` smaller than the requested size yields the whole area rather than
/// a rect hanging off its edge.
#[must_use]
pub fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect {
        x: area.x.saturating_add(area.width.saturating_sub(width) / 2),
        y: area
            .y
            .saturating_add(area.height.saturating_sub(height) / 2),
        width,
        height,
    }
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::style::Modifier;

    use super::*;

    /// A three-choice permission prompt: "allow always" is refused here.
    fn dialog(body: &str) -> Dialog<u8> {
        Dialog::new(
            "Permission",
            body,
            vec![
                Choice::custom("Allow once", 0u8),
                Choice::disabled_custom("Allow always", 1u8, "read-only session"),
                Choice::custom("Reject", 2u8),
            ],
        )
    }

    fn labels(d: &Dialog<u8>) -> Vec<String> {
        d.choices
            .entries
            .iter()
            .map(|entry| entry.label.clone().unwrap_or_default())
            .collect()
    }

    /// A test terminal, or `None` rather than an unwrap in a test.
    fn terminal(width: u16, height: u16) -> Option<Terminal<TestBackend>> {
        Terminal::new(TestBackend::new(width, height)).ok()
    }

    /// Draw `d` into a `width`×`height` test terminal, returning the buffer.
    fn draw(d: &mut Dialog<u8>, width: u16, height: u16) -> Option<Buffer> {
        let theme = Theme::dark();
        let labels = labels(d);
        let hints: Vec<Option<String>> = vec![
            Some("y".to_owned()),
            Some("a".to_owned()),
            Some("n".to_owned()),
        ];
        let mut terminal = terminal(width, height)?;
        terminal
            .draw(|f| d.draw(f, &theme, f.area(), &labels, &hints))
            .ok()?;
        Some(terminal.backend().buffer().clone())
    }

    /// The text painted on row `y` of `buffer`.
    fn row(buffer: &Buffer, y: u16, width: u16) -> String {
        (0..width)
            .map(|x| buffer[(x, y)].symbol().to_owned())
            .collect()
    }

    #[test]
    fn a_new_dialog_selects_the_first_activatable_choice() {
        let d = dialog("Run `ls`?");
        assert_eq!(d.choices.selected, 0);
        assert_eq!(d.selected_choice().map(|c| c.action), Some(0));
        assert_eq!(d.rect, Rect::default(), "nothing painted yet");
    }

    #[test]
    fn the_dialog_is_centered_in_its_area() {
        let mut d = dialog("Run `ls`?");
        let Some(buffer) = draw(&mut d, 60, 20) else {
            return;
        };
        assert!(d.rect.width > 0 && d.rect.height > 0);
        assert_eq!(
            d.rect.x,
            (60 - d.rect.width) / 2,
            "equal margins left and right"
        );
        assert_eq!(d.rect.y, (20 - d.rect.height) / 2);
        // The border box is really painted where the rect says it is.
        assert_eq!(buffer[(d.rect.x, d.rect.y)].symbol(), "\u{250c}");
        assert_eq!(
            buffer[(d.rect.right() - 1, d.rect.bottom() - 1)].symbol(),
            "\u{2518}"
        );
    }

    #[test]
    fn the_title_paints_in_the_top_border() {
        let mut d = dialog("Run `ls`?");
        let Some(buffer) = draw(&mut d, 60, 20) else {
            return;
        };
        assert!(
            row(&buffer, d.rect.y, 60).contains("Permission"),
            "the title names the dialog on its top edge"
        );
    }

    #[test]
    fn the_body_wraps_at_the_inner_width() {
        let long = "the agent would like to run a command in your working tree \
                    and needs your permission before it does";
        let mut d = dialog(long);
        let Some(buffer) = draw(&mut d, 80, 20) else {
            return;
        };
        let inner_width = d.rect.width.saturating_sub(4);
        let lines = d.body_lines(&Theme::dark(), inner_width);
        assert!(lines.len() > 1, "a long body occupies several rows");
        assert!(
            lines
                .iter()
                .all(|line| line.width() <= usize::from(inner_width)),
            "no wrapped line overflows the inner width"
        );
        // The first wrapped row is painted just inside the top border.
        let first = row(&buffer, d.rect.y + 1, 80);
        assert!(first.contains("the agent would like"));
        assert!(
            !first.contains("permission"),
            "the tail wrapped onto a later row"
        );
    }

    #[test]
    fn the_choices_are_the_last_rows_inside_the_box() {
        let mut d = dialog("Run `ls`?");
        let Some(buffer) = draw(&mut d, 60, 20) else {
            return;
        };
        assert_eq!(d.rows.height, 3, "one row per choice");
        assert_eq!(
            d.rows.bottom(),
            d.rect.bottom() - 1,
            "above the bottom edge"
        );
        assert!(row(&buffer, d.rows.y, 60).contains("Allow once"));
        assert!(row(&buffer, d.rows.y + 2, 60).contains("Reject"));
    }

    #[test]
    fn a_hint_is_right_aligned_against_the_far_edge() {
        let mut d = dialog("Run `ls`?");
        let Some(buffer) = draw(&mut d, 60, 20) else {
            return;
        };
        let last = d.rows.right() - 1;
        assert_eq!(buffer[(last, d.rows.y)].symbol(), "y");
        assert_eq!(buffer[(last, d.rows.y + 2)].symbol(), "n");
    }

    #[test]
    fn the_selected_choice_carries_the_selection_accent() {
        let theme = Theme::dark();
        let selection = theme.role(ThemeRole::Selection).to_ratatui();
        let dim = theme.style(ThemeRole::LineNumber).fg;
        let mut d = dialog("Run `ls`?");
        let Some(buffer) = draw(&mut d, 60, 20) else {
            return;
        };
        let cell = buffer[(d.rows.x, d.rows.y)].clone();
        assert_eq!(cell.bg, selection);
        assert!(cell.modifier.contains(Modifier::BOLD));
        assert_ne!(
            buffer[(d.rows.x, d.rows.y + 2)].bg,
            selection,
            "only one row is selected"
        );
        assert_eq!(
            buffer[(d.rows.x, d.rows.y + 1)].fg,
            dim.unwrap_or_default(),
            "the refused choice renders dimmed"
        );
    }

    #[test]
    fn keyboard_navigation_skips_the_disabled_choice_and_saturates() {
        let mut d = dialog("Run `ls`?");
        d.select_by(1);
        assert_eq!(d.choices.selected, 2, "the refused choice is skipped");
        d.select_by(1);
        assert_eq!(d.choices.selected, 2, "the end never wraps to the start");
        d.select_by(-4);
        assert_eq!(d.choices.selected, 0);
    }

    #[test]
    fn the_pointer_resolves_to_the_choice_it_rests_on() {
        let mut d = dialog("Run `ls`?");
        if draw(&mut d, 60, 20).is_none() {
            return;
        }
        assert_eq!(d.row_at(d.rows.x, d.rows.y), Some(0));
        assert_eq!(
            d.row_at(d.rows.x, d.rows.y + 1),
            Some(1),
            "a refused choice still answers, so a click can explain itself"
        );
        assert_eq!(d.row_at(d.rows.x, d.rect.y), None, "the body is not a row");
        assert_eq!(d.row_at(d.rows.x, d.rect.bottom()), None, "below the box");

        d.set_hover(Some((d.rows.x, d.rows.y + 2)));
        assert_eq!(d.choices.hover, Some(2));
        d.set_hover(Some((d.rows.x, d.rows.y + 1)));
        assert_eq!(d.choices.hover, None, "a refused choice clears the accent");
        d.set_hover(None);
        assert_eq!(d.choices.hover, None);
    }

    #[test]
    fn an_area_too_small_for_the_dialog_still_paints_the_choices() {
        let mut d = dialog("a body long enough that it cannot possibly fit");
        let Some(buffer) = draw(&mut d, 14, 5) else {
            return;
        };
        assert_eq!(d.rect, Rect::new(0, 0, 14, 5), "clamped to the whole area");
        assert!(d.rows.height > 0, "the answers survive the squeeze");
        assert_eq!(d.rows.bottom(), d.rect.bottom() - 1);
        assert!(row(&buffer, d.rows.y, 14).contains("Allow"));
    }

    #[test]
    fn a_degenerate_area_paints_nothing_and_never_panics() {
        let mut d = dialog("Run `ls`?");
        // A frame is only drawn for a non-empty terminal, so exercise the
        // zero-sized area through a real frame with an empty sub-rect.
        let theme = Theme::dark();
        let names = labels(&d);
        let hints = vec![None; names.len()];
        let Some(mut terminal) = terminal(10, 4) else {
            return;
        };
        let drawn = terminal.draw(|f| {
            d.draw(f, &theme, Rect::new(0, 0, 0, 0), &names, &hints);
        });
        assert!(drawn.is_ok());
        assert_eq!(d.rect, Rect::default());
        assert_eq!(d.rows, Rect::default());
        assert_eq!(d.row_at(0, 0), None);
    }

    #[test]
    fn an_empty_body_reserves_no_rows() {
        let mut d: Dialog<u8> = Dialog::new("Discard?", "", vec![Choice::custom("Yes", 0u8)]);
        let labels = vec!["Yes".to_owned()];
        let hints = vec![None];
        let theme = Theme::dark();
        let Some(mut terminal) = terminal(40, 10) else {
            return;
        };
        let drawn = terminal.draw(|f| d.draw(f, &theme, f.area(), &labels, &hints));
        assert!(drawn.is_ok());
        assert_eq!(d.rect.height, 3, "borders plus the single choice");
        assert!(d.body.is_empty());
        assert_eq!(d.body.source(), "");
    }

    #[test]
    fn a_body_is_plain_text_by_default() {
        assert_eq!(DialogBody::from("hi"), DialogBody::Plain("hi".to_owned()));
        assert_eq!(
            DialogBody::from("hi".to_owned()),
            DialogBody::Plain("hi".to_owned())
        );
    }

    #[test]
    fn centering_clamps_to_an_area_smaller_than_the_box() {
        let area = Rect::new(4, 2, 10, 4);
        assert_eq!(centered(area, 4, 2), Rect::new(7, 3, 4, 2));
        assert_eq!(centered(area, 40, 40), area, "clamped, never overhanging");
        assert_eq!(
            centered(Rect::default(), 10, 10),
            Rect::default(),
            "an empty area yields an empty rect"
        );
    }

    #[cfg(feature = "markdown")]
    #[test]
    fn a_markdown_body_paints_through_the_theme() {
        use karet_core::StandardToken;

        let theme = Theme::dark();
        let mut d: Dialog<u8> = Dialog::new(
            "Permission",
            DialogBody::Markdown("# Heading".to_owned()),
            vec![Choice::custom("Allow", 0u8)],
        );
        let lines = d.body_lines(&theme, 20);
        assert!(!lines.is_empty(), "a markdown body renders");
        let labels = vec!["Allow".to_owned()];
        let hints = vec![None];
        let Some(mut terminal) = terminal(40, 10) else {
            return;
        };
        let drawn = terminal.draw(|f| d.draw(f, &theme, f.area(), &labels, &hints));
        assert!(drawn.is_ok());
        let buffer = terminal.backend().buffer().clone();
        let cell = buffer[(d.rows.x, d.rect.y + 1)].clone();
        assert_eq!(cell.symbol(), "#");
        assert!(
            cell.modifier.contains(Modifier::BOLD),
            "headings render bold"
        );
        assert_eq!(
            cell.fg,
            theme.color(StandardToken::MarkupHeading.id()).to_ratatui()
        );
    }
}
