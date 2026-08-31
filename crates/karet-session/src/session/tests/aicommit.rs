    // The AI commit-message `Command`/`Event` contract.
    //
    // These never reach a model. What they pin is the seam either side of it:
    // which requests are refused before anything is launched, which answers are
    // attributed to which request, and what the client is told about the
    // configuration before it asks for anything.

    /// A session over `root`, driven directly the way the actor would.
    fn ai_session(root: PathBuf) -> (Session, EventRx) {
        let (session, events, _snaps) = Session::new(SessionConfig {
            roots: vec![root],
            ..SessionConfig::default()
        });
        (session, events)
    }

    /// Drain the events currently queued.
    fn drain(events: &mut EventRx) -> Vec<(Option<RequestId>, Event)> {
        let mut out = Vec::new();
        while let Some(pair) = events.try_recv() {
            out.push(pair);
        }
        out
    }

    /// The first `CommitMessageFailed` in `events`, with the request it answers.
    fn failure(events: &[(Option<RequestId>, Event)]) -> Option<(Option<RequestId>, String)> {
        events.iter().find_map(|(id, event)| match event {
            Event::CommitMessageFailed { message } => Some((*id, message.clone())),
            _ => None,
        })
    }

    /// The last `AiCommitAvailability` in `events`.
    fn availability(
        events: &[(Option<RequestId>, Event)],
    ) -> Option<crate::api::AiCommitAvailability> {
        events.iter().rev().find_map(|(_, event)| match event {
            Event::AiCommitAvailability { status } => Some((**status).clone()),
            _ => None,
        })
    }

    #[test]
    fn generation_is_refused_when_the_setting_is_off() {
        let Some((_dir, root, _file)) = init_temp_repo() else {
            return;
        };
        let (mut session, mut events) = ai_session(root);
        session.config.settings.git.ai_commit.enabled = false;

        session.handle(RequestId(1), Command::GenerateCommitMessage);

        let found = failure(&drain(&mut events));
        assert!(found.is_some(), "a refusal answers the request");
        let (id, message) = found.unwrap_or_default();
        assert_eq!(id, Some(RequestId(1)), "attributed to the asker");
        assert!(message.contains("disabled"), "{message}");
    }

    #[test]
    fn generation_is_refused_with_nothing_staged() {
        let Some((_dir, root, _file)) = init_temp_repo() else {
            return;
        };
        // A tokio runtime, because the real path spawns onto one.
        let Ok(runtime) = tokio::runtime::Runtime::new() else {
            return;
        };
        let (mut session, mut events) = runtime.block_on(async { ai_session(root) });

        runtime.block_on(async {
            session.handle(RequestId(1), Command::GenerateCommitMessage);
            // The diff read is a blocking task; give it a moment to answer.
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        });

        let found = failure(&drain(&mut events));
        assert!(found.is_some(), "nothing staged is a failure");
        let (id, message) = found.unwrap_or_default();
        assert_eq!(id, Some(RequestId(1)));
        assert!(message.contains("stage changes first"), "{message}");
    }

    #[test]
    fn a_session_without_a_reactor_answers_rather_than_panicking() {
        let Some((_dir, root, _file)) = init_temp_repo() else {
            return;
        };
        let (mut session, mut events) = ai_session(root);

        // No runtime here at all: the request must still be answered.
        session.handle(RequestId(1), Command::GenerateCommitMessage);

        let found = failure(&drain(&mut events));
        assert!(found.is_some(), "answered even without a reactor");
        let (id, message) = found.unwrap_or_default();
        assert_eq!(id, Some(RequestId(1)));
        assert!(message.contains("runtime"), "{message}");
    }

    #[test]
    fn availability_reports_the_configuration_without_probing_when_disabled() {
        let Some((_dir, root, _file)) = init_temp_repo() else {
            return;
        };
        let (mut session, mut events) = ai_session(root);
        session.config.settings.git.ai_commit.enabled = false;

        session.handle(RequestId(1), Command::ProbeAiCommit);

        let found = availability(&drain(&mut events));
        assert!(found.is_some(), "availability is pushed");
        let Some(status) = found else { return };
        assert!(!status.enabled);
        assert!(
            status.agents.is_empty(),
            "a disabled workspace launches nothing to probe with"
        );
        assert!(
            status.blocker().is_some_and(|b| b.contains("disabled")),
            "{status:?}"
        );
    }

    #[test]
    fn start_seeds_the_client_with_availability() {
        let Some((_dir, root, _file)) = init_temp_repo() else {
            return;
        };
        let (mut session, mut events) = ai_session(root);
        // Off, so `start` reports without launching anything — the point here is
        // that the client is told *something* before it asks.
        session.config.settings.git.ai_commit.enabled = false;

        session.start();

        let found = availability(&drain(&mut events));
        assert!(found.is_some(), "startup pushes availability");
        let Some(status) = found else { return };
        assert_eq!(status.options, session.config.settings.git.ai_commit);
    }

    #[test]
    fn a_repository_cannot_choose_which_executable_runs() {
        let Some((_dir, root, _file)) = init_temp_repo() else {
            return;
        };
        let (mut session, _events) = ai_session(root);
        session.config.settings.git.ai_commit.binary = Some("./payload.sh".to_string());

        // Attributed to the user layer: the machine owner's own file, honoured.
        session.config.loaded_config.explicit.insert(
            "git.aiCommit.binary".to_string(),
            crate::config::ConfigLayer::User,
        );
        assert_eq!(
            session.ai_commit_options().binary.as_deref(),
            Some("./payload.sh")
        );

        // Attributed to the project layer — a file that arrived with the clone.
        session.config.loaded_config.explicit.insert(
            "git.aiCommit.binary".to_string(),
            crate::config::ConfigLayer::Project,
        );
        assert_eq!(
            session.ai_commit_options().binary,
            None,
            "a repository may not name the program karet launches"
        );
        // Everything else the repository asked for is still honoured.
        session.config.settings.git.ai_commit.model = "haiku".to_string();
        assert_eq!(session.ai_commit_options().model, "haiku");
    }

    #[test]
    fn changing_the_settings_re_reports_availability() {
        let Some((_dir, root, _file)) = init_temp_repo() else {
            return;
        };
        let (mut session, mut events) = ai_session(root);
        session.config.settings.git.ai_commit.enabled = false;

        // A reload that leaves `git.aiCommit` alone says nothing about it...
        let mut report = crate::config::LoadedConfig::from_settings(session.config.settings.clone());
        session.apply_config_report(report.clone());
        assert!(
            availability(&drain(&mut events)).is_none(),
            "an unrelated reload is not an availability change"
        );

        // ...but one that changes it must, or a client that refused locally on
        // the old answer would keep refusing until it restarted.
        report.settings.git.ai_commit.enabled = true;
        report.settings.git.ai_commit.binary = Some("karet-no-such-agent".to_string());
        session.apply_config_report(report);
        let found = availability(&drain(&mut events));
        assert!(found.is_some(), "a changed setting re-reports availability");
        let Some(status) = found else { return };
        assert!(status.enabled);
    }
