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

    let open = Command::OpenDocument {
        path: PathBuf::from("src/main.rs"),
        language: None,
    };
    assert_eq!(rt(&open)?, serde_json::to_string(&open)?);

    let apply = Command::ApplyChange {
        doc: DocumentId(3),
        change: Change::new(
            7,
            vec![karet_core::TextEdit {
                range: karet_core::Range::default(),
                new_text: "x".into(),
            }],
        ),
        cause: EditCause::Type,
    };
    assert_eq!(rt(&apply)?, serde_json::to_string(&apply)?);

    let vcs = Command::VcsAction {
        action: VcsAction::SwitchBranch(BranchTarget::Local("main".into())),
    };
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

    let ask = Command::SignatureHelp {
        doc: DocumentId(3),
        position: LineCol::new(0, 4),
        manual: true,
    };
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

    let hunk = Command::ApplyIndexPatch {
        patch: "diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1 +1 @@\n-a\n+b\n".into(),
        reverse: true,
    };
    assert_eq!(rt(&hunk)?, serde_json::to_string(&hunk)?);

    let prepare = Command::PrepareChange {
        path: PathBuf::from("src/main.rs"),
        staged: true,
    };
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
    let command = Command::GithubLogin {
        token: GithubToken::new("github_pat_secret".into()),
    };
    let result = serde_json::to_string(&command);
    assert!(result.is_err(), "a credential must not cross the seam");
}

#[test]
fn ids_and_payloads_construct() {
    assert_eq!(DocumentId(1), DocumentId(1));
    assert_ne!(RequestId(1), RequestId(2));
    let _cmd = Command::Save {
        doc: DocumentId(7),
        cause: SaveCause::Manual,
    };
    let _cmd = Command::RetargetDocument {
        doc: DocumentId(7),
        path: PathBuf::from("new.txt"),
    };
    let _cmd = Command::BuildLatex { doc: DocumentId(7) };
    let _ev = Event::Saved { doc: DocumentId(7) };
    let _ev = Event::Retargeted {
        doc: DocumentId(7),
        path: PathBuf::from("new.txt"),
    };
    let _ev = Event::LatexBuildFinished {
        doc: DocumentId(7),
        root: PathBuf::from("main.tex"),
        pdf: Some(PathBuf::from("main.pdf")),
        diagnostics: Vec::new(),
        error: None,
    };
    let _cfg = Command::LoadedConfig;
    let server = LanguageServerId::Texlab;
    assert_eq!(server.key(), "texlab");
    assert_eq!(server.display_name(), "texlab");
    let plan = LanguageServerPlanId(9);
    let change = LanguageServerChange {
        server: server.clone(),
        current: Some("1.0.0".into()),
        target: "2.0.0".into(),
        download_bytes: Some(42),
    };
    let status = LanguageServerStatus {
        ever_installed: false,
        declined: false,
        server: server.clone(),
        languages: vec!["tex".into()],
        enabled: true,
        managed: true,
        manual_install_reason: None,
        installed: Some("1.0.0".into()),
        cleanup_pending: false,
        instances: vec![LanguageServerInstanceStatus {
            root: PathBuf::from("/tmp"),
            source: LanguageServerSource::Managed,
            command: Some("texlab".into()),
            args: Vec::new(),
            runtime: LanguageServerRuntimeState::Running,
            open_documents: 1,
            error: None,
        }],
    };
    let _commands = [
        Command::InstallLanguageServer {
            server: server.clone(),
        },
        Command::ApplyLanguageServerPlan {
            plan,
            servers: vec![server.clone()],
        },
        Command::UninstallLanguageServer {
            server: server.clone(),
        },
        Command::RestartLanguageServer { server },
    ];
    let _events = [
        Event::LanguageServerUpdatePlan {
            plan,
            changes: vec![change],
        },
        Event::LanguageServerStatus {
            servers: vec![status],
        },
    ];
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
    let commands = [
        Command::GithubUpdatePullRequestBody {
            number: 12,
            body: "body".to_string(),
        },
        Command::GithubCommentPullRequest {
            number: 12,
            body: "comment".to_string(),
        },
        Command::GithubMergePullRequest {
            number: 12,
            head_sha: "bbbbbbbb".to_string(),
        },
        Command::GithubSetPullRequestDraft {
            node_id: "PR_node".to_string(),
            number: 12,
            draft: true,
        },
    ];
    assert_eq!(commands.len(), 4);
    Ok(())
}
