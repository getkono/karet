//! ANSI (SGR) escape parsing into ratatui spans, for surfaces that show
//! program output verbatim: debug-adapter consoles, notebook tracebacks.
//!
//! The supported subset is the one real tools emit: SGR (`ESC[…m`) styling —
//! reset, bold/dim/italic/underline, 16 named colors, `38;5`/`48;5` indexed,
//! `38;2`/`48;2` truecolor. Every other escape (cursor movement, erase, OSC
//! titles) is stripped, never rendered as garbage. Unstyled text passes
//! through untouched.

use ratatui::style::Color;
use ratatui::style::Modifier;
use ratatui::style::Style;
use ratatui::text::Span;

/// Split one line of program output into styled spans.
///
/// Style state carries across escape sequences within the line but not across
/// calls — feed one line at a time (the debug console and tracebacks are
/// line-oriented; a reset at each line start is how terminals behave after a
/// hard wrap anyway).
#[must_use]
pub fn ansi_spans(line: &str) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut style = Style::default();
    let mut plain = String::new();
    let mut chars = line.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '\u{1b}' {
            if !ch.is_control() || ch == '\t' {
                plain.push(ch);
            }
            continue;
        }
        match chars.peek() {
            // CSI: parameters, then one final byte deciding the command.
            Some('[') => {
                chars.next();
                let mut params = String::new();
                let mut command = None;
                for ch in chars.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&ch) {
                        command = Some(ch);
                        break;
                    }
                    params.push(ch);
                }
                if command == Some('m') {
                    if !plain.is_empty() {
                        spans.push(Span::styled(std::mem::take(&mut plain), style));
                    }
                    style = apply_sgr(style, &params);
                }
                // Every non-SGR CSI (cursor, erase) is stripped.
            },
            // OSC: swallow until BEL or ST.
            Some(']') => {
                chars.next();
                let mut last = '\0';
                for ch in chars.by_ref() {
                    if ch == '\u{7}' || (last == '\u{1b}' && ch == '\\') {
                        break;
                    }
                    last = ch;
                }
            },
            // A two-byte escape (ESC c, ESC 7, …): drop the follower.
            _ => {
                chars.next();
            },
        }
    }
    if !plain.is_empty() || spans.is_empty() {
        spans.push(Span::styled(plain, style));
    }
    spans
}

/// Fold one SGR parameter list into `style`.
fn apply_sgr(mut style: Style, params: &str) -> Style {
    let mut codes = params.split([';', ':']).map(|p| p.parse::<u16>());
    while let Some(code) = codes.next() {
        let Ok(code) = code else {
            continue;
        };
        style = match code {
            0 => Style::default(),
            1 => style.add_modifier(Modifier::BOLD),
            2 => style.add_modifier(Modifier::DIM),
            3 => style.add_modifier(Modifier::ITALIC),
            4 => style.add_modifier(Modifier::UNDERLINED),
            7 => style.add_modifier(Modifier::REVERSED),
            9 => style.add_modifier(Modifier::CROSSED_OUT),
            22 => style.remove_modifier(Modifier::BOLD | Modifier::DIM),
            23 => style.remove_modifier(Modifier::ITALIC),
            24 => style.remove_modifier(Modifier::UNDERLINED),
            27 => style.remove_modifier(Modifier::REVERSED),
            29 => style.remove_modifier(Modifier::CROSSED_OUT),
            30..=37 => style.fg(named_color(code - 30, false)),
            39 => style.fg(Color::Reset),
            40..=47 => style.bg(named_color(code - 40, false)),
            49 => style.bg(Color::Reset),
            90..=97 => style.fg(named_color(code - 90, true)),
            100..=107 => style.bg(named_color(code - 100, true)),
            38 | 48 => {
                let color = match codes.next() {
                    Some(Ok(5)) => codes
                        .next()
                        .and_then(Result::ok)
                        .map(|index| Color::Indexed(u8::try_from(index).unwrap_or(u8::MAX))),
                    Some(Ok(2)) => {
                        let mut channel = || {
                            codes
                                .next()
                                .and_then(Result::ok)
                                .map(|value| u8::try_from(value).unwrap_or(u8::MAX))
                        };
                        match (channel(), channel(), channel()) {
                            (Some(r), Some(g), Some(b)) => Some(Color::Rgb(r, g, b)),
                            _ => None,
                        }
                    },
                    _ => None,
                };
                match (code, color) {
                    (38, Some(color)) => style.fg(color),
                    (48, Some(color)) => style.bg(color),
                    _ => style,
                }
            },
            _ => style,
        };
    }
    style
}

/// The 16 named terminal colors.
fn named_color(index: u16, bright: bool) -> Color {
    match (index, bright) {
        (0, false) => Color::Black,
        (1, false) => Color::Red,
        (2, false) => Color::Green,
        (3, false) => Color::Yellow,
        (4, false) => Color::Blue,
        (5, false) => Color::Magenta,
        (6, false) => Color::Cyan,
        (7, false) => Color::Gray,
        (0, true) => Color::DarkGray,
        (1, true) => Color::LightRed,
        (2, true) => Color::LightGreen,
        (3, true) => Color::LightYellow,
        (4, true) => Color::LightBlue,
        (5, true) => Color::LightMagenta,
        (6, true) => Color::LightCyan,
        _ => Color::White,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_passes_through_as_one_span() {
        let spans = ansi_spans("hello world");
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].content, "hello world");
        assert_eq!(spans[0].style, Style::default());
    }

    #[test]
    fn named_colors_and_reset_split_spans() {
        let spans = ansi_spans("\u{1b}[31mred\u{1b}[0m plain");
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0].content, "red");
        assert_eq!(spans[0].style.fg, Some(Color::Red));
        assert_eq!(spans[1].content, " plain");
        assert_eq!(spans[1].style, Style::default());
    }

    #[test]
    fn bold_composes_with_color_until_cleared() {
        let spans = ansi_spans("\u{1b}[1;32mok\u{1b}[22m still green");
        assert_eq!(spans.len(), 2);
        assert!(spans[0].style.add_modifier.contains(Modifier::BOLD));
        assert_eq!(spans[0].style.fg, Some(Color::Green));
        assert!(!spans[1].style.add_modifier.contains(Modifier::BOLD));
        assert_eq!(spans[1].style.fg, Some(Color::Green));
    }

    #[test]
    fn indexed_and_truecolor_parse() {
        let spans = ansi_spans("\u{1b}[38;5;208morange\u{1b}[48;2;10;20;30m on rgb");
        assert_eq!(spans[0].style.fg, Some(Color::Indexed(208)));
        assert_eq!(spans[1].style.bg, Some(Color::Rgb(10, 20, 30)));
    }

    #[test]
    fn bright_colors_map() {
        let spans = ansi_spans("\u{1b}[91mbright red\u{1b}[103m on bright yellow");
        assert_eq!(spans[0].style.fg, Some(Color::LightRed));
        assert_eq!(spans[1].style.bg, Some(Color::LightYellow));
    }

    #[test]
    fn non_sgr_escapes_are_stripped() {
        let spans = ansi_spans("\u{1b}[2J\u{1b}[Ha\u{1b}]0;title\u{7}b\u{1b}7c");
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].content, "abc");
    }

    #[test]
    fn control_bytes_are_dropped_but_tabs_stay() {
        let spans = ansi_spans("a\rb\tc\u{8}");
        assert_eq!(spans[0].content, "ab\tc");
    }

    #[test]
    fn empty_input_yields_one_empty_span() {
        let spans = ansi_spans("");
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].content, "");
    }

    #[test]
    fn malformed_sgr_parameters_are_ignored() {
        let spans = ansi_spans("\u{1b}[38;9;4mx\u{1b}[38;2;1my\u{1b}[;31mz");
        assert_eq!(spans.len(), 3);
        assert_eq!(spans[0].style.fg, None);
        assert_eq!(spans[1].style.fg, None);
        assert_eq!(spans[2].style.fg, Some(Color::Red));
    }
}
