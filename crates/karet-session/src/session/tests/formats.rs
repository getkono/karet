    // Binary document formats across the seam: CBOR decode/encode and DOCX
    // conversion (split from tests/documents.rs for the file-size ceiling).

    #[cfg(feature = "cbor")]
    #[test]
    fn corrupt_cbor_answers_not_utf8_like_binary_text() -> TestResult {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("broken.cbor");
        // A map header promising an entry, with none: undecodable CBOR.
        std::fs::write(&path, [0xa1u8])?;
        let (mut session, mut events, _snaps) = Session::new(SessionConfig::default());
        session.handle(
            RequestId(1),
            DocumentCommand::Open {
                path: path.clone(),
                language: None,
            },
        );
        let mut not_utf8_path = None;
        let mut opened = false;
        while let Some((_, ev)) = events.try_recv() {
            match ev {
                Event::NotUtf8 { path } => not_utf8_path = Some(path),
                Event::Opened { .. } => opened = true,
                _ => {},
            }
        }
        assert_eq!(not_utf8_path, Some(path));
        assert!(!opened, "undecodable CBOR must not report as Opened");
        Ok(())
    }

    #[cfg(feature = "docx")]
    #[test]
    fn convert_document_answers_with_markdown() -> TestResult {
        use std::io::Write as _;
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("report.docx");
        // A minimal DOCX (one Heading1 paragraph) zipped in-memory.
        const DOCUMENT_XML: &str = r#"<?xml version="1.0"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>
<w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>Report</w:t></w:r></w:p>
</w:body></w:document>"#;
        let mut bytes = Vec::new();
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut bytes));
        writer.start_file("word/document.xml", zip::write::SimpleFileOptions::default())?;
        writer.write_all(DOCUMENT_XML.as_bytes())?;
        writer.finish()?;
        std::fs::write(&path, &bytes)?;
        let (mut session, mut events, _snaps) = Session::new(SessionConfig::default());
        session.handle(RequestId(1), DocumentCommand::Convert { path: path.clone() });
        // The conversion runs on its own thread; await its answer.
        let markdown = await_event(&mut events, EVENT_DEADLINE, |_, event| match event {
            Event::DocumentConverted { markdown, .. } => Some(markdown),
            _ => None,
        });
        assert_eq!(markdown, Some(Ok("# Report".to_string())));
        Ok(())
    }

    #[cfg(feature = "cbor")]
    #[test]
    fn cbor_opens_decoded_and_save_reencodes() -> TestResult {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("data.cbor");
        let original = karet_cbor::CborValue::Array(vec![
            karet_cbor::CborValue::Integer(1),
            karet_cbor::CborValue::Integer(2),
        ]);
        let bytes = karet_cbor::encode(&original)?;
        std::fs::write(&path, &bytes)?;

        let (mut session, mut events, mut snaps) = Session::new(SessionConfig::default());
        session.handle(
            RequestId(1),
            DocumentCommand::Open {
                path: path.clone(),
                language: None,
            },
        );
        let doc = opened_doc(&mut events).ok_or("no Opened event")?;
        // The buffer holds decoded diagnostic notation, not the raw CBOR bytes.
        let text = session.document(doc).map(|v| v.buffer().text());
        assert_eq!(text.as_deref(), Some("[\n  1,\n  2\n]"));
        while snaps.try_recv().is_some() {}

        // Edit the "2" (line 2, col 2) to "3".
        let change = Change::new(
            0,
            vec![TextEdit {
                range: Range {
                    start: LineCol::new(2, 2),
                    end: LineCol::new(2, 3),
                },
                new_text: "3".to_string(),
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
        while events.try_recv().is_some() {}

        // Save re-encodes to CBOR; the file on disk decodes to the edited value.
        session.handle(RequestId(3), DocumentCommand::Save {
                doc,
                cause: SaveCause::Manual,
            });
        let mut saved = false;
        while let Some((_, ev)) = events.try_recv() {
            if let Event::Saved { .. } = ev {
                saved = true;
            }
        }
        assert!(saved, "a cbor save should succeed");
        let disk = std::fs::read(&path)?;
        let expected = karet_cbor::CborValue::Array(vec![
            karet_cbor::CborValue::Integer(1),
            karet_cbor::CborValue::Integer(3),
        ]);
        assert_eq!(karet_cbor::decode(&disk).ok(), Some(expected));
        Ok(())
    }

    #[cfg(feature = "cbor")]
    #[test]
    fn cbor_save_of_malformed_edit_leaves_file_untouched() -> TestResult {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("bad.cbor");
        let original = karet_cbor::CborValue::Array(vec![
            karet_cbor::CborValue::Integer(1),
            karet_cbor::CborValue::Integer(2),
        ]);
        let bytes = karet_cbor::encode(&original)?;
        std::fs::write(&path, &bytes)?;

        let (mut session, mut events, _snaps) = Session::new(SessionConfig::default());
        session.handle(
            RequestId(1),
            DocumentCommand::Open {
                path: path.clone(),
                language: None,
            },
        );
        let doc = opened_doc(&mut events).ok_or("no Opened event")?;

        // Delete the closing ']' (line 3, col 0), making the text un-parseable.
        let change = Change::new(
            0,
            vec![TextEdit {
                range: Range {
                    start: LineCol::new(3, 0),
                    end: LineCol::new(3, 1),
                },
                new_text: String::new(),
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
        while events.try_recv().is_some() {}

        // Save fails to encode; no Saved event, and the file is unchanged.
        session.handle(RequestId(3), DocumentCommand::Save {
                doc,
                cause: SaveCause::Manual,
            });
        let mut saved = false;
        let mut failed = false;
        while let Some((_, ev)) = events.try_recv() {
            match ev {
                Event::Saved { .. } => saved = true,
                Event::Notification {
                    severity: Severity::Error,
                    ..
                } => failed = true,
                _ => {},
            }
        }
        assert!(!saved, "a malformed cbor buffer must not save");
        assert!(
            failed,
            "the failure should surface as an error notification"
        );
        assert_eq!(std::fs::read(&path)?, bytes, "the file is untouched");
        Ok(())
    }

