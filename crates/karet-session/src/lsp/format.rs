//! Serving one formatting request on a server task: the whole document, for a
//! save or for Format Document, or each selected range, for Format Selection.
//!
//! Every ending but a successful reply leaves the file unformatted, and each is
//! reported as such so the session can fall back on its own formatter. A
//! connection that died, and a request that errored, format exactly as much as
//! a server that never offered the method: nothing.

use std::path::Path;

use karet_core::ServerFeature;
use karet_core::TextEdit;
use karet_lsp::LspClient;
use karet_lsp::LspError;

use super::code_actions::StepContext;
use super::commands::report_unsupported;
use super::message::FormattingAsk;
use super::message::FormattingScope;
use super::runtime::formatting_deadline;
use crate::RequestId;

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
/// completions, a `didChange` flush -- queues behind this `await`.
pub(super) async fn serve(
    client: &LspClient,
    path: &Path,
    ask: &FormattingAsk,
    request: RequestId,
    context: &mut StepContext<'_>,
) -> (bool, Vec<TextEdit>) {
    if *context.dead {
        return (false, Vec::new());
    }
    // No capability check of its own: a server that never advertised the
    // method is refused by the client before any request reaches the wire
    // (`LspError::Unsupported`, issue #279), which costs no round trip and
    // lands in the same unformatted answer below.
    let requests = async {
        match &ask.scope {
            FormattingScope::Save | FormattingScope::Document => {
                client.formatting(path, ask.indentation).await
            },
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
    match tokio::time::timeout(formatting_deadline(ask.save_timeout), requests).await {
        Ok(answer) => {
            let feature = match ask.scope {
                FormattingScope::Save => None,
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
