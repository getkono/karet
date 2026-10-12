//! Painting a language server's log: one row per line, coloured by how urgent
//! the server said each entry was.
//!
//! The text arrives already made safe to paint (the session strips every
//! control and reordering character), so this only lays it out.

use karet_core::Severity;
use karet_core::ThemeRole;
use karet_session::LanguageServerLogEntry;
use karet_theme::Theme;
use ratatui::text::Line;
use ratatui::text::Span;

/// The level tag that opens an entry's first row, padded to one width so the
/// text after it lines up.
fn tag(severity: Severity) -> &'static str {
    match severity {
        Severity::Error => "[error] ",
        Severity::Warning => "[warn]  ",
        Severity::Information => "[info]  ",
        _ => "[log]   ",
    }
}

/// The role an entry is painted in. Routine log detail is muted, so the
/// errors and warnings among it stand out.
fn role(severity: Severity) -> ThemeRole {
    match severity {
        Severity::Error => ThemeRole::DiagnosticError,
        Severity::Warning => ThemeRole::DiagnosticWarning,
        Severity::Information => ThemeRole::DiagnosticInfo,
        _ => ThemeRole::Muted,
    }
}

/// Every entry's rows, oldest first. A multi-line entry's continuation rows
/// are indented under its tag.
pub(super) fn lines(theme: &Theme, entries: &[LanguageServerLogEntry]) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    for entry in entries {
        let style = theme.style(role(entry.severity));
        let tag = tag(entry.severity);
        for (index, text) in entry.text.split('\n').enumerate() {
            let lead = if index == 0 {
                tag.to_owned()
            } else {
                " ".repeat(tag.len())
            };
            out.push(Line::from(vec![
                Span::styled(lead, style),
                Span::styled(text.to_owned(), style),
            ]));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn entry(severity: Severity, text: &str) -> LanguageServerLogEntry {
        LanguageServerLogEntry {
            root: PathBuf::from("/w"),
            severity,
            text: text.to_owned(),
        }
    }

    fn text(line: &Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    #[test]
    fn each_line_is_a_row_tagged_and_coloured_by_its_entry() {
        let theme = Theme::default();
        let rows = lines(
            &theme,
            &[
                entry(Severity::Error, "build failed\n  at main.rs"),
                entry(Severity::Hint, "indexed"),
            ],
        );
        let shown: Vec<String> = rows.iter().map(text).collect();
        assert_eq!(
            shown,
            vec![
                "[error] build failed",
                "          at main.rs",
                "[log]   indexed"
            ]
        );
        assert_eq!(
            rows[0].spans[1].style,
            theme.style(ThemeRole::DiagnosticError)
        );
        assert_eq!(rows[2].spans[1].style, theme.style(ThemeRole::Muted));
    }
}
