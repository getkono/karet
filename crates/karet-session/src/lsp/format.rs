//! Serving one formatting request on a server task: the whole document, for a
//! save or for Format Document, or each selected range, for Format Selection.
//!
//! Every ending but a successful reply leaves the file unformatted, and each is
//! reported as such so the session can fall back on its own formatter. A
//! connection that died, and a request that errored, format exactly as much as
//! a server that never offered the method: nothing.
//!
//! A save first asks for the server's will-save edits
//! (`textDocument/willSaveWaitUntil`) where the session says why it saves.
//! Those that come back are the answer on their own: the server formats the
//! text it was last sent, and the session has yet to apply them to it. Both
//! requests share the one [`formatting_deadline`] the save was handed, so a
//! server that stalls on the first leaves no time for the second, and the
//! task is given back once, not twice, past the point the save went to disk.

use std::path::Path;

use karet_core::ServerFeature;
use karet_core::TextEdit;
use karet_lsp::LspClient;
use karet_lsp::LspError;
use karet_lsp::SaveReason;
use tokio::time::Instant;

use super::code_actions::StepContext;
use super::commands::report_unsupported;
use super::message::EditingAnswer;
use super::message::EditingReply;
use super::message::FormattingAsk;
use super::message::FormattingScope;
use super::message::LspUpdate;
use super::runtime::formatting_deadline;
use crate::RequestId;
use crate::api::DocumentId;

/// Serve a [`super::message::ServerCmd::Formatting`] and send its answer: the
/// save's will-save edits as an [`EditingReply::WillSave`] when there are any
/// or the save does not format, and otherwise the formatting itself as an
/// [`LspUpdate::Formatting`].
pub(super) async fn answer(
    client: &LspClient,
    (request, doc, version): (RequestId, DocumentId, u64),
    path: &Path,
    ask: FormattingAsk,
    context: &mut StepContext<'_>,
) {
    let deadline = Instant::now() + formatting_deadline(ask.save_timeout);
    let saving = matches!(
        ask.scope,
        FormattingScope::Save | FormattingScope::SaveUnformatted
    );
    let will_save = match ask.will_save {
        Some(reason) if saving => will_save(client, path, reason, deadline, context).await,
        _ => Vec::new(),
    };
    let then_format = ask.scope == FormattingScope::Save;
    if !will_save.is_empty() || (saving && !then_format) {
        let reply = EditingReply::WillSave {
            edits: will_save,
            then_format,
        };
        let answer = EditingAnswer {
            request,
            doc,
            version,
            reply,
        };
        let generation = context.generation;
        let _ = context
            .updates
            .send(LspUpdate::Editing { generation, answer });
        return;
    }
    let (formatted, edits) = serve(client, path, &ask, (request, deadline), context).await;
    let _ = context.updates.send(LspUpdate::Formatting {
        generation: context.generation,
        request,
        doc,
        version,
        formatted,
        edits,
        scope: ask.scope,
    });
}

/// The server's will-save edits for `path`, empty when it asked for none,
/// failed, or was still working at `deadline`. A refusal stays silent, as a
/// save's formatting does, and a failure is charged to the connection.
async fn will_save(
    client: &LspClient,
    path: &Path,
    reason: SaveReason,
    deadline: Instant,
    context: &mut StepContext<'_>,
) -> Vec<TextEdit> {
    if *context.dead {
        return Vec::new();
    }
    let request = client.will_save_wait_until(path, reason);
    let Ok(answer) = tokio::time::timeout_at(deadline, request).await else {
        tracing::warn!(
            language = %context.key,
            "will-save edits outlasted the time allowed for them; saving without them"
        );
        return Vec::new();
    };
    match context.tally.observe(answer) {
        Ok(edits) => edits,
        Err(error) => {
            context.tally.note::<()>(
                Err(error),
                context.dead,
                context.updates,
                context.key,
                context.token,
            );
            Vec::new()
        },
    }
}

/// Ask `client` to format `path` as `ask` says: whether a server formatted it,
/// and the edits it returned, in UTF-16 columns.
///
/// A range request per selection, one after another on this task, so the
/// server answers every one against the same text and the edits combine into
/// one change. A format the user asked for by hand reports a server that does
/// not offer the method ([`report_unsupported`]); a save's stays silent.
///
/// The wait is bounded here as well as by the save, because the two waits cost
/// different things: the save's deadline gives up on the answer, while this
/// one gives the *task* back. Every command for this server -- diagnostics,
/// completions, a `didChange` flush -- queues behind this `await`. A
/// `deadline` the will-save request already spent sends nothing at all: the
/// save it would format has been written without it.
async fn serve(
    client: &LspClient,
    path: &Path,
    ask: &FormattingAsk,
    (request, deadline): (RequestId, Instant),
    context: &mut StepContext<'_>,
) -> (bool, Vec<TextEdit>) {
    if *context.dead {
        return (false, Vec::new());
    }
    if Instant::now() >= deadline {
        tracing::warn!(
            language = %context.key,
            "will-save edits spent the time allowed for formatting; leaving the file unformatted"
        );
        return (false, Vec::new());
    }
    // No capability check of its own: a server that never advertised the
    // method is refused by the client before any request reaches the wire
    // (`LspError::Unsupported`, issue #279), which costs no round trip and
    // lands in the same unformatted answer below.
    let requests = async {
        match &ask.scope {
            FormattingScope::Save
            | FormattingScope::SaveUnformatted
            | FormattingScope::Document => client.formatting(path, ask.indentation).await,
            FormattingScope::Ranges(ranges) => {
                let mut edits = Vec::new();
                for range in ranges {
                    edits.extend(
                        client
                            .range_formatting(path, *range, ask.indentation)
                            .await?,
                    );
                }
                Ok::<_, LspError>(edits)
            },
        }
    };
    match tokio::time::timeout_at(deadline, requests).await {
        Ok(answer) => {
            let feature = match ask.scope {
                FormattingScope::Save | FormattingScope::SaveUnformatted => None,
                FormattingScope::Document => Some(ServerFeature::Formatting),
                FormattingScope::Ranges(_) => Some(ServerFeature::RangeFormatting),
            };
            if let Some(feature) = feature {
                report_unsupported(
                    &answer,
                    context.updates,
                    context.generation,
                    request,
                    context.key,
                    feature,
                );
            }
            match context.tally.observe(answer) {
                Ok(edits) => (true, edits),
                Err(error) => {
                    context.tally.note::<()>(
                        Err(error),
                        context.dead,
                        context.updates,
                        context.key,
                        context.token,
                    );
                    (false, Vec::new())
                },
            }
        },
        // Deliberately not charged to the connection and not a death: a
        // formatter slower than one save can wait for is not a server that has
        // stopped answering. The request is left to expire on its own in the
        // JSON-RPC layer.
        Err(_elapsed) => {
            tracing::warn!(
                language = %context.key,
                "formatting outlasted the time allowed for it; leaving the file unformatted"
            );
            (false, Vec::new())
        },
    }
}
