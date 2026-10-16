//! What a request is answered with when no live connection can answer it:
//! every request is answered, and what the answer says depends on whether the
//! server is reconnecting or gone for good.

use super::*;
use crate::DocumentId;
use crate::RequestId;
use crate::lsp::CodeLensAnswer;
use crate::lsp::CodeLensCmd;

fn lens_listing() -> ServerCmd {
    ServerCmd::CodeLens(CodeLensCmd::List {
        request: RequestId(4),
        doc: DocumentId(5),
        version: 6,
        path: PathBuf::from("main.rs"),
        lines: (0, 40),
    })
}

/// A server gone for good has no lenses, so the editor may clear them; one
/// that is reconnecting has said nothing, so the lenses on screen stand.
#[test]
fn a_lens_listing_is_cleared_only_by_a_server_that_is_gone() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    answer_empty(&tx, lens_listing(), 7);
    answer_reconnecting(&tx, lens_listing(), 7);
    let lenses: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok())
        .filter_map(|update| match update {
            LspUpdate::CodeLens {
                generation: 7,
                answer:
                    CodeLensAnswer::Listed {
                        request: RequestId(4),
                        lenses,
                        ..
                    },
            } => Some(lenses),
            _ => None,
        })
        .collect();
    assert_eq!(lenses, [Some(Vec::new()), None]);
}

/// A lens run asked of no connection fails with the reason, under the
/// lens's title, whichever way the server is away.
#[test]
fn an_unanswerable_lens_run_fails_under_its_title() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let run = || {
        ServerCmd::CodeLens(CodeLensCmd::Run {
            request: RequestId(8),
            lens: Box::new(karet_core::CodeLens {
                title: Some("Run test".into()),
                ..karet_core::CodeLens::default()
            }),
        })
    };
    answer_empty(&tx, run(), 7);
    answer_reconnecting(&tx, run(), 7);
    for _ in 0..2 {
        assert!(matches!(
            rx.try_recv(),
            Ok(LspUpdate::CodeLens {
                answer: CodeLensAnswer::Ran {
                    request: RequestId(8),
                    ref title,
                    result: Err(ref reason),
                },
                ..
            }) if title == "Run test" && reason == UNAVAILABLE
        ));
    }
}

/// With no connection, nothing formatted the file — and the session has to
/// hear exactly that, because it is what sends it to its own formatter.
///
/// Reporting the server's *capability* here instead answered a question this
/// path cannot know the answer to, and the session read it as "a server
/// handled this". TOML then lost its built-in taplo pass for as long as the
/// server was away: a whole 300s circuit-breaker cooldown, a retry backoff,
/// or the rest of the session for a server that never launched.
#[test]
fn an_unanswerable_formatting_request_reports_that_nothing_formatted() {
    let (tx, mut rx) = mpsc::unbounded_channel();

    answer_empty(
        &tx,
        ServerCmd::Formatting {
            request: RequestId(1),
            doc: DocumentId(2),
            version: 3,
            path: PathBuf::from("Cargo.toml"),
            ask: crate::lsp::FormattingAsk::default(),
        },
        7,
    );

    assert!(
        matches!(
            rx.try_recv(),
            Ok(LspUpdate::Formatting {
                generation: 7,
                request: RequestId(1),
                doc: DocumentId(2),
                version: 3,
                formatted: false,
                ref edits,
                scope: crate::lsp::FormattingScope::Save,
            }) if edits.is_empty()
        ),
        "an unreachable server must not be reported as having formatted the file"
    );

    // A format asked for by hand is answered as one, so its empty answer
    // is never mistaken for a save's.
    answer_empty(
        &tx,
        ServerCmd::Formatting {
            request: RequestId(4),
            doc: DocumentId(2),
            version: 3,
            path: PathBuf::from("Cargo.toml"),
            ask: crate::lsp::FormattingAsk {
                scope: crate::lsp::FormattingScope::Document,
                ..crate::lsp::FormattingAsk::default()
            },
        },
        7,
    );
    assert!(matches!(
        rx.try_recv(),
        Ok(LspUpdate::Formatting {
            request: RequestId(4),
            formatted: false,
            scope: crate::lsp::FormattingScope::Document,
            ..
        })
    ));
}

/// A rename asked of no connection still answers, both halves of it, so a
/// client holding an open rename prompt is never left waiting.
#[test]
fn unanswerable_rename_requests_still_answer() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    answer_empty(
        &tx,
        ServerCmd::PrepareRename {
            request: RequestId(1),
            doc: DocumentId(2),
            version: 3,
            path: PathBuf::from("main.rs"),
            position: karet_core::LineCol::new(0, 0),
            caret: karet_core::LineCol::new(0, 0),
        },
        7,
    );
    assert!(matches!(
        rx.try_recv(),
        Ok(LspUpdate::PrepareRename {
            generation: 7,
            request: RequestId(1),
            doc: DocumentId(2),
            version: 3,
            answer: RenameAnswer::NoServer,
            ..
        })
    ));
    answer_empty(
        &tx,
        ServerCmd::Rename {
            request: RequestId(4),
            doc: DocumentId(2),
            version: 3,
            path: PathBuf::from("main.rs"),
            position: karet_core::LineCol::new(0, 0),
            new_name: "renamed".into(),
        },
        7,
    );
    assert!(matches!(
        rx.try_recv(),
        Ok(LspUpdate::WorkspaceEdit {
            request: RequestId(4),
            edit: Ok(ref edit),
            ..
        }) if edit.is_empty()
    ));
}

/// A code-action step asked of no connection answers with the reason,
/// through the same path as every other request.
#[test]
fn an_unanswerable_code_action_step_still_answers() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    answer_empty(
        &tx,
        ServerCmd::CodeAction(super::super::CodeActionCmd::List {
            request: RequestId(1),
            doc: DocumentId(2),
            version: 3,
            path: PathBuf::from("main.rs"),
            range: karet_core::Range::default(),
            kinds: Vec::new(),
            trigger: crate::api::CodeActionTrigger::Automatic,
        }),
        7,
    );
    assert!(matches!(
        rx.try_recv(),
        Ok(LspUpdate::CodeAction {
            generation: 7,
            answer: super::super::CodeActionAnswer::Listed {
                request: RequestId(1),
                actions: Err(ref reason),
                ..
            },
        }) if reason == UNAVAILABLE
    ));
}

fn hint_request() -> ServerCmd {
    ServerCmd::InlayHints {
        request: RequestId(4),
        doc: DocumentId(5),
        version: 6,
        path: PathBuf::from("main.rs"),
        range: karet_core::Range::default(),
    }
}

/// A server that is gone for good has no hints: the editor may clear them
/// and stop asking.
#[test]
fn a_server_that_is_gone_answers_a_hint_request_with_none() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    answer_empty(&tx, hint_request(), 7);
    assert!(matches!(
        rx.try_recv(),
        Ok(LspUpdate::InlayHints { ref hints, .. }) if hints.is_empty()
    ));
}

/// A server that is reconnecting has said nothing about the document, so
/// the hints on screen must survive the back-off.
#[test]
fn a_reconnecting_server_reports_a_hint_request_unanswered() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    answer_reconnecting(&tx, hint_request(), 7);
    assert!(matches!(
        rx.try_recv(),
        Ok(LspUpdate::InlayHintsFailed {
            generation: 7,
            request: RequestId(4),
            doc: DocumentId(5),
            version: 6,
        })
    ));
}

/// A resolve no server can serve is answered with the hint as it was, so
/// the editor stops waiting on it and spends its payload.
#[test]
fn an_unserved_resolve_hands_the_hint_back_unchanged() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let hint = karet_core::InlayHint {
        payload: Some("{}".to_owned()),
        ..karet_core::InlayHint::new(LineCol::new(1, 2), ": i32", karet_core::InlayHintKind::Type)
    };
    answer_reconnecting(
        &tx,
        ServerCmd::ResolveInlayHint {
            request: RequestId(3),
            doc: DocumentId(5),
            hint: hint.clone(),
        },
        7,
    );
    assert!(matches!(
        rx.try_recv(),
        Ok(LspUpdate::InlayHintResolved {
            generation: 7,
            request: RequestId(3),
            doc: DocumentId(5),
            hint: ref answered,
        }) if *answered == hint
    ));
}

/// Everything but a hint request is answered exactly as [`answer_empty`]
/// answers it -- formatting included, which must still fall back.
#[test]
fn a_reconnecting_server_answers_other_requests_empty() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    answer_reconnecting(
        &tx,
        ServerCmd::Formatting {
            request: RequestId(1),
            doc: DocumentId(2),
            version: 3,
            path: PathBuf::from("Cargo.toml"),
            ask: crate::lsp::FormattingAsk::default(),
        },
        7,
    );
    assert!(matches!(
        rx.try_recv(),
        Ok(LspUpdate::Formatting {
            formatted: false,
            ..
        })
    ));
}

/// With no connection, every caret still gets its (empty) chain, so the
/// client can line the answer up with its carets and fall back on each.
#[test]
fn an_unanswerable_selection_range_request_answers_an_empty_chain_per_caret() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let positions = vec![LineCol::new(0, 1), LineCol::new(2, 3)];
    answer_empty(
        &tx,
        ServerCmd::SelectionRanges {
            request: RequestId(1),
            doc: DocumentId(2),
            version: 3,
            path: PathBuf::from("main.rs"),
            positions,
        },
        7,
    );
    let expected = SelectionRangesAnswer {
        request: RequestId(1),
        doc: DocumentId(2),
        version: 3,
        chains: vec![Vec::new(), Vec::new()],
    };
    assert!(matches!(
        rx.try_recv(),
        Ok(LspUpdate::SelectionRanges { generation: 7, answer }) if answer == expected
    ));
}
