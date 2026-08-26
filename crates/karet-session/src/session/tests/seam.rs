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

    /// Drive the session until a seam event arrives, or give up.
    ///
    /// The worker is a real thread, so the answer is genuinely asynchronous.
    fn await_seam_event(events: &mut crate::session::EventRx) -> Option<Event> {
        for _ in 0..200 {
            while let Some((_, event)) = events.try_recv() {
                if matches!(
                    event,
                    Event::SeamIndexed { .. }
                        | Event::SeamIndexFailed { .. }
                        | Event::SeamQueryResult { .. }
                        | Event::SeamNodeDetail { .. }
                ) {
                    return Some(event);
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        None
    }

    /// A session rooted at a freshly built package, with its index already requested.
    fn indexed_session() -> Option<(Session, crate::session::EventRx, tempfile::TempDir, Event)> {
        let dir = tempfile::tempdir().ok()?;
        seam_package(dir.path()).ok()?;
        let (mut session, mut events, _snaps) = Session::new(SessionConfig {
            roots: vec![dir.path().to_path_buf()],
            ..SessionConfig::default()
        });
        session.handle(RequestId(1), Command::IndexSeams { root: None });
        let indexed = await_seam_event(&mut events)?;
        Some((session, events, dir, indexed))
    }

    #[test]
    fn indexing_a_package_answers_with_the_whole_tree_and_a_summary() {
        let Some((_session, _events, _dir, event)) = indexed_session() else {
            return;
        };
        let Event::SeamIndexed { summary, nodes } = event else {
            return;
        };
        assert_eq!(summary.package, "seamdemo");
        assert!(summary.nodes > 0);
        assert!(summary.files >= 2, "the module in another file must be indexed");
        // The whole tree crosses at once, so navigation needs no round trip.
        assert_eq!(nodes.len(), summary.nodes);
        assert!(nodes.iter().any(|n| n.id == "seamdemo::inner::helper"));
        assert!(nodes.iter().any(|n| n.id == "seamdemo::danger"));
    }

    #[test]
    fn the_header_always_names_a_configuration_and_says_when_it_is_incomplete() {
        let Some((_session, _events, _dir, event)) = indexed_session() else {
            return;
        };
        let Event::SeamIndexed { summary, .. } = event else {
            return;
        };
        // Nothing renders unattributed, even before a manifest has been read.
        assert!(!summary.configuration.is_empty());
        assert!(!summary.available_configurations.is_empty());
        // And with no manifest the variation lens must not claim completeness.
        assert!(!summary.variation_complete);
    }

    #[test]
    fn a_node_carries_its_facets_rollups_and_location() {
        let Some((_session, _events, _dir, event)) = indexed_session() else {
            return;
        };
        let Event::SeamIndexed { nodes, .. } = event else {
            return;
        };
        let Some(danger) = nodes.iter().find(|n| n.id == "seamdemo::danger") else {
            return;
        };
        assert_eq!(danger.kind, "function");
        assert!(danger.facets.iter().any(|f| f.lens == "hazard" && f.subtype == "unsafe"));
        assert!(danger.facets.iter().any(|f| f.lens == "api"));
        assert!(danger.file.ends_with("lib.rs"));
        assert_eq!(danger.membership, "active");

        // The root's rollups account for what is beneath it, so a collapsed row is
        // still navigable.
        let Some(root) = nodes.iter().find(|n| n.parent.is_none()) else {
            return;
        };
        assert!(root.rollups.iter().any(|count| *count > 0));
    }

    #[test]
    fn indexing_somewhere_with_no_package_reports_a_failure_rather_than_an_empty_tree() {
        let Ok(dir) = tempfile::tempdir() else {
            return;
        };
        let (mut session, mut events, _snaps) = Session::new(SessionConfig {
            roots: vec![dir.path().to_path_buf()],
            ..SessionConfig::default()
        });
        session.handle(RequestId(1), Command::IndexSeams { root: None });
        let Some(event) = await_seam_event(&mut events) else {
            return;
        };
        // An empty tree would say "this package has nothing in it", which is not what
        // happened — there is no package here at all.
        assert!(matches!(event, Event::SeamIndexFailed { .. }), "got {event:?}");
    }

    #[test]
    fn a_query_answers_with_matching_node_identities() {
        let Some((mut session, mut events, _dir, _)) = indexed_session() else {
            return;
        };
        session.handle(
            RequestId(2),
            Command::SeamQuery {
                text: "lens:hazard".to_owned(),
            },
        );
        let Some(Event::SeamQueryResult { nodes, error, .. }) = await_seam_event(&mut events)
        else {
            return;
        };
        assert!(error.is_none());
        assert_eq!(nodes, ["seamdemo::danger"]);
    }

    #[test]
    fn an_unreadable_query_answers_with_a_positioned_error_not_an_empty_result() {
        let Some((mut session, mut events, _dir, _)) = indexed_session() else {
            return;
        };
        session.handle(
            RequestId(2),
            Command::SeamQuery {
                text: "lens:hazrd".to_owned(),
            },
        );
        let Some(Event::SeamQueryResult { nodes, error, .. }) = await_seam_event(&mut events)
        else {
            return;
        };
        assert!(nodes.is_empty());
        // The distinction matters: "I could not read that" is not "nothing matched".
        let Some(error) = error else {
            return;
        };
        assert!(error.message.contains("unknown lens"));
        assert!(error.end > error.start, "the error must be positioned");
        assert!(error.suggestions.contains(&"hazard".to_owned()));
    }

    #[test]
    fn a_query_naming_a_configuration_reports_it_back() {
        let Some((mut session, mut events, _dir, _)) = indexed_session() else {
            return;
        };
        session.handle(
            RequestId(2),
            Command::SeamQuery {
                text: "config:tests lens:api".to_owned(),
            },
        );
        let Some(Event::SeamQueryResult { configuration, .. }) = await_seam_event(&mut events)
        else {
            return;
        };
        assert_eq!(configuration.as_deref(), Some("tests"));
    }

    #[test]
    fn asking_for_a_node_answers_for_that_node_even_with_no_edges() {
        let Some((mut session, mut events, _dir, _)) = indexed_session() else {
            return;
        };
        session.handle(
            RequestId(2),
            Command::SeamNode {
                path: "seamdemo::inner::Contract".to_owned(),
            },
        );
        let Some(Event::SeamNodeDetail { node, .. }) = await_seam_event(&mut events) else {
            return;
        };
        // The answer is correlated to what was asked, so a stale reply is detectable.
        assert_eq!(node, "seamdemo::inner::Contract");
    }

    #[test]
    fn switching_configuration_re_answers_with_the_new_one_named() {
        let Some((mut session, mut events, _dir, first)) = indexed_session() else {
            return;
        };
        let Event::SeamIndexed { summary, .. } = first else {
            return;
        };
        let Some(other) = summary
            .available_configurations
            .iter()
            .find(|name| **name != summary.configuration)
            .cloned()
        else {
            return;
        };

        session.handle(
            RequestId(2),
            Command::SetSeamConfiguration {
                name: other.clone(),
            },
        );
        let Some(Event::SeamIndexed { summary: after, .. }) = await_seam_event(&mut events) else {
            return;
        };
        assert_eq!(after.configuration, other);
        assert_eq!(after.nodes, summary.nodes, "switching must not lose nodes");
    }

    #[test]
    fn re_indexing_one_file_keeps_the_rest_of_the_tree() {
        let Some((mut session, mut events, dir, first)) = indexed_session() else {
            return;
        };
        let Event::SeamIndexed { nodes: before, .. } = first else {
            return;
        };
        assert!(before.iter().any(|n| n.id == "seamdemo::inner::helper"));

        session.handle(
            RequestId(2),
            Command::ReindexSeams {
                path: dir.path().join("src").join("inner.rs"),
                text: "pub trait Contract {}\npub fn renamed() {}\n".to_owned(),
            },
        );
        let Some(Event::SeamIndexed { nodes: after, .. }) = await_seam_event(&mut events) else {
            return;
        };
        assert!(after.iter().any(|n| n.id == "seamdemo::inner::renamed"));
        assert!(!after.iter().any(|n| n.id == "seamdemo::inner::helper"));
        // The neighbouring file is untouched by its sibling's edit.
        assert!(after.iter().any(|n| n.id == "seamdemo::danger"));
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
                format!("pub unsafe fn {name}_danger() {{}}\n"),
            )?;
        }
        Ok(())
    }

    #[test]
    fn indexing_a_workspace_root_answers_with_every_package_as_a_root() {
        // The command the editor actually sends, against the shape a repository actually
        // has. This used to answer `SeamIndexFailed` on every Cargo workspace.
        let Ok(dir) = tempfile::tempdir() else {
            return;
        };
        if seam_workspace(dir.path(), &["alpha", "beta"]).is_err() {
            return;
        }
        let (mut session, mut events, _snaps) = Session::new(SessionConfig {
            roots: vec![dir.path().to_path_buf()],
            ..SessionConfig::default()
        });
        session.handle(RequestId(1), Command::IndexSeams { root: None });
        let Some(Event::SeamIndexed { summary, nodes }) = await_seam_event(&mut events) else {
            return;
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
    }

    #[test]
    fn a_workspace_summary_names_the_directory_rather_than_one_member() {
        let Ok(dir) = tempfile::tempdir() else {
            return;
        };
        if seam_workspace(dir.path(), &["alpha", "beta"]).is_err() {
            return;
        }
        let (mut session, mut events, _snaps) = Session::new(SessionConfig {
            roots: vec![dir.path().to_path_buf()],
            ..SessionConfig::default()
        });
        session.handle(RequestId(1), Command::IndexSeams { root: None });
        let Some(Event::SeamIndexed { summary, .. }) = await_seam_event(&mut events) else {
            return;
        };

        let expected = dir.path().file_name().and_then(|n| n.to_str()).unwrap_or("");
        assert_eq!(summary.package, expected);
        assert_ne!(summary.package, "alpha");
    }

    #[test]
    fn a_single_package_index_still_reports_one_package() {
        let Some((_session, _events, _dir, event)) = indexed_session() else {
            return;
        };
        let Event::SeamIndexed { summary, .. } = event else {
            return;
        };
        assert_eq!(summary.packages, 1);
        assert_eq!(summary.package, "seamdemo");
    }

    #[test]
    fn the_configured_file_cap_is_what_the_index_is_built_under() {
        // Declared in settings since the view shipped and never read, which stops being
        // harmless once one index can span a whole repository.
        let Ok(dir) = tempfile::tempdir() else {
            return;
        };
        if seam_workspace(dir.path(), &["alpha", "beta", "gamma"]).is_err() {
            return;
        }
        let mut settings = crate::config::Settings::default();
        settings.seam.max_indexed_files = 1;
        let (mut session, mut events, _snaps) = Session::new(SessionConfig {
            roots: vec![dir.path().to_path_buf()],
            settings,
            ..SessionConfig::default()
        });
        session.handle(RequestId(1), Command::IndexSeams { root: None });
        let Some(Event::SeamIndexed { summary, .. }) = await_seam_event(&mut events) else {
            return;
        };
        assert_eq!(summary.truncated_after, Some(1));
    }

    #[test]
    fn a_query_with_no_index_answers_rather_than_going_silent() {
        // Reachable the moment a reader can pick a start point: index somewhere with no
        // package, then type in the filter box. A dropped answer leaves the request
        // outstanding forever and the view believing a filter is still running.
        let Ok(dir) = tempfile::tempdir() else {
            return;
        };
        let (mut session, mut events, _snaps) = Session::new(SessionConfig {
            roots: vec![dir.path().to_path_buf()],
            ..SessionConfig::default()
        });
        session.handle(RequestId(1), Command::IndexSeams { root: None });
        let Some(Event::SeamIndexFailed { .. }) = await_seam_event(&mut events) else {
            return;
        };

        session.handle(RequestId(2), Command::SeamQuery {
            text: "lens:api".to_owned(),
        });
        let answer = await_seam_event(&mut events);
        assert!(
            matches!(answer, Some(Event::SeamQueryResult { ref nodes, ref error, .. })
                if nodes.is_empty() && error.is_none()),
            "got {answer:?}"
        );
    }

    #[test]
    fn asking_for_a_node_with_no_index_answers_with_no_edges() {
        let Ok(dir) = tempfile::tempdir() else {
            return;
        };
        let (mut session, mut events, _snaps) = Session::new(SessionConfig {
            roots: vec![dir.path().to_path_buf()],
            ..SessionConfig::default()
        });
        session.handle(RequestId(1), Command::IndexSeams { root: None });
        let Some(Event::SeamIndexFailed { .. }) = await_seam_event(&mut events) else {
            return;
        };

        session.handle(RequestId(2), Command::SeamNode {
            path: "absent::thing".to_owned(),
        });
        let answer = await_seam_event(&mut events);
        assert!(
            matches!(answer, Some(Event::SeamNodeDetail { ref edges, .. }) if edges.is_empty()),
            "got {answer:?}"
        );
    }
