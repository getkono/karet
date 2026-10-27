//! The top-level view switcher: the one row of chrome above everything else.
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
