    // ---- undoing a save's own rewrites (#283).
    //
    // A save may rewrite the whole buffer (format-on-save, whitespace cleanup).
    // Undoing that rewrite must put the caret back where the client last said
    // it was, and a save that both formats and cleans up is one undo step.

    /// Open `path` with `editor.formatOnSave` on, keeping the snapshot stream.
    fn undo_save_session(
        path: &std::path::Path,
    ) -> Option<(Session, DocumentId, EventRx, SnapshotRx)> {
        let mut settings = crate::config::Settings::default();
        settings.editor.format_on_save = true;
        let (mut session, mut events, mut snaps) = Session::new(SessionConfig {
            settings,
            ..SessionConfig::default()
        });
        session.handle(
            RequestId(1),
            Command::OpenDocument {
                path: path.to_path_buf(),
                language: None,
            },
        );
        let doc = opened_doc(&mut events)?;
        while snaps.try_recv().is_some() {}
        Some((session, doc, events, snaps))
    }

    fn caret_at(line: u32, col: u32) -> CursorState {
        CursorState::single(Selection::caret(LineCol::new(line, col)))
    }

    /// Undo, and return the caret head the resulting snapshot carries.
    fn undo_caret(session: &mut Session, snaps: &mut SnapshotRx, doc: DocumentId) -> Option<LineCol> {
        while snaps.try_recv().is_some() {}
        session.handle(RequestId(90), Command::Undo { doc });
        let mut last = None;
        while let Some((_, snap)) = snaps.try_recv() {
            last = Some(snap);
        }
        last.and_then(|snap| snap.cursor.as_ref().map(|c| c.primary().head))
    }

    fn text_of(session: &Session, doc: DocumentId) -> String {
        session
            .document(doc)
            .map(|d| d.buffer().text())
            .unwrap_or_default()
    }

    fn report_cursor(session: &mut Session, doc: DocumentId, cursors: CursorState) {
        session.handle(
            RequestId(80),
            Command::SetCursor {
                doc,
                view: crate::api::ViewId(1),
                cursors,
            },
        );
    }

    fn save_now(session: &mut Session, doc: DocumentId) {
        session.handle(
            RequestId(81),
            Command::Save {
                doc,
                cause: SaveCause::Manual,
            },
        );
    }

    #[test]
    fn undoing_a_save_cleanup_restores_the_reported_caret() {
        let Ok(dir) = tempfile::tempdir() else {
            return;
        };
        let path = dir.path().join("notes.txt");
        if std::fs::write(&path, "alpha  \nbeta\n").is_err() {
            return;
        }
        let Some((mut session, doc, _events, mut snaps)) = undo_save_session(&path) else {
            return;
        };
        report_cursor(&mut session, doc, caret_at(1, 2));
        save_now(&mut session, doc);
        assert_eq!(std::fs::read_to_string(&path).unwrap_or_default(), "alpha\nbeta\n");

        assert_eq!(
            undo_caret(&mut session, &mut snaps, doc),
            Some(LineCol::new(1, 2)),
            "undoing the cleanup must not jump the caret to the top of the file"
        );
        assert_eq!(text_of(&session, doc), "alpha  \nbeta\n");
    }

    #[test]
    fn a_reported_caret_past_the_buffer_is_clamped_into_it() {
        let Ok(dir) = tempfile::tempdir() else {
            return;
        };
        let path = dir.path().join("notes.txt");
        if std::fs::write(&path, "alpha  \nbeta\n").is_err() {
            return;
        }
        let Some((mut session, doc, _events, mut snaps)) = undo_save_session(&path) else {
            return;
        };
        report_cursor(&mut session, doc, caret_at(40, 9));
        save_now(&mut session, doc);

        assert_eq!(
            undo_caret(&mut session, &mut snaps, doc),
            Some(LineCol::new(2, 0)),
            "a caret beyond the text lands at its end, never outside it"
        );
    }

    #[test]
    fn a_caret_reported_before_a_later_edit_is_not_trusted() {
        let Ok(dir) = tempfile::tempdir() else {
            return;
        };
        let path = dir.path().join("notes.txt");
        if std::fs::write(&path, "alpha\nbeta\n").is_err() {
            return;
        }
        let Some((mut session, doc, _events, mut snaps)) = undo_save_session(&path) else {
            return;
        };
        report_cursor(&mut session, doc, caret_at(1, 2));
        // An edit after the report: it no longer says where the user is.
        let change = Change::new(
            0,
            vec![TextEdit {
                range: Range {
                    start: LineCol::new(0, 5),
                    end: LineCol::new(0, 5),
                },
                new_text: "  ".to_string(),
            }],
        );
        session.handle(
            RequestId(2),
            Command::ApplyChange {
                doc,
                change,
                cause: EditCause::Replace,
            },
        );
        save_now(&mut session, doc);

        assert_eq!(
            undo_caret(&mut session, &mut snaps, doc),
            Some(LineCol::new(0, 0)),
            "a stale report falls back to the rewrite's own start"
        );
    }

    #[test]
    fn a_save_that_formats_and_cleans_up_is_one_undo_step() {
        let Ok(dir) = tempfile::tempdir() else {
            return;
        };
        let path = dir.path().join("main.rs");
        if std::fs::write(&path, "fn  main() {}\n").is_err() {
            return;
        }
        let Some((mut session, doc, mut events, mut snaps)) = undo_save_session(&path) else {
            return;
        };
        report_cursor(&mut session, doc, caret_at(0, 7));
        // Park the save on a formatting request, as a live server would.
        let request = RequestId(3);
        session.pending_format_saves.insert(
            request,
            crate::session::PendingFormatSave {
                doc,
                issued_ms: 0,
            },
        );
        let version = session.document(doc).map(|d| d.version()).unwrap_or(0);
        // The formatter answers with one whole-document edit that leaves
        // trailing whitespace, so the save's cleanup rewrites it again.
        session.apply_lsp_update(crate::lsp::LspUpdate::Formatting {
            generation: 0,
            request,
            doc,
            version,
            formatted: true,
            edits: vec![TextEdit {
                range: Range {
                    start: LineCol::new(0, 0),
                    end: LineCol::new(1, 0),
                },
                new_text: "fn main() {}   \n".to_string(),
            }],
        });
        let mut saved = false;
        while let Some((_, ev)) = events.try_recv() {
            saved |= matches!(ev, Event::Saved { .. });
        }
        assert!(saved);
        assert_eq!(std::fs::read_to_string(&path).unwrap_or_default(), "fn main() {}\n");

        assert_eq!(
            undo_caret(&mut session, &mut snaps, doc),
            Some(LineCol::new(0, 7)),
            "the save's one undo step restores the caret from before it"
        );
        assert_eq!(
            text_of(&session, doc),
            "fn  main() {}\n",
            "one undo reverts both the formatting and the cleanup"
        );
    }
