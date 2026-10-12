//! The LSP signature-help popup: the parameter hints of the call around the
//! caret.
//!
//! [`SignaturePopup`] renders a `karet-core` [`SignatureHelp`] the backend
//! supplied over the event stream. The first rows are the shown signature's
//! label, wrapped to the popup's width, with the active parameter bold and
//! underlined; an `n/m` counter leads it when the server offered overloads.
//! Below it, muted, come the active parameter's documentation and then the
//! signature's, as plain text, at most [`MAX_DOC_ROWS`] rows of them.
//!
//! The caller places the popup (above the caret, so the completion popup that
//! opens below it can show at the same time) and owns which overload is
//! shown; [`SignaturePopup::desired_size`] says how much room it wants.

use karet_core::Markup;
use karet_core::Signature;
use karet_core::SignatureHelp;
use karet_core::ThemeRole;
use karet_theme::Theme;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::widgets::Widget;
use unicode_width::UnicodeWidthChar;

/// The most documentation rows shown under the signature; the rest is cut.
pub const MAX_DOC_ROWS: u16 = 6;

/// The widest the popup asks to be, however long the signature.
pub const MAX_WIDTH: u16 = 80;

/// A signature-help popup over one [`SignatureHelp`] answer.
pub struct SignaturePopup<'a> {
    /// The answer, supplied by the backend.
    pub help: &'a SignatureHelp,
    /// The overload shown, an index into the answer's signatures.
    pub shown: usize,
    /// The theme resolving the popup's colors.
    pub theme: &'a Theme,
}

impl<'a> SignaturePopup<'a> {
    /// A popup showing the signature the server marked active.
    #[must_use]
    pub fn new(help: &'a SignatureHelp, theme: &'a Theme) -> Self {
        Self {
            help,
            shown: help.active_signature,
            theme,
        }
    }

    /// Show overload `index` instead, clamped to the ones there are.
    #[must_use]
    pub fn shown(mut self, index: usize) -> Self {
        self.shown = index;
        self
    }

    /// The signature painted, after clamping; `None` when there is none.
    #[must_use]
    pub fn signature(&self) -> Option<&'a Signature> {
        let last = self.help.signatures.len().checked_sub(1)?;
        self.help.signatures.get(self.shown.min(last))
    }

    /// The popup's content wrapped to `width` columns: the label rows, then
    /// at most [`MAX_DOC_ROWS`] documentation rows. Empty when there is no
    /// signature to show or no column to show it in.
    #[must_use]
    pub fn lines(&self, width: u16) -> Vec<Line<'static>> {
        let Some(signature) = self.signature() else {
            return Vec::new();
        };
        if width == 0 {
            return Vec::new();
        }
        let foreground = self.theme.role(ThemeRole::Foreground).to_ratatui();
        let muted = self.theme.role(ThemeRole::Muted).to_ratatui();
        let plain = Style::default().fg(foreground);
        let active = plain.add_modifier(Modifier::BOLD | Modifier::UNDERLINED);

        let mut pieces = Vec::new();
        let count = self.help.signatures.len();
        if count > 1 {
            let index = self.shown.min(count - 1) + 1;
            pieces.push((format!("{index}/{count} "), Style::default().fg(muted)));
        }
        let label = signature.label.as_str();
        match active_parameter_span(signature, self.help.active_parameter) {
            Some((start, end)) => {
                pieces.push((label[..start].to_owned(), plain));
                pieces.push((label[start..end].to_owned(), active));
                pieces.push((label[end..].to_owned(), plain));
            },
            None => pieces.push((label.to_owned(), plain)),
        }
        let mut lines = wrap_styled(&pieces, width);

        let parameter_docs = signature
            .parameters
            .get(self.help.active_parameter)
            .and_then(|parameter| parameter.documentation.as_ref());
        let docs: Vec<String> = [parameter_docs, signature.documentation.as_ref()]
            .into_iter()
            .flatten()
            .flat_map(doc_lines)
            .collect();
        let doc_style = Style::default().fg(muted);
        let doc_rows = docs
            .iter()
            .flat_map(|line| wrap_styled(&[(line.clone(), doc_style)], width))
            .take(usize::from(MAX_DOC_ROWS));
        lines.extend(doc_rows);
        lines
    }

    /// The `(width, height)` the popup wants within `max_width` columns: as
    /// wide as its longest unwrapped row (capped at [`MAX_WIDTH`]) and as
    /// tall as its content wraps to at that width. `(0, 0)` when there is
    /// nothing to show.
    #[must_use]
    pub fn desired_size(&self, max_width: u16) -> (u16, u16) {
        let cap = max_width.min(MAX_WIDTH);
        let widest = self
            .lines(u16::MAX)
            .iter()
            .map(|line| u16::try_from(line.width()).unwrap_or(u16::MAX))
            .max()
            .unwrap_or(0);
        let width = widest.min(cap);
        let height = u16::try_from(self.lines(width).len()).unwrap_or(u16::MAX);
        if height == 0 { (0, 0) } else { (width, height) }
    }
}

impl Widget for SignaturePopup<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        let background = self.theme.role(ThemeRole::Background).to_ratatui();
        for y in area.top()..area.bottom() {
            for x in area.left()..area.right() {
                if let Some(cell) = buf.cell_mut((x, y)) {
                    cell.set_char(' ')
                        .set_style(Style::default().bg(background));
                }
            }
        }
        for (row, line) in self
            .lines(area.width)
            .iter()
            .take(usize::from(area.height))
            .enumerate()
        {
            let y = area.y + u16::try_from(row).unwrap_or(u16::MAX);
            buf.set_line(area.x, y, line, area.width);
        }
    }
}

/// The byte range of parameter `active` inside `signature`'s label, or
/// `None` when the label does not spell it out (or there is no such
/// parameter -- past the last one of a variadic call, say).
///
/// Parameters are looked for in order, each after the one before and all
/// after the label's first `(`, so a parameter named `a` is not found inside
/// the function name `add`, nor a repeated type inside an earlier parameter.
#[must_use]
pub fn active_parameter_span(signature: &Signature, active: usize) -> Option<(usize, usize)> {
    let label = signature.label.as_str();
    let mut from = label.find('(').map_or(0, |open| open + 1);
    for (index, parameter) in signature.parameters.iter().enumerate() {
        if parameter.label.is_empty() {
            return None;
        }
        let start = from + label.get(from..)?.find(parameter.label.as_str())?;
        let end = start + parameter.label.len();
        if index == active {
            return Some((start, end));
        }
        from = end;
    }
    None
}

/// The documentation's lines as plain text: code-fence markers dropped,
/// blank lines at either end trimmed.
fn doc_lines(markup: &Markup) -> Vec<String> {
    let lines: Vec<String> = markup
        .value
        .lines()
        .filter(|line| !line.trim_start().starts_with("```"))
        .map(|line| line.trim_end().to_owned())
        .collect();
    let first = lines.iter().position(|line| !line.is_empty());
    let last = lines.iter().rposition(|line| !line.is_empty());
    match (first, last) {
        (Some(first), Some(last)) => lines[first..=last].to_vec(),
        _ => Vec::new(),
    }
}

/// Wrap styled `pieces` into rows of at most `width` columns, breaking
/// between characters (a signature has no spaces to spare).
fn wrap_styled(pieces: &[(String, Style)], width: u16) -> Vec<Line<'static>> {
    let width = usize::from(width);
    let mut lines = Vec::new();
    let mut row: Vec<Span<'static>> = Vec::new();
    let mut used = 0;
    for (text, style) in pieces {
        let mut run = String::new();
        for character in text.chars() {
            let cols = character.width().unwrap_or(0);
            if used + cols > width && used > 0 {
                if !run.is_empty() {
                    row.push(Span::styled(std::mem::take(&mut run), *style));
                }
                lines.push(Line::from(std::mem::take(&mut row)));
                used = 0;
            }
            run.push(character);
            used += cols;
        }
        if !run.is_empty() {
            row.push(Span::styled(run, *style));
        }
    }
    if !row.is_empty() || lines.is_empty() {
        lines.push(Line::from(row));
    }
    lines
}

#[cfg(test)]
mod tests {
    use karet_core::MarkupKind;
    use karet_core::ParamInfo;

    use super::*;

    fn signature(label: &str, parameters: &[&str]) -> Signature {
        Signature {
            label: label.to_owned(),
            documentation: None,
            parameters: parameters
                .iter()
                .map(|label| ParamInfo {
                    label: (*label).to_owned(),
                    documentation: None,
                })
                .collect(),
        }
    }

    fn help(signatures: Vec<Signature>, active_parameter: usize) -> SignatureHelp {
        SignatureHelp {
            signatures,
            active_signature: 0,
            active_parameter,
        }
    }

    fn markdown(value: &str) -> Markup {
        Markup {
            kind: MarkupKind::Markdown,
            value: value.to_owned(),
        }
    }

    fn row_text(buf: &Buffer, y: u16) -> String {
        (buf.area.left()..buf.area.right())
            .filter_map(|x| buf.cell((x, y)).map(|cell| cell.symbol().to_owned()))
            .collect()
    }

    /// Falsified by a plain substring search: `a` would be found inside the
    /// function name `add`, and the second `i32` parameter inside the first.
    #[test]
    fn the_active_parameter_is_found_after_the_name_and_the_ones_before_it() {
        let add = signature("fn add(a: i32, b: i32) -> i32", &["a", "i32"]);
        assert_eq!(active_parameter_span(&add, 0), Some((7, 8)));
        let same = signature("f(i32, i32)", &["i32", "i32"]);
        assert_eq!(active_parameter_span(&same, 1), Some((7, 10)));
    }

    /// Past the last parameter, a parameter the label does not spell, or an
    /// empty parameter label highlights nothing rather than the wrong text.
    #[test]
    fn an_unplaceable_parameter_highlights_nothing() {
        let add = signature("add(a, b)", &["a", "b"]);
        assert_eq!(active_parameter_span(&add, 2), None);
        let missing = signature("add(a, b)", &["a", "zz"]);
        assert_eq!(active_parameter_span(&missing, 1), None);
        let empty = signature("add()", &[""]);
        assert_eq!(active_parameter_span(&empty, 0), None);
    }

    /// The active parameter's cells are bold and underlined, its neighbours
    /// are neither; with one signature there is no counter.
    #[test]
    fn the_active_parameter_renders_bold_and_underlined() {
        let theme = Theme::dark();
        let answer = help(vec![signature("add(a, b)", &["a", "b"])], 1);
        let area = Rect::new(0, 0, 12, 1);
        let mut buf = Buffer::empty(area);
        SignaturePopup::new(&answer, &theme).render(area, &mut buf);
        assert_eq!(row_text(&buf, 0), "add(a, b)   ");
        let modifiers = |x: u16| buf.cell((x, 0)).map(|cell| cell.modifier);
        let marked = Some(Modifier::BOLD | Modifier::UNDERLINED);
        assert_eq!(modifiers(7), marked, "`b` is the active parameter");
        assert_eq!(modifiers(4), Some(Modifier::empty()), "`a` is not");
    }

    /// Overloads get an `n/m` counter naming the one shown, which the caller
    /// picks; an index past the last clamps to it.
    #[test]
    fn overloads_show_which_one_is_shown() {
        let theme = Theme::dark();
        let answer = help(
            vec![signature("f(a)", &["a"]), signature("f(a, b)", &["a", "b"])],
            0,
        );
        let first = SignaturePopup::new(&answer, &theme).lines(40);
        assert_eq!(first[0].to_string(), "1/2 f(a)");
        let second = SignaturePopup::new(&answer, &theme).shown(1).lines(40);
        assert_eq!(second[0].to_string(), "2/2 f(a, b)");
        let clamped = SignaturePopup::new(&answer, &theme).shown(9).lines(40);
        assert_eq!(clamped[0].to_string(), "2/2 f(a, b)");
    }

    /// The parameter's docs come before the signature's, fences dropped, and
    /// no more than [`MAX_DOC_ROWS`] of them.
    #[test]
    fn docs_follow_the_label_parameter_first_and_capped() {
        let theme = Theme::dark();
        let mut documented = signature("f(a)", &["a"]);
        documented.parameters[0].documentation = Some(markdown("the a"));
        documented.documentation = Some(markdown("```rust\nf\n```\n\nline\n2\n3\n4\n5\n6\n7"));
        let answer = help(vec![documented], 0);
        let lines: Vec<String> = SignaturePopup::new(&answer, &theme)
            .lines(40)
            .iter()
            .map(ToString::to_string)
            .collect();
        assert_eq!(lines, ["f(a)", "the a", "f", "", "line", "2", "3"]);
    }

    /// A label wider than the popup wraps rather than being cut, and the
    /// desired size is the longest row capped at the room offered.
    #[test]
    fn a_long_label_wraps_and_sizes_to_the_room() {
        let theme = Theme::dark();
        let answer = help(
            vec![signature("function(alpha, beta)", &["alpha", "beta"])],
            0,
        );
        let popup = SignaturePopup::new(&answer, &theme);
        let rows: Vec<String> = popup.lines(10).iter().map(ToString::to_string).collect();
        assert_eq!(rows, ["function(a", "lpha, beta", ")"]);
        assert_eq!(popup.desired_size(100), (21, 1));
        assert_eq!(popup.desired_size(10), (10, 3));
    }

    /// No signature, nothing to draw: the caller places no popup.
    #[test]
    fn an_empty_answer_wants_no_room() {
        let theme = Theme::dark();
        let answer = help(Vec::new(), 0);
        assert_eq!(
            SignaturePopup::new(&answer, &theme).desired_size(80),
            (0, 0)
        );
    }
}
