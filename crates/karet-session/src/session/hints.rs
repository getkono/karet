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
    std::fs::read_to_string(governing_lockfile(manifest)?).ok()
}

/// Locate the `Cargo.lock` cargo would use for `manifest`, without reading
/// the lock. A lock beside the manifest wins. Otherwise the package must be a
/// member of a workspace whose root holds the lock: the root its
/// `package.workspace` key names, or else the nearest ancestor `Cargo.toml`
/// declaring `[workspace]`, which must list the package in `members` (globs
/// allowed) and not under `exclude`. A package outside that workspace (a
/// `cargo fuzz` crate, an excluded or vendored package) yields `None`, as
/// does an unreadable or unparsable manifest on the way — an unrelated
/// lockfile's versions would mislabel the hints.
#[cfg(feature = "deps")]
fn governing_lockfile(manifest: &std::path::Path) -> Option<std::path::PathBuf> {
    let dir = manifest.parent()?;
    let sibling = dir.join("Cargo.lock");
    if sibling.is_file() {
        return Some(sibling);
    }
    let own = read_toml(manifest);
    if own.as_ref().is_some_and(|t| t.contains_key("workspace")) {
        // A workspace root with no lock of its own: nothing above governs it.
        return None;
    }
    let declared_root = own.as_ref().and_then(|t| {
        t.get("package")?
            .get("workspace")?
            .as_str()
            .map(str::to_owned)
    });
    if let Some(root) = declared_root {
        // Cargo itself rejects the package unless `root` is its workspace.
        let lock = dir.join(root).join("Cargo.lock");
        return lock.is_file().then_some(lock);
    }
    for ancestor in dir.ancestors().skip(1) {
        let candidate = ancestor.join("Cargo.toml");
        if !candidate.is_file() {
            continue;
        }
        let table = read_toml(&candidate)?;
        let Some(workspace) = table.get("workspace") else {
            continue;
        };
        // Cargo stops at the first `[workspace]` above: it is this package's
        // workspace only if it claims the package.
        let relative = dir.strip_prefix(ancestor).ok()?;
        if !workspace_claims(workspace, relative) {
            return None;
        }
        let lock = ancestor.join("Cargo.lock");
        return lock.is_file().then_some(lock);
    }
    None
}

/// Parse a `Cargo.toml` from disk; `None` when unreadable or invalid.
#[cfg(feature = "deps")]
fn read_toml(path: &std::path::Path) -> Option<toml::Table> {
    std::fs::read_to_string(path).ok()?.parse().ok()
}

/// Whether a `[workspace]` table lists the package at `relative` (its path
/// from the workspace root) as a member: some `members` glob matches it and
/// no `exclude` path contains it.
#[cfg(feature = "deps")]
fn workspace_claims(workspace: &toml::Value, relative: &std::path::Path) -> bool {
    let Some(components) = relative
        .components()
        .map(|c| match c {
            std::path::Component::Normal(part) => part.to_str(),
            _ => None,
        })
        .collect::<Option<Vec<&str>>>()
    else {
        return false;
    };
    let entries = |key: &str| -> Vec<Vec<String>> {
        workspace
            .get(key)
            .and_then(toml::Value::as_array)
            .map(|list| {
                list.iter()
                    .filter_map(toml::Value::as_str)
                    .map(path_segments)
                    .collect()
            })
            .unwrap_or_default()
    };
    let excluded = entries("exclude").iter().any(|prefix| {
        prefix.len() <= components.len() && prefix.iter().zip(&components).all(|(p, c)| p == c)
    });
    !excluded
        && entries("members")
            .iter()
            .any(|pattern| glob_segments(pattern, &components))
}

/// Split a workspace path entry into its components, dropping `.` and empty
/// segments (`./crates/*/` → `["crates", "*"]`).
#[cfg(feature = "deps")]
fn path_segments(entry: &str) -> Vec<String> {
    entry
        .split(['/', '\\'])
        .filter(|s| !s.is_empty() && *s != ".")
        .map(str::to_owned)
        .collect()
}

/// Match path components against glob components: `**` spans any number of
/// components, and within one, `*` spans any run of characters and `?` one.
/// A character class (`[...]`) never matches, so it yields no lockfile rather
/// than a wrong one.
#[cfg(feature = "deps")]
fn glob_segments(pattern: &[String], path: &[&str]) -> bool {
    match pattern.split_first() {
        None => path.is_empty(),
        Some((first, rest)) if first == "**" => {
            (0..=path.len()).any(|skip| glob_segments(rest, &path[skip..]))
        },
        Some((first, rest)) => path.split_first().is_some_and(|(head, tail)| {
            glob_component(first.as_bytes(), head.as_bytes()) && glob_segments(rest, tail)
        }),
    }
}

/// Match one path component against one glob component (`*`, `?`, literals).
#[cfg(feature = "deps")]
fn glob_component(pattern: &[u8], name: &[u8]) -> bool {
    match pattern.split_first() {
        None => name.is_empty(),
        Some((b'*', rest)) => (0..=name.len()).any(|skip| glob_component(rest, &name[skip..])),
        Some((b'[', _)) => false,
        Some((b'?', rest)) => name
            .split_first()
            .is_some_and(|(_, tail)| glob_component(rest, tail)),
        Some((c, rest)) => name
            .split_first()
            .is_some_and(|(head, tail)| head == c && glob_component(rest, tail)),
    }
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

    /// A workspace root at `root` with `workspace` as its `[workspace]` body,
    /// its lock, and a member manifest at `member` (relative); returns the
    /// member manifest's path.
    fn workspace(
        root: &std::path::Path,
        workspace: &str,
        member: &str,
        member_manifest: &str,
    ) -> Result<std::path::PathBuf, std::io::Error> {
        std::fs::write(root.join("Cargo.toml"), format!("[workspace]\n{workspace}"))?;
        std::fs::write(root.join("Cargo.lock"), LOCK)?;
        let dir = root.join(member);
        std::fs::create_dir_all(&dir)?;
        let manifest = dir.join("Cargo.toml");
        std::fs::write(&manifest, member_manifest)?;
        Ok(manifest)
    }

    const PACKAGE: &str = "[package]\nname = \"m\"\n";

    #[test]
    fn a_member_manifest_reads_the_workspace_lockfile() -> TestResult {
        let tmp = tempfile::tempdir()?;
        let manifest = workspace(
            tmp.path(),
            "members = [\"crates/member\"]\n",
            "crates/member",
            PACKAGE,
        )?;
        assert_eq!(manifest_lockfile(&manifest).as_deref(), Some(LOCK));
        Ok(())
    }

    #[test]
    fn a_member_matched_by_a_glob_reads_the_workspace_lockfile() -> TestResult {
        let tmp = tempfile::tempdir()?;
        let manifest = workspace(
            tmp.path(),
            "members = [\"./crates/kar*/\"]\n",
            "crates/karet-x",
            PACKAGE,
        )?;
        assert_eq!(manifest_lockfile(&manifest).as_deref(), Some(LOCK));
        let deep = tempfile::tempdir()?;
        let manifest = workspace(deep.path(), "members = [\"**/m?\"]\n", "a/b/m1", PACKAGE)?;
        assert_eq!(manifest_lockfile(&manifest).as_deref(), Some(LOCK));
        Ok(())
    }

    #[test]
    fn a_nested_non_member_reads_no_lockfile() -> TestResult {
        // The cargo-fuzz layout: `fuzz/` is its own package, not a member.
        let tmp = tempfile::tempdir()?;
        let manifest = workspace(tmp.path(), "members = [\"crates/*\"]\n", "fuzz", PACKAGE)?;
        assert_eq!(manifest_lockfile(&manifest), None);
        Ok(())
    }

    #[test]
    fn an_excluded_package_reads_no_lockfile() -> TestResult {
        let tmp = tempfile::tempdir()?;
        let manifest = workspace(
            tmp.path(),
            "members = [\"crates/*\"]\nexclude = [\"crates/vendored\"]\n",
            "crates/vendored",
            PACKAGE,
        )?;
        assert_eq!(manifest_lockfile(&manifest), None);
        Ok(())
    }

    #[test]
    fn a_lock_above_without_a_workspace_root_is_not_read() -> TestResult {
        let tmp = tempfile::tempdir()?;
        std::fs::write(tmp.path().join("Cargo.toml"), PACKAGE)?;
        std::fs::write(tmp.path().join("Cargo.lock"), LOCK)?;
        let nested = tmp.path().join("nested");
        std::fs::create_dir_all(&nested)?;
        std::fs::write(nested.join("Cargo.toml"), PACKAGE)?;
        assert_eq!(manifest_lockfile(&nested.join("Cargo.toml")), None);
        Ok(())
    }

    #[test]
    fn the_nearest_workspace_root_decides() -> TestResult {
        // An inner workspace without a lock shadows the outer one that lists it.
        let tmp = tempfile::tempdir()?;
        let manifest = workspace(
            tmp.path(),
            "members = [\"inner/member\"]\n",
            "inner/member",
            PACKAGE,
        )?;
        std::fs::write(
            tmp.path().join("inner").join("Cargo.toml"),
            "[workspace]\nmembers = [\"member\"]\n",
        )?;
        assert_eq!(manifest_lockfile(&manifest), None);
        Ok(())
    }

    #[test]
    fn a_package_workspace_key_names_the_root() -> TestResult {
        let tmp = tempfile::tempdir()?;
        let manifest = workspace(
            tmp.path(),
            "members = []\n",
            "elsewhere/m",
            "[package]\nname = \"m\"\nworkspace = \"../..\"\n",
        )?;
        assert_eq!(manifest_lockfile(&manifest).as_deref(), Some(LOCK));
        Ok(())
    }

    #[test]
    fn a_workspace_root_without_a_lock_reads_none() -> TestResult {
        let tmp = tempfile::tempdir()?;
        let manifest = workspace(
            tmp.path(),
            "members = [\"inner\"]\n",
            "inner",
            "[workspace]\nmembers = []\n",
        )?;
        assert_eq!(manifest_lockfile(&manifest), None);
        Ok(())
    }

    #[test]
    fn glob_components_match_cargo_member_patterns() {
        use super::glob_component;
        assert!(glob_component(b"karet-*", b"karet-core"));
        assert!(glob_component(b"*", b""));
        assert!(glob_component(b"a?c", b"abc"));
        assert!(!glob_component(b"a?c", b"ac"));
        assert!(!glob_component(b"karet-*", b"blameline"));
        assert!(!glob_component(b"[ab]", b"a"));
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
