use std::time::Duration;
use std::time::Instant;

use alacritty_terminal::event::VoidListener;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::Column;
use alacritty_terminal::index::Line;
use alacritty_terminal::index::Point;
use alacritty_terminal::index::Side;
use alacritty_terminal::selection::Selection;
use alacritty_terminal::selection::SelectionType;

use super::*;

/// The text of every screen row of `term` (history excluded), right-trimmed.
fn screen<T>(term: &Term<T>) -> Vec<String> {
    let columns = term.columns();
    (0..term.screen_lines())
        .map(|row| {
            let line = Line(i32::try_from(row).unwrap_or_default());
            let text: String = (0..columns)
                .map(|column| term.grid()[Point::new(line, Column(column))].c)
                .collect();
            text.trim_end().to_owned()
        })
        .collect()
}

fn feed(term: &mut Term<VoidListener>, bytes: &[u8]) {
    let mut processor: Processor = Processor::new();
    processor.advance(term, bytes);
}

#[test]
fn the_emulator_keeps_scrolled_off_lines_as_history() {
    let mut term = Emulator::new_term(10, 3, VoidListener);
    feed(&mut term, b"one\r\ntwo\r\nthree\r\nfour\r\nfive");
    assert_eq!(screen(&term), ["three", "four", "five"]);
    assert_eq!(term.grid().history_size(), 2);
    term.scroll_display(Scroll::Top);
    assert_eq!(term.grid().display_offset(), 2, "the view can scroll back");
    term.scroll_display(Scroll::Bottom);
    assert_eq!(term.grid().display_offset(), 0);
}

#[test]
fn a_selection_copies_the_text_it_covers() {
    let mut term = Emulator::new_term(20, 2, VoidListener);
    feed(&mut term, b"hello world");
    let mut selection = Selection::new(
        SelectionType::Simple,
        Point::new(Line(0), Column(6)),
        Side::Left,
    );
    selection.update(Point::new(Line(0), Column(10)), Side::Right);
    term.selection = Some(selection);
    assert_eq!(term.selection_to_string().as_deref(), Some("world"));
}

#[test]
fn osc8_hyperlinks_are_kept_on_their_cells() {
    let mut term = Emulator::new_term(20, 2, VoidListener);
    feed(
        &mut term,
        b"\x1b]8;;https://example.com\x1b\\link\x1b]8;;\x1b\\ plain",
    );
    let uri = |column| {
        term.grid()[Point::new(Line(0), Column(column))]
            .hyperlink()
            .map(|link| link.uri().to_owned())
    };
    assert_eq!(uri(0).as_deref(), Some("https://example.com"));
    assert_eq!(uri(3).as_deref(), Some("https://example.com"));
    assert_eq!(uri(5), None, "the link ends where the program closed it");
}

#[test]
fn titles_are_stripped_of_control_characters() {
    assert_eq!(sanitize_title("make\u{1b}[2J \u{7}build"), "make[2J build");
}

#[test]
fn grid_sizes_never_collapse_to_zero() {
    assert_eq!(clamp_size(0, 0), (MIN_COLUMNS, MIN_ROWS));
    assert_eq!(clamp_size(80, 24), (80, 24));
}

/// Apply `rx`'s events to `pane` the way the app does, until `done` holds or a
/// 10-second bound passes; returns whether it held.
#[cfg(unix)]
fn wait_for(
    rx: &mut TerminalEvents,
    pane: &mut TerminalPane,
    done: impl Fn(&TerminalPane) -> bool,
) -> bool {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        while let Ok(event) = rx.try_recv() {
            assert_eq!(event.id, pane.id());
            match &event.kind {
                TerminalEventKind::Output => pane.acknowledge_output(),
                TerminalEventKind::Title(title) => pane.set_title(Some(title)),
                TerminalEventKind::Exited(status) => pane.mark_exited(status.clone()),
                _ => {},
            }
        }
        if done(pane) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(unix)]
fn script(cwd: &Path, body: &str) -> CommandBuilder {
    let mut command = CommandBuilder::new("/bin/sh");
    command.args(["-c", body]);
    command.cwd(cwd);
    command
}

#[cfg(unix)]
fn screen_contains(pane: &TerminalPane, needle: &str) -> bool {
    screen(&pane.term()).iter().any(|row| row.contains(needle))
}

#[cfg(unix)]
#[test]
fn a_shell_starts_in_the_given_directory_and_reports_its_exit()
-> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let root = std::fs::canonicalize(dir.path())?;
    let mut host = TerminalHost::default();
    let mut rx = host.take_receiver().ok_or("no receiver")?;
    assert!(
        host.take_receiver().is_none(),
        "the receiver is handed out once"
    );
    let mut pane = host.spawn_command(
        script(&root, "pwd -P; printf '\\033]0;build log\\007'; exit 3"),
        80,
        5,
    )?;
    let root_text = root.to_string_lossy().into_owned();
    // The output may land after the waiter reports the exit, so wait for both.
    let finished = wait_for(&mut rx, &mut pane, |pane| {
        pane.exit_status().is_some() && pane.title().is_some() && screen_contains(pane, &root_text)
    });
    assert!(finished, "{:?}", screen(&pane.term()));
    assert_eq!(pane.exit_status(), Some("exited with code 3"));
    assert_eq!(pane.title(), Some("build log"));
    Ok(())
}

#[cfg(unix)]
#[test]
fn typed_input_reaches_the_shell_and_resizes_reach_the_pty()
-> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let mut host = TerminalHost::default();
    let mut rx = host.take_receiver().ok_or("no receiver")?;
    let mut pane = host.spawn_command(
        script(
            dir.path(),
            "read line; echo \"got:$line\"; stty size; read wait",
        ),
        80,
        5,
    )?;
    pane.resize(40, 6);
    assert_eq!(pane.size(), (40, 6));
    pane.input(b"hi\r".to_vec());
    let answered = wait_for(&mut rx, &mut pane, |pane| screen_contains(pane, "6 40"));
    assert!(answered, "{:?}", screen(&pane.term()));
    assert!(screen_contains(&pane, "got:hi"));

    // Select the echoed line and read it back the way Copy does.
    let row = screen(&pane.term())
        .iter()
        .position(|line| line.starts_with("got:"))
        .and_then(|row| u16::try_from(row).ok())
        .unwrap_or_default();
    pane.start_selection(SelectionType::Simple, 0, row);
    pane.extend_selection(5, row);
    pane.finish_selection();
    assert_eq!(pane.selection_text().as_deref(), Some("got:hi"));
    pane.clear_selection();
    assert_eq!(pane.selection_text(), None);
    Ok(())
}

/// Whether `path` comes to hold `expected` within a 10-second bound.
#[cfg(unix)]
fn file_says(path: &Path, expected: &str) -> bool {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if std::fs::read_to_string(path).is_ok_and(|text| text.trim() == expected) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(unix)]
#[test]
fn closing_a_pane_hangs_up_its_running_shell() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let marker = dir.path().join("hup");
    let mut host = TerminalHost::default();
    let mut rx = host.take_receiver().ok_or("no receiver")?;
    let mut pane = host.spawn_command(
        script(
            dir.path(),
            &format!(
                "trap 'echo hung-up > {}; exit' HUP; echo ready; while :; do sleep 0.05; done",
                marker.display()
            ),
        ),
        80,
        5,
    )?;
    // The trap is set once the shell has printed.
    assert!(wait_for(&mut rx, &mut pane, |pane| screen_contains(
        pane, "ready"
    )));
    drop(pane);
    assert!(file_says(&marker, "hung-up"), "the shell saw SIGHUP");
    Ok(())
}

#[cfg(unix)]
#[test]
fn closing_a_pane_hangs_up_a_job_that_ignores_sighup() -> Result<(), Box<dyn std::error::Error>> {
    // The job keeps the terminal open and keeps printing; only the terminal
    // itself going away (every copy of the master closed) stops its writes.
    let dir = tempfile::tempdir()?;
    let marker = dir.path().join("gone");
    let mut host = TerminalHost::default();
    let mut rx = host.take_receiver().ok_or("no receiver")?;
    let mut pane = host.spawn_command(
        script(
            dir.path(),
            &format!(
                "(trap '' HUP; exec 2>/dev/null; while printf .; do sleep 0.05; done; \
                 echo gone > {}) & echo ready; wait",
                marker.display()
            ),
        ),
        80,
        5,
    )?;
    assert!(wait_for(&mut rx, &mut pane, |pane| screen_contains(
        pane, "ready"
    )));
    drop(pane);
    assert!(
        file_says(&marker, "gone"),
        "the job's terminal was hung up under it"
    );
    Ok(())
}
