//! Settling a retirement: what the user is owed when a provider stops serving.
//!
//! Split out of [`super::updates`] to keep that module under the workspace's
//! per-file code-line ceiling. The pieces belong together in any case -- a
//! retirement clears the provider's markers, reports that it stopped, and
//! republishes the inventory the panel is drawn from, and doing only some of
//! those is the defect this change exists to remove.

use super::*;

impl Session {
    /// Drop one server instance's diagnostic layer, republishing the documents
    /// that carried it.
    ///
    /// The layer key is the slot's -- `{provider}@{root}` -- so removing it is
    /// already scoped to the instance that died: a provider still running at
    /// another repository root keeps its own markers. Other servers'
    /// diagnostics, spell-check and lint results share the merged set and are
    /// untouched.
    /// Settle what a retirement owes the user: markers off the screen, and the
    /// client told the provider is no longer serving.
    ///
    /// Both halves happen here, together, for every path that retires a slot --
    /// a document closing, a settings change, an explicit restart. They used to
    /// be spread across those paths in different combinations, and no path did
    /// both, which is how a retired provider kept its squiggles *and* went on
    /// being reported as running.
    ///
    /// The report is emitted directly rather than routed back through the task
    /// channel. It is the manager's own statement about a slot it has already
    /// removed, so there is no incarnation to attribute it to and nothing for a
    /// fence to decide -- and a message would have had to be exempted from the
    /// very fence that keeps a retired task quiet.
    pub(crate) fn adopt_retirement(&mut self, retired: crate::lsp::Retired) {
        let keys = retired.into_keys();
        if keys.is_empty() {
            return;
        }
        for key in keys {
            self.clear_lsp_diagnostic_layer(&key);
            self.emit(
                None,
                Event::LanguageServerRuntimeChanged {
                    server: key.provider,
                    root: key.root,
                    state: crate::api::LanguageServerRuntimeState::Idle,
                    error: None,
                },
            );
        }
        self.publish_language_server_inventory();
    }

    /// Push the whole provider inventory, unsolicited.
    ///
    /// The per-transition event above carries a provider's *state*, which is all
    /// the badge needs and all a client can usefully be given at that latency.
    /// It is not enough to keep a client's cached row right: `open_documents`
    /// has no event of its own, so a client patching state field by field
    /// finishes with a row that says idle and still has documents attached --
    /// and offers a Restart for a process that is gone. The inventory is
    /// authoritative for every field at once, so it is sent whole.
    ///
    /// Sent once per retirement rather than once per slot, and only on
    /// retirement: the document count only falls when a slot goes, and this is
    /// not cheap to build. Per provider it reads the managed-install registry
    /// four times, and per provider *and root* it re-resolves the executable --
    /// probing the project directory and scanning `PATH`. Nothing memoises any
    /// of it. If that ever shows up in a profile, the fix is to split
    /// `inventory` into a settings-and-registry half (which changes only on
    /// config reload, install, uninstall and decline, each of which already has
    /// an event) and a live half that is pure over the slot map -- not to add a
    /// second cache, which is the thing this change exists to remove.
    ///
    /// `replace` on the client already accepts an untagged snapshot, so this
    /// needs no new vocabulary.
    fn publish_language_server_inventory(&mut self) {
        let paths = self
            .store
            .docs
            .values()
            .map(|document| document.path.clone())
            .collect::<Vec<_>>();
        let servers = self.lsp.inventory(paths);
        self.emit(None, Event::LanguageServerStatus { servers });
    }

    pub(super) fn clear_lsp_diagnostic_layer(&mut self, server: &crate::lsp::SlotKey) {
        let affected = self
            .store
            .docs
            .iter_mut()
            .filter(|(_, document)| document.lsp_diagnostics.contains_key(server))
            .map(|(doc_id, document)| {
                document.lsp_diagnostics.remove(server);
                *doc_id
            })
            .collect::<Vec<_>>();
        for doc_id in affected {
            self.publish_document_diagnostics(doc_id);
        }
    }
}
