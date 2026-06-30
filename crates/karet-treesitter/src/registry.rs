//! The compile-time registry of bundled tree-sitter grammars.
//!
//! Each entry is wired by a `lang-*` cargo feature, so [`all`] returns exactly the
//! grammars compiled in. [`LanguageId`] values are stable and must **never** be
//! renumbered once shipped.

use std::sync::OnceLock;

use crate::LanguageId;

/// Static metadata for one registered grammar.
pub(crate) struct GrammarInfo {
    /// The grammar's stable identifier.
    pub id: LanguageId,
    /// A human-readable display name (e.g. `"Rust"`).
    pub name: &'static str,
    /// File extensions (lowercase, without the dot) handled by this grammar.
    pub extensions: &'static [&'static str],
    /// Returns the tree-sitter `Language` for this grammar.
    pub language: fn() -> tree_sitter::Language,
    /// The grammar's highlights query source.
    pub highlights: &'static str,
}

// Stable language ids — never renumber once shipped. Each is cfg-gated to its
// grammar feature so feature-subset builds don't carry unused constants.
#[cfg(feature = "lang-rust")]
pub(crate) const RUST: LanguageId = LanguageId(1);
#[cfg(feature = "lang-python")]
pub(crate) const PYTHON: LanguageId = LanguageId(2);
#[cfg(feature = "lang-javascript")]
pub(crate) const JAVASCRIPT: LanguageId = LanguageId(3);
#[cfg(feature = "lang-typescript")]
pub(crate) const TYPESCRIPT: LanguageId = LanguageId(4);
#[cfg(feature = "lang-typescript")]
pub(crate) const TSX: LanguageId = LanguageId(5);
#[cfg(feature = "lang-json")]
pub(crate) const JSON: LanguageId = LanguageId(6);
#[cfg(feature = "lang-go")]
pub(crate) const GO: LanguageId = LanguageId(7);
#[cfg(feature = "lang-c")]
pub(crate) const C: LanguageId = LanguageId(8);
#[cfg(feature = "lang-cpp")]
pub(crate) const CPP: LanguageId = LanguageId(9);
#[cfg(feature = "lang-csharp")]
pub(crate) const CSHARP: LanguageId = LanguageId(10);
#[cfg(feature = "lang-java")]
pub(crate) const JAVA: LanguageId = LanguageId(11);
#[cfg(feature = "lang-ruby")]
pub(crate) const RUBY: LanguageId = LanguageId(12);
#[cfg(feature = "lang-php")]
pub(crate) const PHP: LanguageId = LanguageId(13);
#[cfg(feature = "lang-bash")]
pub(crate) const BASH: LanguageId = LanguageId(14);
#[cfg(feature = "lang-toml")]
pub(crate) const TOML: LanguageId = LanguageId(15);
#[cfg(feature = "lang-html")]
pub(crate) const HTML: LanguageId = LanguageId(16);
#[cfg(feature = "lang-css")]
pub(crate) const CSS: LanguageId = LanguageId(17);
#[cfg(feature = "lang-yaml")]
pub(crate) const YAML: LanguageId = LanguageId(18);
#[cfg(feature = "lang-markdown")]
pub(crate) const MARKDOWN: LanguageId = LanguageId(19);

/// All grammars compiled into this build, in id order.
// The pushes are `#[cfg]`-gated per grammar, which `vec![]` cannot express.
#[allow(clippy::vec_init_then_push)]
pub(crate) fn all() -> &'static [GrammarInfo] {
    static REG: OnceLock<Vec<GrammarInfo>> = OnceLock::new();
    REG.get_or_init(|| {
        #[allow(unused_mut)]
        let mut v: Vec<GrammarInfo> = Vec::new();

        #[cfg(feature = "lang-rust")]
        v.push(GrammarInfo {
            id: RUST,
            name: "Rust",
            extensions: &["rs"],
            language: || tree_sitter_rust::LANGUAGE.into(),
            highlights: tree_sitter_rust::HIGHLIGHTS_QUERY,
        });
        #[cfg(feature = "lang-python")]
        v.push(GrammarInfo {
            id: PYTHON,
            name: "Python",
            extensions: &["py", "pyi"],
            language: || tree_sitter_python::LANGUAGE.into(),
            highlights: tree_sitter_python::HIGHLIGHTS_QUERY,
        });
        #[cfg(feature = "lang-javascript")]
        v.push(GrammarInfo {
            id: JAVASCRIPT,
            name: "JavaScript",
            extensions: &["js", "mjs", "cjs", "jsx"],
            language: || tree_sitter_javascript::LANGUAGE.into(),
            highlights: tree_sitter_javascript::HIGHLIGHT_QUERY, // singular
        });
        #[cfg(feature = "lang-typescript")]
        v.push(GrammarInfo {
            id: TYPESCRIPT,
            name: "TypeScript",
            extensions: &["ts", "mts", "cts"],
            language: || tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            highlights: tree_sitter_typescript::HIGHLIGHTS_QUERY,
        });
        #[cfg(feature = "lang-typescript")]
        v.push(GrammarInfo {
            id: TSX,
            name: "TSX",
            extensions: &["tsx"],
            language: || tree_sitter_typescript::LANGUAGE_TSX.into(),
            highlights: tree_sitter_typescript::HIGHLIGHTS_QUERY,
        });
        #[cfg(feature = "lang-json")]
        v.push(GrammarInfo {
            id: JSON,
            name: "JSON",
            extensions: &["json", "jsonc"],
            language: || tree_sitter_json::LANGUAGE.into(),
            highlights: tree_sitter_json::HIGHLIGHTS_QUERY,
        });
        #[cfg(feature = "lang-go")]
        v.push(GrammarInfo {
            id: GO,
            name: "Go",
            extensions: &["go"],
            language: || tree_sitter_go::LANGUAGE.into(),
            highlights: tree_sitter_go::HIGHLIGHTS_QUERY,
        });
        #[cfg(feature = "lang-c")]
        v.push(GrammarInfo {
            id: C,
            name: "C",
            extensions: &["c", "h"],
            language: || tree_sitter_c::LANGUAGE.into(),
            highlights: tree_sitter_c::HIGHLIGHT_QUERY, // singular
        });
        #[cfg(feature = "lang-cpp")]
        v.push(GrammarInfo {
            id: CPP,
            name: "C++",
            extensions: &["cc", "cpp", "cxx", "hpp", "hh", "hxx"],
            language: || tree_sitter_cpp::LANGUAGE.into(),
            highlights: tree_sitter_cpp::HIGHLIGHT_QUERY, // singular
        });
        #[cfg(feature = "lang-csharp")]
        v.push(GrammarInfo {
            id: CSHARP,
            name: "C#",
            extensions: &["cs"],
            language: || tree_sitter_c_sharp::LANGUAGE.into(),
            highlights: tree_sitter_c_sharp::HIGHLIGHTS_QUERY,
        });
        #[cfg(feature = "lang-java")]
        v.push(GrammarInfo {
            id: JAVA,
            name: "Java",
            extensions: &["java"],
            language: || tree_sitter_java::LANGUAGE.into(),
            highlights: tree_sitter_java::HIGHLIGHTS_QUERY,
        });
        #[cfg(feature = "lang-ruby")]
        v.push(GrammarInfo {
            id: RUBY,
            name: "Ruby",
            extensions: &["rb"],
            language: || tree_sitter_ruby::LANGUAGE.into(),
            highlights: tree_sitter_ruby::HIGHLIGHTS_QUERY,
        });
        #[cfg(feature = "lang-php")]
        v.push(GrammarInfo {
            id: PHP,
            name: "PHP",
            extensions: &["php"],
            language: || tree_sitter_php::LANGUAGE_PHP.into(),
            highlights: tree_sitter_php::HIGHLIGHTS_QUERY,
        });
        #[cfg(feature = "lang-bash")]
        v.push(GrammarInfo {
            id: BASH,
            name: "Bash",
            extensions: &["sh", "bash"],
            language: || tree_sitter_bash::LANGUAGE.into(),
            highlights: tree_sitter_bash::HIGHLIGHT_QUERY, // singular
        });
        #[cfg(feature = "lang-toml")]
        v.push(GrammarInfo {
            id: TOML,
            name: "TOML",
            extensions: &["toml"],
            language: || tree_sitter_toml_ng::LANGUAGE.into(),
            highlights: tree_sitter_toml_ng::HIGHLIGHTS_QUERY,
        });
        #[cfg(feature = "lang-html")]
        v.push(GrammarInfo {
            id: HTML,
            name: "HTML",
            extensions: &["html", "htm"],
            language: || tree_sitter_html::LANGUAGE.into(),
            highlights: tree_sitter_html::HIGHLIGHTS_QUERY,
        });
        #[cfg(feature = "lang-css")]
        v.push(GrammarInfo {
            id: CSS,
            name: "CSS",
            extensions: &["css"],
            language: || tree_sitter_css::LANGUAGE.into(),
            highlights: tree_sitter_css::HIGHLIGHTS_QUERY,
        });
        #[cfg(feature = "lang-yaml")]
        v.push(GrammarInfo {
            id: YAML,
            name: "YAML",
            extensions: &["yml", "yaml"],
            language: || tree_sitter_yaml::LANGUAGE.into(),
            highlights: tree_sitter_yaml::HIGHLIGHTS_QUERY,
        });
        // Markdown uses tree-sitter-md's *block* grammar + its block highlights
        // query (headings, code fences, list markers, …). The companion inline
        // grammar (emphasis/links via injection) is a future refinement.
        #[cfg(feature = "lang-markdown")]
        v.push(GrammarInfo {
            id: MARKDOWN,
            name: "Markdown",
            extensions: &["md", "markdown", "mdown", "mkd"],
            language: || tree_sitter_md::LANGUAGE.into(),
            highlights: tree_sitter_md::HIGHLIGHT_QUERY_BLOCK,
        });

        v
    })
}

/// The grammar registered under `id`, if compiled in.
pub(crate) fn grammar(id: LanguageId) -> Option<&'static GrammarInfo> {
    all().iter().find(|g| g.id == id)
}
