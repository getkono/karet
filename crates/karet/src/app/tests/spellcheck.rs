    fn spell_diagnostic(message: &str) -> Diagnostic {
        Diagnostic {
            range: Range {
                start: LineCol::new(0, 0),
                end: LineCol::new(0, 4),
            },
            severity: Severity::Warning,
            message: message.to_string(),
            source: Some("karet-spell".to_string()),
            code: Some("en_US".to_string()),
            tags: Vec::new(),
            related: Vec::new(),
        }
    }

    fn spelling_app(root: PathBuf, message: &str) -> App {
        let mut app = App::new(root, Vec::new(), Vec::new(), false);
        app.push_tab(text_tab("notes.md", "wrod\n"));
        if let TabKind::Code { doc, .. } = &mut app.tabs[app.active].kind {
            *doc = Some(DocumentId(9));
        }
        app.document_diagnostics
            .insert(DocumentId(9), vec![spell_diagnostic(message)]);
        app.pane_frames = vec![content_frame(
            &app,
            Rect {
                x: 0,
                y: 0,
                width: 40,
                height: 5,
            },
        )];
        app
    }

    fn open_spelling_menu(app: &mut App) {
        let click = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            // One-line editors have a three-cell gutter; column 4 is inside "wrod".
            column: 4,
            row: 0,
            modifiers: KeyModifiers::NONE,
        };
        app.handle_editor_click(click);
        app.handle_editor_click(click);
    }

    #[test]
    fn double_clicking_a_spell_warning_opens_replacements_and_add_word() {
        let mut app = spelling_app(
            PathBuf::from("."),
            "Unknown word “wrod”; try word, rod",
        );

        open_spelling_menu(&mut app);

        let labels: Vec<&str> = app
            .context_menu
            .as_ref()
            .map(|menu| {
                menu.entries
                    .iter()
                    .filter_map(|entry| entry.label.as_deref())
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            labels,
            vec![
                "Replace with “word”",
                "Replace with “rod”",
                "Add “wrod” to Project Dictionary",
            ]
        );
        assert_eq!(
            app.tabs[app.active].editor.selection_range(),
            Some(Range {
                start: LineCol::new(0, 0),
                end: LineCol::new(0, 4),
            })
        );
    }

    #[test]
    fn warning_without_matches_renders_a_clear_empty_message() {
        let mut app = spelling_app(PathBuf::from("."), "Unknown word “wrod”");

        open_spelling_menu(&mut app);

        let menu = app.context_menu.as_ref().expect("spelling menu");
        assert_eq!(
            menu.entries[0].label.as_deref(),
            Some("No similar words found")
        );
        assert!(!menu.entries[0].enabled);
        assert_eq!(
            menu.entries[1].label.as_deref(),
            Some("Add “wrod” to Project Dictionary")
        );
        let painted = screen(&mut app, 80, 12).join("\n");
        assert!(painted.contains("No similar words found"), "{painted}");
    }

    #[test]
    fn accepting_a_spelling_suggestion_is_one_atomic_edit() {
        let backend = Arc::new(RecordingBackend::new());
        let mut app = spelling_app(
            PathBuf::from("."),
            "Unknown word “wrod”; try word",
        );
        app.backend = Some(backend.clone());
        open_spelling_menu(&mut app);

        app.accept_context_menu();

        assert_eq!(code_tab_text(&app), "word\n");
        let changes = backend
            .sent
            .lock()
            .map(|sent| {
                sent.iter()
                    .filter(|(_, command)| {
                        matches!(command, SessionCommand::ApplyChange { .. })
                    })
                    .count()
            })
            .unwrap_or_default();
        assert_eq!(changes, 1);
    }

    #[test]
    fn add_word_updates_an_existing_project_settings_file_without_prompt()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        std::fs::create_dir(dir.path().join(".git"))?;
        write_file(
            dir.path(),
            ".karet/setting.jsonc",
            b"{\n  // keep\n  \"spellcheck\": { \"enabled\": true }\n}\n",
        );
        let mut app = spelling_app(dir.path().to_path_buf(), "Unknown word “wrod”");
        open_spelling_menu(&mut app);
        if let Some(menu) = app.context_menu.as_mut() {
            menu.selected = 1;
        }

        app.accept_context_menu();

        assert!(app.overlay.is_none());
        let text = std::fs::read_to_string(dir.path().join(".karet/setting.jsonc"))?;
        assert!(text.contains("// keep"));
        assert!(text.contains("\"wrod\""));
        Ok(())
    }

    #[test]
    fn add_word_requires_typed_confirmation_before_creating_project_settings()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        std::fs::create_dir(dir.path().join(".git"))?;
        let path = dir.path().join(".karet/setting.jsonc");
        let mut app = spelling_app(dir.path().to_path_buf(), "Unknown word “wrod”");
        open_spelling_menu(&mut app);
        if let Some(menu) = app.context_menu.as_mut() {
            menu.selected = 1;
        }

        app.accept_context_menu();

        assert!(!path.exists());
        let overlay = app.overlay.as_mut().expect("typed creation prompt");
        assert!(overlay.title().contains("Type create"));
        overlay.push_str("create");
        app.overlay_accept();
        assert!(path.exists());
        assert!(std::fs::read_to_string(path)?.contains("\"wrod\""));
        Ok(())
    }
