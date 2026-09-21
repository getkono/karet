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

/// A live server task, and what the manager knows about it.
///
/// The slot is the *only* record of a provider's runtime state. There is
/// deliberately no map beside it: a state with no slot is then unrepresentable,
/// so "no slot means idle" cannot be violated by forgetting to clean something
/// up. A stale entry in such a map is what made the Language Servers panel offer
/// a Restart for a process that did not exist.
pub(super) struct ServerSlot {
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
    pub(super) fn new(tx: mpsc::Sender<ServerCmd>, primary: bool) -> Self {
        Self {
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
