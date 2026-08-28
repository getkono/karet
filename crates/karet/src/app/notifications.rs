//! Notification and diagnostic state updates.

use super::*;

impl App {
    /// Push a notification onto the center. Errors and warnings persist until
    /// dismissed; info and success auto-expire after a few seconds.
    pub(super) fn notify(
        &mut self,
        severity: Severity,
        kind: NotificationKind,
        title: impl Into<String>,
    ) {
        self.notify_tagged(severity, kind, title, None);
    }

    /// Push or replace a persistent condition identified by `tag`.
    pub(super) fn notify_tagged(
        &mut self,
        severity: Severity,
        kind: NotificationKind,
        title: impl Into<String>,
        tag: Option<String>,
    ) {
        let title = title.into();
        match severity {
            Severity::Error => {
                tracing::error!(notification_kind = ?kind, message = %title, "notification");
            },
            Severity::Warning => {
                tracing::warn!(notification_kind = ?kind, message = %title, "notification");
            },
            _ => {},
        }
        let timeout = match severity {
            Severity::Error | Severity::Warning => None,
            _ => Some(Duration::from_secs(4)),
        };
        self.notifications.push(
            Notification {
                id: NotificationId(0),
                severity,
                kind,
                title,
                body: None,
                tag,
                timeout,
                dismissable: true,
            },
            Instant::now(),
        );
    }

    /// Push or replace the card for a running operation.
    ///
    /// Persistent (no timeout) because the work is still going: an auto-expiring
    /// card would vanish mid-download and leave the user with no sign anything is
    /// happening. `NotificationCenter::push` replaces any active card sharing this
    /// `tag`, so progress updates in place rather than stacking, and the eventual
    /// success or failure supersedes it under the same tag.
    pub(super) fn notify_progress(
        &mut self,
        kind: NotificationKind,
        tag: String,
        title: impl Into<String>,
        body: Option<String>,
    ) {
        self.notifications.push(
            Notification {
                id: NotificationId(0),
                severity: Severity::Information,
                kind,
                title: title.into(),
                body,
                tag: Some(tag),
                timeout: None,
                // Dismissable: the user may not care about a background download,
                // and the manager tab still has the detail.
                dismissable: true,
            },
            Instant::now(),
        );
    }

    /// Surface a dropped backend-submission error as a persistent notification, so a
    /// closed or wedged backend never fails silently.
    pub(super) fn notify_backend_error(&mut self, error: BackendError) {
        self.notify(
            Severity::Error,
            NotificationKind::System,
            format!("backend: {error}"),
        );
    }

    /// Replace the complete merged diagnostic set for one document.
    pub(super) fn replace_document_diagnostics(
        &mut self,
        doc: DocumentId,
        diagnostics: Vec<Diagnostic>,
    ) {
        if diagnostics.is_empty() {
            self.docs.diagnostics.remove(&doc);
        } else {
            self.docs.diagnostics.insert(doc, diagnostics);
        }
    }
}
