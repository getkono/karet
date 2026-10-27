use super::*;

/// The seam's serde-ready claim, enforced: representative [`Command`]s and
/// [`Event`]s must round-trip through JSON. Every type a variant carries is
/// pulled into this guarantee by the derive, so a leak that breaks
/// serializability fails to compile and a behavioral regression fails here.
#[test]
fn commands_and_events_round_trip_through_serde() -> Result<(), serde_json::Error> {
    fn rt<T: serde::Serialize + serde::de::DeserializeOwned + std::fmt::Debug>(
        value: &T,
    ) -> Result<String, serde_json::Error> {
        let json = serde_json::to_string(value)?;
        let back: T = serde_json::from_str(&json)?;
        serde_json::to_string(&back)
    }

    let open = Command::from(DocumentCommand::Open {
        path: PathBuf::from("src/main.rs"),
        language: None,
    });
    assert_eq!(rt(&open)?, serde_json::to_string(&open)?);

    let apply = Command::from(DocumentCommand::ApplyChange {
        doc: DocumentId(3),
        change: Change::new(
            7,
            vec![karet_core::TextEdit {
                range: karet_core::Range::default(),
                new_text: "x".into(),
            }],
        ),
        cause: EditCause::Type,
    });
    assert_eq!(rt(&apply)?, serde_json::to_string(&apply)?);

    let vcs = Command::from(VcsCommand::Action {
        action: VcsAction::SwitchBranch(BranchTarget::Local("main".into())),
    });
    assert_eq!(rt(&vcs)?, serde_json::to_string(&vcs)?);

    let diag = Event::DiagnosticsPublished {
        doc: DocumentId(3),
        diagnostics: vec![Diagnostic {
            range: karet_core::Range::default(),
            severity: Severity::Warning,
            message: "m".into(),
            source: None,
            code: None,
            code_href: None,
            tags: Vec::new(),
            related: Vec::new(),
        }],
    };
    assert_eq!(rt(&diag)?, serde_json::to_string(&diag)?);

    let ask = Command::from(LspCommand::SignatureHelp {
        doc: DocumentId(3),
        position: LineCol::new(0, 4),
        manual: true,
    });
    assert_eq!(rt(&ask)?, serde_json::to_string(&ask)?);

    let signature = Event::SignatureHelp {
        doc: DocumentId(3),
        help: Some(SignatureHelp {
            signatures: vec![karet_core::Signature {
                label: "f(a: i32)".into(),
                documentation: None,
                parameters: vec![karet_core::ParamInfo {
                    label: "a: i32".into(),
                    documentation: None,
                }],
            }],
            active_signature: 0,
            active_parameter: 0,
        }),
        options: SignatureHelpOptions {
            trigger_characters: vec!['('],
            retrigger_characters: vec![','],
        },
    };
    assert_eq!(rt(&signature)?, serde_json::to_string(&signature)?);

    let hunk = Command::from(VcsCommand::ApplyIndexPatch {
        patch: "diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1 +1 @@\n-a\n+b\n".into(),
        reverse: true,
    });
    assert_eq!(rt(&hunk)?, serde_json::to_string(&hunk)?);

    let prepare = Command::from(VcsCommand::PrepareChange {
        path: PathBuf::from("src/main.rs"),
        staged: true,
    });
    assert_eq!(rt(&prepare)?, serde_json::to_string(&prepare)?);

    let status = Event::VcsStatus {
        staged: vec![ChangeSummary {
            path: PathBuf::from("src/main.rs"),
            old_path: None,
            status: karet_vcs::StatusKind::Modified,
            is_binary: false,
            added: 2,
            removed: 1,
        }],
        working: Vec::new(),
    };
    assert_eq!(rt(&status)?, serde_json::to_string(&status)?);

    let prepared = Event::ChangePrepared {
        path: PathBuf::from("src/main.rs"),
        staged: false,
        result: Ok(Box::new(PreparedChange {
            path: PathBuf::from("src/main.rs"),
            old_path: None,
            status: karet_vcs::StatusKind::Modified,
            language: "Rust".into(),
            diff: karet_diff::PreparedDiff::new(
                karet_diff::diff_text("a\n", "b\n", &karet_diff::DiffOptions::default()),
                Vec::new(),
                Vec::new(),
            ),
        })),
    };
    assert_eq!(rt(&prepared)?, serde_json::to_string(&prepared)?);
    Ok(())
}

/// The one secret in the vocabulary must refuse to serialize: a transport can
/// never move a GitHub token, by construction.
#[test]
fn github_token_refuses_to_serialize() {
    let command = Command::from(GithubCommand::Login {
        token: GithubToken::new("github_pat_secret".into()),
    });
    let result = serde_json::to_string(&command);
    assert!(result.is_err(), "a credential must not cross the seam");
}

/// A GitHub failure names its operation as a typed variant, so a client reads
/// the same operation back after the seam rather than a label to re-match.
#[test]
fn github_error_carries_its_operation_across_the_seam() -> Result<(), serde_json::Error> {
    let error = Event::GithubError {
        operation: GithubOperation::CommitVerification,
        message: "HTTP 422".into(),
    };
    let back: Event = serde_json::from_str(&serde_json::to_string(&error)?)?;
    assert!(
        matches!(
            back,
            Event::GithubError {
                operation: GithubOperation::CommitVerification,
                ..
            }
        ),
        "{back:?}"
    );
    Ok(())
}

/// The identifiers' and defaults' documented answers. Constructing a payload
/// proves nothing the compiler does not, so only observable answers are pinned.
#[test]
fn ids_server_names_and_document_defaults_answer_as_documented() {
    assert_eq!(DocumentId(1), DocumentId(1));
    assert_ne!(RequestId(1), RequestId(2));
    let server = LanguageServerId::Texlab;
    assert_eq!(server.key(), "texlab");
    assert_eq!(server.display_name(), "texlab");
    assert_eq!(
        DocumentSettings::default(),
        DocumentSettings {
            insert_spaces: true,
            indent_size: 4,
            tab_width: 4,
            trim_trailing_whitespace: true,
            insert_final_newline: true,
            line_ending: None,
            encoding: None,
            spelling_language: None,
        }
    );
    assert_eq!(
        SpellingLanguage::parse("en-GB").map(SpellingLanguage::display_name),
        Some("English (UK)")
    );
    assert_eq!(
        SpellingLanguage::parse("en_US").map(SpellingLanguage::locale),
        Some("en_US")
    );
    assert!(SpellingLanguage::parse("fr_FR").is_none());
}

#[test]
fn github_token_debug_never_exposes_the_secret() {
    let token = GithubToken::new("github_pat_super_secret".to_string());
    let debug = format!("{token:?}");
    assert_eq!(debug, "GithubToken(***)");
    assert!(!debug.contains("super_secret"));
}

#[test]
fn pull_request_conversation_models_remain_serde_ready() -> Result<(), serde_json::Error> {
    let commit = GithubPullRequestCommit {
        sha: "bbbbbbbb".to_string(),
        summary: "Add feature".to_string(),
        author: "Octo Cat".to_string(),
        committed_unix: 2,
        parents: vec!["aaaaaaaa".to_string()],
        html_url: "https://github.com/o/r/commit/bbbbbbbb".to_string(),
    };
    let check = GithubCheckRun {
        id: 9,
        name: "CI".to_string(),
        status: "completed".to_string(),
        conclusion: Some("success".to_string()),
        html_url: "https://github.com/o/r/runs/9".to_string(),
    };
    let activity = GithubPullRequestActivity {
        id: Some(3),
        kind: "committed".to_string(),
        actor: Some("octocat".to_string()),
        commit_id: Some(commit.sha.clone()),
        before: None,
        after: None,
        created_unix: Some(2),
    };
    let commit_json = serde_json::to_string(&commit)?;
    let check_json = serde_json::to_string(&check)?;
    let activity_json = serde_json::to_string(&activity)?;
    assert_eq!(
        serde_json::from_str::<GithubPullRequestCommit>(&commit_json)?,
        commit
    );
    assert_eq!(serde_json::from_str::<GithubCheckRun>(&check_json)?, check);
    assert_eq!(
        serde_json::from_str::<GithubPullRequestActivity>(&activity_json)?,
        activity
    );
    Ok(())
}

/// Every family converts into its own [`Command`] arm, never a neighbour's,
/// and travels as `{"Family": {"Variant": ...}}`. A `From` impl wired to the
/// wrong arm would route a command to a handler that cannot serve it.
#[test]
fn each_family_converts_into_its_own_command_arm() -> Result<(), serde_json::Error> {
    let cases: [(Command, &str); 9] = [
        (
            DocumentCommand::RecoverSwaps.into(),
            r#"{"Document":"RecoverSwaps"}"#,
        ),
        (
            LspCommand::DocumentSymbols { doc: DocumentId(3) }.into(),
            r#"{"Lsp":{"DocumentSymbols":{"doc":3}}}"#,
        ),
        (
            LanguageServerCommand::Status.into(),
            r#"{"LanguageServer":"Status"}"#,
        ),
        (VcsCommand::Refresh.into(), r#"{"Vcs":"Refresh"}"#),
        (
            GithubCommand::IssueMetadata.into(),
            r#"{"Github":"IssueMetadata"}"#,
        ),
        (DebugCommand::StepOver.into(), r#"{"Debug":"StepOver"}"#),
        (
            NotebookCommand::Interrupt.into(),
            r#"{"Notebook":"Interrupt"}"#,
        ),
        (
            SeamCommand::Query { text: "q".into() }.into(),
            r#"{"Seam":{"Query":{"text":"q"}}}"#,
        ),
        (
            WorkspaceCommand::DependencyGraph.into(),
            r#"{"Workspace":"DependencyGraph"}"#,
        ),
    ];
    for (command, expected) in &cases {
        assert_eq!(serde_json::to_string(command)?, *expected);
    }
    assert!(matches!(cases[0].0, Command::Document(_)));
    assert!(matches!(cases[1].0, Command::Lsp(_)));
    assert!(matches!(cases[2].0, Command::LanguageServer(_)));
    assert!(matches!(cases[3].0, Command::Vcs(_)));
    assert!(matches!(cases[4].0, Command::Github(_)));
    assert!(matches!(cases[5].0, Command::Debug(_)));
    assert!(matches!(cases[6].0, Command::Notebook(_)));
    assert!(matches!(cases[7].0, Command::Seam(_)));
    assert!(matches!(cases[8].0, Command::Workspace(_)));
    Ok(())
}
