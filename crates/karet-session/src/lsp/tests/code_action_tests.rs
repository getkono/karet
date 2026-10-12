//! Code actions over a live connection: listing them in buffer columns, and
//! running one through resolve, the workspace-edit path, and its command.

use super::*;
use crate::api::AppliedWorkspaceEdit;
use crate::api::CodeActionTrigger;

/// The `CodeActions` answering `request`, skipping everything else.
async fn await_actions(
    events: &mut EventRx,
    request: RequestId,
) -> Option<(u64, Vec<karet_core::CodeAction>, Option<String>)> {
    while let Some((id, event)) = next_event(events).await {
        if let Event::CodeActions {
            version,
            actions,
            reason,
            ..
        } = event
            && id == Some(request)
        {
            return Some((version, actions, reason));
        }
    }
    None
}

/// The `WorkspaceEdit` answering `request`, skipping everything else.
async fn await_applied(events: &mut EventRx, request: RequestId) -> Option<AppliedWorkspaceEdit> {
    while let Some((id, event)) = next_event(events).await {
        if let Event::WorkspaceEdit { applied } = event
            && id == Some(request)
        {
            return Some(applied);
        }
    }
    None
}

fn caret(col: u32) -> Range {
    Range {
        start: LineCol::new(0, col),
        end: LineCol::new(0, col),
    }
}

#[tokio::test]
async fn actions_list_in_buffer_columns_and_run_through_resolve_edit_and_command() -> TestResult {
    let dir = tempfile::tempdir()?;
    // '😀' is one buffer column but two UTF-16 units.
    let path = rust_file(&dir, "main.rs", "😀ab\n").ok_or("write failed")?;
    let (observed_tx, mut observed_rx) = mpsc::unbounded_channel();
    let spawns = Arc::new(AtomicUsize::new(0));
    let (session, mut events) =
        session_with_connector(test_connector(Behavior::Normal, Some(observed_tx), spawns));
    let backend = local_session(session, None);
    backend.send(
        backend.next_id(),
        Command::OpenDocument {
            path: path.clone(),
            language: None,
        },
    )?;
    let (doc, version) = await_opened(&mut events).await.ok_or("no Opened")?;

    let send = |id: RequestId, command: Command| backend.send(id, command);
    let list = |id| {
        send(
            id,
            Command::CodeActions {
                doc,
                range: caret(3),
                kinds: Vec::new(),
                trigger: CodeActionTrigger::Invoked,
            },
        )
    };
    let request = backend.next_id();
    list(request)?;
    let (listed_at, actions, reason) = await_actions(&mut events, request)
        .await
        .ok_or("no CodeActions")?;
    assert_eq!(listed_at, version);
    assert_eq!(reason, None);
    let [replace, fix] = actions.as_slice() else {
        return Err(format!("expected two actions, got {actions:?}").into());
    };
    // The server's UTF-16 2..4 is buffer columns 1..3, after the emoji.
    let edit = replace.edit.as_ref().ok_or("an inline edit")?;
    assert_eq!(
        edit.changes[0].1[0].range,
        Range {
            start: LineCol::new(0, 1),
            end: LineCol::new(0, 3),
        }
    );
    assert!(
        fix.preferred && fix.edit.is_none(),
        "the fix defers its edit"
    );

    let run = backend.next_id();
    send(
        run,
        Command::ApplyCodeAction {
            doc,
            version,
            action: replace.clone(),
        },
    )?;
    let applied = await_applied(&mut events, run).await.ok_or("no answer")?;
    assert!(applied.result.applied, "{applied:?}");
    assert_eq!(applied.label.as_deref(), Some("Replace here"));

    // Listed again at the new version, the deferred fix resolves, applies,
    // then runs its command.
    let request = backend.next_id();
    list(request)?;
    let (version, actions, _) = await_actions(&mut events, request)
        .await
        .ok_or("no CodeActions")?;
    let fix = actions.get(1).ok_or("the fix")?.clone();
    let run = backend.next_id();
    send(
        run,
        Command::ApplyCodeAction {
            doc,
            version,
            action: fix,
        },
    )?;
    let applied = await_applied(&mut events, run).await.ok_or("no answer")?;
    assert!(applied.result.applied, "{applied:?}");
    assert_eq!(applied.edited, vec![path.clone()]);

    let mut methods = Vec::new();
    let mut command = None;
    let mut text_at_command = None;
    let mut latest_text = None;
    while let Ok(Some(msg)) =
        tokio::time::timeout(Duration::from_millis(500), observed_rx.recv()).await
    {
        let method = msg["method"].as_str().unwrap_or_default().to_owned();
        match method.as_str() {
            "textDocument/didChange" => {
                latest_text = msg["params"]["contentChanges"][0]["text"]
                    .as_str()
                    .map(str::to_owned);
            },
            "workspace/executeCommand" => {
                command = Some(msg["params"].clone());
                text_at_command.clone_from(&latest_text);
            },
            // The caret at buffer column 3 of `😀ab` went out as UTF-16
            // column 4. (The second listing, over `😀X`, clamps to its end.)
            "textDocument/codeAction"
                if !methods.iter().any(|m| m == "textDocument/codeAction") =>
            {
                assert_eq!(msg["params"]["range"]["start"]["character"], json!(4));
            },
            _ => {},
        }
        methods.push(method);
    }
    let resolve = methods.iter().position(|m| m == "codeAction/resolve");
    let execute = methods.iter().position(|m| m == "workspace/executeCommand");
    assert!(resolve.is_some() && resolve < execute, "{methods:?}");
    let command = command.ok_or("the command ran")?;
    assert_eq!(command["command"], "double.fix");
    assert_eq!(command["arguments"], json!([7]));
    // Both edits landed in the document -- the resolved one converted from
    // UTF-16 -- and the server saw them before it was asked to run the
    // command that follows them.
    assert_eq!(text_at_command.as_deref(), Some("fixed 😀X\n"));
    Ok(())
}

#[tokio::test]
async fn a_probe_of_a_server_without_code_actions_stays_quiet() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = rust_file(&dir, "main.rs", "fn main() {}\n").ok_or("write failed")?;
    let spawns = Arc::new(AtomicUsize::new(0));
    let (session, mut events) =
        session_with_connector(test_connector(Behavior::Bare, None, spawns));
    let backend = local_session(session, None);
    backend.send(
        backend.next_id(),
        Command::OpenDocument {
            path,
            language: None,
        },
    )?;
    let (doc, _) = await_opened(&mut events).await.ok_or("no Opened")?;

    for trigger in [CodeActionTrigger::Automatic, CodeActionTrigger::Invoked] {
        let request = backend.next_id();
        backend.send(
            request,
            Command::CodeActions {
                doc,
                range: caret(0),
                kinds: Vec::new(),
                trigger,
            },
        )?;
        let mut unsupported = false;
        let answer = loop {
            let (id, event) = next_event(&mut events).await.ok_or("no answer")?;
            match event {
                Event::FeatureUnsupported { .. } if id == Some(request) => unsupported = true,
                Event::CodeActions {
                    actions, reason, ..
                } if id == Some(request) => break (actions, reason),
                _ => {},
            }
        };
        assert!(answer.0.is_empty());
        assert!(
            answer.1.is_some(),
            "the reason travels with the empty answer"
        );
        // Only the request made by hand explains itself.
        assert_eq!(unsupported, trigger == CodeActionTrigger::Invoked);
    }
    Ok(())
}
