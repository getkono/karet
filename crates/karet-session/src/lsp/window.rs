//! What a connected server says to the user, and what the session tells it
//! back outside document sync.
//!
//! Inbound: work-done progress, show/log messages, and `showMessageRequest`
//! questions are relayed from each connection to the actor as
//! [`WindowUpdate`]s, fenced on the slot like every other report a task makes
//! about itself. The actor keeps each provider's log and holds each open
//! question here, in [`WindowState`], until the client answers it.
//!
//! Outbound: settings pushes, watched-file changes, and work cancellation go
//! to the task as one [`ServerCmd::Notify`](super::message::ServerCmd::Notify),
//! so the server task gains one arm rather than three and a notice that finds
//! the connection down is simply dropped -- settings survive in the slot's
//! cell for the reconnect, and file changes or a cancel for a connection that
//! is gone mean nothing to its successor.

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::collections::VecDeque;
use std::path::Path;
use std::sync::Arc;

use karet_core::Severity;
use karet_lsp::FileChange;
use karet_lsp::FileChangeKind;
use karet_lsp::LspClient;
use karet_lsp::MessageLevel;
use karet_lsp::MessageRequest;
use karet_lsp::ProgressToken;
use karet_lsp::ServerMessage;
use karet_lsp::WorkProgress;
use karet_lsp::WorkProgressKind;
use karet_watch::FsEvent;
use karet_watch::FsEventKind;
use serde_json::Value;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::mpsc;

use super::message::LspUpdate;
use super::message::ServerCmd;
use super::slot::SlotKey;
use super::slot::SlotToken;
use crate::api::LanguageServerId;
use crate::api::LanguageServerLogEntry;
use crate::api::LanguageServerMessageId;
use crate::api::LanguageServerWork;
use crate::api::WorkPhase;
use crate::api::WorkToken;

/// Log entries kept per provider; the oldest go first.
const LOG_CAPACITY: usize = 2000;

/// Questions held at once. A client answers each one (backing out of the
/// prompt answers "none"), so this only bounds a client that never does; the
/// oldest is then answered "none" to make room.
const QUESTION_CAPACITY: usize = 32;

/// One thing a server said, on its way to the actor.
pub(crate) enum WindowUpdate {
    /// `$/progress` work-done progress.
    Progress(WorkProgress),
    /// `window/showMessage` or `window/logMessage`.
    Message(ServerMessage),
    /// `window/showMessageRequest`, carrying its own reply.
    Request(MessageRequest),
}

/// A notification for the server task to send on its live connection.
pub(crate) enum Notice {
    /// `workspace/didChangeConfiguration`, replacing what
    /// `workspace/configuration` is answered from.
    Configure(Value),
    /// File changes, filtered by the server's own registered watchers.
    WatchedFiles(Arc<[FileChange]>),
    /// `window/workDoneProgress/cancel`.
    CancelWork(ProgressToken),
}

impl Notice {
    /// Send this notice on `client`. A failure means the connection is going,
    /// which the task notices on its own.
    pub(super) fn send(self, client: &LspClient) {
        let sent = match self {
            Self::Configure(settings) => client.did_change_configuration(settings),
            Self::WatchedFiles(changes) => client.did_change_watched_files(&changes).map(drop),
            Self::CancelWork(token) => client.cancel_work(&token),
        };
        if let Err(error) = sent {
            tracing::debug!(%error, "language-server notice not sent");
        }
    }
}

/// Relay one connection's progress, messages and questions to the actor.
///
/// Detached like the other relays in `forward`: each ends when its stream
/// closes with the connection.
pub(super) fn forward_window(
    client: &LspClient,
    updates: &mpsc::UnboundedSender<LspUpdate>,
    key: &SlotKey,
    token: SlotToken,
) {
    // `move` of a `Copy` token only, so the closure is `Copy` into each task.
    let relay =
        move |update: WindowUpdate, updates: &mpsc::UnboundedSender<LspUpdate>, key: &SlotKey| {
            updates
                .send(LspUpdate::Window {
                    token,
                    key: key.clone(),
                    update,
                })
                .is_ok()
        };
    let mut progress = client.work_progress();
    let (progress_updates, progress_key) = (updates.clone(), key.clone());
    tokio::spawn(async move {
        loop {
            match progress.recv().await {
                Ok(report) => {
                    if !relay(
                        WindowUpdate::Progress(report),
                        &progress_updates,
                        &progress_key,
                    ) {
                        return;
                    }
                },
                Err(RecvError::Lagged(skipped)) => {
                    tracing::debug!(skipped, "language-server progress subscriber lagged");
                },
                Err(RecvError::Closed) => return,
            }
        }
    });
    let mut messages = client.messages();
    let (message_updates, message_key) = (updates.clone(), key.clone());
    tokio::spawn(async move {
        loop {
            match messages.recv().await {
                Ok(message) => {
                    if !relay(
                        WindowUpdate::Message(message),
                        &message_updates,
                        &message_key,
                    ) {
                        return;
                    }
                },
                Err(RecvError::Lagged(skipped)) => {
                    tracing::debug!(skipped, "language-server message subscriber lagged");
                },
                Err(RecvError::Closed) => return,
            }
        }
    });
    // A question nobody relays is answered "none" by its own `Drop`.
    if let Some(mut questions) = client.message_requests() {
        let (question_updates, question_key) = (updates.clone(), key.clone());
        tokio::spawn(async move {
            while let Some(question) = questions.recv().await {
                if !relay(
                    WindowUpdate::Request(question),
                    &question_updates,
                    &question_key,
                ) {
                    return;
                }
            }
        });
    }
}

/// The severity a client shows a server's message level with. Log and debug
/// detail read as hints.
pub(crate) fn severity(level: MessageLevel) -> Severity {
    match level {
        MessageLevel::Error => Severity::Error,
        MessageLevel::Warning => Severity::Warning,
        MessageLevel::Info => Severity::Information,
        _ => Severity::Hint,
    }
}

/// The neutral report for one progress update from `key`'s connection.
pub(crate) fn work(key: &SlotKey, progress: WorkProgress) -> LanguageServerWork {
    LanguageServerWork {
        server: key.provider.clone(),
        root: key.root.clone(),
        token: match progress.token {
            ProgressToken::Number(n) => WorkToken::Number(n),
            ProgressToken::String(text) => WorkToken::Text(text),
        },
        phase: match progress.kind {
            WorkProgressKind::Begin => WorkPhase::Begin,
            WorkProgressKind::Report => WorkPhase::Report,
            _ => WorkPhase::End,
        },
        title: progress.title,
        message: progress.message,
        percentage: progress.percentage,
        cancellable: progress.cancellable,
    }
}

/// The wire token a client's [`WorkToken`] names.
pub(crate) fn progress_token(token: &WorkToken) -> ProgressToken {
    match token {
        WorkToken::Number(n) => ProgressToken::Number(*n),
        WorkToken::Text(text) => ProgressToken::String(text.clone()),
    }
}

/// The file changes one debounced filesystem event stands for.
///
/// A rename carries both ends and no direction, so each path is read off the
/// disk: one that exists was created, one that does not was deleted.
pub(crate) fn file_changes(event: &FsEvent) -> Vec<FileChange> {
    let kind = match event.kind {
        FsEventKind::Created => Some(FileChangeKind::Created),
        FsEventKind::Modified => Some(FileChangeKind::Changed),
        FsEventKind::Removed => Some(FileChangeKind::Deleted),
        FsEventKind::Renamed => None,
        _ => return Vec::new(),
    };
    event
        .paths
        .iter()
        .map(|path| {
            let kind = kind.unwrap_or_else(|| {
                if path.exists() {
                    FileChangeKind::Created
                } else {
                    FileChangeKind::Deleted
                }
            });
            FileChange::new(path.clone(), kind)
        })
        .collect()
}

impl super::LspManager {
    /// Hand one filesystem event to every running server, each of which sends
    /// on only the changes its own registered watchers asked for.
    pub(crate) fn files_changed(&self, event: &FsEvent) {
        if self.servers.is_empty() {
            return;
        }
        let changes: Arc<[FileChange]> = file_changes(event).into();
        if changes.is_empty() {
            return;
        }
        for (key, slot) in &self.servers {
            let notice = Notice::WatchedFiles(Arc::clone(&changes));
            if slot.tx.try_send(ServerCmd::Notify(notice)).is_err() {
                tracing::debug!(slot = %key, "watched-file changes not queued");
            }
        }
    }

    /// Ask `server` at `root` to cancel the work `token` names. A slot that is
    /// gone has no work left to cancel.
    pub(crate) fn cancel_work(&self, server: &LanguageServerId, root: &Path, token: &WorkToken) {
        let key = SlotKey::new(server.clone(), root);
        if let Some(slot) = self.servers.get(&key) {
            let notice = Notice::CancelWork(progress_token(token));
            let _ = slot.tx.try_send(ServerCmd::Notify(notice));
        }
    }
}

/// Each provider's log, and the questions waiting on the client.
#[derive(Default)]
pub(crate) struct WindowState {
    logs: HashMap<LanguageServerId, VecDeque<LanguageServerLogEntry>>,
    /// Ordered by id, which is arrival order, so the oldest is first to go.
    questions: BTreeMap<LanguageServerMessageId, MessageRequest>,
    next_question: u64,
}

impl WindowState {
    /// Append `entry` to `server`'s log.
    pub(crate) fn log(&mut self, server: &LanguageServerId, entry: LanguageServerLogEntry) {
        let log = self.logs.entry(server.clone()).or_default();
        if log.len() >= LOG_CAPACITY {
            log.pop_front();
        }
        log.push_back(entry);
    }

    /// `server`'s log, oldest first.
    pub(crate) fn entries(&self, server: &LanguageServerId) -> Vec<LanguageServerLogEntry> {
        self.logs
            .get(server)
            .map(|log| log.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Hold `question` until the client answers it, returning its id.
    pub(crate) fn ask(&mut self, question: MessageRequest) -> LanguageServerMessageId {
        while self.questions.len() >= QUESTION_CAPACITY {
            // Dropping answers the server "none".
            let _ = self.questions.pop_first();
        }
        self.next_question = self.next_question.wrapping_add(1);
        let id = LanguageServerMessageId(self.next_question);
        self.questions.insert(id, question);
        id
    }

    /// Answer question `id`, returning whether it was still open.
    pub(crate) fn answer(&mut self, id: LanguageServerMessageId, action: Option<usize>) -> bool {
        self.questions
            .remove(&id)
            .map(|question| question.respond(action))
            .is_some()
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn entry(text: &str) -> LanguageServerLogEntry {
        LanguageServerLogEntry {
            root: PathBuf::from("/w"),
            severity: Severity::Hint,
            text: text.to_owned(),
        }
    }

    #[test]
    fn a_log_keeps_the_newest_entries_per_provider() {
        let mut state = WindowState::default();
        for line in 0..LOG_CAPACITY + 2 {
            state.log(&LanguageServerId::Gopls, entry(&line.to_string()));
        }
        state.log(&LanguageServerId::Pyright, entry("other"));
        let gopls = state.entries(&LanguageServerId::Gopls);
        assert_eq!(gopls.len(), LOG_CAPACITY);
        assert_eq!(gopls.first().map(|e| e.text.as_str()), Some("2"));
        assert_eq!(state.entries(&LanguageServerId::Pyright).len(), 1);
        assert!(state.entries(&LanguageServerId::RustAnalyzer).is_empty());
    }

    #[test]
    fn levels_map_onto_severities() {
        assert_eq!(severity(MessageLevel::Error), Severity::Error);
        assert_eq!(severity(MessageLevel::Warning), Severity::Warning);
        assert_eq!(severity(MessageLevel::Info), Severity::Information);
        assert_eq!(severity(MessageLevel::Log), Severity::Hint);
        assert_eq!(severity(MessageLevel::Debug), Severity::Hint);
    }

    #[test]
    fn a_client_token_names_the_wire_token() {
        assert_eq!(
            progress_token(&WorkToken::Number(3)),
            ProgressToken::Number(3)
        );
        assert_eq!(
            progress_token(&WorkToken::Text("t".into())),
            ProgressToken::String("t".into())
        );
    }

    #[test]
    fn filesystem_events_become_file_changes() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let present = dir.path().join("present.toml");
        std::fs::write(&present, "")?;
        let gone = dir.path().join("gone.toml");
        let event = |kind, paths: Vec<PathBuf>| FsEvent { kind, paths };
        assert_eq!(
            file_changes(&event(FsEventKind::Modified, vec![present.clone()])),
            vec![FileChange::new(present.clone(), FileChangeKind::Changed)]
        );
        assert_eq!(
            file_changes(&event(FsEventKind::Created, vec![gone.clone()]))[0].kind,
            FileChangeKind::Created
        );
        assert_eq!(
            file_changes(&event(FsEventKind::Removed, vec![gone.clone()]))[0].kind,
            FileChangeKind::Deleted
        );
        assert_eq!(
            file_changes(&event(
                FsEventKind::Renamed,
                vec![gone.clone(), present.clone()]
            )),
            vec![
                FileChange::new(gone, FileChangeKind::Deleted),
                FileChange::new(present.clone(), FileChangeKind::Created),
            ]
        );
        assert!(file_changes(&event(FsEventKind::WatchDegraded, vec![present])).is_empty());
        Ok(())
    }
}
