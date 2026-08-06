    #[test]
    fn missing_document_opens_empty_and_explicit_save_creates_it() {
        let Ok(dir) = tempfile::tempdir() else {
            return;
        };
        let path = dir.path().join("README.md");
        let (mut session, mut events, _snaps) = Session::new(SessionConfig::default());

        session.handle(
            RequestId(1),
            Command::OpenDocument {
                path: path.clone(),
                language: None,
            },
        );
        let Some(doc) = opened_doc(&mut events) else {
            return;
        };
        assert!(
            session
                .document(doc)
                .is_some_and(|view| view.buffer().text().is_empty())
        );
        assert!(!path.exists());

        session.handle(RequestId(2), Command::Save { doc });

        let mut saved = false;
        while let Some((_, event)) = events.try_recv() {
            saved |= matches!(event, Event::Saved { doc: candidate } if candidate == doc);
        }
        assert!(saved);
        assert_eq!(std::fs::read(&path).ok(), Some(Vec::new()));
    }

    #[test]
    fn first_save_of_missing_document_never_clobbers_a_late_creator() {
        let Ok(dir) = tempfile::tempdir() else {
            return;
        };
        let path = dir.path().join("README.md");
        let (mut session, mut events, _snaps) = Session::new(SessionConfig::default());
        session.handle(
            RequestId(1),
            Command::OpenDocument {
                path: path.clone(),
                language: None,
            },
        );
        let Some(doc) = opened_doc(&mut events) else {
            return;
        };
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
        assert!(std::fs::write(&path, "external\n").is_ok());

        session.handle(RequestId(3), Command::Save { doc });

        let mut conflict = false;
        while let Some((_, event)) = events.try_recv() {
            conflict |= matches!(event, Event::ExternalConflict { doc: candidate } if candidate == doc);
        }
        assert!(conflict);
        assert_eq!(
            std::fs::read_to_string(&path).ok().as_deref(),
            Some("external\n")
        );
        assert!(session.document(doc).is_some_and(|view| view.buffer().is_dirty()));
    }
