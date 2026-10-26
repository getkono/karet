use super::*;

impl LspManager {
    /// Whether this session currently owns a process for `provider`.
    pub(crate) fn is_running(&self, provider: &LanguageServerId) -> bool {
        self.servers.keys().any(|key| key.serves(provider))
    }

    /// Retire one slot. The only way a slot ever leaves the manager.
    ///
    /// Everything a slot owns goes with it in one step: the slot itself, the
    /// runtime state it carried, the suppression that stopped its sync failures
    /// repeating, and -- through the returned receipt -- its diagnostic layer.
    /// Four call sites used to do four different subsets of this, which is why a
    /// provider could be retired and still be reported running, or retired and
    /// still be marking a file.
    pub(super) fn retire(&mut self, key: &SlotKey) -> Retired {
        let mut retired = Retired::default();
        if let Some(slot) = self.servers.remove(key) {
            self.sync_failure_reported.remove(key);
            retired.push(key.clone(), slot.documents.into_iter().collect());
        }
        retired
    }

    /// Retire every slot matching `wanted`.
    pub(super) fn retire_matching(&mut self, wanted: impl Fn(&SlotKey) -> bool) -> Retired {
        let keys: Vec<SlotKey> = self
            .servers
            .keys()
            .filter(|key| wanted(key))
            .cloned()
            .collect();
        let mut retired = Retired::default();
        for key in keys {
            retired.absorb(self.retire(&key));
        }
        retired
    }

    /// Retire the named provider's tasks after an explicit install or restart.
    ///
    /// Scoped to the provider the caller named. It used to clear *every* slot,
    /// so uninstalling Ruff stopped rust-analyzer too, and restarting one server
    /// silently restarted all of them. Returns `None` when nothing was running,
    /// which is the caller's signal not to reopen documents.
    /// `#[must_use]` on the function as well as on [`Retired`]: a dropped
    /// `Option<Retired>` warns about neither, so the type-level guarantee does
    /// not reach the two paths that return one.
    #[must_use]
    pub(crate) fn restart(&mut self, provider: LanguageServerId) -> Option<Retired> {
        self.missing_reported.remove(&provider);
        if !self.is_running(&provider) {
            return None;
        }
        Some(self.retire_matching(|key| key.serves(&provider)))
    }

    /// Take the next slot token.
    ///
    /// Monotonic and never reused. Starting at 1 rather than 0 is deliberate: a
    /// token that a `Default` could produce would match a live slot's, and a
    /// message carrying it would be believed.
    pub(super) fn take_token(&mut self) -> SlotToken {
        let token = self.next_token;
        self.next_token = token.next();
        token
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

    /// Start the task serving `key` with `spec`, unless a slot already holds
    /// it. `None` when there is no async runtime to run a task on: a session
    /// driven synchronously (unit tests, bare library use) runs without LSP.
    ///
    /// `settings_key` is the `lsp.servers` entry the spec's settings came from
    /// (see [`Self::configure`]); `semantic_owner` says whether the slot owns
    /// semantic tokens for the language starting it (see
    /// [`Self::owns_semantic_tokens`]).
    pub(super) fn spawn_slot(
        &mut self,
        key: &SlotKey,
        spec: &LspSpec,
        primary: bool,
        settings_key: String,
        semantic_owner: bool,
    ) -> Option<()> {
        if self.servers.contains_key(key) {
            return Some(());
        }
        let handle = tokio::runtime::Handle::try_current().ok()?;
        let (tx, rx) = mpsc::channel(SERVER_COMMAND_CAPACITY);
        let token = self.take_token();
        let settings: SettingsCell =
            Arc::new(std::sync::RwLock::new(spec.options.settings.clone()));
        let connector = with_settings(&self.connector, Arc::clone(&settings));
        // Inserted before the task is spawned, not after. The task's first act
        // is to report `Starting`, and a report is accepted only against a live
        // slot -- so spawning first leaves a window in which the provider's own
        // opening move is thrown away.
        let mut slot = ServerSlot::new(token, tx, primary, settings);
        slot.settings_key = settings_key;
        slot.semantic_owner = semantic_owner;
        slot.semantic
            .send_replace(semantic_owner && self.semantic_highlighting);
        let semantic = slot.semantic.subscribe();
        self.servers.insert(key.clone(), slot);
        handle.spawn(runtime::server_task(runtime::ServerTask {
            spec: spec.clone(),
            key: key.clone(),
            token,
            rx,
            updates: self.updates.clone(),
            connector,
            generation: self.generation,
            semantic,
        }));
        Some(())
    }

    /// Whether `provider` owns semantic tokens for `language`: the one
    /// `lsp.languages.<language>.semanticTokens` names, else the language's
    /// primary server. Naming a provider that does not serve the language
    /// leaves it without semantic tokens.
    pub(super) fn owns_semantic_tokens(
        &self,
        language: &str,
        provider: &LanguageServerId,
        primary: bool,
    ) -> bool {
        match self
            .settings
            .languages
            .get(language)
            .and_then(|selection| selection.semantic_tokens.as_deref())
        {
            Some(owner) => owner == provider.key(),
            None => primary,
        }
    }

    /// Turn semantic-token fetching on or off for every slot that owns them
    /// (`editor.semanticHighlighting.enabled`), without restarting anything:
    /// a running task stops fetching, or starts, as the value reaches it.
    pub(crate) fn set_semantic_highlighting(&mut self, enabled: bool) {
        self.semantic_highlighting = enabled;
        for slot in self.servers.values() {
            slot.semantic.send_if_modified(|on| {
                let next = slot.semantic_owner && enabled;
                std::mem::replace(on, next) != next
            });
        }
    }

    /// Attach what `lsp.servers` says about `provider` beyond the launch: its
    /// settings, and initialization options that replace any karet chose.
    ///
    /// The provider's own entry, else -- for a language's primary -- the one
    /// keyed by `language`, the key `lsp.servers` has always been documented
    /// by, which a commandless entry now leaves launching karet's provider.
    /// Returns the key the settings are read from, which a reload reads them
    /// from again.
    pub(super) fn configure(
        &self,
        spec: &mut LspSpec,
        provider: &LanguageServerId,
        language: Option<&str>,
    ) -> String {
        let servers = &self.settings.servers;
        let key = std::iter::once(provider.key())
            .chain(language)
            .find(|key| servers.contains_key(*key))
            .unwrap_or(provider.key());
        if let Some(entry) = servers.get(key) {
            spec.options.settings.clone_from(&entry.settings);
            if entry.initialization_options.is_some() {
                spec.options
                    .initialization_options
                    .clone_from(&entry.initialization_options);
            }
        }
        key.to_owned()
    }

    /// Hand every running slot whose provider's settings changed its new
    /// settings: into its cell, for the connections it makes from now on, and
    /// to the live one with `workspace/didChangeConfiguration`.
    pub(super) fn push_settings(&self) {
        for (key, slot) in &self.servers {
            let current = self
                .settings
                .servers
                .get(&slot.settings_key)
                .and_then(|entry| entry.settings.clone());
            let Ok(mut held) = slot.settings.write() else {
                continue;
            };
            if *held == current {
                continue;
            }
            held.clone_from(&current);
            drop(held);
            let notice = Notice::Configure(current.unwrap_or(serde_json::Value::Null));
            if slot.tx.try_send(ServerCmd::Notify(notice)).is_err() {
                tracing::warn!(slot = %key, "settings push not queued; the next connection has them");
            }
        }
    }
}

/// `inner`, with each spec it connects carrying the settings `cell` holds at
/// that moment rather than the ones the slot started with.
fn with_settings(inner: &Connector, cell: SettingsCell) -> Connector {
    let inner = Arc::clone(inner);
    Arc::new(move |mut spec: LspSpec, root| {
        if let Ok(current) = cell.read() {
            spec.options.settings.clone_from(&current);
        }
        inner(spec, root)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::schema::Lsp as LspSettings;

    /// Two slots must never share a token, and none may hold zero.
    ///
    /// Tested here rather than at the `Command`/`Event` seam because a token is
    /// only *observable* there through a zombie -- a task speaking after its slot
    /// is gone -- and by design there is now almost no window in which one can:
    /// the parting report is deleted and retirement is synchronous. That makes
    /// the fence defence in depth against a report already in flight, and leaves
    /// its allocator as the thing an honest test can pin.
    ///
    /// Not a hypothetical. Mutation testing (`mise run mutants`) reported both
    /// halves of this as surviving until this test existed: pinning every token
    /// to the same value left the whole suite green, and a fence that cannot
    /// tell two incarnations apart is not a fence.
    ///
    /// Zero is called out separately because it is the value a `Default` yields,
    /// so a token that was never set would otherwise match the first real slot.
    #[test]
    fn no_two_slots_ever_share_a_token() {
        let (mut manager, _updates) = LspManager::new(LspSettings::default(), None, None, None);
        let mut seen = std::collections::HashSet::new();
        for _ in 0..1000 {
            let token = manager.take_token();
            assert_ne!(
                token,
                SlotToken::default(),
                "a default-valued token would match a live slot"
            );
            assert!(seen.insert(token), "token {token:?} was handed out twice");
        }
    }
}
