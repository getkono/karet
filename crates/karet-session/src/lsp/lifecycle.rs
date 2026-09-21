use super::*;

impl LspManager {
    /// Whether this session currently owns a process for `provider`.
    pub(crate) fn is_running(&self, provider: &LanguageServerId) -> bool {
        self.servers.keys().any(|key| key.serves(provider))
    }

    /// Retire live tasks after an explicit install or restart request.
    ///
    /// All tasks are retired together so late task updates are rejected by one
    /// generation boundary. The session immediately reopens its documents.
    pub(crate) fn restart(&mut self, provider: LanguageServerId) -> bool {
        self.missing_reported.remove(&provider);
        let running = self.is_running(&provider);
        if running {
            self.generation = self.generation.wrapping_add(1);
            self.servers.clear();
        }
        running
    }

    /// Forget a missing-provider suppression after its installation activates.
    pub(crate) fn installed(&mut self, provider: LanguageServerId) {
        self.missing_reported.remove(&provider);
    }

    /// Record a runtime transition on the slot that reported it.
    ///
    /// A report whose slot is gone is dropped rather than stored. That is the
    /// whole of the rule this used to need a side map and a fence to express:
    /// the state lives on the slot, so retiring the slot takes the state with it,
    /// and a task speaking after its retirement has nowhere to write.
    pub(crate) fn note_runtime(
        &mut self,
        key: &SlotKey,
        state: LanguageServerRuntimeState,
        error: Option<String>,
    ) {
        if let Some(slot) = self.servers.get_mut(key) {
            slot.runtime = state;
            slot.error = error;
        }
    }
}
