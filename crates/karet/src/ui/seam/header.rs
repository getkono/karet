//! The header: where you are, what you are reading it under, and what the glyphs mean.
//!
//! The configuration is always named here. A view that shows "the package" without
//! saying which build of it is answering a question the reader did not ask — and the
//! answer changes with the feature set, so leaving it implicit is not a simplification
//! but a wrong answer delivered confidently.

use karet_core::ThemeRole;
use karet_filetype::IconStyle;
use karet_theme::Theme;
use karet_widgets::UiIcon;
use karet_widgets::glyph::slot;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::widgets::Paragraph;

use super::LENS_NAMES;
use super::lens_glyph;
use crate::app::seam::SeamViewState;
use crate::app::seam::geometry::span_rect;

/// Draw the header row.
pub(super) fn draw(
    f: &mut Frame,
    theme: &Theme,
    area: Rect,
    state: &mut SeamViewState,
    icons: IconStyle,
) {
    let muted = theme.style(ThemeRole::Muted);
    let strong = theme
        .style(ThemeRole::Foreground)
        .add_modifier(Modifier::BOLD);

    // Every run is measured as it is placed, so what the reader can click is exactly what
    // was painted — a name long enough to push the legend off the row leaves nothing
    // behind it to click.
    let mut spans = Vec::new();
    let mut crumbs = Vec::new();
    let mut lenses = Vec::new();
    let mut config = Rect::default();
    let mut x = area.x;
    let place = |spans: &mut Vec<Span<'static>>, run: String, style, x: &mut u16| -> Rect {
        let rect = span_rect(area, *x, area.y, karet_widgets::text::width(&run));
        *x = x.saturating_add(rect.width);
        spans.push(Span::styled(run, style));
        rect
    };

    place(&mut spans, "Seam  ".to_owned(), muted, &mut x);
    // The package name widens all the way back out, which is the crumb before every other.
    let root = place(
        &mut spans,
        if state.summary.package.is_empty() {
            "…".to_owned()
        } else {
            state.summary.package.clone()
        },
        strong,
        &mut x,
    );
    crumbs.push((root, 0));
    // Said only when there is more than one, because "1 package" beside a package's own
    // name is noise, and the single-package view is the common one.
    if state.summary.packages > 1 {
        place(
            &mut spans,
            format!(" · {} packages", state.summary.packages),
            muted,
            &mut x,
        );
    }
    // The breadcrumb is the narrow-undo stack made visible: every step in is a step the
    // reader can see and step back out of.
    for (depth, narrow) in state.narrow.iter().enumerate() {
        place(&mut spans, " › ".to_owned(), muted, &mut x);
        let rect = place(&mut spans, narrow.label(), strong, &mut x);
        crumbs.push((rect, depth + 1));
    }

    place(&mut spans, "   ".to_owned(), muted, &mut x);
    for (index, (run, style)) in configuration_runs(theme, state).into_iter().enumerate() {
        let rect = place(&mut spans, run, style, &mut x);
        // The marker and its name; not the caveats after them, since clicking
        // "(variation incomplete)" must not cycle the build.
        if index < 2 {
            config = merge(config, rect);
        }
    }
    place(&mut spans, "   ".to_owned(), muted, &mut x);
    for (index, lens) in LENS_NAMES.iter().enumerate() {
        let on = state.lenses.contains(lens);
        let style = if on { strong } else { muted };
        // The digit that toggles it, so the binding never has to be memorized.
        let glyph = place(
            &mut spans,
            format!("{}{} ", index + 1, slot(lens_glyph(lens, icons), icons)),
            style,
            &mut x,
        );
        let name = place(&mut spans, format!("{}  ", short_name(lens)), style, &mut x);
        lenses.push((merge(glyph, name), index));
    }

    // Right-aligned, and placed last so the row's own content decides whether there is
    // room. The navigator's rows are its scarcest resource, so these cost none of them —
    // and when the header is too narrow they are simply absent rather than overlapping
    // the name, with `s` and `S` still doing the same thing.
    let (sync, force_sync) = place_actions(&mut spans, theme, area, state, icons, x);

    state.hits.crumbs = crumbs;
    state.hits.lenses = lenses;
    state.hits.config = config;
    state.hits.sync = sync;
    state.hits.force_sync = force_sync;
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// Pad out to the right edge and place the two sync affordances there.
///
/// Returns their rects, both zero-width when the row has no room left for them.
fn place_actions(
    spans: &mut Vec<Span<'static>>,
    theme: &Theme,
    area: Rect,
    state: &SeamViewState,
    icons: IconStyle,
    used: u16,
) -> (Rect, Rect) {
    let sync_label = format!(" {}", slot(UiIcon::Refresh.glyph(icons), icons));
    let force_label = format!(" {}!  ", slot(UiIcon::Refresh.glyph(icons), icons));
    let sync_width = karet_widgets::text::width(&sync_label);
    let force_width = karet_widgets::text::width(&force_label);
    let needed = u16::try_from(sync_width.saturating_add(force_width)).unwrap_or(u16::MAX);

    let remaining = area.right().saturating_sub(used);
    if remaining < needed {
        return (Rect::default(), Rect::default());
    }

    let pad = remaining.saturating_sub(needed);
    if pad > 0 {
        spans.push(Span::raw(" ".repeat(usize::from(pad))));
    }
    let mut x = used.saturating_add(pad);

    // Emphasized while a sync is running, so the row the reader clicked says it is
    // working without a spinner stealing the caveat area.
    let running = state.syncing.is_some();
    let style = if running {
        theme
            .style(ThemeRole::DiagnosticInfo)
            .add_modifier(Modifier::BOLD)
    } else {
        theme.style(ThemeRole::Muted)
    };

    let sync = span_rect(area, x, area.y, sync_width);
    spans.push(Span::styled(sync_label, style));
    x = x.saturating_add(u16::try_from(sync_width).unwrap_or(u16::MAX));
    let force = span_rect(area, x, area.y, force_width);
    spans.push(Span::styled(force_label, style));
    (sync, force)
}

/// The smallest rect covering both, treating a zero-width one as absent.
fn merge(left: Rect, right: Rect) -> Rect {
    if left.width == 0 {
        return right;
    }
    if right.width == 0 {
        return left;
    }
    let x = left.x.min(right.x);
    let end = left.right().max(right.right());
    Rect::new(x, left.y, end.saturating_sub(x), 1)
}

/// The configuration marker, and the caveat when the answer is incomplete.
///
/// Runs rather than spans, so the caller can measure each as it places it. The first two
/// are the marker and the name; the rest are caveats, which are read but never clicked.
fn configuration_runs(theme: &Theme, state: &SeamViewState) -> Vec<(String, Style)> {
    let muted = theme.style(ThemeRole::Muted);
    let info = theme.style(ThemeRole::DiagnosticInfo);
    let warn = theme.style(ThemeRole::DiagnosticWarning);
    let mut runs = vec![
        ("config: ".to_owned(), muted),
        (
            if state.summary.configuration.is_empty() {
                "unconfigured".to_owned()
            } else {
                state.summary.configuration.clone()
            },
            info,
        ),
    ];
    // Say what is unknown rather than letting a partial answer look complete.
    if !state.summary.variation_complete {
        runs.push((" (variation incomplete)".to_owned(), warn));
    }
    if let Some(scanned) = state.summary.truncated_after {
        runs.push((format!(" (truncated after {scanned} files)"), warn));
    }
    if !state.summary.unresolved_modules.is_empty() {
        runs.push((
            format!(
                " ({} module{} unresolved)",
                state.summary.unresolved_modules.len(),
                if state.summary.unresolved_modules.len() == 1 {
                    ""
                } else {
                    "s"
                }
            ),
            warn,
        ));
    }
    runs
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
