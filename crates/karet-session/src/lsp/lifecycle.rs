use super::*;

impl LspManager {
    /// Whether this session currently owns a process for `provider`.
    pub(crate) fn is_running(&self, provider: &LanguageServerId) -> bool {
        self.servers
            .values()
            .any(|slot| slot.provider.as_ref() == Some(provider))
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

    /// Record a runtime transition before forwarding it to presentation clients.
    pub(crate) fn note_runtime(
        &mut self,
        server: LanguageServerId,
        root: PathBuf,
        state: LanguageServerRuntimeState,
        error: Option<String>,
    ) {
        self.runtime_states.insert((server, root), (state, error));
    }
}
