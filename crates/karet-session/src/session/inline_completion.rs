//! Inline completions through the session seam: asking every server attached
//! to a document for ghost text ([`Command::InlineCompletion`]), handing what
//! each proposes back with its ranges in buffer columns, and telling the
//! proposing server when one of its items was accepted
//! ([`Command::AcceptInlineCompletion`]) -- and signing in to the account such
//! a server proposes for ([`Command::SignInLanguageServer`]).

use karet_core::InlineCompletionItem;
use karet_core::InlineCompletionTrigger;

use super::columns::utf16_caret;
use super::columns::utf16_range_to_buffer;
use super::*;
use crate::api::AccountOutcome;
use crate::api::LanguageServerId;

impl Session {
    /// Serve [`Command::InlineCompletion`]: convert the caret to the
    /// servers' UTF-16 encoding and ask each attached server. Nothing is
    /// answered when no server takes it: there is no empty answer to send
    /// (see the command's docs).
    pub(super) fn inline_completion(
        &mut self,
        id: RequestId,
        doc_id: DocumentId,
        (position, trigger): (LineCol, InlineCompletionTrigger),
    ) {
        let Some(doc) = Self::doc_or_report(&self.store, &self.events, id, doc_id) else {
            return;
        };
        let version = doc.buffer.version();
        let at = (utf16_caret(doc, position), trigger);
        let asked = self
            .lsp
            .inline_completion(id, doc_id, version, &doc.path, at);
        tracing::trace!(doc = doc_id.0, asked, "inline completion asked");
    }

    /// Serve [`Command::AcceptInlineCompletion`] by telling the server that
    /// proposed the item. A server gone since has nobody left to tell.
    pub(super) fn accept_inline_completion(
        &mut self,
        id: RequestId,
        doc_id: DocumentId,
        server: &LanguageServerId,
        item: InlineCompletionItem,
    ) {
        let Some(doc) = Self::doc_or_report(&self.store, &self.events, id, doc_id) else {
            return;
        };
        if !self.lsp.accept_inline_completion(server, &doc.path, item) {
            tracing::debug!(
                server = server.key(),
                "inline completion accepted with no server to tell"
            );
        }
    }

    /// Serve [`Command::SignInLanguageServer`] (`sign_in`) or
    /// [`Command::SignOutLanguageServer`] on a running instance of `server`.
    /// With none running the answer is why, at once.
    pub(super) fn language_server_account(
        &mut self,
        id: RequestId,
        server: &LanguageServerId,
        sign_in: bool,
    ) {
        if self.lsp.account(server, id, sign_in) {
            return;
        }
        let outcome = AccountOutcome::Failed {
            message: format!(
                "{} is not running: open a file it serves, then try again",
                server.key()
            ),
        };
        let server = server.clone();
        self.emit(Some(id), Event::LanguageServerAccount { server, outcome });
    }

    /// Hand one server's [`LspUpdate::InlineCompletions`] to the client with
    /// every range in buffer columns, or an [`LspUpdate::Account`] as it is.
    /// Items for a document closed since are stale by definition and dropped.
    pub(super) fn deliver_inline_completions(&mut self, update: LspUpdate) {
        if let LspUpdate::Account {
            request,
            server,
            outcome,
            ..
        } = update
        {
            self.emit(
                Some(request),
                Event::LanguageServerAccount { server, outcome },
            );
            return;
        }
        let LspUpdate::InlineCompletions {
            server,
            request,
            doc,
            version,
            mut items,
            ..
        } = update
        else {
            return;
        };
        let Some(open) = self.store.docs.get(&doc) else {
            return;
        };
        for item in &mut items {
            item.range = utf16_range_to_buffer(&open.buffer, item.range);
        }
        self.emit(
            Some(request),
            Event::InlineCompletions {
                doc,
                version,
                server: server.provider,
                items,
            },
        );
    }
}
