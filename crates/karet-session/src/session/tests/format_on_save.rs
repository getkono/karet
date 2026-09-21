    use crate::config::schema::EditorOverride;
    use crate::config::schema::LanguageSelector;

    fn saved(events: &mut EventRx) -> bool {
        let mut found = false;
        while let Some((_, ev)) = events.try_recv() {
            found |= matches!(ev, Event::Saved { .. });
        }
        found
    }

    #[test]
    fn format_on_save_without_a_server_still_writes() {
        let Ok(dir) = tempfile::tempdir() else {
            return;
        };
        let path = dir.path().join("main.rs");
        if std::fs::write(&path, "fn  main() {}\n").is_err() {
            return;
        }
        let mut settings = crate::config::Settings::default();
        settings.editor.format_on_save = true;
        let (mut session, mut events, _snaps) = Session::new(SessionConfig {
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
        session.handle(RequestId(2), Command::Save { doc });
        assert!(saved(&mut events), "save must complete without a formatter");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap_or_default(),
            "fn  main() {}\n"
        );
    }

    #[cfg(feature = "toml-format")]
    #[test]
    fn format_on_save_applies_the_builtin_toml_formatter() {
        let Ok(dir) = tempfile::tempdir() else {
            return;
        };
        let path = dir.path().join("Cargo.toml");
        if std::fs::write(&path, "[package]\nname=\"x\"\n").is_err() {
            return;
        }
        let mut settings = crate::config::Settings::default();
        settings.editor.format_on_save = true;
        let (mut session, mut events, _snaps) = Session::new(SessionConfig {
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
        session.handle(RequestId(2), Command::Save { doc });
        assert!(saved(&mut events));
        assert_eq!(
            std::fs::read_to_string(&path).unwrap_or_default(),
            "[package]\nname = \"x\"\n"
        );
    }

    #[cfg(feature = "toml-format")]
    #[test]
    fn language_override_can_disable_format_on_save() {
        let Ok(dir) = tempfile::tempdir() else {
            return;
        };
        let path = dir.path().join("Cargo.toml");
        let messy = "[package]\nname=\"x\"\n";
        if std::fs::write(&path, messy).is_err() {
            return;
        }
        let mut settings = crate::config::Settings::default();
        settings.editor.format_on_save = true;
        if let Some(selector) = LanguageSelector::from_language("toml") {
            settings.editor.language_overrides.insert(
                selector,
                EditorOverride {
                    format_on_save: Some(false),
                    ..EditorOverride::default()
                },
            );
        }
        let (mut session, mut events, _snaps) = Session::new(SessionConfig {
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
        session.handle(RequestId(2), Command::Save { doc });
        assert!(saved(&mut events));
        assert_eq!(std::fs::read_to_string(&path).unwrap_or_default(), messy);
    }
