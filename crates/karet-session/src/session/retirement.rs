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
        // Recorded, not emitted. A retirement is very often the first half of an
        // operation -- a restart retires a slot and then starts its replacement --
        // and an inventory read here would describe the gap in the middle, where
        // the provider has no process and no documents. `settle_lsp_inventory`
        // sends the signal once the actor has finished the whole unit of work.
        self.lsp_inventory_stale = true;
    }

    /// Tell the client its inventory is stale, if anything this unit of work did
    /// made it so.
    ///
    /// Drained by the actor loop once per message it handles, rather than at the
    /// point of retirement, because retirement is routinely mid-operation.
    /// `restart` retires a provider and reopens its documents against a fresh
    /// process; a signal sent between the two would have the client re-query and
    /// cache `idle, 0 documents` for a provider that is up, and nothing
    /// afterwards would correct it -- the transition event carries state, and
    /// the document count has no event at all.
    ///
    /// Coalescing is the second reason: retiring four slots is one staleness
    /// fact, not four, and a settings reload retires every slot at once.
    ///
    /// A no-op unless something set the flag, so the actor calls it after every
    /// message rather than after the ones that can retire -- that list would
    /// have to be maintained by hand, and an arm that forgot would go back to
    /// silently never telling the client.
    pub(crate) fn settle_lsp_inventory(&mut self) {
        if std::mem::take(&mut self.lsp_inventory_stale) {
            self.emit(None, Event::LanguageServerInventoryStale);
        }
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
