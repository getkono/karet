    #[test]
    fn missing_document_opens_empty_and_explicit_save_creates_it() -> TestResult {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("README.md");
        let (mut session, mut events, _snaps) = Session::new(SessionConfig::default());

        session.handle(
            RequestId(1),
            Command::OpenDocument {
                path: path.clone(),
                language: None,
            },
        );
        let doc = opened_doc(&mut events).ok_or("a missing file must still open")?;
        assert!(
            session
                .document(doc)
                .is_some_and(|view| view.buffer().text().is_empty())
        );
        assert!(!path.exists());

        session.handle(RequestId(2), Command::Save {
                doc,
                cause: SaveCause::Manual,
            });

        let mut saved = false;
        while let Some((_, event)) = events.try_recv() {
            saved |= matches!(event, Event::Saved { doc: candidate } if candidate == doc);
        }
        assert!(saved);
        assert_eq!(std::fs::read(&path)?, Vec::<u8>::new());
        Ok(())
    }

    #[test]
    fn first_save_of_missing_document_never_clobbers_a_late_creator() -> TestResult {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("README.md");
        let (mut session, mut events, _snaps) = Session::new(SessionConfig::default());
        session.handle(
            RequestId(1),
            Command::OpenDocument {
                path: path.clone(),
                language: None,
            },
        );
        let doc = opened_doc(&mut events).ok_or("a missing file must still open")?;
        session.handle(
            RequestId(2),
            Command::ApplyChange {
                doc,
                change: Change::new(
                    0,
                    vec![TextEdit {
                        range: Range::default(),
                        new_text: "mine\n".to_string(),
                    }],
                ),
                cause: EditCause::Replace,
            },
        );
        while events.try_recv().is_some() {}
        std::fs::write(&path, "external\n")?;

        session.handle(RequestId(3), Command::Save {
                doc,
                cause: SaveCause::Manual,
            });

        let mut conflict = false;
        while let Some((_, event)) = events.try_recv() {
            conflict |= matches!(event, Event::ExternalConflict { doc: candidate } if candidate == doc);
        }
        assert!(conflict);
        assert_eq!(std::fs::read_to_string(&path)?, "external\n");
        assert!(session.document(doc).is_some_and(|view| view.buffer().is_dirty()));
        Ok(())
    }
