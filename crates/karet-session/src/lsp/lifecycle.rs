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
        // `Unavailable` is the task's verdict that it will never start here, and
        // it stops looping rather than respawning forever. Retiring its slot with
        // it is what makes the verdict recoverable: `ensure_server` short-circuits
        // on an existing slot, so leaving one behind meant the provider was never
        // resolved again for the life of the session -- installing the binary,
        // fixing its permissions, or putting it on `PATH` changed nothing until
        // the user found the manager and pressed Restart.
        //
        // With the slot gone, the next `document_opened` re-runs resolution from
        // scratch: settings, then the project, then `PATH`, then the install
        // journal. That reads the same journal every other karet process writes
        // through its per-provider lock, so two instances converge rather than
        // fight, and re-resolving is idempotent -- it either finds an executable
        // or reports the same absence again.
        if state == LanguageServerRuntimeState::Unavailable {
            self.servers
                .retain(|_, slot| slot.provider.as_ref() != Some(&server) || slot.root != root);
        }
        self.runtime_states.insert((server, root), (state, error));
    }
}
