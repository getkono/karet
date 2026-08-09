//! File-type identity → language detection.
//!
//! Two layers so a viewer can label a file even when its grammar isn't built in:
//! [`language_id_from_path`] resolves only grammars compiled into this build, while
//! [`language_name_from_path`] also recognizes common languages without a bundled
//! grammar (for the UI label), falling back to "plaintext" rendering.

use std::path::Path;

use crate::LanguageId;
use crate::registry;

/// The [`LanguageId`] of a bundled grammar for `path`'s file type, if one is
/// compiled in. `None` means the caller should render plaintext.
///
/// `karet-filetype` is the single path→language authority; its grammar identity
/// resolves through the same alias table as injection names. A declared identity
/// that doesn't resolve (a registry typo, or a grammar not compiled into this
/// build) degrades to `None` — plaintext — rather than silently shadowing.
#[must_use]
pub fn language_id_from_path(path: &Path) -> Option<LanguageId> {
    language_id_from_injection_name(karet_filetype::file_type_for_path(path).grammar()?)
}

/// A human-readable language name for `path`, for UI labels.
///
/// Defers to the shared [`karet_filetype`] catalogue, keeping display identity
/// independent from whichever grammar happens to parse the file.
/// `None` for unrecognized files (the caller should show "plaintext").
#[must_use]
pub fn language_name_from_path(path: &Path) -> Option<&'static str> {
    let ft = karet_filetype::file_type_for_path(path);
    ft.is_recognized().then(|| ft.name())
}

/// The [`LanguageId`] a grammar-injection language name refers to, if that grammar is
/// compiled in.
///
/// This is the resolver for names that appear *inside* source text rather than in a
/// file path: an injection query's `#set! injection.language "javascript"`, a dynamic
/// `@injection.language` capture, or a markdown code fence's info string (` ```sh `).
/// Matching is case-insensitive and covers each grammar's common aliases, so `rs`,
/// `sh` and `c++` all resolve. Unknown names (`jsdoc`, `regex`, `latex` — languages
/// karet bundles no grammar for) return `None`, and the caller simply leaves that
/// region unhighlighted.
#[must_use]
pub fn language_id_from_injection_name(name: &str) -> Option<LanguageId> {
    let name = name.trim().to_ascii_lowercase();
    registry::all()
        .iter()
        .find(|g| g.names.contains(&name.as_str()))
        .map(|g| g.id)
}

/// Lowercased extension of `path`, without the dot (test helper).
#[cfg(test)]
fn extension(path: &Path) -> Option<String> {
    path.extension()?.to_str().map(str::to_ascii_lowercase)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_extension_is_unlabelled() {
        let p = Path::new("file.unknownext");
        assert_eq!(language_id_from_path(p), None);
        assert_eq!(language_name_from_path(p), None);
    }

    #[cfg(not(feature = "lang-kotlin"))]
    #[test]
    fn non_compiled_language_still_named() {
        // Optional grammars remain recognized for labelling when omitted.
        let p = Path::new("Main.kt");
        assert_eq!(language_id_from_path(p), None);
        assert_eq!(language_name_from_path(p), Some("Kotlin"));
    }

    #[test]
    fn injection_names_are_case_insensitive_with_aliases() {
        // Only meaningful when the grammars are compiled in.
        if let Some(rust) = language_id_from_injection_name("rust") {
            assert_eq!(language_id_from_injection_name("rs"), Some(rust));
            assert_eq!(language_id_from_injection_name("  RuSt "), Some(rust));
        }
    }

    #[test]
    fn unknown_injection_name_is_none() {
        // Languages karet bundles no grammar for — the region stays unhighlighted
        // rather than erroring.
        assert_eq!(language_id_from_injection_name("jsdoc"), None);
        assert_eq!(language_id_from_injection_name("regex"), None);
        assert_eq!(language_id_from_injection_name(""), None);
    }

    #[cfg(feature = "lang-markdown")]
    #[test]
    fn inline_markdown_is_reachable_by_name_but_never_by_path() {
        assert!(language_id_from_injection_name("markdown_inline").is_some());
        // It has no extensions, so no file ever resolves to it directly.
        assert_ne!(
            language_id_from_path(Path::new("x.md")),
            language_id_from_injection_name("markdown_inline")
        );
    }

    #[cfg(all(
        feature = "lang-zig",
        feature = "lang-xml",
        feature = "lang-yaml",
        feature = "lang-astro",
        feature = "lang-svelte",
        feature = "lang-vue"
    ))]
    #[test]
    fn modern_language_paths_resolve_to_their_grammars() {
        for (path, expected) in [
            ("main.zig", "Zig"),
            ("document.xml", "XML"),
            ("vector.svg", "SVG"),
            ("workflow.yaml", "YAML"),
            ("page.astro", "Astro"),
            ("component.svelte", "Svelte"),
            ("component.vue", "Vue"),
        ] {
            assert_eq!(
                language_name_from_path(std::path::Path::new(path)),
                Some(expected),
                "{path}"
            );
            assert!(
                language_id_from_path(std::path::Path::new(path)).is_some(),
                "{path}"
            );
        }
    }

    #[cfg(all(
        feature = "lang-sql",
        feature = "lang-graphql",
        feature = "lang-protobuf"
    ))]
    #[test]
    fn query_and_schema_paths_resolve_to_their_grammars() {
        for (path, expected) in [
            ("schema.sql", "SQL"),
            ("schema.graphql", "GraphQL"),
            ("schema.gql", "GraphQL"),
            ("schema.proto", "Protobuf"),
        ] {
            assert_eq!(language_name_from_path(Path::new(path)), Some(expected));
            assert!(language_id_from_path(Path::new(path)).is_some(), "{path}");
        }
    }

    #[cfg(all(
        feature = "lang-containerfile",
        feature = "lang-make",
        feature = "lang-cmake"
    ))]
    #[test]
    fn build_language_names_and_paths_resolve_to_distinct_grammars() {
        for (path, expected) in [
            ("Dockerfile", "Dockerfile"),
            ("Containerfile", "Dockerfile"),
            ("Makefile", "Makefile"),
            ("GNUmakefile", "Makefile"),
            ("rules.mk", "Makefile"),
            ("CMakeLists.txt", "CMake"),
            ("module.cmake", "CMake"),
        ] {
            assert_eq!(language_name_from_path(Path::new(path)), Some(expected));
            assert!(language_id_from_path(Path::new(path)).is_some(), "{path}");
        }
        assert_ne!(
            language_id_from_path(Path::new("CMakeLists.txt")),
            language_id_from_path(Path::new("Makefile"))
        );
    }

    #[cfg(all(
        feature = "lang-rst",
        feature = "lang-asciidoc",
        feature = "lang-latex"
    ))]
    #[test]
    fn document_markup_paths_and_names_resolve_to_distinct_grammars() {
        for (path, name, alias) in [
            ("guide.rst", "reStructuredText", "rst"),
            ("guide.adoc", "AsciiDoc", "asciidoc"),
            ("paper.tex", "TeX", "latex"),
        ] {
            let path_id = language_id_from_path(Path::new(path));
            assert_eq!(language_name_from_path(Path::new(path)), Some(name));
            assert!(path_id.is_some(), "{path}");
            assert_eq!(path_id, language_id_from_injection_name(alias), "{path}");
        }
    }

    #[cfg(all(
        feature = "lang-zsh",
        feature = "lang-fish",
        feature = "lang-powershell",
        feature = "lang-batch"
    ))]
    #[test]
    fn shell_language_paths_and_names_resolve_to_distinct_grammars() {
        for (path, name, alias) in [
            ("script.zsh", "Zsh", "zsh"),
            ("script.fish", "Fish", "fish"),
            ("profile.ps1", "PowerShell", "pwsh"),
            ("module.psm1", "PowerShell", "powershell"),
            ("build.cmd", "Batch", "cmd"),
            ("build.bat", "Batch", "batch"),
        ] {
            let path_id = language_id_from_path(Path::new(path));
            assert_eq!(language_name_from_path(Path::new(path)), Some(name));
            assert!(path_id.is_some(), "{path}");
            assert_eq!(path_id, language_id_from_injection_name(alias), "{path}");
        }
        assert_eq!(language_id_from_path(Path::new("script.ksh")), None);
    }

    #[cfg(all(
        feature = "lang-kotlin",
        feature = "lang-swift",
        feature = "lang-scala",
        feature = "lang-lua",
        feature = "lang-haskell",
        feature = "lang-ocaml",
        feature = "lang-elixir",
        feature = "lang-erlang",
        feature = "lang-dart",
        feature = "lang-r",
        feature = "lang-zig",
        feature = "lang-perl",
        feature = "lang-clojure",
        feature = "lang-elisp",
        feature = "lang-vim"
    ))]
    #[test]
    fn additional_programming_languages_resolve_paths_and_fence_names() {
        for (path, name, alias) in [
            ("Main.kt", "Kotlin", "kotlin"),
            ("Main.swift", "Swift", "swift"),
            ("build.sbt", "Scala", "sbt"),
            ("init.lua", "Lua", "lua"),
            ("Main.hs", "Haskell", "haskell"),
            ("Main.lhs", "Haskell", "hs"),
            ("main.ml", "OCaml", "ocaml"),
            ("main.mli", "OCaml", "mli"),
            ("app.ex", "Elixir", "elixir"),
            ("app.erl", "Erlang", "erlang"),
            ("main.dart", "Dart", "dart"),
            ("analysis.r", "R", "r"),
            ("main.zig", "Zig", "zig"),
            ("tool.pl", "Perl", "perl"),
            ("core.cljc", "Clojure", "clojure"),
            ("init.el", "Emacs Lisp", "elisp"),
            ("plugin.vim", "Vim script", "vimscript"),
        ] {
            let path_id = language_id_from_path(Path::new(path));
            assert_eq!(language_name_from_path(Path::new(path)), Some(name));
            assert!(path_id.is_some(), "{path}");
            assert_eq!(path_id, language_id_from_injection_name(alias), "{path}");
        }
    }

    /// The cross-registry guard rail: with every grammar compiled in, every
    /// grammar identity `karet-filetype` declares must resolve to a bundled
    /// grammar through the injection-name table, and every grammar's file
    /// extensions must be routed by `karet-filetype` — the single path→language
    /// authority — to that same grammar. A typo on either side fails here
    /// instead of silently rendering plaintext.
    #[cfg(feature = "all-languages")]
    #[test]
    fn filetype_grammar_identities_and_grammar_extensions_agree() {
        let mut problems: Vec<String> = Vec::new();
        for ft in karet_filetype::all_file_types() {
            if let Some(grammar) = ft.grammar()
                && language_id_from_injection_name(grammar).is_none()
            {
                problems.push(format!(
                    "file type {:?} declares grammar {grammar:?}, which no bundled grammar answers",
                    ft.name()
                ));
            }
        }
        for grammar in registry::all() {
            for ext in grammar.extensions {
                let path_string = format!("probe.{ext}");
                let routed = language_id_from_path(Path::new(&path_string));
                if routed != Some(grammar.id) {
                    problems.push(format!(
                        "extension .{ext} (grammar {:?}) routes to {routed:?} via karet-filetype",
                        grammar.name
                    ));
                }
            }
        }
        // Alias uniqueness: a name owned by two grammars resolves by table order,
        // which is exactly the silent-precedence bug this test exists to prevent.
        let mut seen: std::collections::HashMap<&str, &str> = std::collections::HashMap::new();
        for grammar in registry::all() {
            for name in grammar.names {
                if let Some(previous) = seen.insert(name, grammar.name) {
                    problems.push(format!(
                        "injection name {name:?} is claimed by both {previous:?} and {:?}",
                        grammar.name
                    ));
                }
            }
        }
        assert!(problems.is_empty(), "{}", problems.join("\n"));
    }

    #[test]
    fn extension_is_case_insensitive() {
        assert_eq!(extension(Path::new("X.MD")).as_deref(), Some("md"));
        assert_eq!(
            language_name_from_path(Path::new("README.MD")),
            Some("Markdown")
        );
    }

    #[cfg(feature = "lang-latex")]
    #[test]
    fn latex_sources_resolve_to_the_tex_grammar() {
        assert!(language_id_from_path(Path::new("paper.tex")).is_some());
        assert_eq!(language_name_from_path(Path::new("paper.TEX")), Some("TeX"));
        assert!(language_id_from_injection_name("latex").is_some());
    }
}
