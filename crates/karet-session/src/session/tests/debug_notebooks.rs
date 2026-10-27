    // The `Debug*` and `Notebook*` pre-dispatch, driven through
    // `Session::handle`: the debugger manager's own contract is covered in
    // `crate::dap`, so these pin the routing — which command reaches which
    // manager call, and what each answers without a live session.

    /// Wait (5s) for the event `pick` accepts, with its request tag.
    async fn next_matching<T>(
        events: &mut EventRx,
        mut pick: impl FnMut(Option<RequestId>, Event) -> Option<T>,
    ) -> Option<T> {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let (id, event) = tokio::time::timeout_at(deadline, events.recv()).await.ok()??;
            if let Some(found) = pick(id, event) {
                return Some(found);
            }
        }
    }

    async fn next_notification(events: &mut EventRx) -> Option<String> {
        next_matching(events, |_, event| match event {
            Event::Notification { message, .. } => Some(message),
            _ => None,
        })
        .await
    }

    #[tokio::test]
    async fn debug_commands_without_a_session_answer_on_their_request() {
        let (mut session, mut events, _snaps) = Session::new(SessionConfig::default());

        session.handle(RequestId(1), DebugCommand::Continue);
        assert_eq!(
            next_notification(&mut events).await.as_deref(),
            Some("no debug session")
        );
        session.handle(RequestId(2), DebugCommand::Stop);
        assert_eq!(
            next_notification(&mut events).await.as_deref(),
            Some("no debug session to stop")
        );

        session.handle(RequestId(3), DebugCommand::StackTrace);
        let stack = next_matching(&mut events, |id, event| match event {
            Event::DebugStack { frames } => Some((id, frames)),
            _ => None,
        })
        .await;
        assert_eq!(stack, Some((Some(RequestId(3)), Vec::new())));

        session.handle(RequestId(4), DebugCommand::Scopes { frame: 5 });
        let scopes = next_matching(&mut events, |id, event| match event {
            Event::DebugScopes { frame, scopes } => Some((id, frame, scopes.len())),
            _ => None,
        })
        .await;
        assert_eq!(scopes, Some((Some(RequestId(4)), 5, 0)));

        session.handle(RequestId(5), DebugCommand::Variables { reference: 11 });
        let variables = next_matching(&mut events, |id, event| match event {
            Event::DebugVariables {
                reference,
                variables,
            } => Some((id, reference, variables.len())),
            _ => None,
        })
        .await;
        assert_eq!(variables, Some((Some(RequestId(5)), 11, 0)));

        session.handle(
            RequestId(6),
            DebugCommand::Evaluate {
                expression: "x".to_owned(),
                frame: None,
            },
        );
        let evaluated = next_matching(&mut events, |id, event| match event {
            Event::DebugEvaluated { result, reference } => Some((id, result, reference)),
            _ => None,
        })
        .await;
        assert_eq!(
            evaluated,
            Some((Some(RequestId(6)), "no debug session".to_owned(), 0))
        );

        session.handle(
            RequestId(7),
            DebugCommand::SetBreakpoints {
                path: PathBuf::from("/w/main.rs"),
                lines: vec![2],
            },
        );
        let armed = next_matching(&mut events, |_, event| match event {
            Event::DebugBreakpoints { path, breakpoints } => Some((path, breakpoints)),
            _ => None,
        })
        .await;
        assert_eq!(
            armed,
            Some((
                PathBuf::from("/w/main.rs"),
                vec![crate::api::DebugBreakpoint {
                    line: 2,
                    verified: false
                }]
            )),
            "with no adapter the set is echoed back unverified"
        );

        session.handle(RequestId(8), DebugCommand::Start { configuration: None });
        let refused = next_notification(&mut events).await.unwrap_or_default();
        assert!(refused.contains("no matching debug configuration"), "{refused}");
    }

    /// A scripted adapter that acknowledges everything, stops at thread 7 once
    /// configured, and reports every request's command name on `seen`.
    fn recording_connector(
        seen: tokio::sync::mpsc::UnboundedSender<String>,
    ) -> crate::dap::Connector {
        std::sync::Arc::new(move |_launch| {
            let seen = seen.clone();
            Box::pin(async move {
                let (client_end, server_end) = tokio::io::duplex(1 << 16);
                tokio::spawn(recording_adapter(server_end, seen));
                let (read, write) = tokio::io::split(client_end);
                Ok(karet_dap::DapClient::connect(read, write))
            })
        })
    }

    async fn recording_adapter(
        stream: tokio::io::DuplexStream,
        seen: tokio::sync::mpsc::UnboundedSender<String>,
    ) {
        use karet_jsonrpc::framing::content_length as codec;
        use serde_json::json;
        let (read, mut write) = tokio::io::split(stream);
        let mut read = tokio::io::BufReader::new(read);
        let mut seq = 1_i64;
        while let Ok(Some(bytes)) = codec::read_frame(&mut read).await {
            let Ok(message) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
                break;
            };
            let command = message["command"].as_str().unwrap_or_default().to_owned();
            let thread = message["arguments"]["threadId"].as_i64();
            let _ = seen.send(match thread {
                Some(thread) => format!("{command}:{thread}"),
                None => command.clone(),
            });
            let mut frames = vec![json!({
                "seq": seq, "type": "response", "request_seq": message["seq"],
                "command": command, "success": true, "body": {},
            })];
            seq += 1;
            match command.as_str() {
                "initialize" => {
                    frames[0]["body"] = json!({"supportsConfigurationDoneRequest": true});
                    frames.push(json!({"seq": seq, "type": "event", "event": "initialized"}));
                },
                "configurationDone" => frames.push(json!({
                    "seq": seq, "type": "event", "event": "stopped",
                    "body": {"reason": "breakpoint", "threadId": 7},
                })),
                "stackTrace" => frames[0]["body"] = json!({"stackFrames": []}),
                _ => {},
            }
            seq += 1;
            for frame in frames {
                if let Ok(bytes) = serde_json::to_vec(&frame) {
                    let _ = codec::write_frame(&mut write, &bytes).await;
                }
            }
        }
    }

    /// Each run control reaches the adapter as its own request against the
    /// stopped thread; swapping two arms of the dispatch fails here.
    #[tokio::test]
    async fn each_run_control_command_sends_its_own_request() {
        let mut settings = crate::config::Settings::default();
        settings.debug.configurations = vec![crate::config::schema::DebugConfiguration {
            name: "Run".to_owned(),
            adapter: "lldb-dap".to_owned(),
            attach: false,
            arguments: serde_json::json!({"program": "/w/a.out"}),
        }];
        let (mut session, mut events, _snaps) = Session::new(SessionConfig {
            settings,
            ..SessionConfig::default()
        });
        let (seen_tx, mut seen) = tokio::sync::mpsc::unbounded_channel();
        session.debug.set_connector(recording_connector(seen_tx));

        session.handle(RequestId(1), DebugCommand::Start { configuration: None });
        let stopped = next_matching(&mut events, |_, event| match event {
            Event::DebugStopped { thread, .. } => Some(thread),
            _ => None,
        })
        .await;
        assert_eq!(stopped, Some(7));
        while seen.try_recv().is_ok() {} // the handshake

        let mut sent = Vec::new();
        for (n, command) in [
            DebugCommand::StepIn,
            DebugCommand::StepOut,
            DebugCommand::StepOver,
            DebugCommand::Pause,
            DebugCommand::Continue,
        ]
        .into_iter()
        .enumerate()
        {
            session.handle(RequestId(10 + n as u64), command);
            let deadline = std::time::Duration::from_secs(5);
            sent.push(tokio::time::timeout(deadline, seen.recv()).await.ok().flatten());
        }
        assert_eq!(
            sent,
            ["stepIn:7", "stepOut:7", "next:7", "pause:7", "continue:7"]
                .map(|s| Some(s.to_owned()))
        );
    }

    #[cfg(feature = "notebook-kernel")]
    #[tokio::test]
    async fn notebook_commands_reach_the_kernel_manager() -> TestResult {
        use crate::api::NotebookCommand;

        let (mut session, mut events, _snaps) = Session::new(SessionConfig::default());

        session.handle(RequestId(1), NotebookCommand::Interrupt);
        assert_eq!(
            next_notification(&mut events).await.as_deref(),
            Some("no notebook kernel to interrupt")
        );
        session.handle(RequestId(2), NotebookCommand::Restart);
        assert_eq!(
            next_notification(&mut events).await.as_deref(),
            Some("no notebook kernel to restart")
        );

        // Running a notebook that does not parse starts a kernel task, which
        // diagnoses the file before it discovers or launches anything.
        let dir = tempfile::tempdir()?;
        for (n, command) in [
            |path| NotebookCommand::RunAll { path },
            |path| NotebookCommand::RunCell { path, cell: 0 },
        ]
        .into_iter()
        .enumerate()
        {
            let path = dir.path().join(format!("broken{n}.ipynb"));
            std::fs::write(&path, "not json")?;
            session.handle(RequestId(3 + n as u64), command(path));
            let message = next_notification(&mut events).await.unwrap_or_default();
            assert!(message.starts_with("notebook:"), "{message}");
        }
        Ok(())
    }

    /// `DocumentCommand::Convert` warms the notebook's kernel when
    /// `notebook.kernel.autoStart` is on and the path is a `.ipynb`, and the
    /// conversion still answers. With the setting off, or for another file
    /// type, no kernel starts: a following `Restart` finds none.
    #[cfg(feature = "notebook-kernel")]
    #[tokio::test]
    async fn convert_warms_the_notebook_kernel_only_when_auto_start_asks() -> TestResult {
        use crate::api::NotebookCommand;

        let dir = tempfile::tempdir()?;
        let notebook = dir.path().join("broken.ipynb");
        std::fs::write(&notebook, "not json")?;
        let document = dir.path().join("broken.docx");
        std::fs::write(&document, "not a zip")?;
        let session_with = |auto_start: bool| {
            let mut settings = crate::config::Settings::default();
            settings.notebook.kernel.auto_start = auto_start;
            Session::new(SessionConfig {
                settings,
                ..SessionConfig::default()
            })
        };

        // Warmed: the kernel task diagnoses the unparsable notebook before it
        // discovers or launches anything, and the conversion answers too.
        let (mut session, mut events, _snaps) = session_with(true);
        session.handle(RequestId(1), DocumentCommand::Convert { path: notebook.clone() });
        let (mut kernel, mut converted) = (None, None);
        let both = next_matching(&mut events, |id, event| {
            match event {
                Event::Notification { message, .. } => kernel = Some(message),
                Event::DocumentConverted { path, markdown } => {
                    converted = Some((id, path, markdown.is_err()));
                },
                _ => {},
            }
            (kernel.is_some() && converted.is_some()).then_some(())
        })
        .await;
        assert_eq!(both, Some(()), "kernel {kernel:?}, converted {converted:?}");
        let kernel = kernel.unwrap_or_default();
        assert!(kernel.starts_with("notebook:"), "{kernel}");
        assert_eq!(converted, Some((Some(RequestId(1)), notebook.clone(), true)));

        // Not warmed: auto-start off for a notebook, or on for another type.
        for (auto_start, path) in [(false, &notebook), (true, &document)] {
            let (mut session, mut events, _snaps) = session_with(auto_start);
            session.handle(RequestId(1), DocumentCommand::Convert { path: path.clone() });
            let converted = next_matching(&mut events, |id, event| match event {
                Event::DocumentConverted { path, .. } => Some((id, path)),
                _ => None,
            })
            .await;
            assert_eq!(converted, Some((Some(RequestId(1)), path.clone())));
            session.handle(RequestId(2), NotebookCommand::Restart);
            assert_eq!(
                next_notification(&mut events).await.as_deref(),
                Some("no notebook kernel to restart"),
                "auto_start {auto_start}, {}",
                path.display()
            );
        }
        Ok(())
    }
