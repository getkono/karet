//! Occurrence highlighting: a caret resting on a word asks for the symbol's
//! document highlights once, paints them (writes apart from reads), falls back
//! to whole-word text matches when no server answers, and takes the paint down
//! when the caret leaves every occurrence, the text changes, or a selection
//! is made.

use karet_core::DocumentHighlight;
use karet_core::HighlightKind;
use karet_core::Range;
use karet_session::LSP_CHANGE_DEBOUNCE;

use super::support::*;
use crate::app::*;

const SOURCE: &str = "let value = 1;\nvalue += values;\nValue(value)\n";
const DOC: DocumentId = DocumentId(9);

/// Every `DocumentHighlights` request a backend received.
fn asks(backend: &RecordingBackend) -> Vec<(RequestId, LineCol)> {
    backend
        .sent
        .lock()
        .map(|sent| {
            sent.iter()
                .filter_map(|(id, command)| match command {
                    SessionCommand::Lsp(LspCommand::DocumentHighlights { position, .. }) => {
                        Some((*id, *position))
                    },
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

fn version(app: &App) -> u64 {
    match &app.tabs[app.active].kind {
        TabKind::Code { buffer, .. } => buffer.version(),
        _ => panic!("expected a code tab"),
    }
}

fn span(line: u32, start: u32, end: u32) -> Range {
    Range {
        start: LineCol::new(line, start),
        end: LineCol::new(line, end),
    }
}

fn answer(app: &mut App, id: RequestId, highlights: Option<Vec<DocumentHighlight>>) {
    let version = version(app);
    app.on_backend_event(
        Some(id),
        SessionEvent::DocumentHighlights {
            doc: DOC,
            version,
            highlights,
        },
    );
}

/// The painted occurrences, as `(range, role)`.
fn painted(app: &App) -> Vec<(Range, Option<ThemeRole>)> {
    app.occurrence_decorations()
        .into_iter()
        .inspect(|deco| assert_eq!(deco.kind, DecorationKind::TextBackground))
        .map(|deco| (deco.range, deco.role))
        .collect()
}

/// Settle the caret: one probe to start the debounce, one once it has run.
fn settle(app: &mut App, start: Instant) -> Instant {
    app.probe_occurrences(start);
    let settled = start + LSP_CHANGE_DEBOUNCE;
    app.probe_occurrences(settled);
    settled
}

fn move_caret(app: &mut App, at: LineCol) {
    let active = app.active;
    app.tabs[active].editor.set_carets(&[at]);
}

fn select(app: &mut App, anchor: LineCol, head: LineCol) {
    let tab = &mut app.tabs[app.active];
    if let TabKind::Code { buffer, .. } = &tab.kind {
        tab.editor.set_selection(buffer, anchor, head);
    }
}

#[test]
fn the_request_waits_for_the_caret_to_settle_and_goes_once() {
    let (backend, mut app) = completion_app(SOURCE, LineCol::new(0, 6));
    let start = Instant::now();
    app.probe_occurrences(start);
    assert!(asks(&backend).is_empty(), "not while the caret moves");
    assert_eq!(app.occurrence_next_wake(start), Some(LSP_CHANGE_DEBOUNCE));

    let settled = start + LSP_CHANGE_DEBOUNCE;
    app.probe_occurrences(settled);
    let positions: Vec<LineCol> = asks(&backend).into_iter().map(|(_, at)| at).collect();
    assert_eq!(positions, [LineCol::new(0, 6)]);
    assert_eq!(app.occurrence_next_wake(settled), None, "sent once");
    app.probe_occurrences(settled + LSP_CHANGE_DEBOUNCE);
    assert_eq!(asks(&backend).len(), 1, "not re-sent while it stands");
}

#[test]
fn the_servers_occurrences_are_painted_writes_apart_from_reads() {
    let (backend, mut app) = completion_app(SOURCE, LineCol::new(0, 6));
    settle(&mut app, Instant::now());
    let (id, _) = asks(&backend)[0];
    let found = vec![
        DocumentHighlight {
            range: span(0, 4, 9),
            kind: HighlightKind::Write,
        },
        DocumentHighlight {
            range: span(1, 0, 5),
            kind: HighlightKind::Read,
        },
        DocumentHighlight {
            range: span(2, 6, 11),
            kind: HighlightKind::Text,
        },
    ];
    answer(&mut app, id, Some(found));
    assert_eq!(
        painted(&app),
        vec![
            (span(0, 4, 9), Some(ThemeRole::WordHighlightStrong)),
            (span(1, 0, 5), Some(ThemeRole::WordHighlight)),
            (span(2, 6, 11), Some(ThemeRole::WordHighlight)),
        ]
    );
}

/// No server answered: the word is matched as text, whole words only and
/// case-sensitively, so `values` and `Value` are not occurrences of `value`.
#[test]
fn with_no_server_answer_the_word_is_matched_as_text() {
    let (backend, mut app) = completion_app(SOURCE, LineCol::new(1, 2));
    settle(&mut app, Instant::now());
    let (id, _) = asks(&backend)[0];
    answer(&mut app, id, None);
    let role = Some(ThemeRole::WordHighlight);
    assert_eq!(
        painted(&app),
        vec![
            (span(0, 4, 9), role),
            (span(1, 0, 5), role),
            (span(2, 6, 11), role),
        ]
    );
}

#[test]
fn a_caret_at_a_words_end_still_takes_the_word() {
    let (backend, mut app) = completion_app(SOURCE, LineCol::new(0, 9));
    settle(&mut app, Instant::now());
    let (id, _) = asks(&backend)[0];
    answer(&mut app, id, None);
    assert_eq!(painted(&app).len(), 3);
}

#[test]
fn a_server_that_finds_nothing_paints_nothing() {
    let (backend, mut app) = completion_app(SOURCE, LineCol::new(0, 6));
    settle(&mut app, Instant::now());
    let (id, _) = asks(&backend)[0];
    answer(&mut app, id, Some(Vec::new()));
    assert!(painted(&app).is_empty(), "no text fallback over a server");
}

#[test]
fn without_a_backend_the_text_fallback_is_painted_at_once() {
    let (_, mut app) = completion_app(SOURCE, LineCol::new(0, 6));
    app.backend = None;
    settle(&mut app, Instant::now());
    assert_eq!(painted(&app).len(), 3);
    assert_eq!(app.occurrence_next_wake(Instant::now()), None);
}

#[test]
fn off_a_word_or_over_a_selection_nothing_is_asked() {
    let (backend, mut app) = completion_app(SOURCE, LineCol::new(0, 10));
    let start = Instant::now();
    settle(&mut app, start);
    assert!(asks(&backend).is_empty(), "the caret is on whitespace");
    assert_eq!(app.occurrence_next_wake(start), None);

    move_caret(&mut app, LineCol::new(0, 6));
    select(&mut app, LineCol::new(0, 4), LineCol::new(0, 9));
    settle(&mut app, start);
    assert!(asks(&backend).is_empty(), "a selection asks for nothing");
}

#[test]
fn moving_within_an_occurrence_keeps_the_paint_and_off_it_clears_it() {
    let (backend, mut app) = completion_app(SOURCE, LineCol::new(0, 6));
    let start = Instant::now();
    let settled = settle(&mut app, start);
    let (id, _) = asks(&backend)[0];
    answer(&mut app, id, None);

    // Onto another occurrence: the paint stays until the next answer...
    move_caret(&mut app, LineCol::new(1, 3));
    app.probe_occurrences(settled);
    assert_eq!(painted(&app).len(), 3);
    // ...and a selection takes it down at once.
    select(&mut app, LineCol::new(1, 0), LineCol::new(1, 5));
    app.probe_occurrences(settled);
    assert!(painted(&app).is_empty());

    // Off every occurrence, it goes too.
    move_caret(&mut app, LineCol::new(0, 6));
    let settled = settle(&mut app, settled);
    let (id, _) = *asks(&backend).last().expect("a request");
    answer(&mut app, id, None);
    assert_eq!(painted(&app).len(), 3);
    move_caret(&mut app, LineCol::new(0, 1));
    app.probe_occurrences(settled);
    assert!(painted(&app).is_empty());
}

#[test]
fn an_edit_takes_the_paint_down() {
    let (backend, mut app) = completion_app(SOURCE, LineCol::new(0, 6));
    let settled = settle(&mut app, Instant::now());
    let (id, _) = asks(&backend)[0];
    answer(&mut app, id, None);
    assert_eq!(painted(&app).len(), 3);

    app.dispatch(Command::InsertChar('x'));
    assert!(painted(&app).is_empty(), "not drawn over changed text");
    app.probe_occurrences(settled);
    assert!(app.occurrences.shown.is_none());
}

#[test]
fn an_answer_for_a_caret_that_moved_on_is_dropped() {
    let (backend, mut app) = completion_app(SOURCE, LineCol::new(0, 6));
    let settled = settle(&mut app, Instant::now());
    let (id, _) = asks(&backend)[0];

    move_caret(&mut app, LineCol::new(2, 1));
    app.probe_occurrences(settled);
    answer(&mut app, id, None);
    assert!(painted(&app).is_empty(), "the answer is about another word");

    // An answer to a request that was never the probe's is dropped too.
    settle(&mut app, settled);
    answer(&mut app, RequestId(77), None);
    assert!(painted(&app).is_empty());
    let (id, at) = *asks(&backend).last().expect("a request");
    assert_eq!(at, LineCol::new(2, 1));
    answer(&mut app, id, None);
    assert_eq!(
        painted(&app),
        vec![(span(2, 0, 5), Some(ThemeRole::WordHighlight))]
    );
}

#[test]
fn the_occurrences_are_drawn_behind_the_text() {
    let (backend, mut app) = completion_app(SOURCE, LineCol::new(1, 2));
    settle(&mut app, Instant::now());
    let (id, _) = asks(&backend)[0];
    let found = vec![DocumentHighlight {
        range: span(0, 4, 9),
        kind: HighlightKind::Write,
    }];
    answer(&mut app, id, Some(found));

    let buffer = frame(&mut app, 80, 20);
    let want = app.theme.role(ThemeRole::WordHighlightStrong).to_ratatui();
    // The cell where `let value` is drawn, found cell by cell: the gutter's
    // glyphs make byte offsets in a joined row useless as columns.
    let spells = |x: u16, y: u16| {
        "let value"
            .chars()
            .zip(x..)
            .all(|(c, x)| x < 80 && buffer[(x, y)].symbol() == c.to_string())
    };
    let (col, row) = (0..20)
        .flat_map(|y| (0..80).map(move |x| (x, y)))
        .find(|&(x, y)| spells(x, y))
        .map(|(x, y)| (x + 4, y))
        .expect("the first line is drawn");
    assert_eq!(buffer[(col, row)].bg, want, "the occurrence is painted");
    assert_ne!(buffer[(col - 1, row)].bg, want, "the space before is not");
}
