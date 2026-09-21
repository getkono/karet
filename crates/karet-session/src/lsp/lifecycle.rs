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
