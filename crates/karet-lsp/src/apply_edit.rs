//! Server→client requests that must wait on the editor: `workspace/applyEdit`.
//!
//! [`karet_jsonrpc::Handler::answer`] runs on the reader task and cannot wait,
//! but applying an edit is the editor's work, not this crate's. So the
//! connection takes the peer-request stream and runs one pump over it: every
//! request this crate answers by itself goes to the handler exactly as before,
//! in arrival order, and `workspace/applyEdit` goes to whoever holds
//! [`LspClient::apply_edit_requests`](crate::LspClient::apply_edit_requests).
//!
//! The server is answered every time. Nobody holds the stream, it is full,
//! the edit names something that is not a file, or the request is dropped
//! unanswered: each of those replies `applied: false` with the reason, and the
//! server is never left waiting.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

use karet_core::ApplyWorkspaceEditResult;
use karet_core::WorkspaceEdit;
use karet_jsonrpc::PeerRequest;
use karet_jsonrpc::Responder;
use karet_jsonrpc::ResponseError;
use tokio::sync::mpsc;

use crate::conn::LspHandler;
use crate::convert;
use crate::window;

/// The method this module exists for.
const APPLY_EDIT: &str = "workspace/applyEdit";

/// Edits waiting for the editor. A server sends one at a time in practice --
/// it waits for each answer before it moves on -- so this only absorbs a burst.
const APPLY_EDIT_CAPACITY: usize = 16;

/// A server's request to apply a [`WorkspaceEdit`] (`workspace/applyEdit`).
///
/// Answer it with [`respond`](Self::respond). Dropping it unanswered replies
/// `applied: false`, so a consumer that gives up never leaves the server
/// waiting.
#[derive(Debug)]
pub struct ApplyEditRequest {
    /// The server's label for the edit, for an undo stack or a notice.
    pub label: Option<String>,
    /// The edit, with columns in UTF-16 code units like every other position
    /// this crate hands out.
    pub edit: WorkspaceEdit,
    responder: Option<Responder>,
}

impl ApplyEditRequest {
    /// Tell the server what became of its edit.
    pub fn respond(mut self, result: ApplyWorkspaceEditResult) {
        if let Some(responder) = self.responder.take() {
            reply(responder, result);
        }
    }
}

impl Drop for ApplyEditRequest {
    fn drop(&mut self) {
        if let Some(responder) = self.responder.take() {
            reply(
                responder,
                ApplyWorkspaceEditResult::failed("the editor did not apply the edit", None),
            );
        }
    }
}

/// A receiving half handed to the consumer once, and whether it has been.
pub(crate) struct Held<T> {
    rx: std::sync::Mutex<Option<mpsc::Receiver<T>>>,
    taken: Arc<AtomicBool>,
}

impl<T> Held<T> {
    /// Hold `rx` until the consumer takes it.
    pub(crate) fn new(rx: mpsc::Receiver<T>) -> Self {
        Self {
            rx: std::sync::Mutex::new(Some(rx)),
            taken: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Take the request stream: once, because each request has one answerer.
    pub(crate) fn take(&self) -> Option<mpsc::Receiver<T>> {
        let taken = self.rx.lock().ok()?.take();
        if taken.is_some() {
            self.taken.store(true, Ordering::SeqCst);
        }
        taken
    }

    /// Whether the stream has been taken, as the pump reads it.
    fn flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.taken)
    }
}

/// The `workspace/applyEdit` stream, before and after the consumer takes it.
pub(crate) type ApplyEdits = Held<ApplyEditRequest>;

/// The streams the pump routes into, each taken once by the consumer.
pub(crate) struct Waiting {
    /// `workspace/applyEdit`.
    pub(crate) edits: ApplyEdits,
    /// `window/showMessageRequest`.
    pub(crate) messages: window::MessageRequests,
}

/// Start the pump over `requests`, answering what `handler` answers and
/// routing the requests that wait on the consumer to the streams returned.
pub(crate) fn start(mut requests: mpsc::Receiver<PeerRequest>, handler: LspHandler) -> Waiting {
    let (tx, rx) = mpsc::channel(APPLY_EDIT_CAPACITY);
    let edits = Held::new(rx);
    let taken = edits.flag();
    let (message_tx, messages) = window::message_requests();
    let messages_taken = messages.flag();
    tokio::spawn(async move {
        while let Some(request) = requests.recv().await {
            if request.method == APPLY_EDIT {
                route_apply_edit(request, &tx, &taken);
            } else if request.method == window::SHOW_MESSAGE_REQUEST {
                window::route_message_request(request, &message_tx, &messages_taken);
            } else {
                let outcome =
                    karet_jsonrpc::Handler::answer(&handler, &request.method, &request.params);
                request.responder.respond(outcome);
            }
        }
    });
    Waiting { edits, messages }
}

/// Decode one `workspace/applyEdit` and hand it to the consumer, or answer it
/// here when there is nobody to hand it to.
fn route_apply_edit(request: PeerRequest, tx: &mpsc::Sender<ApplyEditRequest>, taken: &AtomicBool) {
    let PeerRequest {
        params, responder, ..
    } = request;
    let params: lsp_types::ApplyWorkspaceEditParams = match serde_json::from_value(params) {
        Ok(params) => params,
        Err(error) => {
            responder.error(ResponseError::new(
                karet_jsonrpc::INVALID_PARAMS,
                format!("malformed {APPLY_EDIT}: {error}"),
            ));
            return;
        },
    };
    let edit = match convert::workspace_edit_from_lsp(params.edit) {
        Ok(edit) => edit,
        Err(error) => {
            reply(
                responder,
                ApplyWorkspaceEditResult::failed(error.to_string(), None),
            );
            return;
        },
    };
    if !taken.load(Ordering::SeqCst) {
        reply(
            responder,
            ApplyWorkspaceEditResult::failed("this client does not apply server edits", None),
        );
        return;
    }
    let request = ApplyEditRequest {
        label: params.label,
        edit,
        responder: Some(responder),
    };
    // A full or closed stream hands the request back, and dropping it answers
    // the server with the refusal `Drop` writes.
    if let Err(refused) = tx.try_send(request) {
        tracing::warn!(error = %refused, "refusing a workspace edit nobody can take");
    }
}

/// Encode `result` as the reply to one `workspace/applyEdit`.
fn reply(responder: Responder, result: ApplyWorkspaceEditResult) {
    match serde_json::to_value(convert::apply_result_to_lsp(result)) {
        Ok(value) => responder.ok(value),
        Err(error) => responder.error(ResponseError::internal_error(APPLY_EDIT, error)),
    }
}

#[cfg(test)]
#[path = "apply_edit_tests.rs"]
mod tests;
