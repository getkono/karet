//! Indexing a whole package: crate roots, then every module reachable from them.
//!
//! The tree the view shows is semantic, not filesystem — `src/model.rs` appears as
//! `karet-core::model`, and there are no file rows at all. Building that means starting
//! at each crate root and *following* module declarations to the files holding them,
//! rather than sweeping a directory and hoping the shape matches.
//!
//! Two failure modes are represented rather than hidden. A module whose file cannot be
//! found is still a node, recorded in [`SeamIndex::unresolved_modules`] with the paths
//! that were tried. And an index cut short by the file cap marks itself truncated, so a
//! partial tree can never be mistaken for a complete one.

use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;

use karet_treesitter::ParserPool;

use crate::extract::ExtractError;
use crate::extract::extract_file;
use crate::id::SeamId;
use crate::id::SeamPath;
use crate::id::SeamSegment;
use crate::index::SeamIndex;
use crate::model::ConfigMembership;
use crate::model::Node;
use crate::model::NodeKind;
use crate::model::SeamLocation;
use crate::modules::ModuleSource;
use crate::modules::resolve;
use crate::rollup::Rollups;

/// Errors indexing a package.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum PackageError {
    /// No `Cargo.toml` was found at the given root.
    #[error("no Cargo.toml at {0}")]
    NoManifest(PathBuf),
    /// The manifest exists but declares no package (a virtual workspace root).
    #[error("{0} declares a workspace, not a package")]
    VirtualManifest(PathBuf),
    /// No crate root could be found, so there is nothing to index.
    #[error("no crate root (src/lib.rs or src/main.rs) under {0}")]
    NoCrateRoot(PathBuf),
    /// Every candidate crate root failed to extract.
    #[error("the package could not be indexed: {0}")]
    Extract(#[from] ExtractError),
}

/// How much of a package to index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IndexOptions {
    /// Stop after this many files, marking the index truncated.
    pub max_files: usize,
}

impl Default for IndexOptions {
    fn default() -> Self {
        // Far above any real package, so the cap is a runaway guard rather than a policy.
        Self { max_files: 20_000 }
    }
}

/// One file waiting to be indexed, and the module node it belongs to.
struct Pending {
    file: PathBuf,
    parent: SeamId,
    crate_root: bool,
}

/// Index the package rooted at `root`, following module declarations across files.
///
/// # Errors
/// [`PackageError::NoManifest`] when there is no `Cargo.toml`,
/// [`PackageError::VirtualManifest`] when it declares only a workspace, and
/// [`PackageError::NoCrateRoot`] when no entry point can be found.
pub fn index_package(root: &Path, options: IndexOptions) -> Result<SeamIndex, PackageError> {
    let manifest_path = root.join("Cargo.toml");
    let manifest = std::fs::read_to_string(&manifest_path)
        .map_err(|_| PackageError::NoManifest(manifest_path.clone()))?;
    let name = dependable_core::parse_package_name(&manifest)
        .ok_or_else(|| PackageError::VirtualManifest(manifest_path.clone()))?;

    let mut index = SeamIndex::new();
    let package = add_package_node(&mut index, &name, root);

    let roots = crate_roots(root);
    if roots.is_empty() {
        return Err(PackageError::NoCrateRoot(root.to_path_buf()));
    }

    let mut pool = ParserPool::new();
    let mut queue: Vec<Pending> = roots
        .into_iter()
        .map(|file| Pending {
            file,
            parent: package,
            crate_root: true,
        })
        .collect();
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut scanned = 0usize;

    while let Some(pending) = queue.pop() {
        let canonical = pending
            .file
            .canonicalize()
            .unwrap_or_else(|_| pending.file.clone());
        // A `#[path]` cycle would otherwise walk forever.
        if !seen.insert(canonical) {
            continue;
        }
        if scanned >= options.max_files {
            index.mark_truncated(scanned);
            break;
        }
        scanned += 1;

        let Ok(text) = std::fs::read_to_string(&pending.file) else {
            // Unreadable or not UTF-8: skip the file, keep the module node.
            continue;
        };
        index_one_file(&mut index, &mut pool, &mut queue, &pending, &text);
    }

    index.recompute_rollups();
    Ok(index)
}

/// Extract one file and enqueue the modules it declares.
fn index_one_file(
    index: &mut SeamIndex,
    pool: &mut ParserPool,
    queue: &mut Vec<Pending>,
    pending: &Pending,
    text: &str,
) {
    let Some(language) = crate::lang::rust::language_id() else {
        return;
    };
    let file_id = index.intern_file(&pending.file);
    let Ok(outcome) = extract_file(index, pool, pending.parent, file_id, language, text) else {
        return;
    };

    for declaration in outcome.external_modules {
        match resolve(
            &pending.file,
            pending.crate_root,
            &declaration.inline_path,
            &declaration.name,
            declaration.path_attribute.as_deref(),
        ) {
            ModuleSource::File(file) => queue.push(Pending {
                file,
                parent: declaration.id,
                crate_root: false,
            }),
            ModuleSource::Missing { candidates } => {
                // The module stays in the tree. Dropping it would claim the package has
                // no such module, when the truth is that its text could not be found.
                index.mark_module_unresolved(declaration.id, candidates);
            },
            ModuleSource::Inline => {},
        }
    }
}

/// Add the package's root node, which every declaration ultimately hangs from.
fn add_package_node(index: &mut SeamIndex, name: &str, root: &Path) -> SeamId {
    let path = SeamPath::new(vec![SeamSegment::new(name)]);
    let id = index.intern(path);
    let file = index.intern_file(&root.join("Cargo.toml"));
    index.insert(Node {
        id,
        kind: NodeKind::Package,
        name: name.to_owned(),
        detail: None,
        location: SeamLocation {
            file,
            range: karet_core::Range::default(),
            span: karet_core::Span::default(),
            selection: karet_core::Range::default(),
        },
        parent: None,
        children: Vec::new(),
        facets: Vec::new(),
        visibility: None,
        rollups: Rollups::new(),
        membership: ConfigMembership::Active,
        provisional: false,
    });
    id
}

/// The entry points to start walking from.
///
/// Conventional locations only, for now — the manifest tier supplies the full declared
/// target set, including the ones a manifest relocates with an explicit `path`.
fn crate_roots(root: &Path) -> Vec<PathBuf> {
    ["src/lib.rs", "src/main.rs"]
        .into_iter()
        .map(|relative| root.join(relative))
        .filter(|candidate| candidate.is_file())
        .collect()
}

/// Re-index a single file in place, replacing the subtree it previously contributed.
///
/// This is the incremental path an edit takes. Only the edited file's nodes are removed
/// and rebuilt, and only the ancestor spine above them has its rollups recomputed —
/// re-walking the package on every keystroke would make the view unusable.
///
/// # Errors
/// Propagates [`ExtractError`] when the file cannot be parsed or has no mapping.
pub fn reindex_file(
    index: &mut SeamIndex,
    pool: &mut ParserPool,
    file: &Path,
    text: &str,
) -> Result<(), ExtractError> {
    let Some(language) = crate::lang::rust::language_id() else {
        return Err(ExtractError::NoGrammar);
    };
    let file_id = index.intern_file(file);
    let Some(parent) = index.owner_of_file(file_id) else {
        return Err(ExtractError::NoMapping);
    };
    index.remove_nodes_in_file(file_id, parent);
    extract_file(index, pool, parent, file_id, language, text)?;
    index.recompute_rollups_from(parent);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    /// Build a package on disk: a manifest plus `(relative path, contents)` files.
    fn package(name: &str, files: &[(&str, &str)]) -> Result<tempfile::TempDir, std::io::Error> {
        let dir = tempfile::tempdir()?;
        std::fs::write(
            dir.path().join("Cargo.toml"),
            format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\n"),
        )?;
        for (relative, contents) in files {
            let path = dir.path().join(relative);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(path, contents)?;
        }
        Ok(dir)
    }

    fn paths(index: &SeamIndex) -> Vec<String> {
        let mut out: Vec<String> = index
            .nodes()
            .filter_map(|node| index.path(node.id).map(ToString::to_string))
            .collect();
        out.sort();
        out
    }

    #[test]
    fn a_missing_manifest_is_an_error() -> TestResult {
        let dir = tempfile::tempdir()?;
        assert!(matches!(
            index_package(dir.path(), IndexOptions::default()),
            Err(PackageError::NoManifest(_))
        ));
        Ok(())
    }

    #[test]
    fn a_virtual_workspace_root_is_not_a_package() -> TestResult {
        let dir = tempfile::tempdir()?;
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/*\"]\n",
        )?;
        assert!(matches!(
            index_package(dir.path(), IndexOptions::default()),
            Err(PackageError::VirtualManifest(_))
        ));
        Ok(())
    }

    #[test]
    fn a_package_with_no_entry_point_reports_so() -> TestResult {
        let dir = package("empty", &[])?;
        assert!(matches!(
            index_package(dir.path(), IndexOptions::default()),
            Err(PackageError::NoCrateRoot(_))
        ));
        Ok(())
    }

    #[test]
    fn modules_appear_by_semantic_path_not_as_files() -> TestResult {
        if crate::lang::rust::language_id().is_none() {
            return Ok(());
        }
        let dir = package(
            "demo",
            &[
                ("src/lib.rs", "pub mod model;\npub mod net;"),
                ("src/model.rs", "pub struct Symbol;"),
                ("src/net/mod.rs", "pub fn connect() {}"),
            ],
        )?;
        let index = index_package(dir.path(), IndexOptions::default())?;
        let found = paths(&index);
        assert!(
            found.contains(&"demo::model::Symbol".to_owned()),
            "got {found:?}"
        );
        assert!(
            found.contains(&"demo::net::connect".to_owned()),
            "got {found:?}"
        );
        // No file ever becomes a row.
        assert!(!found.iter().any(|p| p.contains(".rs")), "got {found:?}");
        Ok(())
    }

    #[test]
    fn a_non_root_file_owns_a_subdirectory() -> TestResult {
        if crate::lang::rust::language_id().is_none() {
            return Ok(());
        }
        let dir = package(
            "demo",
            &[
                ("src/lib.rs", "pub mod client;"),
                ("src/client.rs", "pub mod net;"),
                ("src/client/net.rs", "pub fn connect() {}"),
            ],
        )?;
        let index = index_package(dir.path(), IndexOptions::default())?;
        assert!(paths(&index).contains(&"demo::client::net::connect".to_owned()));
        Ok(())
    }

    #[test]
    fn an_inline_module_nests_where_its_children_are_sought() -> TestResult {
        if crate::lang::rust::language_id().is_none() {
            return Ok(());
        }
        let dir = package(
            "demo",
            &[
                ("src/lib.rs", "pub mod outer { pub mod inner; }"),
                ("src/outer/inner.rs", "pub fn deep() {}"),
            ],
        )?;
        let index = index_package(dir.path(), IndexOptions::default())?;
        assert!(
            paths(&index).contains(&"demo::outer::inner::deep".to_owned()),
            "got {:?}",
            paths(&index)
        );
        Ok(())
    }

    #[test]
    fn a_path_attribute_relocates_a_module() -> TestResult {
        if crate::lang::rust::language_id().is_none() {
            return Ok(());
        }
        let dir = package(
            "demo",
            &[
                (
                    "src/lib.rs",
                    "#[path = \"vendored/thing.rs\"]\npub mod thing;",
                ),
                ("src/vendored/thing.rs", "pub fn f() {}"),
            ],
        )?;
        let index = index_package(dir.path(), IndexOptions::default())?;
        assert!(paths(&index).contains(&"demo::thing::f".to_owned()));
        Ok(())
    }

    #[test]
    fn an_unresolvable_module_stays_in_the_tree_with_its_evidence() -> TestResult {
        if crate::lang::rust::language_id().is_none() {
            return Ok(());
        }
        let dir = package(
            "demo",
            &[("src/lib.rs", "pub mod absent;\npub fn here() {}")],
        )?;
        let index = index_package(dir.path(), IndexOptions::default())?;

        // Dropping it would claim the package has no such module, which is not what
        // happened — its text simply could not be found.
        assert!(paths(&index).contains(&"demo::absent".to_owned()));
        let unresolved = index.unresolved_modules();
        assert_eq!(unresolved.len(), 1);
        assert!(!unresolved[0].1.is_empty(), "the attempted paths are kept");
        Ok(())
    }

    #[test]
    fn the_file_cap_marks_the_index_truncated_rather_than_lying() -> TestResult {
        if crate::lang::rust::language_id().is_none() {
            return Ok(());
        }
        let dir = package(
            "demo",
            &[
                ("src/lib.rs", "pub mod a;\npub mod b;"),
                ("src/a.rs", "pub fn a() {}"),
                ("src/b.rs", "pub fn b() {}"),
            ],
        )?;
        let index = index_package(dir.path(), IndexOptions { max_files: 1 })?;
        assert_eq!(index.truncated_after(), Some(1));
        Ok(())
    }

    #[test]
    fn rollups_reach_the_package_root_across_files() -> TestResult {
        if crate::lang::rust::language_id().is_none() {
            return Ok(());
        }
        let dir = package(
            "demo",
            &[
                ("src/lib.rs", "pub mod deep;"),
                ("src/deep.rs", "pub unsafe fn danger() {}"),
            ],
        )?;
        let index = index_package(dir.path(), IndexOptions::default())?;
        let root = index
            .roots()
            .first()
            .and_then(|id| index.node(*id))
            .ok_or("package root")?;
        assert!(root.rollups.get(crate::model::Lens::Hazard) >= 1);
        Ok(())
    }

    #[test]
    fn reindexing_one_file_replaces_only_its_own_nodes() -> TestResult {
        if crate::lang::rust::language_id().is_none() {
            return Ok(());
        }
        let dir = package(
            "demo",
            &[
                ("src/lib.rs", "pub mod a;\npub mod b;"),
                ("src/a.rs", "pub fn original() {}"),
                ("src/b.rs", "pub fn untouched() {}"),
            ],
        )?;
        let mut index = index_package(dir.path(), IndexOptions::default())?;
        assert!(paths(&index).contains(&"demo::a::original".to_owned()));

        let mut pool = ParserPool::new();
        reindex_file(
            &mut index,
            &mut pool,
            &dir.path().join("src/a.rs"),
            "pub fn renamed() {}\npub unsafe fn added() {}",
        )?;

        let found = paths(&index);
        assert!(
            found.contains(&"demo::a::renamed".to_owned()),
            "got {found:?}"
        );
        assert!(
            !found.contains(&"demo::a::original".to_owned()),
            "stale node survived"
        );
        // The other file is untouched by its neighbour's edit.
        assert!(
            found.contains(&"demo::b::untouched".to_owned()),
            "got {found:?}"
        );

        // And the ancestor spine sees the new facet without a full re-walk.
        let root = index
            .roots()
            .first()
            .and_then(|id| index.node(*id))
            .ok_or("package root")?;
        assert!(root.rollups.get(crate::model::Lens::Hazard) >= 1);
        Ok(())
    }
}
