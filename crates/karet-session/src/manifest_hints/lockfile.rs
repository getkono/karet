//! Locate the `Cargo.lock` cargo would use for a manifest, following cargo's
//! workspace rules: which ancestor is the workspace root, and whether that
//! workspace actually counts the package as a member. Runs on the hints
//! worker, off the session thread, since it reads manifests from disk.

use std::collections::HashSet;
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;

/// How many directories and manifests one lookup may visit (glob expansion
/// plus the path-dependency walk) before it gives up with no lockfile, so a
/// pathological tree cannot stall the worker.
const VISIT_BUDGET: usize = 4096;

/// The text of the `Cargo.lock` governing `manifest`. `None` when there is
/// none, it cannot be read, or the package belongs to no workspace that holds
/// one; the hints then fall back to the manifest alone.
pub(crate) fn read(manifest: &Path) -> Option<String> {
    std::fs::read_to_string(governing(manifest)?).ok()
}

/// Locate the `Cargo.lock` cargo would use for `manifest`, without reading
/// the lock. A lock beside the manifest wins. Otherwise the package must be a
/// member of a workspace whose root holds the lock: the root its
/// `package.workspace` key names, or else the nearest ancestor `Cargo.toml`
/// declaring `[workspace]` that does not exclude it. That workspace must count
/// the package as a member (see [`Workspace::contains`]). A package outside it
/// (a `cargo fuzz` crate, an excluded or vendored package) yields `None`, as
/// does an unreadable or unparsable manifest on the way — an unrelated
/// lockfile's versions would mislabel the hints.
fn governing(manifest: &Path) -> Option<PathBuf> {
    let dir = normalize(manifest.parent()?);
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
        let Some(workspace) = Workspace::new(ancestor, &table) else {
            continue;
        };
        let relative = segments_of(dir.strip_prefix(ancestor).ok()?)?;
        if workspace.excludes(&relative) {
            // Cargo passes over a root that excludes the package and keeps
            // looking further up.
            continue;
        }
        // Otherwise this root is the package's workspace, and cargo rejects
        // the package unless the workspace counts it as a member.
        if !workspace.contains(&dir) {
            return None;
        }
        let lock = ancestor.join("Cargo.lock");
        return lock.is_file().then_some(lock);
    }
    None
}

/// A `[workspace]` root: its directory, its manifest, and its path lists.
struct Workspace<'a> {
    /// The directory holding the root `Cargo.toml`.
    root: &'a Path,
    /// The root manifest (it may also declare a `[package]`).
    table: &'a toml::Table,
    /// The `members` entries, split into path components.
    members: Vec<Vec<String>>,
    /// The `exclude` entries, split into path components.
    exclude: Vec<Vec<String>>,
}

impl<'a> Workspace<'a> {
    /// The workspace `table` declares at `root`, or `None` without a
    /// `[workspace]` table.
    fn new(root: &'a Path, table: &'a toml::Table) -> Option<Self> {
        let workspace = table.get("workspace")?;
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
        Some(Self {
            root,
            table,
            members: entries("members"),
            exclude: entries("exclude"),
        })
    }

    /// Whether the package at `relative` (its components below the root) is
    /// excluded: some `exclude` path contains it and no `members` entry, read
    /// as a literal path, also does — cargo lets an explicit member override
    /// a broader exclude.
    fn excludes(&self, relative: &[&str]) -> bool {
        let covers = |prefix: &Vec<String>| {
            prefix.len() <= relative.len() && prefix.iter().zip(relative).all(|(p, c)| p == c)
        };
        self.exclude.iter().any(covers) && !self.members.iter().any(covers)
    }

    /// Whether the package in `dir` is a member, as cargo counts them: a
    /// `members` glob matches it, or it is reached through `path`
    /// dependencies (of any kind, transitively) from the root package or a
    /// member, sitting inside the root directory and not excluded. The second
    /// rule is what makes a path dependency of a root `[package]` with an
    /// empty `[workspace]` a member.
    fn contains(&self, dir: &Path) -> bool {
        let Some(relative) = dir.strip_prefix(self.root).ok().and_then(segments_of) else {
            return false;
        };
        if self.members.iter().any(|m| glob_segments(m, &relative)) {
            return true;
        }
        let mut budget = VISIT_BUDGET;
        let mut queue = Vec::new();
        if self.table.contains_key("package") {
            queue.push(self.root.to_path_buf());
        }
        for pattern in &self.members {
            expand(self.root.to_path_buf(), pattern, &mut queue, &mut budget);
        }
        let mut seen = HashSet::new();
        while let Some(member) = queue.pop() {
            let member = normalize(&member);
            if !seen.insert(member.clone()) {
                continue;
            }
            if budget == 0 {
                return false;
            }
            budget -= 1;
            let parsed;
            let table = if member == self.root {
                self.table
            } else {
                let excluded = member
                    .strip_prefix(self.root)
                    .ok()
                    .and_then(segments_of)
                    .is_some_and(|relative| self.excludes(&relative));
                if excluded {
                    // An excluded `members` match is not a member either.
                    continue;
                }
                let Some(table) = read_toml(&member.join("Cargo.toml")) else {
                    continue;
                };
                parsed = table;
                &parsed
            };
            for dep in self.path_dependencies(&member, table) {
                let Some(dep_relative) = dep.strip_prefix(self.root).ok().and_then(segments_of)
                else {
                    // Outside the root: cargo does not make it a member.
                    continue;
                };
                if self.excludes(&dep_relative) {
                    continue;
                }
                if dep == dir {
                    return true;
                }
                if dep.join("Cargo.toml").is_file() {
                    queue.push(dep);
                }
            }
        }
        false
    }

    /// The directories of the `path` dependencies `table` (the manifest in
    /// `dir`) declares, in every dependency section including per-target
    /// ones; `{ workspace = true }` resolves through the root's
    /// `[workspace.dependencies]`.
    fn path_dependencies(&self, dir: &Path, table: &toml::Table) -> Vec<PathBuf> {
        const SECTIONS: [&str; 5] = [
            "dependencies",
            "dev-dependencies",
            "dev_dependencies",
            "build-dependencies",
            "build_dependencies",
        ];
        let inherited = self
            .table
            .get("workspace")
            .and_then(|w| w.get("dependencies"));
        let targets = table
            .get("target")
            .and_then(toml::Value::as_table)
            .into_iter()
            .flat_map(|targets| targets.values().filter_map(toml::Value::as_table));
        let mut dirs = Vec::new();
        for scope in std::iter::once(table).chain(targets) {
            let deps = SECTIONS
                .iter()
                .filter_map(|section| scope.get(*section)?.as_table())
                .flatten();
            for (name, spec) in deps {
                if let Some(path) = spec.get("path").and_then(toml::Value::as_str) {
                    dirs.push(normalize(&dir.join(path)));
                } else if spec.get("workspace").and_then(toml::Value::as_bool) == Some(true)
                    && let Some(path) = inherited
                        .and_then(|deps| deps.get(name))
                        .and_then(|spec| spec.get("path"))
                        .and_then(toml::Value::as_str)
                {
                    dirs.push(normalize(&self.root.join(path)));
                }
            }
        }
        dirs
    }
}

/// Push onto `out` every directory below `dir` that `pattern` (glob
/// components) matches and that holds a `Cargo.toml`, as cargo expands a
/// `members` entry. Symlinked directories are not followed, and each
/// directory visited spends one unit of `budget`.
fn expand(dir: PathBuf, pattern: &[String], out: &mut Vec<PathBuf>, budget: &mut usize) {
    if *budget == 0 {
        return;
    }
    *budget -= 1;
    match pattern.split_first() {
        None => {
            if dir.join("Cargo.toml").is_file() {
                out.push(dir);
            }
        },
        Some((first, rest)) if first == "**" => {
            for sub in subdirectories(&dir) {
                expand(sub, pattern, out, budget);
            }
            expand(dir, rest, out, budget);
        },
        Some((first, rest)) if !first.contains(['*', '?', '[']) => {
            expand(dir.join(first), rest, out, budget);
        },
        Some((first, rest)) => {
            for sub in subdirectories(&dir) {
                let matches = sub
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| glob_component(first.as_bytes(), name.as_bytes()));
                if matches {
                    expand(sub, rest, out, budget);
                }
            }
        },
    }
}

/// The subdirectories of `dir` (not following symlinks); none when unreadable.
fn subdirectories(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
                .map(|entry| entry.path())
                .collect()
        })
        .unwrap_or_default()
}

/// Parse a `Cargo.toml` from disk; `None` when unreadable or invalid.
fn read_toml(path: &Path) -> Option<toml::Table> {
    std::fs::read_to_string(path).ok()?.parse().ok()
}

/// Resolve `.` and `..` components lexically (no filesystem access), so paths
/// built from manifests compare equal to the directory they name.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {},
            Component::ParentDir => {
                if !out.pop() {
                    out.push(component);
                }
            },
            other => out.push(other),
        }
    }
    out
}

/// The components of a relative path as strings; `None` when one is not
/// plain UTF-8 (such a path never matches a manifest entry).
fn segments_of(relative: &Path) -> Option<Vec<&str>> {
    relative
        .components()
        .map(|c| match c {
            Component::Normal(part) => part.to_str(),
            _ => None,
        })
        .collect()
}

/// Split a workspace path entry into its components, dropping `.` and empty
/// segments (`./crates/*/` → `["crates", "*"]`).
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

#[cfg(test)]
mod tests {
    use super::read;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    const LOCK: &str = "version = 3\n";

    const PACKAGE: &str = "[package]\nname = \"m\"\n";

    /// Write `contents` to `root/relative/Cargo.toml`, creating directories;
    /// returns the manifest's path.
    fn manifest(
        root: &std::path::Path,
        relative: &str,
        contents: &str,
    ) -> Result<std::path::PathBuf, std::io::Error> {
        let dir = root.join(relative);
        std::fs::create_dir_all(&dir)?;
        let path = dir.join("Cargo.toml");
        std::fs::write(&path, contents)?;
        Ok(path)
    }

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
        manifest(root, member, member_manifest)
    }

    #[test]
    fn a_member_manifest_reads_the_workspace_lockfile() -> TestResult {
        let tmp = tempfile::tempdir()?;
        let path = workspace(
            tmp.path(),
            "members = [\"crates/member\"]\n",
            "crates/member",
            PACKAGE,
        )?;
        assert_eq!(read(&path).as_deref(), Some(LOCK));
        Ok(())
    }

    #[test]
    fn a_member_matched_by_a_glob_reads_the_workspace_lockfile() -> TestResult {
        let tmp = tempfile::tempdir()?;
        let path = workspace(
            tmp.path(),
            "members = [\"./crates/kar*/\"]\n",
            "crates/karet-x",
            PACKAGE,
        )?;
        assert_eq!(read(&path).as_deref(), Some(LOCK));
        let deep = tempfile::tempdir()?;
        let path = workspace(deep.path(), "members = [\"**/m?\"]\n", "a/b/m1", PACKAGE)?;
        assert_eq!(read(&path).as_deref(), Some(LOCK));
        Ok(())
    }

    #[test]
    fn a_nested_non_member_reads_no_lockfile() -> TestResult {
        // The cargo-fuzz layout: `fuzz/` is its own package, not a member.
        let tmp = tempfile::tempdir()?;
        manifest(tmp.path(), "crates/a", PACKAGE)?;
        let path = workspace(tmp.path(), "members = [\"crates/*\"]\n", "fuzz", PACKAGE)?;
        assert_eq!(read(&path), None);
        Ok(())
    }

    #[test]
    fn an_excluded_package_reads_no_lockfile() -> TestResult {
        let tmp = tempfile::tempdir()?;
        let path = workspace(
            tmp.path(),
            "members = [\"crates/*\"]\nexclude = [\"crates/vendored\"]\n",
            "crates/vendored",
            PACKAGE,
        )?;
        assert_eq!(read(&path), None);
        Ok(())
    }

    #[test]
    fn an_explicit_member_overrides_a_broader_exclude() -> TestResult {
        let tmp = tempfile::tempdir()?;
        let path = workspace(
            tmp.path(),
            "members = [\"crates/vendored\"]\nexclude = [\"crates\"]\n",
            "crates/vendored",
            PACKAGE,
        )?;
        assert_eq!(read(&path).as_deref(), Some(LOCK));
        Ok(())
    }

    #[test]
    fn an_excluding_root_defers_to_the_workspace_above() -> TestResult {
        // The inner root excludes `m` and has no lock; the outer one claims it.
        let tmp = tempfile::tempdir()?;
        let path = workspace(tmp.path(), "members = [\"inner/m\"]\n", "inner/m", PACKAGE)?;
        manifest(
            tmp.path(),
            "inner",
            "[workspace]\nmembers = []\nexclude = [\"m\"]\n",
        )?;
        assert_eq!(read(&path).as_deref(), Some(LOCK));
        Ok(())
    }

    #[test]
    fn a_path_dependency_of_the_root_package_is_a_member() -> TestResult {
        // A root `[package]` with an empty `[workspace]`: cargo makes its
        // path dependencies members without any `members` entry.
        let tmp = tempfile::tempdir()?;
        std::fs::write(
            tmp.path().join("Cargo.toml"),
            "[package]\nname = \"app\"\n[workspace]\n[dependencies]\nm = { path = \"m\" }\n",
        )?;
        std::fs::write(tmp.path().join("Cargo.lock"), LOCK)?;
        let path = manifest(tmp.path(), "m", PACKAGE)?;
        assert_eq!(read(&path).as_deref(), Some(LOCK));
        Ok(())
    }

    #[test]
    fn path_dependencies_of_members_are_members_transitively() -> TestResult {
        let tmp = tempfile::tempdir()?;
        let path = workspace(
            tmp.path(),
            "members = [\"crates/*\"]\n[workspace.dependencies]\nleaf = { path = \"tools/leaf\" }\n",
            "tools/mid",
            "[package]\nname = \"mid\"\n[target.'cfg(unix)'.dev-dependencies]\nleaf = { workspace = true }\n",
        )?;
        manifest(
            tmp.path(),
            "crates/a",
            "[package]\nname = \"a\"\n[build-dependencies]\nmid = { path = \"../../tools/mid\" }\n",
        )?;
        let leaf = manifest(tmp.path(), "tools/leaf", PACKAGE)?;
        assert_eq!(read(&path).as_deref(), Some(LOCK));
        assert_eq!(read(&leaf).as_deref(), Some(LOCK));
        Ok(())
    }

    #[test]
    fn an_excluded_path_dependency_is_not_a_member() -> TestResult {
        let tmp = tempfile::tempdir()?;
        std::fs::write(
            tmp.path().join("Cargo.toml"),
            "[package]\nname = \"app\"\n[workspace]\nexclude = [\"m\"]\n[dependencies]\nm = { path = \"m\" }\n",
        )?;
        std::fs::write(tmp.path().join("Cargo.lock"), LOCK)?;
        let path = manifest(tmp.path(), "m", PACKAGE)?;
        assert_eq!(read(&path), None);
        Ok(())
    }

    #[test]
    fn a_lock_above_without_a_workspace_root_is_not_read() -> TestResult {
        let tmp = tempfile::tempdir()?;
        std::fs::write(tmp.path().join("Cargo.toml"), PACKAGE)?;
        std::fs::write(tmp.path().join("Cargo.lock"), LOCK)?;
        let path = manifest(tmp.path(), "nested", PACKAGE)?;
        assert_eq!(read(&path), None);
        Ok(())
    }

    #[test]
    fn the_nearest_workspace_root_decides() -> TestResult {
        // An inner workspace without a lock shadows the outer one that lists it.
        let tmp = tempfile::tempdir()?;
        let path = workspace(
            tmp.path(),
            "members = [\"inner/member\"]\n",
            "inner/member",
            PACKAGE,
        )?;
        manifest(tmp.path(), "inner", "[workspace]\nmembers = [\"member\"]\n")?;
        assert_eq!(read(&path), None);
        Ok(())
    }

    #[test]
    fn a_package_workspace_key_names_the_root() -> TestResult {
        let tmp = tempfile::tempdir()?;
        let path = workspace(
            tmp.path(),
            "members = []\n",
            "elsewhere/m",
            "[package]\nname = \"m\"\nworkspace = \"../..\"\n",
        )?;
        assert_eq!(read(&path).as_deref(), Some(LOCK));
        Ok(())
    }

    #[test]
    fn a_workspace_root_without_a_lock_reads_none() -> TestResult {
        let tmp = tempfile::tempdir()?;
        let path = workspace(
            tmp.path(),
            "members = [\"inner\"]\n",
            "inner",
            "[workspace]\nmembers = []\n",
        )?;
        assert_eq!(read(&path), None);
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
        assert_eq!(read(&tmp.path().join("Cargo.toml")).as_deref(), Some(LOCK));
        Ok(())
    }

    #[test]
    fn a_manifest_without_any_lockfile_reads_none() -> TestResult {
        let tmp = tempfile::tempdir()?;
        assert_eq!(read(&tmp.path().join("Cargo.toml")), None);
        Ok(())
    }
}
