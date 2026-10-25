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
        let lockfile = manifest_lockfile(&doc.path);
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

/// The text of the `Cargo.lock` governing `manifest`: beside it, or for a
/// workspace member, the workspace root's above it. `None` when there is none
/// or it cannot be read; the hints then fall back to the manifest alone.
#[cfg(feature = "deps")]
fn manifest_lockfile(manifest: &std::path::Path) -> Option<String> {
    let lock = crate::find_cargo_lock(manifest.parent()?)?;
    std::fs::read_to_string(lock).ok()
}

#[cfg(not(feature = "deps"))]
impl Session {
    /// Without the `deps` feature there are no manifest hints.
    pub(crate) fn refresh_manifest_hints(&mut self, _doc: DocumentId) {}
}

#[cfg(all(test, feature = "deps"))]
mod tests {
    use super::manifest_lockfile;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    const LOCK: &str = "version = 3\n";

    #[test]
    fn a_member_manifest_reads_the_workspace_lockfile() -> TestResult {
        let tmp = tempfile::tempdir()?;
        let member = tmp.path().join("crates").join("member");
        std::fs::create_dir_all(&member)?;
        std::fs::write(tmp.path().join("Cargo.lock"), LOCK)?;
        assert_eq!(
            manifest_lockfile(&member.join("Cargo.toml")).as_deref(),
            Some(LOCK)
        );
        Ok(())
    }

    #[test]
    fn a_sibling_lockfile_is_still_read() -> TestResult {
        let tmp = tempfile::tempdir()?;
        std::fs::write(tmp.path().join("Cargo.lock"), LOCK)?;
        assert_eq!(
            manifest_lockfile(&tmp.path().join("Cargo.toml")).as_deref(),
            Some(LOCK)
        );
        Ok(())
    }

    #[test]
    fn a_manifest_without_any_lockfile_reads_none() -> TestResult {
        let tmp = tempfile::tempdir()?;
        assert_eq!(manifest_lockfile(&tmp.path().join("Cargo.toml")), None);
        Ok(())
    }
}
