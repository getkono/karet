//! Dispatch for the manifest-hints producer (see [`crate::manifest_hints`]).

use super::*;

#[cfg(feature = "deps")]
impl Session {
    /// Queue a dependency-freshness check for `doc_id` when it is a
    /// `Cargo.toml` and `deps.enabled` is on; the worker spawns on first use.
    pub(crate) fn refresh_manifest_hints(&mut self, doc_id: DocumentId) {
        if !self.config.settings.deps.enabled {
            return;
        }
        let Some(doc) = self.store.docs.get(&doc_id) else {
            return;
        };
        if doc
            .path
            .file_name()
            .is_none_or(|name| !name.eq_ignore_ascii_case("Cargo.toml"))
        {
            return;
        }
        let lockfile = doc
            .path
            .parent()
            .map(|dir| dir.join("Cargo.lock"))
            .and_then(|path| std::fs::read_to_string(path).ok());
        let job = crate::manifest_hints::HintJob {
            doc: doc_id,
            version: doc.buffer.version(),
            manifest: doc.buffer.text(),
            lockfile,
        };
        let worker = self
            .manifest_hints_worker
            .get_or_insert_with(|| crate::manifest_hints::spawn(self.events.clone()));
        let _ = worker.send(job);
    }
}

#[cfg(not(feature = "deps"))]
impl Session {
    /// Without the `deps` feature there are no manifest hints.
    pub(crate) fn refresh_manifest_hints(&mut self, _doc: DocumentId) {}
}
