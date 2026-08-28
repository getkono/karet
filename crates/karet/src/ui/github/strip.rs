//! The GitHub view's page strip, and the body beneath it.
//!
//! The strip is the tab bar's counterpart for a surface that is not made of tabs: the
//! dashboard sits leftmost and carries no close button, and each detail page the user
//! opened follows it. It reuses the chrome button styling the view switcher and the
//! activity bar already share rather than inventing a third look for the same thing.

use super::*;

/// One page's label: its name, and a close button for everything but the dashboard.
fn page_text(title: &str, closeable: bool) -> String {
    if closeable {
        format!(" {title} \u{00d7} ")
    } else {
        format!(" {title} ")
    }
}

/// Draw the whole GitHub view: the page strip, then the active page beneath it.
///
/// When the workspace is not a GitHub repository there are no pages, and the view says
/// so rather than showing an empty frame the user cannot act on.
pub(in crate::ui) fn draw_github_view(
    f: &mut Frame,
    app: &mut App,
    theme: &Theme,
    area: Rect,
    hits: &mut ScrollHits,
) {
    app.github.page_hits.clear();
    app.github.close_hits.clear();
    f.render_widget(
        Block::default().style(Style::default().bg(theme.role(ThemeRole::Background).to_ratatui())),
        area,
    );
    if !app.github.is_active() {
        draw_no_repository(f, theme, area);
        return;
    }
    if area.height == 0 {
        return;
    }

    let rows = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).split(area);
    draw_page_strip(f, app, theme, rows[0]);
    let active = app.github.active();
    if let Some(page) = app.github.pages_mut().get_mut(active) {
        draw_github(f, theme, rows[1], page, hits);
    }
}

/// Draw the strip across `area`, recording each page's span for the click handler.
fn draw_page_strip(f: &mut Frame, app: &mut App, theme: &Theme, area: Rect) {
    let titles: Vec<String> = app
        .github
        .pages()
        .iter()
        .map(GithubViewState::title)
        .collect();
    let active = app.github.active();
    let mut spans = Vec::with_capacity(titles.len());
    let mut x = area.x;
    for (index, title) in titles.iter().enumerate() {
        // The dashboard is the surface's floor: there is no view without it, so it
        // gets no affordance suggesting otherwise.
        let closeable = index > 0;
        let text = page_text(title, closeable);
        let width = cell_width(&text);
        let state = if index == active {
            ChromeButtonState::Active
        } else {
            ChromeButtonState::Normal
        };
        let end = x.saturating_add(width);
        app.github.page_hits.push((index, span_rect(area, x, end)));
        if closeable {
            // The `×` and the space after it, so the target is not a single cell.
            let close = end.saturating_sub(3);
            app.github
                .close_hits
                .push((index, span_rect(area, close, end)));
        }
        spans.push(Span::styled(text, chrome_button_style(theme, state)));
        x = end;
        if x >= area.right() {
            break;
        }
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// A one-row rect spanning `[start, end)` of `area`, clipped to it.
fn span_rect(area: Rect, start: u16, end: u16) -> Rect {
    let start = start.min(area.right());
    let end = end.min(area.right());
    Rect::new(start, area.y, end.saturating_sub(start), 1)
}

/// The body shown when the workspace has no GitHub repository behind it.
fn draw_no_repository(f: &mut Frame, theme: &Theme, area: Rect) {
    if area.height == 0 {
        return;
    }
    let lines = vec![
        Line::styled(
            "This workspace is not a GitHub repository.",
            theme
                .style(ThemeRole::LineNumberActive)
                .add_modifier(Modifier::BOLD),
        ),
        Line::styled(
            "Open a checkout whose origin is on GitHub to see its issues, pull requests, and runs.",
            theme.style(ThemeRole::Muted),
        ),
    ];
    let top = area.y + area.height / 3;
    let rect = Rect::new(
        area.x,
        top.min(area.bottom().saturating_sub(1)),
        area.width,
        area.height.saturating_sub(top - area.y).min(2),
    );
    f.render_widget(Paragraph::new(lines).alignment(Alignment::Center), rect);
}
