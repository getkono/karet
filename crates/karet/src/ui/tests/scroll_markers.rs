//! The editor's vertical scrollbar doubles as an overview of the document.

use karet_core::Decoration;
use karet_core::DecorationKind;
use karet_core::LineCol;
use karet_core::Range;
use karet_core::ThemeRole;
use ratatui::buffer::Buffer;

use crate::app::tests::support::app;
use crate::app::tests::support::frame;
use crate::app::tests::support::text_tab;
use crate::tab::TabKind;

/// The `(x, y)` of every groove tick painted in `role`'s colour.
fn ticks(buffer: &Buffer, colour: ratatui::style::Color) -> Vec<(u16, u16)> {
    let area = buffer.area;
    (area.y..area.bottom())
        .flat_map(|y| (area.x..area.right()).map(move |x| (x, y)))
        .filter(|&(x, y)| buffer[(x, y)].symbol() == "━" && buffer[(x, y)].fg == colour)
        .collect()
}

/// A code tab over `lines` numbered lines with a search match on `matched`.
fn app_with_match(lines: usize, matched: u32) -> crate::app::App {
    let text: String = (0..lines).map(|n| format!("line {n}\n")).collect();
    let mut app = app();
    let mut tab = text_tab("notes.txt", &text);
    if let TabKind::Code { search_decos, .. } = &mut tab.kind {
        search_decos.push(Decoration {
            range: Range {
                start: LineCol::new(matched, 0),
                end: LineCol::new(matched, 4),
            },
            kind: DecorationKind::TextBackground,
            role: Some(ThemeRole::SearchMatch),
        });
    }
    app.tabs.push(tab);
    app.active = app.tabs.len() - 1;
    app
}

#[test]
fn a_search_match_far_below_the_viewport_ticks_the_scrollbar() {
    let mut app = app_with_match(1000, 900);
    let colour = app.theme.role(ThemeRole::SearchMatch).to_ratatui();
    let buffer = frame(&mut app, 80, 24);

    let found = ticks(&buffer, colour);
    assert_eq!(found.len(), 1, "one match, one tick: {found:?}");
    // The match sits 90% of the way down, so its tick is in the track's lower part.
    let (_, y) = found[0];
    assert!(y > buffer.area.height / 2, "tick at row {y}");
}

#[test]
fn a_document_that_fits_keeps_its_scrollbar_column_clean() {
    let mut app = app_with_match(5, 2);
    let colour = app.theme.role(ThemeRole::SearchMatch).to_ratatui();
    let buffer = frame(&mut app, 80, 24);

    assert!(ticks(&buffer, colour).is_empty());
}
