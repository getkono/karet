    // The call tier through the session seam: a node's detail carries its measures and its
    // calls, and both follow edits and configuration changes.

    /// A session over a package whose functions call one another, indexed.
    fn calling_session() -> Option<(Session, crate::session::EventRx, tempfile::TempDir)> {
        karet_seam::lang::rust::language_id()?;
        let dir = tempfile::tempdir().ok()?;
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"calls\"\nversion = \"0.1.0\"\n",
        )
        .ok()?;
        std::fs::create_dir_all(dir.path().join("src")).ok()?;
        std::fs::write(
            dir.path().join("src").join("lib.rs"),
            "pub fn top() { mid(); mid(); }\nfn mid() { leaf(); }\nfn leaf() {}\n\
             pub struct Shape;\n#[cfg(test)]\nmod tests {\n    fn t() { super::leaf(); }\n}\n",
        )
        .ok()?;
        let (mut session, mut events, _snaps) = Session::new(SessionConfig {
            roots: vec![dir.path().to_path_buf()],
            ..SessionConfig::default()
        });
        session.handle(RequestId(1), Command::IndexSeams {
            root: None,
            mode: crate::api::SeamSync::Incremental,
        });
        let indexed = await_seam_event(&mut events);
        assert!(
            matches!(indexed, Some(Event::SeamIndexed { .. })),
            "the package must index: {indexed:?}"
        );
        Some((session, events, dir))
    }

    /// Ask for one node's detail and wait for it, failing rather than passing silently.
    fn detail(
        session: &mut Session,
        events: &mut crate::session::EventRx,
        path: &str,
    ) -> (Vec<crate::api::SeamEdgeView>, Option<crate::api::SeamCallMetricsView>) {
        session.handle(RequestId(9), Command::SeamNode {
            path: path.to_owned(),
        });
        let answer = await_seam_event(events);
        assert!(
            matches!(&answer, Some(Event::SeamNodeDetail { node, .. }) if node == path),
            "expected the detail of {path}, got {answer:?}"
        );
        let Some(Event::SeamNodeDetail { edges, metrics, .. }) = answer else {
            return (Vec::new(), None);
        };
        (edges, metrics)
    }

    #[test]
    #[allow(
        clippy::panic,
        reason = "a missing event must fail the test, not pass it silently"
    )]
    fn a_functions_detail_carries_its_measures_and_aggregated_calls() {
        let Some((mut session, mut events, _dir)) = calling_session() else {
            return;
        };
        let (edges, metrics) = detail(&mut session, &mut events, "calls::top");
        let metrics = metrics.unwrap_or_else(|| panic!("a function has measures"));
        assert_eq!((metrics.code_lines, metrics.inclusive, metrics.depth), (1, 3, 2));
        assert_eq!(metrics.fan_out, 1);
        // Two call sites, one row.
        let to_mid: Vec<_> = edges
            .iter()
            .filter(|edge| edge.kind == "calls" && edge.target.as_deref() == Some("calls::mid"))
            .collect();
        assert_eq!(to_mid.len(), 1, "{edges:?}");
        assert_eq!(to_mid.first().map(|edge| edge.count), Some(2));
        assert_eq!(
            to_mid.first().and_then(|edge| edge.resolution.as_deref()),
            Some("exact")
        );

        // A type is no function and has nothing to measure.
        let (_, metrics) = detail(&mut session, &mut events, "calls::Shape");
        assert!(metrics.is_none());

        // The rows follow the configuration the measures do: the cfg(test) caller is not
        // listed beside an `in 1`.
        let (edges, metrics) = detail(&mut session, &mut events, "calls::leaf");
        assert_eq!(metrics.map(|m| m.fan_in), Some(1));
        let callers: Vec<&str> = edges
            .iter()
            .filter(|edge| edge.kind == "calls" && !edge.outgoing)
            .filter_map(|edge| edge.target.as_deref())
            .collect();
        assert_eq!(callers, ["calls::mid"]);
    }

    #[test]
    #[allow(
        clippy::panic,
        reason = "a missing event must fail the test, not pass it silently"
    )]
    fn an_edit_and_a_configuration_change_both_reach_the_measures() {
        let Some((mut session, mut events, dir)) = calling_session() else {
            return;
        };
        let (_, before) = detail(&mut session, &mut events, "calls::leaf");
        assert_eq!(before.map(|m| m.fan_in), Some(1), "only `mid` calls it");

        // An edit adds a caller: the graph must not answer from before the edit.
        session.handle(RequestId(3), Command::ReindexSeams {
            path: dir.path().join("src").join("lib.rs"),
            text: "pub fn top() { mid(); mid(); }\nfn mid() { leaf(); }\nfn leaf() {}\n\
                   pub fn extra() { leaf(); }\npub struct Shape;\n"
                .to_owned(),
        });
        assert!(matches!(
            await_seam_event(&mut events),
            Some(Event::SeamIndexed { .. })
        ));
        let (edges, after) = detail(&mut session, &mut events, "calls::leaf");
        assert_eq!(after.map(|m| m.fan_in), Some(2));
        assert!(
            edges
                .iter()
                .any(|edge| !edge.outgoing && edge.target.as_deref() == Some("calls::extra")),
            "{edges:?}"
        );

        // Back to the original text, then measured under the test configuration: the test
        // caller counts there, and not under the default.
        session.handle(RequestId(4), Command::ReindexSeams {
            path: dir.path().join("src").join("lib.rs"),
            text: "pub fn top() { mid(); mid(); }\nfn mid() { leaf(); }\nfn leaf() {}\n\
                   pub struct Shape;\n#[cfg(test)]\nmod tests {\n    fn t() { super::leaf(); }\n}\n"
                .to_owned(),
        });
        assert!(matches!(
            await_seam_event(&mut events),
            Some(Event::SeamIndexed { .. })
        ));
        session.handle(RequestId(5), Command::SetSeamConfiguration {
            name: karet_seam::Configuration::unconfigured()
                .for_target("test")
                .name,
        });
        assert!(matches!(
            await_seam_event(&mut events),
            Some(Event::SeamIndexed { .. })
        ));
        let (_, testing) = detail(&mut session, &mut events, "calls::leaf");
        assert_eq!(testing.map(|m| m.fan_in), Some(2), "`tests::t` calls it too");
    }
