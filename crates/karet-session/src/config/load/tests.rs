
use super::*;
use crate::config::schema::AutoSave;
use crate::config::schema::LineNumbers;

/// Create a temp dir and write `body` to `name` inside it. Returns `None` (so the
/// test skips) on any IO failure, matching the crate's unwrap-free test idiom.
fn scratch(name: &str, body: &str) -> Option<(tempfile::TempDir, PathBuf)> {
    let dir = tempfile::tempdir().ok()?;
    let path = dir.path().join(name);
    std::fs::write(&path, body).ok()?;
    Some((dir, path))
}

#[test]
fn absent_files_yield_defaults_without_diagnostics() {
    let (settings, diags) = load_layers([PathBuf::from("/nonexistent/setting.jsonc")]);
    assert_eq!(settings, Settings::default());
    assert!(diags.is_empty());
}

#[test]
fn jsonc_comments_and_trailing_commas_parse() {
    let Some((_dir, path)) = scratch(
        "setting.jsonc",
        r#"{
                // a line comment
                "editor": { "tabSize": 2, }, /* trailing comma + block comment */
            }"#,
    ) else {
        return;
    };
    let (settings, diags) = load_layers([path]);
    assert!(diags.is_empty(), "{diags:?}");
    assert_eq!(settings.editor.tab_size, 2);
    // Unset fields keep their sane defaults.
    assert!(settings.editor.insert_spaces);
}

#[test]
fn later_layers_override_earlier_and_merge_deeply() {
    let Some((dir, _system)) = scratch(
        "system.jsonc",
        r#"{ "editor": { "tabSize": 8, "lineNumbers": "relative" } }"#,
    ) else {
        return;
    };
    let system = dir.path().join("system.jsonc");
    let project = dir.path().join("project.jsonc");
    if std::fs::write(&project, r#"{ "editor": { "tabSize": 2 } }"#).is_err() {
        return;
    }
    // Order is low → high precedence.
    let (settings, diags) = load_layers([system, project]);
    assert!(diags.is_empty(), "{diags:?}");
    // project wins tabSize; system's lineNumbers survives the deep merge.
    assert_eq!(settings.editor.tab_size, 2);
    assert_eq!(settings.editor.line_numbers, LineNumbers::Relative);
}

#[test]
fn ordered_layers_lists_system_then_user_then_project_skipping_absent_tiers() {
    let (system, user, project) = (
        PathBuf::from("/s.jsonc"),
        PathBuf::from("/u.jsonc"),
        PathBuf::from("/p.jsonc"),
    );
    let all: Vec<_> = ordered_layers(
        Some(system.clone()),
        Some(user.clone()),
        Some(project.clone()),
    )
    .collect();
    assert_eq!(
        all,
        vec![
            (ConfigLayer::System, system.clone()),
            (ConfigLayer::User, user),
            (ConfigLayer::Project, project.clone()),
        ]
    );
    let sparse: Vec<_> =
        ordered_layers(Some(system.clone()), None, Some(project.clone())).collect();
    assert_eq!(
        sparse,
        vec![
            (ConfigLayer::System, system),
            (ConfigLayer::Project, project)
        ]
    );
    assert_eq!(ordered_layers(None, None, None).count(), 0);
}

#[test]
fn project_beats_user_beats_system_in_the_load_report_order() {
    let Ok(dir) = tempfile::tempdir() else {
        return;
    };
    let write = |name: &str, body: &str| {
        let path = dir.path().join(name);
        std::fs::write(&path, body).ok().map(|()| path)
    };
    let (Some(system), Some(user), Some(project)) = (
        write(
            "system.jsonc",
            r#"{ "editor": { "tabSize": 8, "lineNumbers": "relative" } }"#,
        ),
        write(
            "user.jsonc",
            r#"{ "editor": { "tabSize": 6, "insertSpaces": false } }"#,
        ),
        write("project.jsonc", r#"{ "editor": { "tabSize": 2 } }"#),
    ) else {
        return;
    };

    let report = load_layer_reports(ordered_layers(
        Some(system.clone()),
        Some(user.clone()),
        Some(project),
    ));
    assert!(report.diagnostics.is_empty(), "{:?}", report.diagnostics);
    assert_eq!(
        report.settings.editor.tab_size, 2,
        "project wins over user and system"
    );
    assert_eq!(
        report.explicit.get("editor.tabSize"),
        Some(&ConfigLayer::Project)
    );
    assert_eq!(
        report.explicit.get("editor.insertSpaces"),
        Some(&ConfigLayer::User)
    );
    assert_eq!(
        report.explicit.get("editor.lineNumbers"),
        Some(&ConfigLayer::System)
    );
    assert_eq!(report.settings.editor.line_numbers, LineNumbers::Relative);

    // Without a project tier the user layer wins over the system one.
    let report = load_layer_reports(ordered_layers(Some(system), Some(user), None));
    assert_eq!(report.settings.editor.tab_size, 6, "user wins over system");
    assert_eq!(
        report.explicit.get("editor.tabSize"),
        Some(&ConfigLayer::User)
    );
}

#[test]
fn language_specificity_wins_after_layer_merging() {
    let Some((dir, _system)) = scratch(
        "system.jsonc",
        r#"{
                "editor": {
                    "tabSize": 8,
                    "[rust]": {
                        "tabSize": 6,
                        "completion": { "enabled": false, "autoTrigger": true }
                    }
                }
            }"#,
    ) else {
        return;
    };
    let system = dir.path().join("system.jsonc");
    let project = dir.path().join("project.jsonc");
    if std::fs::write(
        &project,
        r#"{
                "editor": {
                    "tabSize": 2,
                    "[rust]": { "completion": { "autoTrigger": false } }
                }
            }"#,
    )
    .is_err()
    {
        return;
    }

    let report = load_layer_reports([
        (ConfigLayer::System, system),
        (ConfigLayer::Project, project),
    ]);
    assert!(report.diagnostics.is_empty(), "{:?}", report.diagnostics);
    let rust = report.settings.editor.for_language(Some("Rust"));
    let python = report.settings.editor.for_language(Some("python"));
    assert_eq!(rust.tab_size(), 6, "selector specificity wins");
    assert!(!rust.completion().enabled());
    assert!(!rust.completion().auto_trigger(), "nested patches merge");
    assert_eq!(
        python.tab_size(),
        2,
        "project global wins for other languages"
    );
    assert_eq!(
        report.explicit.get("editor.[rust].completion.autoTrigger"),
        Some(&ConfigLayer::Project)
    );
    assert_eq!(
        report.explicit.get("editor.[rust].tabSize"),
        Some(&ConfigLayer::System)
    );
}

#[test]
fn report_tracks_loaded_layers_and_explicit_leaf_sources() {
    let Some((dir, _system)) = scratch(
        "system.jsonc",
        r#"{ "editor": { "tabSize": 4, "lineNumbers": "relative" } }"#,
    ) else {
        return;
    };
    let system = dir.path().join("system.jsonc");
    let user = dir.path().join("user.jsonc");
    let project = dir.path().join("project.jsonc");
    if std::fs::write(&project, r#"{ "editor": { "tabSize": 2 } }"#).is_err() {
        return;
    }

    let report = load_layer_reports([
        (ConfigLayer::System, system),
        (ConfigLayer::User, user),
        (ConfigLayer::Project, project),
    ]);
    assert_eq!(report.settings.editor.tab_size, 2);
    assert_eq!(
        report.explicit.get("editor.tabSize"),
        Some(&ConfigLayer::Project)
    );
    assert_eq!(
        report.explicit.get("editor.lineNumbers"),
        Some(&ConfigLayer::System)
    );
    assert_eq!(report.layers.len(), 3);
    assert!(matches!(
        report.layers[1].status,
        ConfigLayerStatus::Missing
    ));
}

#[test]
fn manager_rereads_only_changed_layers_and_falls_back_after_deletion() {
    let Some((dir, path)) = scratch("setting.jsonc", r#"{ "editor": { "tabSize": 2 } }"#) else {
        return;
    };
    let report = load_layer_reports([(ConfigLayer::Project, path.clone())]);
    let Some(mut manager) = ConfigManager::from_loaded(&report) else {
        return;
    };
    assert!(
        manager
            .reload(&[dir.path().join("unrelated.jsonc")])
            .is_none()
    );

    if std::fs::write(&path, r#"{ "editor": { "tabSize": 6 } }"#).is_err() {
        return;
    }
    let Some(updated) = manager.reload(std::slice::from_ref(&path)) else {
        return;
    };
    assert_eq!(updated.settings.editor.tab_size, 6);
    assert_eq!(
        updated.explicit.get("editor.tabSize"),
        Some(&ConfigLayer::Project)
    );

    if std::fs::remove_file(&path).is_err() {
        return;
    }
    let Some(missing) = manager.reload(std::slice::from_ref(&path)) else {
        return;
    };
    assert_eq!(missing.settings.editor.tab_size, 4);
    assert!(matches!(
        missing.layers[0].status,
        ConfigLayerStatus::Missing
    ));
}

#[test]
fn manager_retains_last_valid_layer_until_an_invalid_edit_recovers() {
    let Some((_dir, path)) = scratch("setting.jsonc", r#"{ "editor": { "tabSize": 2 } }"#) else {
        return;
    };
    let report = load_layer_reports([(ConfigLayer::Project, path.clone())]);
    let Some(mut manager) = ConfigManager::from_loaded(&report) else {
        return;
    };

    if std::fs::write(&path, r#"{ "editor": { "tabSize": "wide" } }"#).is_err() {
        return;
    }
    let Some(invalid) = manager.reload(std::slice::from_ref(&path)) else {
        return;
    };
    assert_eq!(
        invalid.settings.editor.tab_size, 2,
        "a malformed live edit must not replace the last valid layer"
    );
    assert!(matches!(
        invalid.layers[0].status,
        ConfigLayerStatus::Invalid(_)
    ));
    assert_eq!(invalid.diagnostics.len(), 1);

    if std::fs::write(&path, r#"{ "editor": { "tabSize": 3 } }"#).is_err() {
        return;
    }
    let Some(recovered) = manager.reload(std::slice::from_ref(&path)) else {
        return;
    };
    assert_eq!(recovered.settings.editor.tab_size, 3);
    assert!(recovered.diagnostics.is_empty());
    assert!(matches!(
        recovered.layers[0].status,
        ConfigLayerStatus::Loaded
    ));
}

#[test]
fn invalid_sections_do_not_remain_explicit() {
    let Some((_dir, path)) = scratch(
        "setting.jsonc",
        r#"{ "editor": { "wat": 1 }, "files": { "backup": true } }"#,
    ) else {
        return;
    };
    let report = load_layer_reports([(ConfigLayer::Project, path)]);
    assert!(!report.explicit.contains_key("editor.wat"));
    assert!(!report.explicit.contains_key("editor.tabSize"));
    assert_eq!(
        report.explicit.get("files.backup"),
        Some(&ConfigLayer::Project)
    );
}

#[test]
fn unknown_key_warns_but_keeps_valid_settings() {
    let Some((_dir, path)) = scratch(
        "setting.jsonc",
        r#"{ "editor": { "tabSize": 3 }, "nonsense": true }"#,
    ) else {
        return;
    };
    let (settings, diags) = load_layers([path]);
    assert_eq!(settings.editor.tab_size, 3);
    assert_eq!(diags.len(), 1);
    assert!(diags[0].message.contains("unknown setting `nonsense`"));
}

#[test]
fn bad_section_resets_only_itself_and_warns() {
    // `editor.wat` is unknown (deny_unknown_fields) → the editor section is
    // rejected; `files` is untouched and still applies.
    let Some((_dir, path)) = scratch(
        "setting.jsonc",
        r#"{ "editor": { "wat": 1 }, "files": { "autoSave": "afterDelay" } }"#,
    ) else {
        return;
    };
    let (settings, diags) = load_layers([path]);
    assert_eq!(settings.editor, Default::default());
    assert_eq!(settings.files.auto_save, AutoSave::AfterDelay);
    assert_eq!(diags.len(), 1);
    assert!(diags[0].message.contains("invalid `editor` settings"));
}

#[test]
fn syntax_error_is_reported_and_degrades_to_defaults() {
    let Some((_dir, path)) = scratch("setting.jsonc", "{ this is not json ") else {
        return;
    };
    let (settings, diags) = load_layers([path]);
    assert_eq!(settings, Settings::default());
    assert_eq!(diags.len(), 1);
    assert!(diags[0].message.contains("invalid JSONC"));
}

#[test]
fn project_layer_is_discovered_under_the_git_root() {
    let Some(tmp) = tempfile::tempdir().ok() else {
        return;
    };
    let root = tmp.path().join("repo");
    let nested = root.join("crates").join("thing");
    if std::fs::create_dir_all(&nested).is_err()
        || std::fs::create_dir_all(root.join(".git")).is_err()
    {
        return;
    }
    // From a nested workspace root, the project layer resolves to the git root.
    assert_eq!(
        project_config_path(std::slice::from_ref(&nested)),
        Some(root.join(".karet").join("setting.jsonc"))
    );
    // The directory other per-project config files share.
    assert_eq!(project_config_dir(&[nested]), Some(root.join(".karet")));
}

/// Every entry in [`SECTIONS`] must actually be deserialized into
/// [`Settings`].
///
/// A section listed there but missing a match arm in
/// [`deserialize_sections`] is invisible twice over: it is not "unknown",
/// so no diagnostic fires, and it never reaches `Settings`, so the value
/// silently stays at its default. Five sections shipped that way. This
/// pins each one to a value that differs from the default, so a new
/// section cannot be added to the list and forgotten.
#[test]
fn every_section_round_trips_a_value() {
    /// One section's probe: its key, a `setting.jsonc` body holding a
    /// non-default value, and the check that the value landed.
    type Probe = (&'static str, &'static str, fn(&Settings) -> bool);

    // One non-default value per section, as it appears in `setting.jsonc`.
    let probes: &[Probe] = &[
        ("editor", r#"{"tabSize": 7}"#, |s| s.editor.tab_size == 7),
        ("files", r#"{"autoSave": "afterDelay"}"#, |s| {
            s.files.auto_save != Settings::default().files.auto_save
        }),
        ("workbench", r#"{"colorTheme": "probe"}"#, |s| {
            s.workbench.color_theme == "probe"
        }),
        ("search", r#"{"exclude": ["probe"]}"#, |s| {
            s.search.exclude.iter().any(|e| e == "probe")
        }),
        ("seam", r#"{"hideInactive": true}"#, |s| {
            s.seam.hide_inactive
        }),
        ("spellcheck", r#"{"enabled": false}"#, |s| {
            !s.spellcheck.enabled
        }),
        ("latex", r#"{"outputDirectory": "probe"}"#, |s| {
            s.latex.output_directory == "probe"
        }),
        ("git", r#"{"issueUrl": "probe"}"#, |s| {
            s.git.issue_url.as_deref() == Some("probe")
        }),
        ("lsp", r#"{"enabled": false}"#, |s| !s.lsp.enabled),
        ("markdown", r#"{"listContinuation": false}"#, |s| {
            !s.markdown.list_continuation
        }),
        ("wakatime", r#"{"enabled": true}"#, |s| s.wakatime.enabled),
        ("deps", r#"{"enabled": false}"#, |s| !s.deps.enabled),
        ("toml", r#"{"format": false}"#, |s| !s.toml.format),
        ("debug", r#"{"configurations": [{"name": "probe"}]}"#, |s| {
            s.debug.configurations.iter().any(|c| c.name == "probe")
        }),
        ("notebook", r#"{"kernel": {"autoStart": true}}"#, |s| {
            s.notebook.kernel.auto_start
        }),
    ];
    assert_eq!(
        probes.len(),
        SECTIONS.len(),
        "a section was added to SECTIONS without a probe here"
    );

    for (name, body, applied) in probes {
        assert!(
            SECTIONS.contains(name),
            "{name} is probed but not in SECTIONS"
        );
        let Ok(value) = serde_json::from_str::<Value>(body) else {
            continue;
        };
        let mut merged = Map::new();
        merged.insert((*name).to_owned(), value);
        let mut diags = Vec::new();
        let mut invalid = BTreeSet::new();
        let settings = deserialize_sections(&merged, &mut diags, &mut invalid);
        assert!(diags.is_empty(), "{name} reported {diags:?}");
        assert!(
            applied(&settings),
            "`{name}` parsed without complaint but never reached Settings"
        );
    }
}

#[test]
fn git_root_is_found_by_ascending() {
    let Some(tmp) = tempfile::tempdir().ok() else {
        return;
    };
    let root = tmp.path().join("repo");
    let nested = root.join("crates").join("thing");
    if std::fs::create_dir_all(&nested).is_err()
        || std::fs::create_dir_all(root.join(".git")).is_err()
    {
        return;
    }
    assert_eq!(git_root(&nested).as_deref(), Some(root.as_path()));
}
