//! Discovery reads manifests and lists directories, never source — so unlike the rest of
//! the crate's suites, these tests need no grammar compiled in and run under
//! `--no-default-features`.

use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Write a file, creating the directories above it.
fn write(root: &Path, relative: &str, contents: &str) -> Result<(), std::io::Error> {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, contents)
}

/// A Cargo package manifest.
fn package_manifest(name: &str) -> String {
    format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\n")
}

/// Discover under `root` with the default bounds, as `(name, relative path)` pairs.
fn found(root: &Path) -> Vec<(String, String)> {
    discover(root, DiscoveryOptions::default())
        .into_iter()
        .map(|package| {
            (
                package.name,
                package
                    .root
                    .strip_prefix(root)
                    .unwrap_or(&package.root)
                    .to_string_lossy()
                    .replace('\\', "/"),
            )
        })
        .collect()
}

fn names(root: &Path) -> Vec<String> {
    found(root).into_iter().map(|(name, _)| name).collect()
}

// --- Cargo topology -----------------------------------------------------------

#[test]
fn a_virtual_workspace_root_yields_every_member() -> TestResult {
    // The case that made the view unusable: a root manifest declaring only a workspace is
    // not a package, but it is emphatically something to index.
    let dir = tempfile::tempdir()?;
    write(
        dir.path(),
        "Cargo.toml",
        "[workspace]\nmembers = [\"crates/*\"]\n",
    )?;
    write(
        dir.path(),
        "crates/alpha/Cargo.toml",
        &package_manifest("alpha"),
    )?;
    write(
        dir.path(),
        "crates/beta/Cargo.toml",
        &package_manifest("beta"),
    )?;

    assert_eq!(names(dir.path()), ["alpha", "beta"]);
    Ok(())
}

#[test]
fn a_root_that_is_both_a_package_and_a_workspace_lists_itself_first() -> TestResult {
    let dir = tempfile::tempdir()?;
    write(
        dir.path(),
        "Cargo.toml",
        "[package]\nname = \"top\"\nversion = \"0.1.0\"\n\n[workspace]\nmembers = [\"sub\"]\n",
    )?;
    write(dir.path(), "sub/Cargo.toml", &package_manifest("sub"))?;

    // The root is the thing the directory is named after, so it leads.
    assert_eq!(
        found(dir.path()),
        [
            ("top".to_owned(), String::new()),
            ("sub".to_owned(), "sub".to_owned()),
        ]
    );
    Ok(())
}

#[test]
fn members_expand_in_a_stable_sorted_order() -> TestResult {
    // The discovery order is the order the view lists packages in, so it cannot depend on
    // whatever order the filesystem hands back.
    let dir = tempfile::tempdir()?;
    write(
        dir.path(),
        "Cargo.toml",
        "[workspace]\nmembers = [\"crates/*\", \"xtask\"]\n",
    )?;
    for name in ["zeta", "alpha", "mid"] {
        write(
            dir.path(),
            &format!("crates/{name}/Cargo.toml"),
            &package_manifest(name),
        )?;
    }
    write(dir.path(), "xtask/Cargo.toml", &package_manifest("xtask"))?;

    // Sorted within the glob, but the literal keeps its declared position after it.
    assert_eq!(names(dir.path()), ["alpha", "mid", "zeta", "xtask"]);
    Ok(())
}

#[test]
fn an_excluded_member_is_not_discovered() -> TestResult {
    let dir = tempfile::tempdir()?;
    write(
        dir.path(),
        "Cargo.toml",
        "[workspace]\nmembers = [\"crates/*\"]\nexclude = [\"crates/skipme\"]\n",
    )?;
    write(
        dir.path(),
        "crates/keep/Cargo.toml",
        &package_manifest("keep"),
    )?;
    write(
        dir.path(),
        "crates/skipme/Cargo.toml",
        &package_manifest("skipme"),
    )?;

    assert_eq!(names(dir.path()), ["keep"]);
    Ok(())
}

#[test]
fn default_members_add_nothing_the_member_list_already_has() -> TestResult {
    let dir = tempfile::tempdir()?;
    write(
        dir.path(),
        "Cargo.toml",
        "[workspace]\nmembers = [\"a\"]\ndefault-members = [\"a\"]\n",
    )?;
    write(dir.path(), "a/Cargo.toml", &package_manifest("a"))?;

    // Taking the union must not list the same package twice.
    assert_eq!(names(dir.path()), ["a"]);
    Ok(())
}

#[test]
fn a_member_that_is_itself_a_workspace_contributes_its_own_members() -> TestResult {
    let dir = tempfile::tempdir()?;
    write(
        dir.path(),
        "Cargo.toml",
        "[workspace]\nmembers = [\"inner\"]\n",
    )?;
    write(
        dir.path(),
        "inner/Cargo.toml",
        "[workspace]\nmembers = [\"deep\"]\n",
    )?;
    write(
        dir.path(),
        "inner/deep/Cargo.toml",
        &package_manifest("deep"),
    )?;

    assert_eq!(names(dir.path()), ["deep"]);
    Ok(())
}

#[test]
fn packages_below_a_manifestless_root_are_found_by_scanning() -> TestResult {
    // The `rust/`-under-a-polyglot-repo layout: nothing at the top says "Cargo" at all.
    let dir = tempfile::tempdir()?;
    write(dir.path(), "rust/api/Cargo.toml", &package_manifest("api"))?;
    write(
        dir.path(),
        "rust/worker/Cargo.toml",
        &package_manifest("worker"),
    )?;
    write(dir.path(), "web/index.html", "<html>")?;

    assert_eq!(
        found(dir.path()),
        [
            ("api".to_owned(), "rust/api".to_owned()),
            ("worker".to_owned(), "rust/worker".to_owned()),
        ]
    );
    Ok(())
}

#[test]
fn the_scan_does_not_look_inside_a_package_it_already_found() -> TestResult {
    // A crate's `examples/` hold manifests of their own. They are that crate's business.
    let dir = tempfile::tempdir()?;
    write(
        dir.path(),
        "sub/thing/Cargo.toml",
        &package_manifest("thing"),
    )?;
    write(
        dir.path(),
        "sub/thing/examples/demo/Cargo.toml",
        &package_manifest("demo"),
    )?;

    assert_eq!(names(dir.path()), ["thing"]);
    Ok(())
}

#[test]
fn the_scan_stops_at_the_depth_cap() -> TestResult {
    let dir = tempfile::tempdir()?;
    write(dir.path(), "a/b/c/d/Cargo.toml", &package_manifest("deep"))?;

    assert!(discover(dir.path(), DiscoveryOptions::default()).is_empty());
    assert_eq!(
        discover(
            dir.path(),
            DiscoveryOptions {
                max_depth: 6,
                ..DiscoveryOptions::default()
            }
        )
        .len(),
        1
    );
    Ok(())
}

#[test]
fn build_output_is_never_mistaken_for_source() -> TestResult {
    let dir = tempfile::tempdir()?;
    write(
        dir.path(),
        "target/package/vendored/Cargo.toml",
        &package_manifest("vendored"),
    )?;
    write(dir.path(), "real/Cargo.toml", &package_manifest("real"))?;

    assert_eq!(names(dir.path()), ["real"]);
    Ok(())
}

#[test]
fn discovery_stops_at_the_package_cap() -> TestResult {
    let dir = tempfile::tempdir()?;
    write(
        dir.path(),
        "Cargo.toml",
        "[workspace]\nmembers = [\"crates/*\"]\n",
    )?;
    for name in ["a", "b", "c"] {
        write(
            dir.path(),
            &format!("crates/{name}/Cargo.toml"),
            &package_manifest(name),
        )?;
    }

    let capped = discover(
        dir.path(),
        DiscoveryOptions {
            max_packages: 2,
            ..DiscoveryOptions::default()
        },
    );
    assert_eq!(capped.len(), 2);
    Ok(())
}

// --- Python -------------------------------------------------------------------

#[test]
fn a_src_layout_project_roots_at_its_importable_package() -> TestResult {
    let dir = tempfile::tempdir()?;
    write(
        dir.path(),
        "pyproject.toml",
        "[project]\nname = \"my-app\"\n",
    )?;
    write(dir.path(), "src/my_app/__init__.py", "")?;

    // The distribution is `my-app`; the import path is `my_app`. A seam path is an import
    // path, so the directory wins over the manifest.
    assert_eq!(
        found(dir.path()),
        [("my_app".to_owned(), "src/my_app".to_owned())]
    );
    Ok(())
}

#[test]
fn a_flat_layout_project_skips_the_directories_that_sit_beside_a_package() -> TestResult {
    let dir = tempfile::tempdir()?;
    write(dir.path(), "setup.py", "from setuptools import setup")?;
    write(dir.path(), "mypkg/__init__.py", "")?;
    write(dir.path(), "tests/__init__.py", "")?;
    write(dir.path(), "docs/__init__.py", "")?;

    assert_eq!(names(dir.path()), ["mypkg"]);
    Ok(())
}

#[test]
fn a_src_layout_never_consults_the_support_directory_list() -> TestResult {
    // A package legitimately named `test` under `src/` is a package: the layout has
    // already said where packages live, and second-guessing it would drop a real one.
    let dir = tempfile::tempdir()?;
    write(dir.path(), "pyproject.toml", "[project]\nname = \"p\"\n")?;
    write(dir.path(), "src/test/__init__.py", "")?;

    assert_eq!(names(dir.path()), ["test"]);
    Ok(())
}

#[test]
fn a_project_of_loose_modules_roots_at_its_own_directory() -> TestResult {
    let dir = tempfile::tempdir()?;
    write(dir.path(), "setup.cfg", "[metadata]\nname = tool\n")?;
    write(dir.path(), "tool.py", "def run(): pass\n")?;

    assert_eq!(
        found(dir.path()),
        [(
            dir.path()
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default()
                .to_owned(),
            String::new()
        )]
    );
    Ok(())
}

#[test]
fn loose_python_with_no_project_file_is_not_a_package() -> TestResult {
    // Build scripts and one-off tools live all over a repository. Rooting the view at one
    // would fill the package column with things nobody would call a package.
    let dir = tempfile::tempdir()?;
    write(dir.path(), "scripts/release.py", "print('go')\n")?;

    assert!(discover(dir.path(), DiscoveryOptions::default()).is_empty());
    Ok(())
}

#[test]
fn a_virtual_environment_inside_a_project_is_not_discovered() -> TestResult {
    let dir = tempfile::tempdir()?;
    write(dir.path(), "pyproject.toml", "[project]\nname = \"p\"\n")?;
    write(dir.path(), "src/p/__init__.py", "")?;
    write(dir.path(), ".venv/lib/site.py", "")?;
    write(dir.path(), ".venv/pyvenv.cfg", "home = /usr")?;

    assert_eq!(names(dir.path()), ["p"]);
    Ok(())
}

// --- Both together ------------------------------------------------------------

#[test]
fn a_mixed_repository_yields_its_cargo_and_its_python() -> TestResult {
    // The whole point of running both walks: half an answer about a polyglot repository is
    // a quieter kind of wrong than no answer.
    let dir = tempfile::tempdir()?;
    write(
        dir.path(),
        "Cargo.toml",
        "[workspace]\nmembers = [\"rust/core\"]\n",
    )?;
    write(
        dir.path(),
        "rust/core/Cargo.toml",
        &package_manifest("core"),
    )?;
    write(
        dir.path(),
        "py/pyproject.toml",
        "[project]\nname = \"svc\"\n",
    )?;
    write(dir.path(), "py/src/svc/__init__.py", "")?;

    assert_eq!(
        found(dir.path()),
        [
            ("core".to_owned(), "rust/core".to_owned()),
            ("svc".to_owned(), "py/src/svc".to_owned()),
        ]
    );
    Ok(())
}

#[test]
fn a_directory_with_nothing_indexable_discovers_nothing() -> TestResult {
    let dir = tempfile::tempdir()?;
    write(dir.path(), "README.md", "# nothing here")?;

    // Empty rather than an error: a caller offering start points can act on this.
    assert!(discover(dir.path(), DiscoveryOptions::default()).is_empty());
    Ok(())
}

#[test]
fn every_discovered_package_points_at_a_file_that_exists() -> TestResult {
    let dir = tempfile::tempdir()?;
    write(dir.path(), "Cargo.toml", "[workspace]\nmembers = [\"a\"]\n")?;
    write(dir.path(), "a/Cargo.toml", &package_manifest("a"))?;
    write(dir.path(), "py/pyproject.toml", "[project]\nname = \"p\"\n")?;
    write(dir.path(), "py/src/p/__init__.py", "")?;

    for package in discover(dir.path(), DiscoveryOptions::default()) {
        assert!(package.anchor.is_file(), "{package:?} has no anchor");
        assert!(package.root.is_dir(), "{package:?} has no root");
    }
    Ok(())
}

// --- the file-tree ecosystems -----------------------------------------------

/// The kinds of everything discovered under `root`, in discovery order.
fn kinds(root: &std::path::Path) -> Vec<PackageKind> {
    discover(root, DiscoveryOptions::default())
        .into_iter()
        .map(|package| package.kind)
        .collect()
}

#[test]
fn a_node_package_is_rooted_at_its_source_directory() {
    let Ok(dir) = tempfile::tempdir() else {
        return;
    };
    let root = dir.path().join("web");
    let _ = std::fs::create_dir_all(root.join("src"));
    let _ = std::fs::write(root.join("package.json"), "{}");
    let _ = std::fs::write(root.join("src/index.ts"), "");
    let discovered = discover(dir.path(), DiscoveryOptions::default());
    let node = discovered.iter().find(|p| p.kind == PackageKind::Node);
    // `src` says where the sources are, not what they are called, so the name comes from
    // the package directory.
    assert_eq!(node.map(|p| p.name.as_str()), Some("web"));
    assert_eq!(node.map(|p| p.root.clone()), Some(root.join("src")));
}

#[test]
fn a_node_package_with_no_src_directory_is_rooted_at_itself() {
    let Ok(dir) = tempfile::tempdir() else {
        return;
    };
    let _ = std::fs::write(dir.path().join("package.json"), "{}");
    let _ = std::fs::write(dir.path().join("main.js"), "");
    let discovered = discover(dir.path(), DiscoveryOptions::default());
    assert!(discovered.iter().any(|p| p.kind == PackageKind::Node));
}

#[test]
fn a_swift_package_yields_one_root_per_target() {
    let Ok(dir) = tempfile::tempdir() else {
        return;
    };
    let _ = std::fs::write(dir.path().join("Package.swift"), "");
    for target in ["Widgets", "Support"] {
        let _ = std::fs::create_dir_all(dir.path().join("Sources").join(target));
        let _ = std::fs::write(dir.path().join("Sources").join(target).join("a.swift"), "");
    }
    let names: Vec<String> = discover(dir.path(), DiscoveryOptions::default())
        .into_iter()
        .filter(|p| p.kind == PackageKind::Swift)
        .map(|p| p.name)
        .collect();
    // Sorted, because discovery order is what the view's first column shows.
    assert_eq!(names, ["Support", "Widgets"]);
}

#[test]
fn a_single_target_swift_package_uses_sources_itself() {
    let Ok(dir) = tempfile::tempdir() else {
        return;
    };
    let root = dir.path().join("Widgets");
    let _ = std::fs::create_dir_all(root.join("Sources"));
    let _ = std::fs::write(root.join("Package.swift"), "");
    let _ = std::fs::write(root.join("Sources/a.swift"), "");
    let swift = discover(dir.path(), DiscoveryOptions::default())
        .into_iter()
        .find(|p| p.kind == PackageKind::Swift);
    assert_eq!(swift.as_ref().map(|p| p.name.as_str()), Some("Widgets"));
    assert_eq!(swift.map(|p| p.root), Some(root.join("Sources")));
}

#[test]
fn a_gradle_module_yields_a_root_per_source_set() {
    let Ok(dir) = tempfile::tempdir() else {
        return;
    };
    let root = dir.path().join("app");
    let _ = std::fs::create_dir_all(root.join("src/main/kotlin"));
    let _ = std::fs::create_dir_all(root.join("src/test/kotlin"));
    let _ = std::fs::write(root.join("build.gradle.kts"), "");
    let roots: Vec<PathBuf> = discover(dir.path(), DiscoveryOptions::default())
        .into_iter()
        .filter(|p| p.kind == PackageKind::Gradle)
        .map(|p| p.root)
        .collect();
    // Every set, not just `main`: which one is "the" one is a build-tool question.
    assert_eq!(
        roots,
        [root.join("src/main/kotlin"), root.join("src/test/kotlin")]
    );
}

#[test]
fn a_gradle_module_with_no_sources_contributes_nothing() {
    let Ok(dir) = tempfile::tempdir() else {
        return;
    };
    let _ = std::fs::write(dir.path().join("build.gradle"), "");
    assert!(kinds(dir.path()).is_empty());
}

#[test]
fn a_polyglot_repository_answers_about_all_of_itself() {
    let Ok(dir) = tempfile::tempdir() else {
        return;
    };
    let rust = dir.path().join("engine");
    let _ = std::fs::create_dir_all(rust.join("src"));
    let _ = std::fs::write(rust.join("Cargo.toml"), "[package]\nname = \"engine\"\n");
    let _ = std::fs::write(rust.join("src/lib.rs"), "");
    let web = dir.path().join("web");
    let _ = std::fs::create_dir_all(web.join("src"));
    let _ = std::fs::write(web.join("package.json"), "{}");
    let _ = std::fs::write(web.join("src/index.ts"), "");
    let kinds = kinds(dir.path());
    // One repository, both halves. Answering with only one would be a quieter wrong.
    assert!(kinds.contains(&PackageKind::Cargo), "{kinds:?}");
    assert!(kinds.contains(&PackageKind::Node), "{kinds:?}");
}

#[test]
fn an_ecosystems_extensions_and_index_names_match_its_conventions() {
    assert!(PackageKind::Node.extensions().contains(&"tsx"));
    assert_eq!(PackageKind::Swift.extensions(), ["swift"]);
    assert!(PackageKind::Gradle.extensions().contains(&"kt"));
    // Only Node has the `__init__.py`/`mod.rs` convention.
    assert_eq!(PackageKind::Node.index_names(), ["index"]);
    assert!(PackageKind::Swift.index_names().is_empty());
    // Cargo declares its tree and Python has a walk of its own.
    assert!(PackageKind::Cargo.extensions().is_empty());
    assert!(PackageKind::Python.extensions().is_empty());
}
