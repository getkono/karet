//! The top-level view switcher: the one row of chrome above everything else, and
//! the placeholder body a view renders before its surface exists.
//!
//! Only the Agents view still needs that placeholder. The GitHub view draws its own
//! surface, and says so itself when the workspace has no GitHub repository behind it.
//!
//! The switcher is the sidebar activity bar one level up — persistent chrome over
//! a body that swaps between N surfaces — so it reuses that bar's button styling
//! (`chrome_button_style`) rather than inventing a second visual language for the
//! same affordance.

use super::*;

/// One switcher button's text: the view's icon, its name when the row is wide
/// enough for every name, and the spaces that separate the buttons.
fn button_text(view: View, icon_style: karet_filetype::IconStyle, labelled: bool) -> String {
    let glyph = view.icon().glyph(icon_style);
    if labelled {
        format!(" {glyph} {} ", view.title())
    } else {
        format!(" {glyph} ")
    }
}

/// Draw the view switcher across `area`, recording each button's column span in
/// `app.view_hits` for the click handler.
///
/// Labels are dropped for every button at once when they do not all fit, so the
/// row never shows a mix of named and unnamed views.
pub(super) fn draw_view_chrome(f: &mut Frame, app: &mut App, theme: &Theme, area: Rect) {
    app.view_chrome_rect = area;
    app.view_hits.clear();
    f.render_widget(
        Block::default().style(Style::default().bg(theme.role(ThemeRole::Background).to_ratatui())),
        area,
    );
    if area.width == 0 {
        return;
    }

    let icon_style = app.icon_style;
    // Measured rather than estimated: the icon's width is tier-dependent, so a
    // constant would over- or under-count it and truncate the last button.
    let labelled: u16 = View::ALL
        .iter()
        .map(|&view| cell_width(&button_text(view, icon_style, true)))
        .sum();
    let with_labels = labelled <= area.width;

    let mut spans = Vec::with_capacity(View::ALL.len());
    let mut x = area.x;
    for view in View::ALL {
        let text = button_text(view, icon_style, with_labels);
        let width = cell_width(&text);
        let state = if app.view == view {
            ChromeButtonState::Active
        } else {
            ChromeButtonState::Normal
        };
        spans.push(Span::styled(text, chrome_button_style(theme, state)));
        app.view_hits.push((x, x.saturating_add(width), view));
        x = x.saturating_add(width);
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// Draw the body of a view whose surface is not built yet.
///
/// Stable and centred rather than a spinner or an error: nothing is loading and
/// nothing has failed, so the row must not move or alarm.
pub(super) fn draw_view_placeholder(
    f: &mut Frame,
    theme: &Theme,
    area: Rect,
    view: View,
    icon_style: karet_filetype::IconStyle,
) {
    f.render_widget(
        Block::default().style(Style::default().bg(theme.role(ThemeRole::Background).to_ratatui())),
        area,
    );
    if area.height == 0 {
        return;
    }
    let hint = match view {
        View::Agents => "Agent sessions will appear here.",
        // Unreachable in practice — the editor view draws panes and the GitHub view
        // draws its surface — but a total match keeps the placeholder honest if that
        // ever changes.
        View::Editor | View::GitHub => "",
    };
    let lines = vec![
        Line::styled(
            format!(
                "{} {} — not available yet",
                view.icon().glyph(icon_style),
                view.title()
            ),
            theme
                .style(ThemeRole::LineNumberActive)
                .add_modifier(Modifier::BOLD),
        ),
        Line::styled(hint.to_string(), theme.style(ThemeRole::Muted)),
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
