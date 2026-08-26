//! Package and workspace indexing, from synthetic trees on disk.
//!
//! Split out only because the pair of entry points and their fixtures pushed the
//! module past the repository's code-line ceiling.

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

/// Build a workspace on disk: a virtual root manifest plus `(member, files)` groups.
fn workspace(members: &[&str]) -> Result<tempfile::TempDir, std::io::Error> {
    let dir = tempfile::tempdir()?;
    std::fs::write(
        dir.path().join("Cargo.toml"),
        "[workspace]\nmembers = [\"crates/*\"]\n",
    )?;
    for name in members {
        let member = dir.path().join("crates").join(name);
        std::fs::create_dir_all(member.join("src"))?;
        std::fs::write(
            member.join("Cargo.toml"),
            format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\n"),
        )?;
        std::fs::write(member.join("src").join("lib.rs"), "pub fn thing() {}\n")?;
    }
    Ok(dir)
}

fn root_names(index: &SeamIndex) -> Vec<String> {
    index
        .roots()
        .iter()
        .filter_map(|id| index.node(*id).map(|node| node.name.clone()))
        .collect()
}

#[test]
fn a_virtual_workspace_root_is_readable_as_a_workspace() -> TestResult {
    // The pair that states the change: the same directory is still not *a package*,
    // and is now emphatically something to read.
    let dir = tempfile::tempdir()?;
    std::fs::write(
        dir.path().join("Cargo.toml"),
        "[workspace]\nmembers = [\"crates/*\"]\n",
    )?;
    std::fs::create_dir_all(dir.path().join("crates").join("only").join("src"))?;
    std::fs::write(
        dir.path().join("crates").join("only").join("Cargo.toml"),
        "[package]\nname = \"only\"\nversion = \"0.1.0\"\n",
    )?;
    std::fs::write(
        dir.path()
            .join("crates")
            .join("only")
            .join("src")
            .join("lib.rs"),
        "pub fn thing() {}\n",
    )?;

    assert!(matches!(
        index_package(dir.path(), IndexOptions::default()),
        Err(PackageError::VirtualManifest(_))
    ));
    let index = index_workspace(dir.path(), IndexOptions::default())?;
    assert_eq!(root_names(&index), ["only"]);
    Ok(())
}

#[test]
fn a_workspace_becomes_one_index_with_a_root_per_member() -> TestResult {
    let dir = workspace(&["alpha", "beta", "gamma"])?;
    let index = index_workspace(dir.path(), IndexOptions::default())?;
    assert_eq!(root_names(&index), ["alpha", "beta", "gamma"]);
    Ok(())
}

#[test]
fn the_root_order_is_stable_across_runs() -> TestResult {
    // The root order is what the view lists in its first column, so it cannot depend
    // on whatever order the filesystem happened to hand back.
    let dir = workspace(&["zeta", "alpha", "mid"])?;
    let first = root_names(&index_workspace(dir.path(), IndexOptions::default())?);
    let again = root_names(&index_workspace(dir.path(), IndexOptions::default())?);
    assert_eq!(first, again);
    assert_eq!(first, ["alpha", "mid", "zeta"]);
    Ok(())
}

#[test]
fn nothing_indexable_is_an_error_rather_than_an_empty_tree() -> TestResult {
    let dir = tempfile::tempdir()?;
    std::fs::write(dir.path().join("README.md"), "# nothing")?;
    assert!(matches!(
        index_workspace(dir.path(), IndexOptions::default()),
        Err(PackageError::NothingToIndex(_))
    ));
    Ok(())
}

#[test]
fn a_member_with_no_entry_point_keeps_its_root_and_says_what_was_tried() -> TestResult {
    // Dropping it would say the workspace has no such member, which is not what
    // happened — we simply do not read its layout yet.
    let dir = workspace(&["good"])?;
    let bare = dir.path().join("crates").join("bare");
    std::fs::create_dir_all(&bare)?;
    std::fs::write(
        bare.join("Cargo.toml"),
        "[package]\nname = \"bare\"\nversion = \"0.1.0\"\n",
    )?;
    let index = index_workspace(dir.path(), IndexOptions::default())?;
    assert!(root_names(&index).contains(&"bare".to_owned()));
    let tried = index
        .unresolved_modules()
        .iter()
        .find(|(id, _)| index.node(*id).is_some_and(|node| node.name == "bare"));
    assert!(
        tried.is_some_and(|(_, paths)| !paths.is_empty()),
        "no evidence kept"
    );
    Ok(())
}

#[test]
fn the_file_budget_is_shared_across_packages() -> TestResult {
    if crate::lang::rust::language_id().is_none() {
        return Ok(());
    }
    // One budget for the index, not an allowance per member — otherwise a workspace
    // of thirty crates would quietly index thirty times what the cap allows.
    let dir = workspace(&["alpha", "beta", "gamma"])?;
    let index = index_workspace(dir.path(), IndexOptions { max_files: 2 })?;
    assert_eq!(index.truncated_after(), Some(2));
    let scanned = index
        .files()
        .iter()
        .filter(|path| path.extension().is_some_and(|ext| ext == "rs"))
        .count();
    assert_eq!(scanned, 2, "the budget is per index, not per package");
    // The third package never got a root at all: absent, which `truncated_after`
    // explains, rather than present and falsely empty.
    assert_eq!(root_names(&index), ["alpha", "beta"]);
    Ok(())
}

#[test]
fn a_colliding_root_name_is_disambiguated_rather_than_overwritten() -> TestResult {
    // Cargo forbids duplicate member names, but a Cargo package and a Python package
    // can collide — and identity is the path, so the second would intern to the
    // first's id and silently replace it.
    let dir = tempfile::tempdir()?;
    std::fs::create_dir_all(dir.path().join("rust").join("shared").join("src"))?;
    std::fs::write(
        dir.path().join("rust").join("shared").join("Cargo.toml"),
        "[package]\nname = \"shared\"\nversion = \"0.1.0\"\n",
    )?;
    std::fs::write(
        dir.path()
            .join("rust")
            .join("shared")
            .join("src")
            .join("lib.rs"),
        "pub fn thing() {}\n",
    )?;
    std::fs::create_dir_all(dir.path().join("py").join("src").join("shared"))?;
    std::fs::write(
        dir.path().join("py").join("pyproject.toml"),
        "[project]\nname = \"shared\"\n",
    )?;
    std::fs::write(
        dir.path()
            .join("py")
            .join("src")
            .join("shared")
            .join("__init__.py"),
        "",
    )?;

    let index = index_workspace(dir.path(), IndexOptions::default())?;
    assert_eq!(index.roots().len(), 2, "one root overwrote the other");
    let paths: Vec<String> = index
        .roots()
        .iter()
        .filter_map(|id| index.path(*id).map(ToString::to_string))
        .collect();
    assert_eq!(paths, ["shared", "shared#2"]);
    // The disambiguated identity still round-trips, which is how the view hands a
    // node back to us.
    assert!("shared#2".parse::<SeamPath>().is_ok());
    Ok(())
}

#[test]
fn a_python_package_is_indexed_by_import_path() -> TestResult {
    if crate::lang::python::language_id().is_none() {
        return Ok(());
    }
    let dir = tempfile::tempdir()?;
    std::fs::create_dir_all(dir.path().join("src").join("app").join("net"))?;
    std::fs::write(
        dir.path().join("pyproject.toml"),
        "[project]\nname = \"my-app\"\n",
    )?;
    std::fs::write(
        dir.path().join("src").join("app").join("__init__.py"),
        "class Root:\n    pass\n",
    )?;
    std::fs::write(
        dir.path().join("src").join("app").join("model.py"),
        "class Symbol:\n    pass\n",
    )?;
    std::fs::write(
        dir.path()
            .join("src")
            .join("app")
            .join("net")
            .join("__init__.py"),
        "def connect():\n    pass\n",
    )?;

    let index = index_workspace(dir.path(), IndexOptions::default())?;
    let found = paths(&index);
    assert!(
        found.contains(&"app::model::Symbol".to_owned()),
        "got {found:?}"
    );
    assert!(
        found.contains(&"app::net::connect".to_owned()),
        "got {found:?}"
    );
    // The package's own `__init__.py` attaches to the package, not to a child module
    // named `__init__`.
    assert!(found.contains(&"app::Root".to_owned()), "got {found:?}");
    assert!(
        !found.iter().any(|p| p.contains("__init__")),
        "got {found:?}"
    );
    Ok(())
}

#[test]
fn a_mixed_repository_indexes_both_its_languages() -> TestResult {
    if crate::lang::rust::language_id().is_none() || crate::lang::python::language_id().is_none() {
        return Ok(());
    }
    let dir = workspace(&["core"])?;
    std::fs::create_dir_all(dir.path().join("py").join("src").join("svc"))?;
    std::fs::write(
        dir.path().join("py").join("pyproject.toml"),
        "[project]\nname = \"svc\"\n",
    )?;
    std::fs::write(
        dir.path()
            .join("py")
            .join("src")
            .join("svc")
            .join("__init__.py"),
        "def serve():\n    pass\n",
    )?;

    let index = index_workspace(dir.path(), IndexOptions::default())?;
    assert_eq!(root_names(&index), ["core", "svc"]);
    let found = paths(&index);
    assert!(found.contains(&"core::thing".to_owned()), "got {found:?}");
    assert!(found.contains(&"svc::serve".to_owned()), "got {found:?}");
    Ok(())
}

#[test]
fn reindexing_a_python_file_needs_no_language_argument() -> TestResult {
    if crate::lang::python::language_id().is_none() {
        return Ok(());
    }
    // The attribution map earning its keep: a Python module node sits in the very file
    // it owns, so inferring the owner from the tree would land one level too high.
    let dir = tempfile::tempdir()?;
    std::fs::create_dir_all(dir.path().join("src").join("app"))?;
    std::fs::write(
        dir.path().join("pyproject.toml"),
        "[project]\nname = \"app\"\n",
    )?;
    std::fs::write(dir.path().join("src").join("app").join("__init__.py"), "")?;
    std::fs::write(
        dir.path().join("src").join("app").join("model.py"),
        "def original():\n    pass\n",
    )?;

    let mut index = index_workspace(dir.path(), IndexOptions::default())?;
    assert!(paths(&index).contains(&"app::model::original".to_owned()));

    let mut pool = ParserPool::new();
    reindex_file(
        &mut index,
        &mut pool,
        &dir.path().join("src").join("app").join("model.py"),
        "def renamed():\n    pass\n",
    )?;

    let found = paths(&index);
    assert!(
        found.contains(&"app::model::renamed".to_owned()),
        "got {found:?}"
    );
    assert!(
        !found.contains(&"app::model::original".to_owned()),
        "stale node survived"
    );
    Ok(())
}

#[test]
fn reindexing_a_file_the_index_never_held_changes_nothing() -> TestResult {
    if crate::lang::rust::language_id().is_none() {
        return Ok(());
    }
    let dir = package("demo", &[("src/lib.rs", "pub fn here() {}")])?;
    let mut index = index_package(dir.path(), IndexOptions::default())?;
    let before = index.files().len();

    let mut pool = ParserPool::new();
    let outcome = reindex_file(
        &mut index,
        &mut pool,
        &dir.path().join("src").join("absent.rs"),
        "pub fn nothing() {}",
    );

    assert!(matches!(outcome, Err(ExtractError::NoMapping)));
    // And it did not quietly register the path, which would inflate the file count
    // the header reports for work that never happened.
    assert_eq!(index.files().len(), before);
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
