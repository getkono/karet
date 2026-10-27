    // ---- undoing a save's own rewrites (#283).
    //
    // A save may rewrite the whole buffer (format-on-save, whitespace cleanup).
    // Undoing that rewrite must put the caret back where the client last said
    // it was, and a save that both formats and cleans up is one undo step.

    /// Open `path` with `editor.formatOnSave` on, keeping the snapshot stream.
    fn undo_save_session(
        path: &std::path::Path,
    ) -> Result<(Session, DocumentId, EventRx, SnapshotRx), Box<dyn std::error::Error>> {
        let mut settings = crate::config::Settings::default();
        settings.editor.format_on_save = true;
        let (mut session, mut events, mut snaps) = Session::new(SessionConfig {
            settings,
            ..SessionConfig::default()
        });
        session.handle(
            RequestId(1),
            DocumentCommand::Open {
                path: path.to_path_buf(),
                language: None,
            },
        );
        let doc = opened_doc(&mut events).ok_or("no Opened event")?;
        while snaps.try_recv().is_some() {}
        Ok((session, doc, events, snaps))
    }

    fn caret_at(line: u32, col: u32) -> CursorState {
        CursorState::single(Selection::caret(LineCol::new(line, col)))
    }

    /// Undo, and return the caret head the resulting snapshot carries.
    fn undo_caret(session: &mut Session, snaps: &mut SnapshotRx, doc: DocumentId) -> Option<LineCol> {
        while snaps.try_recv().is_some() {}
        session.handle(RequestId(90), DocumentCommand::Undo { doc });
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
            DocumentCommand::SetCursor {
                doc,
                view: crate::api::ViewId(1),
                cursors,
            },
        );
    }

    fn save_now(session: &mut Session, doc: DocumentId) {
        session.handle(
            RequestId(81),
            DocumentCommand::Save {
                doc,
                cause: SaveCause::Manual,
            },
        );
    }

    #[test]
    fn undoing_a_save_cleanup_restores_the_reported_caret() -> TestResult {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("notes.txt");
        std::fs::write(&path, "alpha  \nbeta\n")?;
        let (mut session, doc, _events, mut snaps) = undo_save_session(&path)?;
        report_cursor(&mut session, doc, caret_at(1, 2));
        save_now(&mut session, doc);
        assert_eq!(std::fs::read_to_string(&path)?, "alpha\nbeta\n");

        assert_eq!(
            undo_caret(&mut session, &mut snaps, doc),
            Some(LineCol::new(1, 2)),
            "undoing the cleanup must not jump the caret to the top of the file"
        );
        assert_eq!(text_of(&session, doc), "alpha  \nbeta\n");
        Ok(())
    }

    #[test]
    fn a_reported_caret_past_the_buffer_is_clamped_into_it() -> TestResult {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("notes.txt");
        std::fs::write(&path, "alpha  \nbeta\n")?;
        let (mut session, doc, _events, mut snaps) = undo_save_session(&path)?;
        report_cursor(&mut session, doc, caret_at(40, 9));
        save_now(&mut session, doc);

        assert_eq!(
            undo_caret(&mut session, &mut snaps, doc),
            Some(LineCol::new(2, 0)),
            "a caret beyond the text lands at its end, never outside it"
        );
        Ok(())
    }

    #[test]
    fn a_caret_reported_before_a_later_edit_is_not_trusted() -> TestResult {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("notes.txt");
        std::fs::write(&path, "alpha\nbeta\n")?;
        let (mut session, doc, _events, mut snaps) = undo_save_session(&path)?;
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
            DocumentCommand::ApplyChange {
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
        Ok(())
    }

    #[test]
    fn a_save_that_formats_and_cleans_up_is_one_undo_step() -> TestResult {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("main.rs");
        std::fs::write(&path, "fn  main() {}\n")?;
        let (mut session, doc, mut events, mut snaps) = undo_save_session(&path)?;
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
        let version = session
            .document(doc)
            .map(|d| d.version())
            .ok_or("opened document is not registered")?;
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
            scope: crate::lsp::FormattingScope::Save,
        });
        let mut saved = false;
        while let Some((_, ev)) = events.try_recv() {
            saved |= matches!(ev, Event::Saved { .. });
        }
        assert!(saved);
        assert_eq!(std::fs::read_to_string(&path)?, "fn main() {}\n");

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
        Ok(())
    }

    #[test]
    fn a_manual_format_is_its_own_undo_step_apart_from_a_later_save() -> TestResult {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("main.rs");
        std::fs::write(&path, "fn  main() {}\n")?;
        // No format-on-save: the save below only cleans up, so the one format
        // is the one asked for by hand.
        let (mut session, mut events, mut snaps) = Session::new(SessionConfig::default());
        session.handle(
            RequestId(1),
            DocumentCommand::Open {
                path: path.clone(),
                language: None,
            },
        );
        let doc = opened_doc(&mut events).ok_or("no Opened event")?;
        let version = session
            .document(doc)
            .map(|d| d.version())
            .ok_or("opened document is not registered")?;
        // Format Document answers with an edit that leaves trailing whitespace,
        // so the save after it has its own cleanup to make.
        session.apply_lsp_update(crate::lsp::LspUpdate::Formatting {
            generation: 0,
            request: RequestId(2),
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
            scope: crate::lsp::FormattingScope::Document,
        });
        assert_eq!(text_of(&session, doc), "fn main() {}   \n");
        save_now(&mut session, doc);
        assert_eq!(std::fs::read_to_string(&path)?, "fn main() {}\n");

        let _ = undo_caret(&mut session, &mut snaps, doc);
        assert_eq!(
            text_of(&session, doc),
            "fn main() {}   \n",
            "the first undo reverts only the save's cleanup, not the format before it"
        );
        let _ = undo_caret(&mut session, &mut snaps, doc);
        assert_eq!(
            text_of(&session, doc),
            "fn  main() {}\n",
            "the next undo reverts the manual format as one step"
        );
        Ok(())
    }

    // ---- undo/redo restore the client's whole selection set (#331).
    //
    // The client reports its selection set right before an edit and right
    // after it. Undo puts back the first, redo the second -- every caret and
    // every selection, not one bare caret at the edit.

    /// Undo (`undo`) or redo, and return the cursor the resulting snapshot carries.
    fn history_cursor(
        session: &mut Session,
        snaps: &mut SnapshotRx,
        doc: DocumentId,
        undo: bool,
    ) -> Option<CursorState> {
        while snaps.try_recv().is_some() {}
        let command = if undo {
            DocumentCommand::Undo { doc }
        } else {
            DocumentCommand::Redo { doc }
        };
        session.handle(RequestId(91), command);
        let mut last = None;
        while let Some((_, snap)) = snaps.try_recv() {
            last = Some(snap);
        }
        last.and_then(|snap| snap.cursor.clone())
    }

    /// Report `before`, apply `edits` at the current version, then report `after`,
    /// the way the client wraps every edit.
    fn reported_edit(
        session: &mut Session,
        doc: DocumentId,
        before: CursorState,
        edits: Vec<TextEdit>,
        after: CursorState,
    ) -> TestResult {
        report_cursor(session, doc, before);
        let version = session
            .document(doc)
            .map(|d| d.version())
            .ok_or("edited document is not registered")?;
        session.handle(
            RequestId(2),
            DocumentCommand::ApplyChange {
                doc,
                change: Change::new(version, edits),
                cause: EditCause::Replace,
            },
        );
        report_cursor(session, doc, after);
        Ok(())
    }

    fn replace(line: u32, from: u32, to: u32, text: &str) -> TextEdit {
        TextEdit {
            range: Range {
                start: LineCol::new(line, from),
                end: LineCol::new(line, to),
            },
            new_text: text.to_string(),
        }
    }

    #[test]
    fn undo_and_redo_restore_every_selection_around_the_edit() -> TestResult {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("notes.txt");
        std::fs::write(&path, "alpha\nbeta\n")?;
        let (mut session, doc, _events, mut snaps) = undo_save_session(&path)?;
        // Two selections, the second primary, typed over with `x`.
        let before = CursorState {
            selections: vec![
                Selection {
                    anchor: LineCol::new(0, 1),
                    head: LineCol::new(0, 3),
                },
                Selection {
                    anchor: LineCol::new(1, 2),
                    head: LineCol::new(1, 0),
                },
            ],
            primary: 1,
        };
        let after = CursorState {
            selections: vec![
                Selection::caret(LineCol::new(0, 2)),
                Selection::caret(LineCol::new(1, 1)),
            ],
            primary: 1,
        };
        reported_edit(
            &mut session,
            doc,
            before.clone(),
            vec![replace(0, 1, 3, "x"), replace(1, 0, 2, "x")],
            after.clone(),
        )?;
        assert_eq!(text_of(&session, doc), "axha\nxta\n");

        assert_eq!(
            history_cursor(&mut session, &mut snaps, doc, true),
            Some(before),
            "undo restores both selections, their directions, and the primary"
        );
        assert_eq!(text_of(&session, doc), "alpha\nbeta\n");
        assert_eq!(
            history_cursor(&mut session, &mut snaps, doc, false),
            Some(after),
            "redo restores the carets the edit left"
        );
        Ok(())
    }

    #[test]
    fn undoing_a_backspace_puts_the_caret_back_after_the_character() -> TestResult {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("notes.txt");
        std::fs::write(&path, "hello\n")?;
        let (mut session, doc, _events, mut snaps) = undo_save_session(&path)?;
        reported_edit(
            &mut session,
            doc,
            caret_at(0, 5),
            vec![replace(0, 4, 5, "")],
            caret_at(0, 4),
        )?;
        assert_eq!(
            history_cursor(&mut session, &mut snaps, doc, true),
            Some(caret_at(0, 5)),
            "the caret was after the `o`, not at the start of the deleted range"
        );
        Ok(())
    }

    #[test]
    fn a_caret_moved_after_the_edit_is_not_what_redo_restores() -> TestResult {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("notes.txt");
        std::fs::write(&path, "hello\n")?;
        let (mut session, doc, _events, mut snaps) = undo_save_session(&path)?;
        reported_edit(
            &mut session,
            doc,
            caret_at(0, 5),
            vec![replace(0, 5, 5, "!")],
            caret_at(0, 6),
        )?;
        // A later report at the same version (e.g. before a save) is a move.
        report_cursor(&mut session, doc, caret_at(0, 0));
        let _ = history_cursor(&mut session, &mut snaps, doc, true);
        // A report made after the undo describes the undone text, not the edit.
        report_cursor(&mut session, doc, caret_at(0, 2));
        assert_eq!(
            history_cursor(&mut session, &mut snaps, doc, false),
            Some(caret_at(0, 6))
        );
        Ok(())
    }
