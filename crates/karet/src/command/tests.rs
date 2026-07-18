use super::resolve::*;
use super::*;

#[test]
fn every_palette_command_has_a_label() {
    for cmd in palette() {
        assert!(!cmd.label().is_empty(), "{cmd:?} has no label");
        assert!(cmd.in_palette(), "{cmd:?} listed but not in_palette");
    }
}

#[test]
fn command_palette_itself_is_not_listed() {
    assert!(!palette().contains(&Command::OpenCommandPalette));
}

#[test]
fn resolve_named_matches_a_title_exactly() {
    assert_eq!(
        resolve_named("Source Control: Commit Graph"),
        Ok(Command::ShowCommitGraph)
    );
}

#[test]
fn resolve_named_is_case_insensitive() {
    assert_eq!(
        resolve_named("source control: commit graph"),
        Ok(Command::ShowCommitGraph)
    );
    assert_eq!(
        resolve_named("SOURCE CONTROL: COMMIT GRAPH"),
        Ok(Command::ShowCommitGraph)
    );
}

#[test]
fn resolve_named_matches_a_unique_slug() {
    assert_eq!(resolve_named("graph"), Ok(Command::ShowCommitGraph));
    assert_eq!(resolve_named("GRAPH"), Ok(Command::ShowCommitGraph));
    assert_eq!(resolve_named("deps"), Ok(Command::ShowDependencyGraph));
}

#[test]
fn resolve_named_rejects_an_ambiguous_slug_with_candidates() {
    // "refresh" is the slug of both Source Control: Refresh and Explorer: Refresh.
    let err = resolve_named("refresh");
    match err {
        Err(ResolveNamedError::Ambiguous { name, candidates }) => {
            assert_eq!(name, "refresh");
            assert!(candidates.contains(&"Source Control: Refresh"));
            assert!(candidates.contains(&"Explorer: Refresh"));
        },
        other => panic!("expected Ambiguous, got {other:?}"),
    }
}

#[test]
fn resolve_named_unknown_offers_close_suggestions() {
    // A typo of a real title still points at it.
    match resolve_named("comit graph") {
        Err(ResolveNamedError::Unknown { name, suggestions }) => {
            assert_eq!(name, "comit graph");
            assert!(
                suggestions.contains(&"Source Control: Commit Graph"),
                "suggestions {suggestions:?} should include the commit graph"
            );
            assert!(suggestions.len() <= MAX_SUGGESTIONS);
        },
        other => panic!("expected Unknown, got {other:?}"),
    }
}

#[test]
fn resolve_named_rejects_non_palette_commands() {
    // A modal-scoped command's title must not be reachable from the CLI.
    assert!(matches!(
        resolve_named("Overlay: Accept"),
        Err(ResolveNamedError::Unknown { .. })
    ));
}

#[test]
fn resolve_named_far_off_garbage_gets_no_suggestions() {
    match resolve_named("zzzzqqqqxxxx") {
        Err(ResolveNamedError::Unknown { suggestions, .. }) => {
            assert!(
                suggestions.is_empty(),
                "garbage should not fish up unrelated titles: {suggestions:?}"
            );
        },
        other => panic!("expected Unknown, got {other:?}"),
    }
}

#[test]
fn resolve_errors_render_a_clear_message() {
    let unknown = ResolveNamedError::Unknown {
        name: "comit".to_string(),
        suggestions: vec!["Source Control: Commit…"],
    };
    assert_eq!(
        unknown.to_string(),
        "unknown command \"comit\"; did you mean: Source Control: Commit…?"
    );
    let ambiguous = ResolveNamedError::Ambiguous {
        name: "refresh".to_string(),
        candidates: vec!["Source Control: Refresh", "Explorer: Refresh"],
    };
    assert_eq!(
        ambiguous.to_string(),
        "\"refresh\" matches more than one command: Source Control: Refresh, \
             Explorer: Refresh; use the full title"
    );
}

#[test]
fn substring_distance_basics() {
    // Exact and contained patterns are free.
    assert_eq!(substring_distance("abc", "abc"), 0);
    assert_eq!(
        substring_distance("commit", "source control: commit graph"),
        0
    );
    // A typo inside a long title costs only its own edits.
    assert_eq!(
        substring_distance("comit graph", "source control: commit graph"),
        1
    );
    // Degenerate inputs.
    assert_eq!(substring_distance("", "anything"), 0);
    assert_eq!(substring_distance("abc", ""), 3);
}

#[test]
fn loaded_config_is_in_the_palette() {
    assert!(palette().contains(&Command::ShowLoadedConfig));
    assert_eq!(
        Command::ShowLoadedConfig.label(),
        "Settings: Show Loaded Configuration"
    );
}

#[test]
fn hint_verbs_are_terse_and_gate_motion_keys() {
    // Advertised commands carry a non-empty terse verb…
    for cmd in [
        Command::Save,
        Command::Copy,
        Command::ScmStage,
        Command::FindNext,
        Command::CloseAllTabs,
    ] {
        assert!(
            cmd.hint_verb().is_some_and(|v| !v.is_empty()),
            "{cmd:?} should advertise a terse verb"
        );
    }
    // …while self-evident motion and text-editing keys are gated out of the bar.
    for cmd in [
        Command::CaretDown,
        Command::PageUp,
        Command::DeleteBackward,
        Command::InsertNewline,
        Command::SelectExtendDown,
    ] {
        assert!(
            cmd.hint_verb().is_none(),
            "{cmd:?} should not be advertised"
        );
    }
}
