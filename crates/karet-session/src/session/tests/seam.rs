    // The seam index's Command/Event contract. Indexing itself is covered hermetically
    // in `karet-seam`; these cover the session's half — that each command reaches the
    // worker and each answer comes back in the shape the presentation layer expects.

    /// Build a small package on disk to index.
    fn seam_package(dir: &std::path::Path) -> std::io::Result<()> {
        std::fs::write(
            dir.join("Cargo.toml"),
            "[package]\nname = \"seamdemo\"\nversion = \"0.1.0\"\n",
        )?;
        std::fs::create_dir_all(dir.join("src"))?;
        std::fs::write(
            dir.join("src").join("lib.rs"),
            "pub mod inner;\npub unsafe fn danger() {}\n",
        )?;
        std::fs::write(
            dir.join("src").join("inner.rs"),
            "pub trait Contract {}\npub fn helper() {}\n",
        )?;
        Ok(())
    }

    /// Drive the session until a seam answer arrives, or give up.
    ///
    /// The worker is a real thread, so the answer is genuinely asynchronous.
    ///
    /// An index arrives as a stream — one `SeamPackageIndexed` per package, closed by one
    /// `SeamIndexFinished` — and this reassembles it into the whole-tree shape, so a test
    /// about *what was indexed* is not also a test about how it was delivered. The
    /// streaming itself is covered separately, below.
    fn await_seam_event(events: &mut crate::session::EventRx) -> Option<Event> {
        let mut streamed: Vec<(usize, Vec<crate::api::SeamNodeView>)> = Vec::new();
        await_event(events, EVENT_DEADLINE, |_, event| match event {
            Event::SeamPackageIndexed { order, nodes, .. } => {
                streamed.push((order, nodes));
                None
            },
            Event::SeamIndexFinished { summary, .. } => {
                // Sorted back into discovery order, which is what the view does
                // with the `order` each package carries.
                let mut packages = std::mem::take(&mut streamed);
                packages.sort_by_key(|(order, _)| *order);
                Some(Event::SeamIndexed {
                    summary,
                    nodes: packages.into_iter().flat_map(|(_, nodes)| nodes).collect(),
                })
            },
            Event::SeamIndexed { .. }
            | Event::SeamIndexFailed { .. }
            | Event::SeamQueryResult { .. }
            | Event::SeamNodeDetail { .. } => Some(event),
            _ => None,
        })
    }

    /// [`await_seam_event`], with a missing answer as an error naming what was awaited.
    fn next_seam_answer(
        events: &mut crate::session::EventRx,
        awaited: &str,
    ) -> Result<Event, Box<dyn std::error::Error>> {
        await_seam_event(events).ok_or_else(|| format!("no {awaited} answer arrived").into())
    }

    /// One package as it crossed the wire.
    struct StreamedPackage {
        order: usize,
        root: String,
        nodes: usize,
    }

    /// A whole index run, as it actually arrived.
    struct StreamedIndex {
        packages: Vec<StreamedPackage>,
        parsed: usize,
        files: usize,
    }

    /// Collect a whole index run as it actually arrives, event by event.
    fn await_index_stream(events: &mut crate::session::EventRx) -> Option<StreamedIndex> {
        let mut packages = Vec::new();
        await_event(events, EVENT_DEADLINE, |_, event| match event {
            Event::SeamPackageIndexed {
                order, root, nodes, ..
            } => {
                packages.push(StreamedPackage {
                    order,
                    root,
                    nodes: nodes.len(),
                });
                None
            },
            Event::SeamIndexFinished { parsed, files, .. } => Some(Some(StreamedIndex {
                packages: std::mem::take(&mut packages),
                parsed,
                files,
            })),
            Event::SeamIndexFailed { .. } => Some(None),
            _ => None,
        })
        .flatten()
    }

    /// What an index-stream test awaits; `await_index_stream` yields `None` on failure too.
    const NO_STREAM: &str = "no index stream finished (timed out or SeamIndexFailed)";

    /// A session rooted at a freshly built package, with its index already requested.
    fn indexed_session() -> Result<
        (Session, crate::session::EventRx, tempfile::TempDir, Event),
        Box<dyn std::error::Error>,
    > {
        let dir = tempfile::tempdir()?;
        seam_package(dir.path())?;
        let (mut session, mut events, _snaps) = Session::new(SessionConfig {
            roots: vec![dir.path().to_path_buf()],
            ..SessionConfig::default()
        });
        session.handle(RequestId(1), Command::IndexSeams {
            root: None,
            mode: crate::api::SeamSync::Incremental,
        });
        let indexed = next_seam_answer(&mut events, "initial SeamIndexed")?;
        Ok((session, events, dir, indexed))
    }

    #[test]
    fn indexing_a_package_answers_with_the_whole_tree_and_a_summary() -> TestResult {
        let (_session, _events, _dir, event) = indexed_session()?;
        let Event::SeamIndexed { summary, nodes } = event else {
            return Err(format!("expected SeamIndexed, got {event:?}").into());
        };
        assert_eq!(summary.package, "seamdemo");
        assert!(summary.nodes > 0);
        assert!(summary.files >= 2, "the module in another file must be indexed");
        // The whole tree crosses at once, so navigation needs no round trip.
        assert_eq!(nodes.len(), summary.nodes);
        assert!(nodes.iter().any(|n| n.id == "seamdemo::inner::helper"));
        assert!(nodes.iter().any(|n| n.id == "seamdemo::danger"));
        Ok(())
    }

    #[test]
    fn the_header_always_names_a_configuration_and_says_when_it_is_incomplete() -> TestResult {
        let (_session, _events, _dir, event) = indexed_session()?;
        let Event::SeamIndexed { summary, .. } = event else {
            return Err(format!("expected SeamIndexed, got {event:?}").into());
        };
        // Nothing renders unattributed, even before a manifest has been read.
        assert!(!summary.configuration.is_empty());
        assert!(!summary.available_configurations.is_empty());
        // And with no manifest the variation lens must not claim completeness.
        assert!(!summary.variation_complete);
        Ok(())
    }

    #[test]
    fn a_node_carries_its_facets_rollups_and_location() -> TestResult {
        let (_session, _events, _dir, event) = indexed_session()?;
        let Event::SeamIndexed { nodes, .. } = event else {
            return Err(format!("expected SeamIndexed, got {event:?}").into());
        };
        let danger = nodes
            .iter()
            .find(|n| n.id == "seamdemo::danger")
            .ok_or("seamdemo::danger is not in the tree")?;
        assert_eq!(danger.kind, "function");
        assert!(danger.facets.iter().any(|f| f.lens == "hazard" && f.subtype == "unsafe"));
        assert!(danger.facets.iter().any(|f| f.lens == "api"));
        assert!(danger.file.ends_with("lib.rs"));
        assert_eq!(danger.membership, "active");

        // The root's rollups account for what is beneath it, so a collapsed row is
        // still navigable.
        let root = nodes
            .iter()
            .find(|n| n.parent.is_none())
            .ok_or("the tree has no root node")?;
        assert!(root.rollups.iter().any(|count| *count > 0));
        Ok(())
    }

    #[test]
    fn indexing_somewhere_with_no_package_reports_a_failure_rather_than_an_empty_tree(
    ) -> TestResult {
        let dir = tempfile::tempdir()?;
        let (mut session, mut events, _snaps) = Session::new(SessionConfig {
            roots: vec![dir.path().to_path_buf()],
            ..SessionConfig::default()
        });
        session.handle(RequestId(1), Command::IndexSeams {
            root: None,
            mode: crate::api::SeamSync::Incremental,
        });
        let event = next_seam_answer(&mut events, "SeamIndexFailed")?;
        // An empty tree would say "this package has nothing in it", which is not what
        // happened — there is no package here at all.
        assert!(matches!(event, Event::SeamIndexFailed { .. }), "got {event:?}");
        Ok(())
    }

    #[test]
    fn a_query_answers_with_matching_node_identities() -> TestResult {
        let (mut session, mut events, _dir, _) = indexed_session()?;
        session.handle(
            RequestId(2),
            Command::SeamQuery {
                text: "lens:hazard".to_owned(),
            },
        );
        let event = next_seam_answer(&mut events, "SeamQueryResult")?;
        let Event::SeamQueryResult { nodes, error, .. } = event else {
            return Err(format!("expected SeamQueryResult, got {event:?}").into());
        };
        assert!(error.is_none());
        assert_eq!(nodes, ["seamdemo::danger"]);
        Ok(())
    }

    #[test]
    fn an_unreadable_query_answers_with_a_positioned_error_not_an_empty_result() -> TestResult {
        let (mut session, mut events, _dir, _) = indexed_session()?;
        session.handle(
            RequestId(2),
            Command::SeamQuery {
                text: "lens:hazrd".to_owned(),
            },
        );
        let event = next_seam_answer(&mut events, "SeamQueryResult")?;
        let Event::SeamQueryResult { nodes, error, .. } = event else {
            return Err(format!("expected SeamQueryResult, got {event:?}").into());
        };
        assert!(nodes.is_empty());
        // The distinction matters: "I could not read that" is not "nothing matched".
        let error = error.ok_or("an unreadable query answered with no error")?;
        assert!(error.message.contains("unknown lens"));
        assert!(error.end > error.start, "the error must be positioned");
        assert!(error.suggestions.contains(&"hazard".to_owned()));
        Ok(())
    }

    #[test]
    fn a_query_naming_a_configuration_reports_it_back() -> TestResult {
        let (mut session, mut events, _dir, _) = indexed_session()?;
        session.handle(
            RequestId(2),
            Command::SeamQuery {
                text: "config:tests lens:api".to_owned(),
            },
        );
        let event = next_seam_answer(&mut events, "SeamQueryResult")?;
        let Event::SeamQueryResult { configuration, .. } = event else {
            return Err(format!("expected SeamQueryResult, got {event:?}").into());
        };
        assert_eq!(configuration.as_deref(), Some("tests"));
        Ok(())
    }

    #[test]
    fn asking_for_a_node_answers_for_that_node_even_with_no_edges() -> TestResult {
        let (mut session, mut events, _dir, _) = indexed_session()?;
        session.handle(
            RequestId(2),
            Command::SeamNode {
                path: "seamdemo::inner::Contract".to_owned(),
            },
        );
        let event = next_seam_answer(&mut events, "SeamNodeDetail")?;
        let Event::SeamNodeDetail { node, .. } = event else {
            return Err(format!("expected SeamNodeDetail, got {event:?}").into());
        };
        // The answer is correlated to what was asked, so a stale reply is detectable.
        assert_eq!(node, "seamdemo::inner::Contract");
        Ok(())
    }

    #[test]
    fn asking_for_a_node_answers_with_its_edges_and_its_source_together() -> TestResult {
        let (mut session, mut events, _dir, _) = indexed_session()?;
        session.handle(
            RequestId(2),
            Command::SeamNode {
                path: "seamdemo::danger".to_owned(),
            },
        );
        let event = next_seam_answer(&mut events, "SeamNodeDetail")?;
        let Event::SeamNodeDetail { preview, .. } = event else {
            return Err(format!("expected SeamNodeDetail, got {event:?}").into());
        };
        assert!(preview.is_ok(), "{preview:?}");
        let Ok(preview) = preview else {
            return Err(format!("expected a source preview, got {preview:?}").into());
        };
        // One answer, not two: the pane can never show one node's source under another
        // node's relations.
        let body = preview.lines[preview.body_start..preview.body_end].join("\n");
        assert!(body.contains("unsafe fn danger"), "{body:?}");
        assert!(preview.file.ends_with("lib.rs"), "{:?}", preview.file);
        Ok(())
    }

    #[test]
    fn a_node_request_with_no_index_says_why_it_has_no_source() -> TestResult {
        let dir = tempfile::tempdir()?;
        let (mut session, mut events, _snaps) = Session::new(SessionConfig {
            roots: vec![dir.path().to_path_buf()],
            ..SessionConfig::default()
        });
        session.handle(RequestId(1), Command::SeamNode {
            path: "absent::thing".to_owned(),
        });
        let event = next_seam_answer(&mut events, "SeamNodeDetail")?;
        let Event::SeamNodeDetail { preview, .. } = event else {
            return Err(format!("expected SeamNodeDetail, got {event:?}").into());
        };
        // A reason, never a blank block: the pane keeps "nothing there" and "nobody
        // looked" apart everywhere else, and the source must not be the exception.
        assert!(preview.is_err(), "{preview:?}");
        Ok(())
    }

    #[test]
    #[ignore = "#657: both offered configurations share one name, so none can be switched to"]
    fn switching_configuration_re_answers_with_the_new_one_named() -> TestResult {
        let (mut session, mut events, _dir, first) = indexed_session()?;
        let Event::SeamIndexed { summary, .. } = first else {
            return Err(format!("expected SeamIndexed, got {first:?}").into());
        };
        let other = summary
            .available_configurations
            .iter()
            .find(|name| **name != summary.configuration)
            .cloned()
            .ok_or("the index offers no second configuration to switch to")?;

        session.handle(
            RequestId(2),
            Command::SetSeamConfiguration {
                name: other.clone(),
            },
        );
        let event = next_seam_answer(&mut events, "re-indexed SeamIndexed")?;
        let Event::SeamIndexed { summary: after, .. } = event else {
            return Err(format!("expected SeamIndexed, got {event:?}").into());
        };
        assert_eq!(after.configuration, other);
        assert_eq!(after.nodes, summary.nodes, "switching must not lose nodes");
        Ok(())
    }

    #[test]
    fn re_indexing_one_file_keeps_the_rest_of_the_tree() -> TestResult {
        let (mut session, mut events, dir, first) = indexed_session()?;
        let Event::SeamIndexed { nodes: before, .. } = first else {
            return Err(format!("expected SeamIndexed, got {first:?}").into());
        };
        assert!(before.iter().any(|n| n.id == "seamdemo::inner::helper"));

        session.handle(
            RequestId(2),
            Command::ReindexSeams {
                path: dir.path().join("src").join("inner.rs"),
                text: "pub trait Contract {}\npub fn renamed() {}\n".to_owned(),
            },
        );
        let event = next_seam_answer(&mut events, "re-indexed SeamIndexed")?;
        let Event::SeamIndexed { nodes: after, .. } = event else {
            return Err(format!("expected SeamIndexed, got {event:?}").into());
        };
        assert!(after.iter().any(|n| n.id == "seamdemo::inner::renamed"));
        assert!(!after.iter().any(|n| n.id == "seamdemo::inner::helper"));
        // The neighbouring file is untouched by its sibling's edit.
        assert!(after.iter().any(|n| n.id == "seamdemo::danger"));
        Ok(())
    }

    /// A virtual Cargo workspace on disk, with `members` under `crates/`.
    fn seam_workspace(dir: &std::path::Path, members: &[&str]) -> std::io::Result<()> {
        std::fs::write(
            dir.join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/*\"]\n",
        )?;
        for name in members {
            let member = dir.join("crates").join(name);
            std::fs::create_dir_all(member.join("src"))?;
            std::fs::write(
                member.join("Cargo.toml"),
                format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\n"),
            )?;
            std::fs::write(
                member.join("src").join("lib.rs"),
                format!("pub mod inner;\npub unsafe fn {name}_danger() {{}}\n"),
            )?;
            // A second file per member, so "only the changed file was re-read" is a
            // claim with something to distinguish.
            std::fs::write(
                member.join("src").join("inner.rs"),
                "pub trait Contract {}\npub fn helper() {}\n",
            )?;
        }
        Ok(())
    }

    #[test]
    fn indexing_a_workspace_root_answers_with_every_package_as_a_root() -> TestResult {
        // The command the editor actually sends, against the shape a repository actually
        // has. This used to answer `SeamIndexFailed` on every Cargo workspace.
        let dir = tempfile::tempdir()?;
        seam_workspace(dir.path(), &["alpha", "beta"])?;
        let (mut session, mut events, _snaps) = Session::new(SessionConfig {
            roots: vec![dir.path().to_path_buf()],
            ..SessionConfig::default()
        });
        session.handle(RequestId(1), Command::IndexSeams {
            root: None,
            mode: crate::api::SeamSync::Incremental,
        });
        let event = next_seam_answer(&mut events, "SeamIndexed")?;
        let Event::SeamIndexed { summary, nodes } = event else {
            return Err(format!("expected SeamIndexed, got {event:?}").into());
        };

        assert_eq!(summary.packages, 2);
        let roots: Vec<&str> = nodes
            .iter()
            .filter(|node| node.parent.is_none())
            .map(|node| node.name.as_str())
            .collect();
        assert_eq!(roots, ["alpha", "beta"]);
        assert!(nodes.iter().any(|n| n.id == "alpha::alpha_danger"));
        assert!(nodes.iter().any(|n| n.id == "beta::beta_danger"));
        Ok(())
    }

    #[test]
    fn a_workspace_summary_names_the_directory_rather_than_one_member() -> TestResult {
        let dir = tempfile::tempdir()?;
        seam_workspace(dir.path(), &["alpha", "beta"])?;
        let (mut session, mut events, _snaps) = Session::new(SessionConfig {
            roots: vec![dir.path().to_path_buf()],
            ..SessionConfig::default()
        });
        session.handle(RequestId(1), Command::IndexSeams {
            root: None,
            mode: crate::api::SeamSync::Incremental,
        });
        let event = next_seam_answer(&mut events, "SeamIndexed")?;
        let Event::SeamIndexed { summary, .. } = event else {
            return Err(format!("expected SeamIndexed, got {event:?}").into());
        };

        let expected = dir
            .path()
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or("the temp dir has no UTF-8 name")?;
        assert_eq!(summary.package, expected);
        assert_ne!(summary.package, "alpha");
        Ok(())
    }

    #[test]
    fn a_single_package_index_still_reports_one_package() -> TestResult {
        let (_session, _events, _dir, event) = indexed_session()?;
        let Event::SeamIndexed { summary, .. } = event else {
            return Err(format!("expected SeamIndexed, got {event:?}").into());
        };
        assert_eq!(summary.packages, 1);
        assert_eq!(summary.package, "seamdemo");
        Ok(())
    }

    #[test]
    fn the_configured_file_cap_is_what_the_index_is_built_under() -> TestResult {
        // Declared in settings since the view shipped and never read, which stops being
        // harmless once one index can span a whole repository.
        let dir = tempfile::tempdir()?;
        seam_workspace(dir.path(), &["alpha", "beta", "gamma"])?;
        let mut settings = crate::config::Settings::default();
        settings.seam.max_indexed_files = 1;
        let (mut session, mut events, _snaps) = Session::new(SessionConfig {
            roots: vec![dir.path().to_path_buf()],
            settings,
            ..SessionConfig::default()
        });
        session.handle(RequestId(1), Command::IndexSeams {
            root: None,
            mode: crate::api::SeamSync::Incremental,
        });
        let event = next_seam_answer(&mut events, "SeamIndexed")?;
        let Event::SeamIndexed { summary, .. } = event else {
            return Err(format!("expected SeamIndexed, got {event:?}").into());
        };
        assert_eq!(summary.truncated_after, Some(1));
        Ok(())
    }

    #[test]
    fn a_query_with_no_index_answers_rather_than_going_silent() -> TestResult {
        // Reachable the moment a reader can pick a start point: index somewhere with no
        // package, then type in the filter box. A dropped answer leaves the request
        // outstanding forever and the view believing a filter is still running.
        let dir = tempfile::tempdir()?;
        let (mut session, mut events, _snaps) = Session::new(SessionConfig {
            roots: vec![dir.path().to_path_buf()],
            ..SessionConfig::default()
        });
        session.handle(RequestId(1), Command::IndexSeams {
            root: None,
            mode: crate::api::SeamSync::Incremental,
        });
        let event = next_seam_answer(&mut events, "SeamIndexFailed")?;
        assert!(matches!(event, Event::SeamIndexFailed { .. }), "got {event:?}");

        session.handle(RequestId(2), Command::SeamQuery {
            text: "lens:api".to_owned(),
        });
        let answer = await_seam_event(&mut events);
        assert!(
            matches!(answer, Some(Event::SeamQueryResult { ref nodes, ref error, .. })
                if nodes.is_empty() && error.is_none()),
            "got {answer:?}"
        );
        Ok(())
    }

    #[test]
    fn asking_for_a_node_with_no_index_answers_with_no_edges() -> TestResult {
        let dir = tempfile::tempdir()?;
        let (mut session, mut events, _snaps) = Session::new(SessionConfig {
            roots: vec![dir.path().to_path_buf()],
            ..SessionConfig::default()
        });
        session.handle(RequestId(1), Command::IndexSeams {
            root: None,
            mode: crate::api::SeamSync::Incremental,
        });
        let event = next_seam_answer(&mut events, "SeamIndexFailed")?;
        assert!(matches!(event, Event::SeamIndexFailed { .. }), "got {event:?}");

        session.handle(RequestId(2), Command::SeamNode {
            path: "absent::thing".to_owned(),
        });
        let answer = await_seam_event(&mut events);
        assert!(
            matches!(answer, Some(Event::SeamNodeDetail { ref edges, .. }) if edges.is_empty()),
            "got {answer:?}"
        );
        Ok(())
    }

    /// A session over `dir`, storing its index in `cache`.
    fn cached_session(
        dir: &std::path::Path,
        cache: &std::path::Path,
    ) -> (Session, crate::session::EventRx) {
        let (session, events, _snaps) = Session::new(SessionConfig {
            roots: vec![dir.to_path_buf()],
            seam_cache_dir: Some(cache.to_path_buf()),
            ..SessionConfig::default()
        });
        (session, events)
    }

    #[test]
    fn an_index_arrives_as_one_event_per_package_closed_by_one_finish() -> TestResult {
        let dir = tempfile::tempdir()?;
        seam_workspace(dir.path(), &["alpha", "beta"])?;
        let (mut session, mut events, _snaps) = Session::new(SessionConfig {
            roots: vec![dir.path().to_path_buf()],
            ..SessionConfig::default()
        });
        session.handle(RequestId(1), Command::IndexSeams {
            root: None,
            mode: crate::api::SeamSync::Incremental,
        });

        let mut stream = await_index_stream(&mut events).ok_or(NO_STREAM)?;
        assert_eq!(
            stream.packages.len(),
            2,
            "one event per package, not one per index"
        );
        assert!(stream.packages.iter().all(|package| package.nodes > 0));
        assert_eq!(stream.files, 4, "two crates of two source files each");
        assert_eq!(
            stream.parsed, stream.files,
            "nothing was stored, so everything was parsed"
        );

        // The order each package carries is discovery order, whatever order they landed in.
        stream.packages.sort_by_key(|package| package.order);
        let names: Vec<&str> = stream
            .packages
            .iter()
            .map(|package| package.root.as_str())
            .collect();
        assert_eq!(names, ["alpha", "beta"]);
        Ok(())
    }

    #[test]
    fn a_second_index_replays_the_stored_one_instead_of_parsing() -> TestResult {
        let (dir, cache) = (tempfile::tempdir()?, tempfile::tempdir()?);
        seam_workspace(dir.path(), &["alpha", "beta"])?;

        // Cold: everything is parsed, and the result is stored.
        let (mut session, mut events) = cached_session(dir.path(), cache.path());
        session.handle(RequestId(1), Command::IndexSeams {
            root: None,
            mode: crate::api::SeamSync::Incremental,
        });
        let cold = await_index_stream(&mut events).ok_or(NO_STREAM)?;
        assert_eq!(cold.parsed, cold.files);
        drop(session);

        // Warm: the same workspace, a fresh session, nothing changed on disk.
        let (mut session, mut events) = cached_session(dir.path(), cache.path());
        session.handle(RequestId(2), Command::IndexSeams {
            root: None,
            mode: crate::api::SeamSync::Incremental,
        });
        let warm = await_index_stream(&mut events).ok_or(NO_STREAM)?;
        assert_eq!(warm.parsed, 0, "an unchanged workspace was parsed again");
        assert_eq!(warm.files, cold.files, "a warm index covers the same files");
        assert_eq!(warm.packages.len(), 2, "every package is still reported");
        Ok(())
    }

    #[test]
    fn a_changed_file_is_the_only_one_reread() -> TestResult {
        let (dir, cache) = (tempfile::tempdir()?, tempfile::tempdir()?);
        seam_workspace(dir.path(), &["alpha", "beta"])?;
        let (mut session, mut events) = cached_session(dir.path(), cache.path());
        session.handle(RequestId(1), Command::IndexSeams {
            root: None,
            mode: crate::api::SeamSync::Incremental,
        });
        await_index_stream(&mut events).ok_or(NO_STREAM)?;
        drop(session);

        let touched = dir.path().join("crates").join("alpha").join("src").join("inner.rs");
        std::fs::write(
            &touched,
            "pub trait Contract {}\npub fn helper() {}\npub fn extra() {}\n",
        )?;

        let (mut session, mut events) = cached_session(dir.path(), cache.path());
        session.handle(RequestId(2), Command::IndexSeams {
            root: None,
            mode: crate::api::SeamSync::Incremental,
        });
        let stream = await_index_stream(&mut events).ok_or(NO_STREAM)?;
        assert_eq!(stream.parsed, 1, "one file changed, so one file should be read");
        Ok(())
    }

    #[test]
    fn a_forced_sync_reads_everything_again() -> TestResult {
        let (dir, cache) = (tempfile::tempdir()?, tempfile::tempdir()?);
        seam_workspace(dir.path(), &["alpha", "beta"])?;
        let (mut session, mut events) = cached_session(dir.path(), cache.path());
        session.handle(RequestId(1), Command::IndexSeams {
            root: None,
            mode: crate::api::SeamSync::Incremental,
        });
        let first = await_index_stream(&mut events).ok_or(NO_STREAM)?;

        // The recourse when the stored index is itself suspect: nothing is trusted.
        session.handle(RequestId(2), Command::IndexSeams {
            root: None,
            mode: crate::api::SeamSync::Forced,
        });
        let forced = await_index_stream(&mut events).ok_or(NO_STREAM)?;
        assert_eq!(
            forced.parsed, forced.files,
            "a forced sync trusted something"
        );
        assert_eq!(forced.files, first.files);
        Ok(())
    }

    /// Not an assertion — a stopwatch over the path a reader actually takes.
    ///
    /// `KARET_BENCH_ROOT=. cargo test --release -p karet-session --all-features \
    ///   -- --ignored --nocapture bench_seam`
    #[test]
    #[ignore = "timing, not a correctness check"]
    fn bench_seam_cold_then_warm() -> TestResult {
        // Skip: the bench needs a real tree to time, named by KARET_BENCH_ROOT.
        let Ok(root) = std::env::var("KARET_BENCH_ROOT") else {
            return Ok(());
        };
        let root = std::path::PathBuf::from(root);
        let cache = tempfile::tempdir()?;

        for (label, request) in [("COLD", RequestId(1)), ("WARM", RequestId(2))] {
            let (mut session, mut events, _snaps) = Session::new(SessionConfig {
                roots: vec![root.clone()],
                seam_cache_dir: Some(cache.path().to_path_buf()),
                ..SessionConfig::default()
            });
            let start = std::time::Instant::now();
            session.handle(request, Command::IndexSeams {
                root: None,
                mode: crate::api::SeamSync::Incremental,
            });
            let stream = await_index_stream(&mut events).ok_or(NO_STREAM)?;
            let elapsed = start.elapsed();
            let nodes: usize = stream.packages.iter().map(|package| package.nodes).sum();
            println!(
                "{label} {elapsed:?} — {} packages, {nodes} nodes, {} of {} files parsed",
                stream.packages.len(),
                stream.parsed,
                stream.files
            );
        }
        Ok(())
    }
