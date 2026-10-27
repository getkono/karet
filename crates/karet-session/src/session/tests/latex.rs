    #[test]
    fn latex_build_command_reports_a_missing_external_compiler() -> TestResult {
        let (dir, path) =
            write_temp("main.tex", "\\documentclass{article}\n").ok_or("temp file not written")?;
        let mut settings = crate::config::Settings::default();
        settings.latex.command = "karet-test-definitely-missing-latex".to_owned();
        let (mut session, mut events, _snaps) = Session::new(SessionConfig {
            roots: vec![dir.path().to_path_buf()],
            settings,
            ..SessionConfig::default()
        });
        session.handle(
            RequestId(1),
            DocumentCommand::Open {
                path: path.clone(),
                language: None,
            },
        );
        let doc = opened_doc(&mut events).ok_or("no Opened event")?;
        session.handle(
            RequestId(2),
            DocumentCommand::ApplyChange {
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

        session.handle(RequestId(3), DocumentCommand::BuildLatex { doc });
        assert!(
            std::fs::read_to_string(path)
                .is_ok_and(|contents| contents.contains("saved before compilation")),
            "an explicit build must safely persist the current buffer first"
        );
        let failure = await_event(&mut events, EVENT_DEADLINE, |id, event| match event {
            Event::LatexBuildFinished { error, .. } if id == Some(RequestId(3)) => Some(error),
            _ => None,
        })
        .ok_or("the build never finished")?;
        assert!(failure.is_some_and(|error| error.contains("could not start")));
        Ok(())
    }
