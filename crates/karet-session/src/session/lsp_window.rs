//! Adopting what a language server says to the user.
//!
//! Split out of [`super::updates`] to keep that module under the workspace's
//! per-file code-line ceiling. Every string here is the server's, so each is
//! made safe to paint before it leaves the session: a notification and a
//! question get one sanitised line, and the log keeps its lines but loses
//! every escape.

use super::*;
use crate::api::LanguageServerLogEntry;
use crate::lsp::WindowUpdate;
use crate::session::notify_text::one_line;
use crate::session::notify_text::printable;

impl Session {
    /// Turn one thing `key`'s server said into what the client sees: a work
    /// report, a notification (and a log entry), a log entry alone, or a
    /// question held until the client answers it.
    pub(super) fn adopt_window_update(&mut self, key: &crate::lsp::SlotKey, update: WindowUpdate) {
        match update {
            WindowUpdate::Progress(progress) => {
                let mut work = crate::lsp::work_report(key, progress);
                work.title = work.title.as_deref().map(one_line);
                work.message = work.message.as_deref().map(one_line);
                self.emit(None, Event::LanguageServerWork { work });
            },
            WindowUpdate::Message(message) => {
                let severity = crate::lsp::message_severity(message.level);
                if message.kind == karet_lsp::ServerMessageKind::Show {
                    let server = key.provider.display_name();
                    self.emit(
                        None,
                        Event::Notification {
                            // A shown message is never only a hint: the server
                            // asked for the user's eyes.
                            severity: severity.min(Severity::Information),
                            kind: NotificationKind::Lsp,
                            message: format!("{server}: {}", one_line(&message.text)),
                        },
                    );
                }
                self.lsp.window.log(
                    &key.provider,
                    LanguageServerLogEntry {
                        root: key.root.clone(),
                        severity,
                        text: printable(&message.text),
                    },
                );
            },
            WindowUpdate::Request(question) => {
                let severity = crate::lsp::message_severity(question.level);
                let message = one_line(&question.message);
                let actions = question
                    .actions
                    .iter()
                    .map(|title| one_line(title))
                    .collect();
                let request = self.lsp.window.ask(question);
                self.emit(
                    None,
                    Event::LanguageServerMessageRequest {
                        request,
                        server: key.provider.clone(),
                        severity,
                        message,
                        actions,
                    },
                );
            },
        }
    }
}
