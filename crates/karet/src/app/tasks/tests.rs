//! Tasks end to end: the picker, a run in a terminal tab, its problems in the
//! panel, and the transcript the matchers read.

use alacritty_terminal::event::VoidListener;
use alacritty_terminal::vte::ansi::Processor;
use karet_session::config::schema::BuiltinMatcher;
use karet_session::config::schema::ProblemMatcher;
use karet_session::config::schema::Task;

use super::*;
use crate::app::tests::support::*;
use crate::terminal::Emulator;
use crate::terminal::TerminalEventKind;
use crate::terminal::TerminalEvents;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn the_transcript_joins_soft_wrapped_rows_and_keeps_the_history() {
    let mut term = Emulator::new_term(10, 3, VoidListener);
    let mut processor: Processor = Processor::new();
    // Six lines on a three-row screen: three scroll into the history. The
    // second is wider than the grid and wraps onto a second row.
    processor.advance(
        &mut term,
        b"one\r\nsrc/main.rs:12:5 long\r\nthree\r\nfour\r\n\xe5\xad\x97x\r\nsix",
    );
    assert_eq!(
        transcript(&term),
        [
            "one",
            "src/main.rs:12:5 long",
            "three",
            "four",
            "字x",
            "six"
        ],
        "a wide character's spacer cell is not text"
    );
}

#[test]
fn the_transcript_drops_the_blank_screen_below_the_output() {
    let mut term = Emulator::new_term(20, 6, VoidListener);
    let mut processor: Processor = Processor::new();
    processor.advance(&mut term, b"done   \r\n\r\n");
    assert_eq!(transcript(&term), ["done"]);
}

/// An app on a scratch workspace with one code tab, whose `tasks` are `tasks`.
fn task_app(tasks: &[(&str, Task)]) -> std::io::Result<(tempfile::TempDir, App, TerminalEvents)> {
    let dir = tempfile::tempdir()?;
    let mut app = app();
    app.root = std::fs::canonicalize(dir.path())?;
    app.push_tab(code_tab("a.rs"));
    app.settings.tasks = tasks
        .iter()
        .map(|(label, task)| ((*label).to_owned(), task.clone()))
        .collect();
    let events = app
        .terminals
        .take_receiver()
        .unwrap_or_else(|| tokio::sync::mpsc::unbounded_channel().1);
    Ok((dir, app, events))
}

fn task(command: &str) -> Task {
    Task {
        command: command.to_owned(),
        problem_matcher: vec![ProblemMatcher::Builtin(BuiltinMatcher::Gcc)],
        ..Task::default()
    }
}

/// Apply terminal events the way the event loop and the painter do (output is
/// acknowledged, as painting it would) until `done` holds or ten seconds pass.
fn pump(app: &mut App, events: &mut TerminalEvents, done: impl Fn(&App) -> bool) -> bool {
    pump_shown(app, events, true, done)
}

/// [`pump`], with the terminals painted (their output acknowledged) only when
/// `shown`, and the throttle's scheduled scans fired as the event loop does.
fn pump_shown(
    app: &mut App,
    events: &mut TerminalEvents,
    shown: bool,
    done: impl Fn(&App) -> bool,
) -> bool {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        app.fire_task_scans(Instant::now());
        while let Ok(event) = events.try_recv() {
            let id = event.id;
            let output = event.kind == TerminalEventKind::Output;
            app.on_terminal_event(event, Instant::now());
            if output && shown {
                for tab in app.all_tabs() {
                    if let TabKind::Terminal(pane) = &tab.kind
                        && pane.id() == id
                    {
                        pane.acknowledge_output();
                    }
                }
            }
        }
        if done(app) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn terminal_titles(app: &App) -> Vec<String> {
    app.all_tabs()
        .filter(|tab| tab.is_terminal())
        .map(|tab| tab.title.clone())
        .collect()
}

fn task_messages(app: &App) -> Vec<String> {
    app.problems
        .entries
        .iter()
        .filter(|entry| entry.diagnostic.source.as_deref() == Some("build"))
        .map(|entry| entry.diagnostic.message.clone())
        .collect()
}

#[test]
fn run_task_with_nothing_configured_says_where_tasks_go() -> TestResult {
    let (_dir, mut app, _events) = task_app(&[])?;
    app.dispatch(Command::RunTask);
    assert!(app.overlay.is_none());
    assert!(
        last_message(&app).is_some_and(|m| m.contains("`tasks`")),
        "{:?}",
        last_message(&app)
    );
    Ok(())
}

#[test]
fn the_picker_lists_tasks_by_label_with_their_commands_beside() -> TestResult {
    let (_dir, mut app, _events) =
        task_app(&[("test", task("make test")), ("build", task("make"))])?;
    app.dispatch(Command::RunTask);
    let overlay = app.overlay.as_ref().ok_or("no picker")?;
    assert_eq!(overlay.title(), RUN_TASK_TITLE);
    assert_eq!(overlay.rows(), ["build", "test"]);
    assert_eq!(
        overlay.row_hints(),
        [Some("make".to_owned()), Some("make test".to_owned())]
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn a_picked_task_runs_in_a_terminal_and_its_problems_reach_the_panel() -> TestResult {
    let mut build = task(
        "mkdir -p src && touch src/main.c && printf '%s\\n' \"$GREETING\" \
         'src/main.c:4:2: error: expected expression' 'src/main.c:9:1: warning: unused'; exit 2",
    );
    build.env.insert("GREETING".to_owned(), "hello".to_owned());
    let (_dir, mut app, mut events) = task_app(&[("build", build)])?;
    app.dispatch(Command::RunTask);
    app.overlay_accept();
    assert_eq!(terminal_titles(&app), ["Task: build"]);

    let finished = pump(&mut app, &mut events, |app| {
        task_messages(app).len() == 2
            && last_message(app).is_some_and(|m| m.contains("exited with code 2"))
    });
    assert!(
        finished,
        "{:?} / {:?}",
        task_messages(&app),
        last_message(&app)
    );
    assert_eq!(task_messages(&app), ["expected expression", "unused"]);
    let first = app.problems.entries.first().ok_or("no entry")?;
    assert_eq!(first.path, app.root.join("src/main.c"));
    assert_eq!(first.diagnostic.range.start, LineCol::new(3, 1));
    assert_eq!(app.problems.counts(), (1, 1), "the status bar counts them");
    assert_eq!(
        last_message(&app).as_deref(),
        Some("task `build` exited with code 2, 2 problems")
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn rerunning_a_finished_task_replaces_its_tab_and_its_problems() -> TestResult {
    let (_dir, mut app, mut events) = task_app(&[(
        "build",
        task("touch m.c; printf 'm.c:1:1: error: run %s\\n' \"$$\""),
    )])?;
    app.run_task("build");
    assert!(pump(&mut app, &mut events, |app| task_messages(app).len() == 1));
    let first = task_messages(&app);
    assert!(pump(&mut app, &mut events, |app| app
        .all_tabs()
        .any(|tab| tab.title.ends_with("(exited)"))));

    app.run_task("build");
    assert_eq!(
        terminal_titles(&app),
        ["Task: build"],
        "the finished run's tab went"
    );
    assert!(pump(&mut app, &mut events, |app| {
        let now = task_messages(app);
        now.len() == 1 && now != first
    }));
    Ok(())
}

#[cfg(unix)]
#[test]
fn running_a_task_that_is_still_running_shows_it_instead() -> TestResult {
    let (_dir, mut app, _events) = task_app(&[("watch", task("sleep 30"))])?;
    app.run_task("watch");
    app.select_tab(0);
    app.run_task("watch");
    assert_eq!(terminal_titles(&app), ["Task: watch"]);
    assert!(app.active_terminal().is_some(), "its tab is shown");
    assert!(
        last_message(&app).is_some_and(|m| m.contains("already running")),
        "{:?}",
        last_message(&app)
    );
    Ok(())
}

#[test]
fn a_task_that_cannot_start_says_why_and_opens_nothing() -> TestResult {
    let bad_matcher = Task {
        problem_matcher: vec![ProblemMatcher::Pattern(
            karet_session::config::schema::PatternMatcher {
                regexp: "(".to_owned(),
                file: 1,
                line: 2,
                column: None,
                severity: None,
                code: None,
                message: None,
                default_severity: karet_session::config::schema::MatcherSeverity::Error,
            },
        )],
        ..task("true")
    };
    let missing_cwd = Task {
        cwd: "no/such/dir".to_owned(),
        ..task("true")
    };
    let (_dir, mut app, _events) = task_app(&[
        ("empty", task("  ")),
        ("matcher", bad_matcher),
        ("cwd", missing_cwd),
    ])?;
    for (label, says) in [
        ("empty", "no `command`"),
        ("matcher", "problem matcher `(`"),
        ("cwd", "no directory"),
        ("gone", "no task named `gone`"),
    ] {
        app.run_task(label);
        assert!(
            last_message(&app).is_some_and(|m| m.contains(says)),
            "{label}: {:?}",
            last_message(&app)
        );
    }
    assert!(terminal_titles(&app).is_empty());
    Ok(())
}

#[cfg(unix)]
#[test]
fn closing_a_task_terminal_keeps_its_problems_until_the_next_run() -> TestResult {
    let (_dir, mut app, mut events) = task_app(&[(
        "build",
        task("touch m.c; echo 'm.c:2:1: error: kept'; sleep 30"),
    )])?;
    app.run_task("build");
    assert!(pump(&mut app, &mut events, |app| task_messages(app).len() == 1));
    let index = app.tabs.iter().position(Tab::is_terminal).ok_or("no tab")?;
    app.close_tab_at(index);
    // The hung-up shell's exit arrives for a terminal no tab shows.
    assert!(pump(&mut app, &mut events, |app| app.tasks.runs.is_empty()));
    assert_eq!(task_messages(&app), ["kept"]);
    Ok(())
}

#[cfg(unix)]
#[test]
fn a_running_task_in_a_background_tab_is_scanned_to_its_last_output() -> TestResult {
    // The second line comes inside the throttle's interval and nothing follows
    // it: only a scheduled scan finds it, and only if the hidden terminal was
    // acknowledged so it could announce it at all.
    let (_dir, mut app, mut events) = task_app(&[(
        "build",
        task(
            "touch m.c; echo 'm.c:1:1: error: one'; sleep 0.1; echo 'm.c:2:1: error: two'; sleep \
             30",
        ),
    )])?;
    app.run_task("build");
    app.select_tab(0);
    assert!(app.active_terminal().is_none(), "the task's tab is hidden");
    let found = pump_shown(&mut app, &mut events, false, |app| {
        task_messages(app).len() == 2
    });
    assert!(found, "{:?}", task_messages(&app));
    assert_eq!(app.task_next_wake(Instant::now()), None, "nothing left due");
    Ok(())
}

#[cfg(unix)]
#[test]
fn throttled_output_schedules_a_scan_for_when_the_interval_is_up() -> TestResult {
    let (_dir, mut app, _events) = task_app(&[("watch", task("sleep 30"))])?;
    app.run_task("watch");
    let id = *app.tasks.runs.keys().next().ok_or("no run")?;
    let start = Instant::now();
    app.task_output(id, start);
    assert_eq!(
        app.task_next_wake(start),
        None,
        "the first batch scans at once"
    );
    app.task_output(id, start + Duration::from_millis(100));
    assert_eq!(
        app.task_next_wake(start),
        Some(running::LIVE_SCAN_INTERVAL),
        "a batch too soon after waits for the interval, not for more output"
    );
    app.fire_task_scans(start + Duration::from_millis(400));
    assert!(app.task_next_wake(start).is_some(), "not due yet");
    app.fire_task_scans(start + running::LIVE_SCAN_INTERVAL);
    assert_eq!(app.task_next_wake(start), None);
    Ok(())
}

#[cfg(unix)]
#[test]
fn the_exit_scan_of_a_hidden_task_counts_its_whole_output() -> TestResult {
    let (_dir, mut app, mut events) = task_app(&[(
        "build",
        task(
            "touch m.c; i=0; while [ $i -lt 300 ]; do i=$((i+1)); echo \"m.c:$i:1: error: e$i\"; done; exit 1",
        ),
    )])?;
    app.run_task("build");
    app.select_tab(0);
    let exited = pump_shown(&mut app, &mut events, false, |app| {
        last_message(app).is_some_and(|m| m.contains("exited"))
    });
    assert!(exited, "{:?}", last_message(&app));
    assert_eq!(
        last_message(&app).as_deref(),
        Some("task `build` exited with code 1, 300 problems"),
        "the exit is announced after the last output is parsed"
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn a_rerun_that_cannot_start_keeps_the_finished_run_and_its_problems() -> TestResult {
    let (_dir, mut app, mut events) =
        task_app(&[("build", task("touch m.c; echo 'm.c:1:1: error: old'"))])?;
    app.run_task("build");
    assert!(pump(&mut app, &mut events, |app| app
        .all_tabs()
        .any(|tab| tab.title.ends_with("(exited)"))));
    // A NUL byte cannot reach `exec`, so the shell never starts.
    let broken = task("echo \0");
    app.settings.tasks.insert("build".to_owned(), broken);
    app.run_task("build");
    assert!(
        last_message(&app).is_some_and(|m| m.starts_with("task `build`:")),
        "{:?}",
        last_message(&app)
    );
    assert_eq!(
        terminal_titles(&app).len(),
        1,
        "the finished run's tab stays"
    );
    assert_eq!(task_messages(&app), ["old"]);
    Ok(())
}

#[cfg(unix)]
#[test]
fn settings_that_drop_a_task_drop_its_problems_and_its_run() -> TestResult {
    let (_dir, mut app, mut events) = task_app(&[(
        "build",
        task("touch m.c; echo 'm.c:1:1: error: gone'; sleep 30"),
    )])?;
    app.run_task("build");
    assert!(pump(&mut app, &mut events, |app| task_messages(app).len() == 1));
    let mut config = app.loaded_config.clone();
    config.settings.tasks.clear();
    app.apply_loaded_config(config, false);
    assert!(task_messages(&app).is_empty());
    assert!(app.tasks.runs.is_empty(), "its terminal is a plain one now");
    assert_eq!(terminal_titles(&app).len(), 1, "and stays open");
    Ok(())
}
