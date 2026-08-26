//! Resolving a module declaration to the file that holds its body.
//!
//! Rust's containment tree spans files, and the mapping between the two is a convention
//! rather than a declaration: `mod net;` inside `src/lib.rs` means `src/net.rs` or
//! `src/net/mod.rs`, while the same line inside `src/client.rs` means
//! `src/client/net.rs`. Getting this wrong does not produce an error — it produces a tree
//! that silently omits half the package — so the rules are implemented in full here.
//!
//! The view shows modules by their semantic path, never as file rows. `src/model.rs`
//! appears as `karet-core::model` because that is what it *is*; the file is an
//! implementation detail of where the text lives.

pub mod python;

use std::path::Path;
use std::path::PathBuf;

/// Where a module declaration's body was found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModuleSource {
    /// The body is written inline, so there is no file to visit.
    Inline,
    /// The body lives in this file.
    File(PathBuf),
    /// No file could be found for the declaration.
    ///
    /// A normal outcome, not an error: the module is still a node in the tree, marked so
    /// the view can say its contents are unavailable rather than showing it as empty.
    Missing {
        /// The paths that were tried, for the explanatory message.
        candidates: Vec<PathBuf>,
    },
}

/// Where a file's child modules are looked for.
///
/// A crate root or a `mod.rs` owns its own directory; any other file owns a
/// subdirectory named after it.
#[must_use]
pub fn module_directory(file: &Path, is_crate_root: bool) -> PathBuf {
    let parent = file.parent().unwrap_or(Path::new(""));
    if is_crate_root || file.file_stem().is_some_and(|stem| stem == "mod") {
        return parent.to_path_buf();
    }
    match file.file_stem() {
        Some(stem) => parent.join(stem),
        None => parent.to_path_buf(),
    }
}

/// Whether `file` is a "mod-rs" file — a crate root or a `mod.rs`.
///
/// The distinction matters only for `#[path]` resolution, where a mod-rs file and an
/// ordinary one anchor the relative path differently.
fn is_mod_rs(file: &Path, is_crate_root: bool) -> bool {
    is_crate_root || file.file_stem().is_some_and(|stem| stem == "mod")
}

/// Where a `#[path = "…"]` on a module declaration is resolved from.
///
/// rustc anchors this differently from ordinary module lookup, and the difference is easy
/// to get wrong: a `#[path]` written at the *top level of a file* is relative to that
/// file's own directory, not to the directory the file's child modules would live in. So
/// `#[path = "convert_tests.rs"] mod tests;` inside `src/convert.rs` means
/// `src/convert_tests.rs` — **not** `src/convert/convert_tests.rs`.
///
/// Inside an inline `mod { … }` block the anchor gains the inline components, and for a
/// non-mod-rs file it additionally gains that file's own name.
fn path_attribute_directory(file: &Path, is_crate_root: bool, inline_path: &[String]) -> PathBuf {
    let parent = file.parent().unwrap_or(Path::new("")).to_path_buf();
    if inline_path.is_empty() {
        return parent;
    }
    let mut directory = if is_mod_rs(file, is_crate_root) {
        parent
    } else {
        match file.file_stem() {
            Some(stem) => parent.join(stem),
            None => parent,
        }
    };
    for segment in inline_path {
        directory = directory.join(segment);
    }
    directory
}

/// Resolve `mod <name>;` declared in `file`, nested under `inline_path` inline modules.
///
/// `path_attribute` is the value of a `#[path = "…"]` on the declaration, which overrides
/// the convention entirely and anchors differently — see [`path_attribute_directory`].
#[must_use]
pub fn resolve(
    file: &Path,
    is_crate_root: bool,
    inline_path: &[String],
    name: &str,
    path_attribute: Option<&str>,
) -> ModuleSource {
    if let Some(relative) = path_attribute {
        let candidate = path_attribute_directory(file, is_crate_root, inline_path)
            .join(relative.trim_matches('"'));
        return if candidate.is_file() {
            ModuleSource::File(candidate)
        } else {
            ModuleSource::Missing {
                candidates: vec![candidate],
            }
        };
    }

    let mut directory = module_directory(file, is_crate_root);
    // An inline module nests the search directory one level per enclosing `mod { … }`.
    for segment in inline_path {
        directory = directory.join(segment);
    }
    let candidates = vec![
        directory.join(format!("{name}.rs")),
        directory.join(name).join("mod.rs"),
    ];
    match candidates.iter().find(|candidate| candidate.is_file()) {
        Some(found) => ModuleSource::File(found.clone()),
        None => ModuleSource::Missing { candidates },
    }
}

/// Whether `file` is a crate root — the entry point of a build target.
#[must_use]
pub fn is_crate_root(file: &Path) -> bool {
    matches!(
        file.file_name().and_then(|name| name.to_str()),
        Some("lib.rs" | "main.rs" | "build.rs")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_crate_root_owns_its_own_directory() {
        assert_eq!(
            module_directory(Path::new("src/lib.rs"), true),
            PathBuf::from("src")
        );
    }

    #[test]
    fn a_mod_file_owns_its_own_directory() {
        assert_eq!(
            module_directory(Path::new("src/net/mod.rs"), false),
            PathBuf::from("src/net")
        );
    }

    #[test]
    fn any_other_file_owns_a_subdirectory_named_after_it() {
        // This is the rule that silently loses half a package when it is wrong.
        assert_eq!(
            module_directory(Path::new("src/client.rs"), false),
            PathBuf::from("src/client")
        );
    }

    #[test]
    fn crate_roots_are_recognized_by_name() {
        assert!(is_crate_root(Path::new("src/lib.rs")));
        assert!(is_crate_root(Path::new("src/main.rs")));
        assert!(is_crate_root(Path::new("build.rs")));
        assert!(!is_crate_root(Path::new("src/model.rs")));
    }

    #[test]
    fn resolves_a_sibling_file() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let src = dir.path().join("src");
        std::fs::create_dir_all(&src)?;
        std::fs::write(src.join("lib.rs"), "mod net;")?;
        std::fs::write(src.join("net.rs"), "")?;

        let resolved = resolve(&src.join("lib.rs"), true, &[], "net", None);
        assert_eq!(resolved, ModuleSource::File(src.join("net.rs")));
        Ok(())
    }

    #[test]
    fn prefers_a_sibling_file_over_a_mod_directory() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let src = dir.path().join("src");
        std::fs::create_dir_all(src.join("net"))?;
        std::fs::write(src.join("lib.rs"), "mod net;")?;
        std::fs::write(src.join("net.rs"), "")?;
        std::fs::write(src.join("net").join("mod.rs"), "")?;

        assert_eq!(
            resolve(&src.join("lib.rs"), true, &[], "net", None),
            ModuleSource::File(src.join("net.rs")),
            "the flat form wins, matching rustc"
        );
        Ok(())
    }

    #[test]
    fn resolves_a_mod_directory_when_there_is_no_sibling() -> Result<(), Box<dyn std::error::Error>>
    {
        let dir = tempfile::tempdir()?;
        let src = dir.path().join("src");
        std::fs::create_dir_all(src.join("net"))?;
        std::fs::write(src.join("lib.rs"), "mod net;")?;
        std::fs::write(src.join("net").join("mod.rs"), "")?;

        assert_eq!(
            resolve(&src.join("lib.rs"), true, &[], "net", None),
            ModuleSource::File(src.join("net").join("mod.rs"))
        );
        Ok(())
    }

    #[test]
    fn resolves_relative_to_a_non_root_file() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let src = dir.path().join("src");
        std::fs::create_dir_all(src.join("client"))?;
        std::fs::write(src.join("client.rs"), "mod net;")?;
        std::fs::write(src.join("client").join("net.rs"), "")?;

        assert_eq!(
            resolve(&src.join("client.rs"), false, &[], "net", None),
            ModuleSource::File(src.join("client").join("net.rs"))
        );
        Ok(())
    }

    #[test]
    fn an_inline_module_nests_the_search_directory() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let src = dir.path().join("src");
        std::fs::create_dir_all(src.join("outer"))?;
        std::fs::write(src.join("lib.rs"), "mod outer { mod inner; }")?;
        std::fs::write(src.join("outer").join("inner.rs"), "")?;

        assert_eq!(
            resolve(
                &src.join("lib.rs"),
                true,
                &["outer".to_owned()],
                "inner",
                None
            ),
            ModuleSource::File(src.join("outer").join("inner.rs"))
        );
        Ok(())
    }

    #[test]
    fn a_path_attribute_overrides_the_convention() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let src = dir.path().join("src");
        std::fs::create_dir_all(&src)?;
        std::fs::write(src.join("lib.rs"), "#[path = \"elsewhere.rs\"] mod net;")?;
        std::fs::write(src.join("elsewhere.rs"), "")?;

        assert_eq!(
            resolve(
                &src.join("lib.rs"),
                true,
                &[],
                "net",
                Some("\"elsewhere.rs\"")
            ),
            ModuleSource::File(src.join("elsewhere.rs"))
        );
        Ok(())
    }

    #[test]
    fn a_path_attribute_anchors_to_the_declaring_file_not_its_module_directory()
    -> Result<(), Box<dyn std::error::Error>> {
        // Regression: `#[path = "convert_tests.rs"] mod tests;` written at the top level
        // of `src/convert.rs` means `src/convert_tests.rs`, because rustc anchors a
        // top-level `#[path]` to the file's own directory. Anchoring it to the module
        // directory instead looks for `src/convert/convert_tests.rs` and silently loses
        // the module — which is exactly what indexing a real crate turned up.
        let dir = tempfile::tempdir()?;
        let src = dir.path().join("src");
        std::fs::create_dir_all(&src)?;
        std::fs::write(
            src.join("convert.rs"),
            "#[path = \"convert_tests.rs\"]\nmod tests;",
        )?;
        std::fs::write(src.join("convert_tests.rs"), "")?;

        assert_eq!(
            resolve(
                &src.join("convert.rs"),
                false,
                &[],
                "tests",
                Some("\"convert_tests.rs\"")
            ),
            ModuleSource::File(src.join("convert_tests.rs"))
        );
        Ok(())
    }

    #[test]
    fn a_path_attribute_inside_an_inline_block_gains_the_file_and_block_names()
    -> Result<(), Box<dyn std::error::Error>> {
        // Inside an inline block the anchor deepens: a non-mod-rs file contributes its own
        // name, then each enclosing block contributes one directory.
        let dir = tempfile::tempdir()?;
        let src = dir.path().join("src");
        std::fs::create_dir_all(src.join("convert").join("inner"))?;
        std::fs::write(
            src.join("convert.rs"),
            "mod inner { #[path = \"t.rs\"] mod tests; }",
        )?;
        std::fs::write(src.join("convert").join("inner").join("t.rs"), "")?;

        assert_eq!(
            resolve(
                &src.join("convert.rs"),
                false,
                &["inner".to_owned()],
                "tests",
                Some("\"t.rs\"")
            ),
            ModuleSource::File(src.join("convert").join("inner").join("t.rs"))
        );
        Ok(())
    }

    #[test]
    fn a_module_with_no_file_reports_what_it_tried() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let src = dir.path().join("src");
        std::fs::create_dir_all(&src)?;
        std::fs::write(src.join("lib.rs"), "mod absent;")?;

        // Missing is a normal outcome carrying evidence, not a silent omission.
        let ModuleSource::Missing { candidates } =
            resolve(&src.join("lib.rs"), true, &[], "absent", None)
        else {
            return Err("expected the module to be unresolved".into());
        };
        assert_eq!(candidates.len(), 2);
        assert!(candidates.iter().any(|c| c.ends_with("absent.rs")));
        assert!(candidates.iter().any(|c| c.ends_with("absent/mod.rs")));
        Ok(())
    }
}
