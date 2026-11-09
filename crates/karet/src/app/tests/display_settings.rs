//! The `editor.*` display settings reach the editor pane, and a configuration
//! reload repaints with them: `lineNumbers`, `cursorLine`, `rulers` and
//! `scrollOff`.

use karet_core::ThemeRole;
use karet_session::Settings;
use karet_session::config::schema::LineNumbers;

use super::harness::*;
use super::support::frame;
use crate::app::*;

const SIZE: (u16, u16) = (40, 12);

/// Thirty numbered lines with the caret on the first.
fn thirty_lines() -> String {
    let body: String = (0..30).map(|n| format!("line {n}\n")).collect();
    format!("|{body}")
}

/// Reload the configuration with `edit` applied, as the watcher does.
fn reload(rig: &mut Rig, edit: impl FnOnce(&mut Settings)) {
    let mut settings = rig.app.settings.clone();
    edit(&mut settings);
    rig.app.on_backend_event(
        None,
        SessionEvent::ConfigChanged {
            report: Box::new(karet_session::LoadedConfig::from_settings(settings)),
        },
    );
}

#[test]
fn a_reload_repaints_the_gutter_cursor_line_and_rulers() {
    let mut rig = Rig::sized(&thirty_lines(), SIZE.0, SIZE.1);
    let screen = frame(&mut rig.app, SIZE.0, SIZE.1);
    let area = rig.app.last_frame.editor_rect();
    // Thirty-one lines: a marker cell, two digits, the change lane and the
    // fold lane.
    assert_eq!(screen[(area.x + 5, area.y)].symbol(), "l");
    let theme = rig.app.theme.clone();
    let color = |role| theme.role(role).to_ratatui();
    assert_eq!(
        screen[(area.x + 8, area.y)].bg,
        color(ThemeRole::CursorLine)
    );

    reload(&mut rig, |settings| {
        settings.editor.line_numbers = LineNumbers::Off;
        settings.editor.cursor_line = false;
        settings.editor.rulers = vec![2];
    });
    let screen = frame(&mut rig.app, SIZE.0, SIZE.1);
    let area = rig.app.last_frame.editor_rect();
    assert_eq!(
        screen[(area.x + 3, area.y)].symbol(),
        "l",
        "the numbers are gone"
    );
    assert_eq!(
        screen[(area.x + 8, area.y)].bg,
        color(ThemeRole::Background)
    );
    assert_eq!(
        screen[(area.x + 5, area.y + 1)].bg,
        color(ThemeRole::IndentGuide),
        "the ruler after column 2"
    );

    // A click maps through the narrower gutter.
    rig.step(click(At::Cell(area.x + 5, area.y + 3)));
    assert_eq!(rig.tab().editor.cursor(), LineCol::new(3, 2));

    reload(&mut rig, |settings| {
        settings.editor.line_numbers = LineNumbers::Relative;
    });
    let screen = frame(&mut rig.app, SIZE.0, SIZE.1);
    let area = rig.app.last_frame.editor_rect();
    let number = |row: u16| -> String {
        (1..3)
            .map(|x| screen[(area.x + x, area.y + row)].symbol().to_owned())
            .collect()
    };
    assert_eq!(
        [number(1), number(3), number(5)],
        [" 2", " 4", " 2"],
        "counted from the caret, which shows its own"
    );
}

#[test]
fn a_reload_sets_the_keyboard_scroll_margin() {
    let mut rig = Rig::sized(&thirty_lines(), SIZE.0, SIZE.1);
    reload(&mut rig, |settings| settings.editor.scroll_off = 0);
    rig.render();
    let rows = u32::from(rig.app.last_frame.editor_rect().height);
    for _ in 0..rows {
        rig.step(Step::Key(KeyCode::Down, KeyModifiers::NONE));
    }
    assert_eq!(rig.scroll_line(), 1, "no margin: the caret on the last row");

    reload(&mut rig, |settings| settings.editor.scroll_off = 3);
    rig.step(Step::Key(KeyCode::Down, KeyModifiers::NONE));
    assert_eq!(rig.scroll_line(), 5, "three rows kept below the caret");
}
