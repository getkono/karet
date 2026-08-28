//! Reporting a running managed operation wherever the user is.
//!
//! An install, update, or uninstall used to report itself only inside the
//! manager tab — but the user who approved one was usually looking at a file
//! when they did, and had no reason to open a tab they had never seen. So the
//! operation carries a notification card: a spinner and the bytes while it
//! runs, superseded in place by the outcome.
//!
//! The manager tab keeps its detailed per-provider table; this is additive.

use super::*;

impl App {
    /// Whether any managed operation is still running (drives the spinner tick).
    pub(in crate::app) fn language_server_operation_running(&self) -> bool {
        self.lsp_runtime
            .operations
            .iter()
            .any(|pending| pending.kind.is_download())
    }

    /// The notification tag every managed-operation card shares.
    ///
    /// One tag, not one per provider: these operations are serialized per
    /// provider anyway, and a stack of five download cards buries whatever else
    /// the editor was trying to say.
    pub(in crate::app) const OPERATION_TAG: &'static str = "lsp.operation";

    /// Repaint the running-operation card, or clear it when nothing is running.
    ///
    /// The manager tab keeps the detailed per-provider table; this is what a user
    /// who approved an install from an editor buffer sees, having never opened
    /// that tab.
    pub(in crate::app) fn sync_language_server_toast(&mut self) {
        let Some(pending) = self
            .lsp_runtime
            .operations
            .iter()
            .find(|pending| pending.kind.is_download())
            .cloned()
        else {
            self.notifications.dismiss_tagged(Self::OPERATION_TAG);
            return;
        };
        let name = pending
            .server
            .as_ref()
            .map(|server| server.display_name().to_string())
            .unwrap_or_else(|| "language servers".to_string());
        let frame = Spinner::new(self.icon_style).frame(pending.since.elapsed());
        let title = format!("{frame} {} {name}", pending.kind.progressive());
        let body = match (pending.downloaded, pending.total) {
            (Some(done), Some(total)) if total > 0 => Some(format!(
                "{} of {} ({}%)",
                human_bytes(done),
                human_bytes(total),
                done.saturating_mul(100) / total
            )),
            // Upstream did not declare a size, so a percentage would be invented.
            (Some(done), _) => Some(human_bytes(done)),
            _ => None,
        };
        self.notify_progress(
            NotificationKind::Lsp,
            Self::OPERATION_TAG.to_string(),
            title,
            body,
        );
    }
}
