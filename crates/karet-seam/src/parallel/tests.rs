//! The parallel walk: that it is reproducible, and that replaying equals reading.
//!
//! These are the tests that make the cache trustworthy. A cache is only sound if a
//! replayed build is indistinguishable from a cold one, and a parallel build is only
//! usable if it does not renumber the tree differently on every run.

use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Mutex;

use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// A workspace of `members`, each a small crate with modules in further files.
fn workspace(members: &[&str]) -> Result<tempfile::TempDir, std::io::Error> {
    let dir = tempfile::tempdir()?;
    let list = members
        .iter()
        .map(|name| format!("\"crates/{name}\""))
        .collect::<Vec<_>>()
        .join(", ");
    std::fs::write(
        dir.path().join("Cargo.toml"),
        format!("[workspace]\nmembers = [{list}]\n"),
    )?;
    for name in members {
        let root = dir.path().join("crates").join(name);
        std::fs::create_dir_all(root.join("src").join("deep"))?;
        std::fs::write(
            root.join("Cargo.toml"),
            format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\n"),
        )?;
        std::fs::write(
            root.join("src").join("lib.rs"),
            "pub mod inner;\npub struct Widget;\npub unsafe fn danger() {}\n",
        )?;
        std::fs::write(
            root.join("src").join("inner.rs"),
            "pub mod deep;\npub trait Shape { fn area(&self) -> u32; }\npub fn helper() {}\n",
        )?;
        std::fs::write(
            root.join("src").join("deep").join("mod.rs"),
            "pub const LIMIT: u32 = 4;\n#[cfg(unix)]\npub fn only_unix() {}\n",
        )?;
    }
    Ok(dir)
}

/// Every node as `id -> path`, which is what "the same numbering" means.
fn numbering(index: &SeamIndex) -> Vec<(u32, String)> {
    let mut out: Vec<(u32, String)> = index
        .nodes()
        .filter_map(|node| Some((node.id.0, index.path(node.id)?.to_string())))
        .collect();
    out.sort();
    out
}

/// Every node's path with its facets and rollups, so a comparison covers content.
fn shape(index: &SeamIndex) -> Vec<String> {
    let mut out: Vec<String> = index
        .nodes()
        .filter_map(|node| {
            let path = index.path(node.id)?.to_string();
            let mut facets: Vec<String> = node
                .facets
                .iter()
                .map(|facet| format!("{}:{}", facet.lens.name(), facet.subtype.name()))
                .collect();
            facets.sort();
            Some(format!(
                "{path} [{}] {:?} {:?}",
                facets.join(","),
                node.kind,
                node.rollups
            ))
        })
        .collect();
    out.sort();
    out
}

/// An observer that stores what it is told and serves it back.
#[derive(Default)]
struct Cache {
    stored: Mutex<HashMap<PathBuf, FileContribution>>,
    hits: Mutex<usize>,
}

impl Cache {
    fn hits(&self) -> usize {
        *self.hits.lock().unwrap_or_else(|held| held.into_inner())
    }
}

impl IndexObserver for Cache {
    fn cached(&self, file: &Path, stamp: FileStamp) -> Option<FileContribution> {
        let stored = self.stored.lock().ok()?;
        let held = stored.get(file)?;
        if !held.matches(stamp) {
            return None;
        }
        if let Ok(mut hits) = self.hits.lock() {
            *hits += 1;
        }
        Some(held.clone())
    }

    fn package_indexed(&self, indexed: &mut IndexedPackage) {
        if let Ok(mut stored) = self.stored.lock() {
            for contribution in &indexed.contributions {
                stored.insert(contribution.file.clone(), contribution.clone());
            }
        }
    }
}

#[test]
fn the_same_workspace_numbers_the_same_way_every_run() -> TestResult {
    if crate::lang::rust::language_id().is_none() {
        return Ok(());
    }
    // Ids are assigned in first-seen order and the walk finishes in whatever order the
    // scheduler produced, so replay order — not completion order — has to fix this.
    let dir = workspace(&["alpha", "beta", "gamma"])?;
    let first = index_workspace_with(dir.path(), IndexOptions::default(), &Unobserved)?;
    for _ in 0..8 {
        let again = index_workspace_with(dir.path(), IndexOptions::default(), &Unobserved)?;
        assert_eq!(numbering(&first), numbering(&again));
    }
    Ok(())
}

#[test]
fn packages_are_rooted_in_discovery_order_not_completion_order() -> TestResult {
    if crate::lang::rust::language_id().is_none() {
        return Ok(());
    }
    let dir = workspace(&["alpha", "beta", "gamma"])?;
    for _ in 0..8 {
        let index = index_workspace_with(dir.path(), IndexOptions::default(), &Unobserved)?;
        let roots: Vec<String> = index
            .roots()
            .iter()
            .filter_map(|id| index.node(*id).map(|node| node.name.clone()))
            .collect();
        assert_eq!(roots, ["alpha", "beta", "gamma"]);
    }
    Ok(())
}

#[test]
fn a_replayed_build_is_indistinguishable_from_a_cold_one() -> TestResult {
    if crate::lang::rust::language_id().is_none() {
        return Ok(());
    }
    // The whole cache rests on this: if the two ever diverge, a warm start is a lie.
    let dir = workspace(&["alpha", "beta"])?;
    let cache = Cache::default();

    let cold = index_workspace_with(dir.path(), IndexOptions::default(), &cache)?;
    assert_eq!(cache.hits(), 0, "nothing was stored before the first run");

    let warm = index_workspace_with(dir.path(), IndexOptions::default(), &cache)?;
    assert!(
        cache.hits() > 0,
        "the second run read nothing from the cache"
    );

    assert_eq!(numbering(&cold), numbering(&warm), "ids diverged");
    assert_eq!(shape(&cold), shape(&warm), "content diverged");
    assert_eq!(cold.files().len(), warm.files().len());
    Ok(())
}

#[test]
fn a_changed_file_is_reread_rather_than_replayed() -> TestResult {
    if crate::lang::rust::language_id().is_none() {
        return Ok(());
    }
    let dir = workspace(&["alpha"])?;
    let cache = Cache::default();
    let before = index_workspace_with(dir.path(), IndexOptions::default(), &cache)?;
    assert!(!shape(&before).iter().any(|row| row.contains("::added")));

    let inner = dir
        .path()
        .join("crates")
        .join("alpha")
        .join("src")
        .join("inner.rs");
    std::fs::write(
        &inner,
        "pub mod deep;\npub trait Shape { fn area(&self) -> u32; }\npub fn helper() {}\npub fn added() {}\n",
    )?;

    let after = index_workspace_with(dir.path(), IndexOptions::default(), &cache)?;
    assert!(
        shape(&after).iter().any(|row| row.contains("::added")),
        "the rewritten file was replayed from a stale entry"
    );
    Ok(())
}

#[test]
fn a_module_cycle_terminates() -> TestResult {
    if crate::lang::rust::language_id().is_none() {
        return Ok(());
    }
    // `#[path]` lets a file name itself. Without the shared seen-set, and without it
    // being consulted before the task is spawned, the frontier would grow forever.
    let dir = tempfile::tempdir()?;
    let root = dir.path();
    std::fs::create_dir_all(root.join("src"))?;
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"knot\"\nversion = \"0.1.0\"\n",
    )?;
    std::fs::write(
        root.join("src").join("lib.rs"),
        "#[path = \"lib.rs\"]\npub mod again;\npub struct Once;\n",
    )?;
    let index = index_workspace_with(root, IndexOptions::default(), &Unobserved)?;
    assert_eq!(index.roots().len(), 1);
    Ok(())
}

#[test]
fn a_colliding_root_name_is_disambiguated_before_the_walk_starts() {
    // Assigned serially and up front, because a worker cannot know what its neighbours
    // have already claimed.
    let discovered = |name: &str| Discovered {
        name: name.to_owned(),
        root: PathBuf::from(name),
        anchor: PathBuf::from(name),
        kind: crate::discover::PackageKind::Cargo,
    };
    let packages = [
        discovered("shared"),
        discovered("shared"),
        discovered("other"),
    ];
    assert_eq!(
        root_segments(&packages)
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        ["shared", "shared#2", "other"]
    );
}

#[test]
fn merging_packages_equals_indexing_them_together() -> TestResult {
    if crate::lang::rust::language_id().is_none() {
        return Ok(());
    }
    // Packages are built apart and joined afterwards; the join must not be visible.
    let dir = workspace(&["alpha", "beta"])?;
    let together = index_workspace_with(dir.path(), IndexOptions::default(), &Unobserved)?;

    let alpha = index_workspace_with(
        &dir.path().join("crates").join("alpha"),
        IndexOptions::default(),
        &Unobserved,
    )?;
    let beta = index_workspace_with(
        &dir.path().join("crates").join("beta"),
        IndexOptions::default(),
        &Unobserved,
    )?;
    let mut joined = SeamIndex::new();
    joined.merge(alpha);
    joined.merge(beta);

    assert_eq!(shape(&together), shape(&joined));
    assert_eq!(numbering(&together), numbering(&joined));
    Ok(())
}
