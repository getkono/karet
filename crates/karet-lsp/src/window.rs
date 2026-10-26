//! What a server says to the user: work-done progress, messages, and the one
//! request that waits on the user's answer.
//!
//! Three streams, each shaped by what the consumer has to do with it:
//!
//! - **`$/progress`** for a token the server created with
//!   `window/workDoneProgress/create` (acknowledged by the handler) arrives as
//!   a [`WorkProgress`] on [`LspClient::work_progress`]. Partial-result
//!   progress shares the method but not the shape, and is left to
//!   [`LspClient::raw_notifications`].
//! - **`window/showMessage`** and **`window/logMessage`** arrive as a
//!   [`ServerMessage`] on [`LspClient::messages`], told apart by
//!   [`ServerMessageKind`].
//! - **`window/showMessageRequest`** waits on a person, so it is handed out
//!   once, through [`LspClient::message_requests`], as a [`MessageRequest`]
//!   carrying its own reply. Nobody holding the stream, the stream full, or the
//!   request dropped unanswered all reply `null` -- "no action chosen" -- so
//!   the server is never left waiting.

use std::fmt;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

use karet_jsonrpc::PeerRequest;
use karet_jsonrpc::Responder;
use serde_json::Value;
use tokio::sync::broadcast;
use tokio::sync::mpsc;

use crate::LspClient;
use crate::LspError;
use crate::apply_edit::Held;

/// The method a [`MessageRequest`] answers.
pub(crate) const SHOW_MESSAGE_REQUEST: &str = "window/showMessageRequest";

/// Requests waiting for the user. A server asks one question at a time in
/// practice; this only absorbs a burst.
const MESSAGE_REQUEST_CAPACITY: usize = 16;

/// Progress broadcast capacity. A lagging subscriber loses the oldest reports,
/// which a later report or the `end` supersedes.
pub(crate) const PROGRESS_CHANNEL_CAPACITY: usize = 64;

/// Message broadcast capacity. Log traffic can be bursty (a server's startup
/// banner), and a lagging subscriber loses the oldest lines.
pub(crate) const MESSAGE_CHANNEL_CAPACITY: usize = 256;

/// A fan-out whose first subscriber also receives what was sent before it
/// subscribed, up to the channel's capacity.
///
/// A consumer can subscribe only once `connect` returns, after the handshake,
/// and a server's startup log and first progress arrive during it. Holding one
/// receiver from the start keeps them for that first subscriber instead of
/// broadcasting them to nobody.
#[derive(Clone)]
pub(crate) struct Backlogged<T> {
    tx: broadcast::Sender<T>,
    first: Arc<Mutex<Option<broadcast::Receiver<T>>>>,
}

impl<T: Clone> Backlogged<T> {
    /// A fan-out keeping up to `capacity` entries for its first subscriber.
    pub(crate) fn new(capacity: usize) -> Self {
        let (tx, rx) = broadcast::channel(capacity);
        Self {
            tx,
            first: Arc::new(Mutex::new(Some(rx))),
        }
    }

    /// Send `value` to every subscriber, and to the first one still to come.
    pub(crate) fn send(&self, value: T) {
        let _ = self.tx.send(value);
    }

    /// The held receiver, backlog included, on the first call; a fresh one
    /// after that.
    pub(crate) fn subscribe(&self) -> broadcast::Receiver<T> {
        self.first
            .lock()
            .ok()
            .and_then(|mut first| first.take())
            .unwrap_or_else(|| self.tx.subscribe())
    }
}

/// The token naming one piece of work-done progress, as the server chose it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ProgressToken {
    /// A numeric token.
    Number(i32),
    /// A string token.
    String(String),
}

impl ProgressToken {
    /// Read a token off the wire: an integer or a string, anything else `None`.
    fn from_value(value: &Value) -> Option<Self> {
        match value {
            Value::String(text) => Some(Self::String(text.clone())),
            Value::Number(number) => number
                .as_i64()
                .and_then(|n| i32::try_from(n).ok())
                .map(Self::Number),
            _ => None,
        }
    }

    /// The token as it goes back on the wire.
    fn to_value(&self) -> Value {
        match self {
            Self::Number(n) => Value::from(*n),
            Self::String(text) => Value::from(text.as_str()),
        }
    }
}

impl fmt::Display for ProgressToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Number(n) => write!(f, "{n}"),
            Self::String(text) => f.write_str(text),
        }
    }
}

/// Which step of a piece of work a [`WorkProgress`] reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum WorkProgressKind {
    /// The work started; carries its title.
    Begin,
    /// The work moved on.
    Report,
    /// The work finished (or was cancelled); nothing follows for this token.
    End,
}

/// One `$/progress` report for work-done progress.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct WorkProgress {
    /// The work this report is about.
    pub token: ProgressToken,
    /// Which step this is.
    pub kind: WorkProgressKind,
    /// What the work is (`Indexing`), on `begin` only.
    pub title: Option<String>,
    /// What it is doing now (`3/10 crates`), when the server says.
    pub message: Option<String>,
    /// How far along, 0–100, when the server can say.
    pub percentage: Option<u32>,
    /// Whether the user may cancel it ([`LspClient::cancel_work`]). Only
    /// `begin` and `report` carry it; `None` means unchanged.
    pub cancellable: Option<bool>,
}

impl WorkProgress {
    /// Decode one `$/progress` notification, or `None` when it is not
    /// work-done progress (partial-result progress shares the method).
    pub(crate) fn from_params(params: &Value) -> Option<Self> {
        let token = ProgressToken::from_value(params.get("token")?)?;
        let value = params.get("value")?;
        let kind = match value.get("kind")?.as_str()? {
            "begin" => WorkProgressKind::Begin,
            "report" => WorkProgressKind::Report,
            "end" => WorkProgressKind::End,
            _ => return None,
        };
        let text = |key: &str| value.get(key).and_then(Value::as_str).map(str::to_owned);
        Some(Self {
            token,
            kind,
            title: text("title"),
            // `end` names its closing words `message` as well.
            message: text("message"),
            percentage: value
                .get("percentage")
                .and_then(Value::as_u64)
                .map(|percent| u32::try_from(percent.min(100)).unwrap_or(100)),
            cancellable: value.get("cancellable").and_then(Value::as_bool),
        })
    }
}

/// How urgent a server's message is (LSP `MessageType`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum MessageLevel {
    /// Something failed.
    Error,
    /// Something may need attention.
    Warning,
    /// For information.
    Info,
    /// Log detail.
    Log,
    /// Debug detail (LSP 3.18).
    Debug,
}

impl MessageLevel {
    /// Decode the wire's `type`, reading an unknown one as [`Self::Log`] -- a
    /// message is still worth keeping when its urgency is not understood.
    fn from_value(value: Option<&Value>) -> Self {
        match value.and_then(Value::as_u64) {
            Some(1) => Self::Error,
            Some(2) => Self::Warning,
            Some(3) => Self::Info,
            Some(5) => Self::Debug,
            _ => Self::Log,
        }
    }
}

/// Whether a [`ServerMessage`] was meant to be shown or only logged.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ServerMessageKind {
    /// `window/showMessage`: the server wants the user to see it.
    Show,
    /// `window/logMessage`: for the server's log.
    Log,
}

/// A `window/showMessage` or `window/logMessage` notification.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct ServerMessage {
    /// Shown or logged.
    pub kind: ServerMessageKind,
    /// How urgent.
    pub level: MessageLevel,
    /// The text, verbatim.
    pub text: String,
}

impl ServerMessage {
    /// Decode a show/log notification, `None` for any other method or a
    /// message with no text.
    pub(crate) fn from_notification(method: &str, params: &Value) -> Option<Self> {
        let kind = match method {
            "window/showMessage" => ServerMessageKind::Show,
            "window/logMessage" => ServerMessageKind::Log,
            _ => return None,
        };
        Some(Self {
            kind,
            level: MessageLevel::from_value(params.get("type")),
            text: params.get("message")?.as_str()?.to_owned(),
        })
    }
}

/// A server's `window/showMessageRequest`: a message with actions to choose
/// from.
///
/// Answer it with [`respond`](Self::respond). Dropping it unanswered replies
/// `null` (no action chosen), so a consumer that gives up never leaves the
/// server waiting.
///
/// The fields are read through getters so the titles shown and the actions
/// answered cannot drift apart: [`respond`](Self::respond) indexes the same
/// list [`actions`](Self::actions) returns.
#[derive(Debug)]
pub struct MessageRequest {
    /// How urgent.
    level: MessageLevel,
    /// The question, verbatim.
    message: String,
    /// The titles of the actions offered, in the server's order.
    actions: Vec<String>,
    /// The actions exactly as sent, so the chosen one goes back with any extra
    /// properties the server attached to it.
    raw_actions: Vec<Value>,
    responder: Option<Responder>,
}

impl MessageRequest {
    /// How urgent the question is.
    #[must_use]
    pub fn level(&self) -> MessageLevel {
        self.level
    }

    /// The question, verbatim.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    /// The titles of the actions offered, in the server's order. An index into
    /// this slice is what [`respond`](Self::respond) takes.
    #[must_use]
    pub fn actions(&self) -> &[String] {
        &self.actions
    }

    /// Tell the server which action was chosen: an index into
    /// [`actions`](Self::actions), or `None` for none. An index out of range
    /// answers as `None`.
    pub fn respond(mut self, action: Option<usize>) {
        if let Some(responder) = self.responder.take() {
            let chosen = action
                .and_then(|index| self.raw_actions.get(index))
                .cloned()
                .unwrap_or(Value::Null);
            responder.ok(chosen);
        }
    }

    /// Decode one request, keeping its responder.
    fn decode(params: &Value, responder: Responder) -> Self {
        let raw_actions: Vec<Value> = params
            .get("actions")
            .and_then(Value::as_array)
            .map(|actions| {
                actions
                    .iter()
                    .filter(|action| action.get("title").and_then(Value::as_str).is_some())
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        Self {
            level: MessageLevel::from_value(params.get("type")),
            message: params
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            actions: raw_actions
                .iter()
                .filter_map(|action| action.get("title").and_then(Value::as_str))
                .map(str::to_owned)
                .collect(),
            raw_actions,
            responder: Some(responder),
        }
    }
}

impl Drop for MessageRequest {
    fn drop(&mut self) {
        if let Some(responder) = self.responder.take() {
            responder.ok(Value::Null);
        }
    }
}

/// The stream of message requests, before and after the consumer takes it.
pub(crate) type MessageRequests = Held<MessageRequest>;

/// A fresh stream and the sender the pump routes into.
pub(crate) fn message_requests() -> (mpsc::Sender<MessageRequest>, MessageRequests) {
    let (tx, rx) = mpsc::channel(MESSAGE_REQUEST_CAPACITY);
    (tx, Held::new(rx))
}

/// Hand one `window/showMessageRequest` to the consumer, or answer `null`
/// here when there is nobody to hand it to.
pub(crate) fn route_message_request(
    request: PeerRequest,
    tx: &mpsc::Sender<MessageRequest>,
    taken: &AtomicBool,
) {
    let PeerRequest {
        params, responder, ..
    } = request;
    let request = MessageRequest::decode(&params, responder);
    if !taken.load(Ordering::SeqCst) {
        // Dropping answers `null`: no one is there to choose.
        return;
    }
    // A full or closed stream hands the request back, and dropping it answers.
    if let Err(refused) = tx.try_send(request) {
        tracing::warn!(error = %refused, "answering a message request nobody can take");
    }
}

/// Broadcast one decoded progress report, ignoring partial-result progress.
pub(crate) fn route_progress(params: &Value, progress: &Backlogged<WorkProgress>) {
    match WorkProgress::from_params(params) {
        Some(report) => progress.send(report),
        None => tracing::debug!("$/progress that is not work-done progress"),
    }
}

impl LspClient {
    /// Subscribe to the server's work-done progress (see [`WorkProgress`]).
    ///
    /// Every `begin` is followed by `report`s and one `end` for the same
    /// token. A slow subscriber drops the oldest entries, so a consumer that
    /// tracks work should treat a missed `end` as possible and clear work the
    /// connection no longer reports (it ends with the connection).
    ///
    /// The first subscriber also receives the reports sent since the
    /// connection started, the handshake included, up to the channel's
    /// capacity; later subscribers see only what follows their subscription.
    #[must_use]
    pub fn work_progress(&self) -> broadcast::Receiver<WorkProgress> {
        self.conn.work_progress()
    }

    /// Ask the server to cancel the work `token` names
    /// (`window/workDoneProgress/cancel`).
    ///
    /// Only meaningful for work the server reported as cancellable; the server
    /// still sends the `end`.
    ///
    /// # Errors
    ///
    /// [`LspError::Closed`] when the connection is gone, or
    /// [`LspError::Protocol`] when the outbound queue is full.
    pub fn cancel_work(&self, token: &ProgressToken) -> Result<(), LspError> {
        self.conn.notify(
            "window/workDoneProgress/cancel",
            serde_json::json!({ "token": token.to_value() }),
        )
    }

    /// Subscribe to the server's `window/showMessage` and `window/logMessage`
    /// notifications (see [`ServerMessage`]).
    ///
    /// The first subscriber also receives the messages sent since the
    /// connection started -- a server's startup log, sent during the
    /// handshake -- up to the channel's capacity; later subscribers see only
    /// what follows their subscription.
    #[must_use]
    pub fn messages(&self) -> broadcast::Receiver<ServerMessage> {
        self.conn.messages()
    }

    /// Take the stream of the server's `window/showMessageRequest`s.
    ///
    /// Once only, because each request has one answerer. Until it is taken,
    /// every request is answered `null` on arrival.
    #[must_use]
    pub fn message_requests(&self) -> Option<mpsc::Receiver<MessageRequest>> {
        self.conn.message_requests()
    }
}
