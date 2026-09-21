//! The unit of language-server ownership: one provider, at one repository root.
//!
//! A [`SlotKey`] is the identity of three things at once -- the manager's slot,
//! the task serving it, and the diagnostic layer that task publishes under. They
//! are one value here so they cannot drift apart. Before this the same identity
//! was spelled two ways: a `"{provider}@{root}"` string on the slot and the
//! layer, and a `(LanguageServerId, PathBuf)` tuple on the runtime-state cache.
//! Two spellings meant two lookups, and the fences that tried to reconcile them
//! are what issue #278 was filed to remove.

use std::collections::HashSet;
use std::fmt;
use std::path::PathBuf;

use tokio::sync::mpsc;

use super::message::ServerCmd;
use crate::api::LanguageServerId;
use crate::api::LanguageServerRuntimeState;

/// Which incarnation of a slot is speaking.
///
/// A newtype rather than a bare `u64`, and the distinction is load-bearing: the
/// task also carries a `generation`, which is also a `u64`, and the two are
/// passed side by side through the report helpers. With both as `u64` a call
/// that handed over the wrong one compiled in silence -- and did, until an
/// adversarial review found deaths being reported under the generation and
/// silently refused by the very fence meant to protect them. Making the two
/// types different makes that class of mistake a build failure.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct SlotToken(u64);

impl SlotToken {
    /// The first token a session hands out.
    ///
    /// One, not zero: zero is what a `Default` yields, so a token nobody set
    /// would otherwise match the session's first real slot.
    pub(crate) const FIRST: Self = Self(1);

    /// The next token, which is never this one.
    pub(crate) fn next(self) -> Self {
        Self(self.0.wrapping_add(1).max(1))
    }
}

/// One language-server provider at one repository root.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct SlotKey {
    /// The provider this slot serves.
    pub(crate) provider: LanguageServerId,
    /// The repository root the server was launched against.
    pub(crate) root: PathBuf,
}

impl SlotKey {
    /// Identify a provider at a root.
    pub(crate) fn new(provider: LanguageServerId, root: impl Into<PathBuf>) -> Self {
        Self {
            provider,
            root: root.into(),
        }
    }

    /// Whether this slot serves `provider`, at any root.
    pub(crate) fn serves(&self, provider: &LanguageServerId) -> bool {
        self.provider == *provider
    }
}

impl fmt::Display for SlotKey {
    /// `{provider}@{root}` -- the spelling this identity has always had in logs.
    ///
    /// Kept stable because it is what a developer reading a trace recognises; it
    /// is no longer *parsed* anywhere, which is the point of the type.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}@{}", self.provider.key(), self.root.display())
    }
}

/// Slots that have just been retired, and whose markers the session still owes
/// the user.
///
/// Returned rather than acted on because the two halves of retiring a provider
/// live in different places: the manager owns the slot, and the diagnostic layer
/// lives on the documents, which it cannot reach. `#[must_use]` is the point of
/// the type -- it makes "retired a slot and forgot its markers" a build failure
/// rather than the silent omission that left a dead server's squiggles on screen
/// pointing at lines the user had since edited away.
#[must_use = "a retired slot's diagnostic layer must be cleared; pass this to `adopt_retirement`"]
#[derive(Default)]
pub(crate) struct Retired(Vec<(SlotKey, Vec<PathBuf>)>);

impl Retired {
    /// Nothing was retired.
    pub(crate) fn none() -> Self {
        Self(Vec::new())
    }

    /// Record one retired slot and the documents it was serving.
    pub(crate) fn push(&mut self, key: SlotKey, documents: Vec<PathBuf>) {
        self.0.push((key, documents));
    }

    /// Fold another retirement into this one.
    pub(crate) fn absorb(&mut self, other: Self) {
        self.0.extend(other.0);
    }

    /// The documents the retired slots were serving, deduplicated.
    ///
    /// What a restart must reopen, and *only* that. Deciding it from the
    /// language instead cannot work: the obvious test -- does this provider
    /// serve that language -- is answerable only for built-in providers, so a
    /// user-configured one would be retired and then never started again.
    pub(crate) fn document_paths(&self) -> Vec<PathBuf> {
        let mut paths: Vec<PathBuf> = self
            .0
            .iter()
            .flat_map(|(_, documents)| documents.iter().cloned())
            .collect();
        paths.sort();
        paths.dedup();
        paths
    }

    /// The retired slots, consuming the receipt.
    pub(crate) fn into_keys(self) -> Vec<SlotKey> {
        self.0.into_iter().map(|(key, _)| key).collect()
    }
}

/// A live server task, and what the manager knows about it.
///
/// The slot is the *only* record of a provider's runtime state. There is
/// deliberately no map beside it: a state with no slot is then unrepresentable,
/// so "no slot means idle" cannot be violated by forgetting to clean something
/// up. A stale entry in such a map is what made the Language Servers panel offer
/// a Restart for a process that did not exist.
pub(super) struct ServerSlot {
    /// Which task holds this slot.
    ///
    /// Distinct from every slot that ever held the same key before it, so a task
    /// that has been retired cannot be mistaken for the one that replaced it --
    /// the two are otherwise identical, since a key is re-taken unchanged.
    pub(super) token: SlotToken,
    /// The task's command inbox.
    pub(super) tx: mpsc::Sender<ServerCmd>,
    /// Documents currently attached to this instance.
    pub(super) documents: HashSet<PathBuf>,
    /// Whether this slot serves the language's primary provider rather than a
    /// diagnostics companion.
    pub(super) primary: bool,
    /// What the task last reported about itself.
    ///
    /// Starts at [`LanguageServerRuntimeState::Starting`] rather than waiting for
    /// the task's own first report, so a slot is never briefly indistinguishable
    /// from one that does not exist.
    pub(super) runtime: LanguageServerRuntimeState,
    /// The most recent concise failure the task reported, if any.
    pub(super) error: Option<String>,
}

impl ServerSlot {
    /// A slot for a task that is starting up.
    pub(super) fn new(token: SlotToken, tx: mpsc::Sender<ServerCmd>, primary: bool) -> Self {
        Self {
            token,
            tx,
            documents: HashSet::new(),
            primary,
            runtime: LanguageServerRuntimeState::Starting,
            error: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    #[test]
    fn a_key_prints_the_spelling_logs_have_always_used() {
        let key = SlotKey::new(LanguageServerId::RustAnalyzer, "/work/repo");
        assert_eq!(key.to_string(), "rust-analyzer@/work/repo");
    }

    #[test]
    fn the_same_provider_at_two_roots_is_two_slots() {
        let one = SlotKey::new(LanguageServerId::RustAnalyzer, "/work/a");
        let two = SlotKey::new(LanguageServerId::RustAnalyzer, "/work/b");
        assert_ne!(one, two);
        assert!(one.serves(&LanguageServerId::RustAnalyzer));
        assert_eq!(one.root, Path::new("/work/a"));
    }

    #[test]
    fn two_providers_at_one_root_are_two_slots() {
        let root = "/work/repo";
        assert_ne!(
            SlotKey::new(LanguageServerId::Pyright, root),
            SlotKey::new(LanguageServerId::Ruff, root)
        );
    }
}
