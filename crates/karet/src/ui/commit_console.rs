//! The commit console: a modal view of what a commit's hooks are printing.
//!
//! Deliberately not one of the [`Overlay`](crate::overlay::Overlay) family, which
//! are pickers and forms over a query and a row list. This is a log: it follows
//! its own tail, scrolls in both directions because a linter's line is wider than
//! any modal, and outlives the keystroke that opened it.

use karet_widgets::scroll::draw_scrollable_lines;

use super::*;
use crate::app::commit_console::ConsoleOutcome;

/// Draw the console over `area`, when it is open.
pub(super) fn draw_commit_console(
    f: &mut Frame,
    app: &mut App,
    theme: &Theme,
    area: Rect,
    hits: &mut ScrollHits,
) {
    if !app.commit_console.open {
        app.commit_console.rect = Rect::default();
        return;
    }
    let width = (u32::from(area.width) * 8 / 10).clamp(24, 100) as u16;
    let height = (u32::from(area.height) * 7 / 10).clamp(6, 24) as u16;
    let rect = centered(area, width, height);
    f.render_widget(Clear, rect);

    let (title, title_role) = match &app.commit_console.outcome {
        None if app.commit_console.cancelling => {
            (" Cancelling… ".to_string(), ThemeRole::LineNumberActive)
        },
        None => (" Committing… ".to_string(), ThemeRole::LineNumberActive),
        Some(ConsoleOutcome::Committed(short)) => {
            (format!(" Committed {short} "), ThemeRole::LineNumberActive)
        },
        Some(ConsoleOutcome::Failed(reason)) => (
            format!(" Commit refused — {reason} "),
            ThemeRole::DiagnosticError,
        ),
        // Not an error: the user asked for this, and painting it red would
        // read as something having gone wrong.
        Some(ConsoleOutcome::Cancelled) => (" Commit cancelled ".to_string(), ThemeRole::Muted),
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(Span::styled(title, theme.style(title_role)))
        .style(
            Style::default()
                .bg(theme.role(ThemeRole::Background).to_ratatui())
                .fg(theme.role(ThemeRole::Foreground).to_ratatui()),
        );
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    app.commit_console.rect = rect;
    if inner.height < 2 || inner.width == 0 {
        return;
    }

    let rows = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).split(inner);
    let lines = app.commit_console.styled(theme);
    // Stick to the tail while a commit is still producing output, unless the
    // reader has scrolled away from it. Asking for the last row and letting
    // `draw_scrollable_lines` clamp is what keeps this exact: the visible height
    // is what remains after *it* reserves its own tracks, which is not something
    // to compute twice.
    if app.commit_console.following {
        app.commit_console.scroll = u16::MAX;
    }
    let painted = draw_scrollable_lines(
        f,
        theme,
        rows[0],
        lines,
        &mut app.commit_console.scroll,
        &mut app.commit_console.column,
    );
    hits.record_both(
        painted,
        ScrollSurface::CommitConsole,
        ScrollSurface::CommitConsoleColumns,
    );

    // Hiding the console and stopping the commit are different things, and the
    // hint has to say which is which — dismissing a console that kept running
    // used to be the only option, and was easy to misread as a cancel.
    let hint = if app.commit_console.outcome.is_some() {
        "Esc close"
    } else if app.commit_console.cancelling {
        "Esc hide · stopping the commit…"
    } else {
        "Esc hide · c cancel the commit"
    };
    f.render_widget(
        Paragraph::new(Line::styled(hint, theme.style(ThemeRole::Muted)))
            .alignment(ratatui::layout::Alignment::Right),
        rows[1],
    );
}

#[cfg(test)]
mod tests {
    use karet_vcs::CommitOutputLine;
    use karet_vcs::OutputStream;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;

    /// Draw the console for `lines` and return the painted rows.
    fn draw(lines: &[&str], outcome: Option<ConsoleOutcome>) -> Vec<String> {
        let mut app = crate::app::tests::support::app();
        app.commit_console_reset();
        app.on_commit_output(
            lines
                .iter()
                .map(|text| CommitOutputLine {
                    stream: OutputStream::Stderr,
                    text: (*text).to_string(),
                })
                .collect(),
        );
        if let Some(outcome) = outcome {
            app.commit_console_finished(outcome);
        }
        let theme = app.theme.clone();
        let mut terminal = Terminal::new(TestBackend::new(60, 20))
            .unwrap_or_else(|_| unreachable!("the test backend is infallible"));
        let mut hits = ScrollHits::default();
        let _ = terminal.draw(|frame| {
            let area = frame.area();
            draw_commit_console(frame, &mut app, &theme, area, &mut hits);
        });
        let buffer = terminal.backend().buffer().clone();
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .filter_map(|x| buffer.cell((x, y)).map(|cell| cell.symbol().to_owned()))
                    .collect()
            })
            .collect()
    }

    #[test]
    fn a_running_commit_says_so_and_shows_what_its_hooks_printed() {
        let rows = draw(&["running checks", "all good"], None);
        let painted = rows.join("\n");
        assert!(painted.contains("Committing…"), "{painted}");
        assert!(painted.contains("running checks"), "{painted}");
        assert!(painted.contains("all good"), "{painted}");
        assert!(
            painted.contains("c cancel the commit"),
            "and says how to stop it: {painted}"
        );
    }

    #[test]
    fn a_refusal_names_its_reason_in_the_title() {
        let rows = draw(
            &["why it refused"],
            Some(ConsoleOutcome::Failed("hook failed".to_string())),
        );
        let painted = rows.join("\n");
        assert!(painted.contains("Commit refused"), "{painted}");
        assert!(painted.contains("hook failed"), "{painted}");
        assert!(painted.contains("why it refused"), "{painted}");
    }

    #[test]
    fn a_long_log_follows_its_tail() {
        let lines: Vec<String> = (0..200).map(|n| format!("line {n}")).collect();
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        let painted = draw(&refs, None).join("\n");
        assert!(
            painted.contains("line 199"),
            "the newest line is on screen:\n{painted}"
        );
        assert!(
            painted.contains("line 198"),
            "and the tail is the last full screenful, not a partial one"
        );
        assert!(
            !painted.contains("line 0\n"),
            "the oldest is not: {painted}"
        );
    }

    #[test]
    fn a_closed_console_paints_nothing_and_leaves_no_hit_rect() {
        let mut app = crate::app::tests::support::app();
        app.commit_console_reset();
        let theme = app.theme.clone();
        let mut terminal = Terminal::new(TestBackend::new(40, 10))
            .unwrap_or_else(|_| unreachable!("the test backend is infallible"));
        let mut hits = ScrollHits::default();
        let _ = terminal.draw(|frame| {
            let area = frame.area();
            draw_commit_console(frame, &mut app, &theme, area, &mut hits);
        });
        assert_eq!(app.commit_console.rect, Rect::default());
    }

    #[test]
    fn a_cancelling_commit_says_it_is_stopping_rather_than_that_it_stopped() {
        let mut app = crate::app::tests::support::app();
        app.commit_console_reset();
        app.on_commit_output(vec![CommitOutputLine {
            stream: OutputStream::Stderr,
            text: "working".to_string(),
        }]);
        app.commit_console.cancelling = true;
        let theme = app.theme.clone();
        let mut terminal = Terminal::new(TestBackend::new(60, 20))
            .unwrap_or_else(|_| unreachable!("the test backend is infallible"));
        let mut hits = ScrollHits::default();
        let _ = terminal.draw(|frame| {
            let area = frame.area();
            draw_commit_console(frame, &mut app, &theme, area, &mut hits);
        });
        let buffer = terminal.backend().buffer().clone();
        let painted: String = (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .filter_map(|x| buffer.cell((x, y)).map(|cell| cell.symbol().to_owned()))
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(painted.contains("Cancelling…"), "{painted}");
        assert!(
            !painted.contains("c cancel the commit"),
            "asking twice does nothing, so it is not offered twice: {painted}"
        );
    }

    #[test]
    fn a_cancelled_commit_titles_itself_without_calling_it_a_failure() {
        let rows = draw(&["working"], Some(ConsoleOutcome::Cancelled));
        let painted = rows.join("\n");
        assert!(painted.contains("Commit cancelled"), "{painted}");
        assert!(
            !painted.contains("refused"),
            "a cancellation the user asked for is not a refusal: {painted}"
        );
        assert!(painted.contains("Esc close"), "{painted}");
    }
}
