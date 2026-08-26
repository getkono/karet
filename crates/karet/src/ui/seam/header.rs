//! The header: where you are, what you are reading it under, and what the glyphs mean.
//!
//! The configuration is always named here. A view that shows "the package" without
//! saying which build of it is answering a question the reader did not ask — and the
//! answer changes with the feature set, so leaving it implicit is not a simplification
//! but a wrong answer delivered confidently.

use karet_core::ThemeRole;
use karet_filetype::IconStyle;
use karet_theme::Theme;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::widgets::Paragraph;

use super::LENS_NAMES;
use super::lens_glyph;
use crate::app::seam::SeamViewState;

/// Draw the header row.
pub(super) fn draw(
    f: &mut Frame,
    theme: &Theme,
    area: Rect,
    state: &SeamViewState,
    icons: IconStyle,
) {
    let muted = theme.style(ThemeRole::Muted);
    let strong = theme
        .style(ThemeRole::Foreground)
        .add_modifier(Modifier::BOLD);

    let mut spans = vec![Span::styled("Seam  ", muted)];
    spans.push(Span::styled(
        if state.summary.package.is_empty() {
            "…".to_owned()
        } else {
            state.summary.package.clone()
        },
        strong,
    ));
    // Said only when there is more than one, because "1 package" beside a package's own
    // name is noise, and the single-package view is the common one.
    if state.summary.packages > 1 {
        spans.push(Span::styled(
            format!(" · {} packages", state.summary.packages),
            muted,
        ));
    }
    // The breadcrumb is the narrow-undo stack made visible: every step in is a step the
    // reader can see and step back out of.
    for narrow in &state.narrow {
        spans.push(Span::styled(" › ", muted));
        spans.push(Span::styled(narrow.label(), strong));
    }

    spans.push(Span::styled("   ", muted));
    spans.extend(configuration_line(theme, state));
    spans.push(Span::styled("   ", muted));
    spans.extend(legend(theme, state, icons));

    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// The configuration marker, and the caveat when the answer is incomplete.
pub(crate) fn configuration_line<'a>(theme: &Theme, state: &'a SeamViewState) -> Vec<Span<'a>> {
    let muted = theme.style(ThemeRole::Muted);
    let mut spans = vec![
        Span::styled("config: ", muted),
        Span::styled(
            if state.summary.configuration.is_empty() {
                "unconfigured".to_owned()
            } else {
                state.summary.configuration.clone()
            },
            theme.style(ThemeRole::DiagnosticInfo),
        ),
    ];
    // Say what is unknown rather than letting a partial answer look complete.
    if !state.summary.variation_complete {
        spans.push(Span::styled(
            " (variation incomplete)",
            theme.style(ThemeRole::DiagnosticWarning),
        ));
    }
    if let Some(scanned) = state.summary.truncated_after {
        spans.push(Span::styled(
            format!(" (truncated after {scanned} files)"),
            theme.style(ThemeRole::DiagnosticWarning),
        ));
    }
    if !state.summary.unresolved_modules.is_empty() {
        spans.push(Span::styled(
            format!(
                " ({} module{} unresolved)",
                state.summary.unresolved_modules.len(),
                if state.summary.unresolved_modules.len() == 1 {
                    ""
                } else {
                    "s"
                }
            ),
            theme.style(ThemeRole::DiagnosticWarning),
        ));
    }
    spans
}

/// The persistent lens legend, with the active lenses emphasized.
///
/// Persistent rather than behind a keypress: a glyph nobody can decode is decoration,
/// and the cost of five short labels is one header row.
fn legend<'a>(theme: &Theme, state: &SeamViewState, icons: IconStyle) -> Vec<Span<'a>> {
    let muted = theme.style(ThemeRole::Muted);
    let active = theme
        .style(ThemeRole::Foreground)
        .add_modifier(Modifier::BOLD);
    let mut spans = Vec::new();
    for (index, lens) in LENS_NAMES.iter().enumerate() {
        let on = state.lenses.contains(lens);
        // The digit that toggles it, so the binding never has to be memorized.
        spans.push(Span::styled(
            format!("{}{} ", index + 1, lens_glyph(lens, icons)),
            if on { active } else { muted },
        ));
        spans.push(Span::styled(
            format!("{}  ", short_name(lens)),
            if on { active } else { muted },
        ));
    }
    spans
}

/// The abbreviated lens name used in the header, where width is scarce.
fn short_name(lens: &str) -> &'static str {
    match lens {
        "api" => "api",
        "substitution" => "sub",
        "variation" => "var",
        "boundary" => "bnd",
        _ => "haz",
    }
}
