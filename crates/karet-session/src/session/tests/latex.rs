    #[test]
    fn latex_build_command_reports_a_missing_external_compiler() {
        let Some((dir, path)) = write_temp("main.tex", "\\documentclass{article}\n") else {
            return;
        };
        let mut settings = crate::config::Settings::default();
        settings.latex.command = "karet-test-definitely-missing-latex".to_owned();
        let (mut session, mut events, _snaps) = Session::new(SessionConfig {
            roots: vec![dir.path().to_path_buf()],
            settings,
            ..SessionConfig::default()
        });
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
                        range: Range {
                            start: LineCol::new(1, 0),
                            end: LineCol::new(1, 0),
                        },
                        new_text: "% saved before compilation\n".to_owned(),
                    }],
                ),
                cause: EditCause::Replace,
            },
        );
        while events.try_recv().is_some() {}

        session.handle(RequestId(3), Command::BuildLatex { doc });
        assert!(
            std::fs::read_to_string(path)
                .is_ok_and(|contents| contents.contains("saved before compilation")),
            "an explicit build must safely persist the current buffer first"
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        let mut failure = None;
        while std::time::Instant::now() < deadline {
            if let Some((id, Event::LatexBuildFinished { error, .. })) = events.try_recv()
                && id == Some(RequestId(3))
            {
                failure = error;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(failure.is_some_and(|error| error.contains("could not start")));
    }
