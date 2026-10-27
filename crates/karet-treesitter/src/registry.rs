//! The compile-time registry of bundled tree-sitter grammars.
//!
//! Each entry is wired by a `lang-*` cargo feature, so [`all`] returns exactly the
//! grammars compiled in. Each entry's id is its typed constant in
//! [`crate::languages`], whose numbering must **never** change once shipped.

use std::sync::OnceLock;

use crate::LanguageId;

mod outlines;
pub(crate) use outlines::outline_query;
pub(crate) mod programming;
mod semantic;
mod shells;
mod structured;
mod web;

/// The karet-authored semantic-block query for `lang`, if its grammar has one.
pub(crate) fn semantic_query(lang: LanguageId) -> Option<&'static str> {
    grammar(lang).and_then(|g| g.semantic)
}

/// Static metadata for one registered grammar.
pub(crate) struct GrammarInfo {
    /// The grammar's stable identifier.
    pub id: LanguageId,
    /// A human-readable display name (e.g. `"Rust"`).
    #[cfg_attr(not(test), allow(dead_code))] // consistency-test reference data
    pub name: &'static str,
    /// File extensions this grammar expects `karet-filetype` to route to it.
    /// Reference data for the cross-registry consistency test ONLY: runtime
    /// path→language routing goes exclusively through `karet-filetype`.
    #[cfg_attr(not(test), allow(dead_code))]
    pub extensions: &'static [&'static str],
    /// Lowercase names by which an injection query — or a markdown code-fence info
    /// string — may refer to this grammar (e.g. `rust`, `rs`). Distinct from
    /// [`extensions`](Self::extensions): a fence says ` ```sh `, not ` ```bash `.
    pub names: &'static [&'static str],
    /// Returns the tree-sitter `Language` for this grammar.
    pub language: fn() -> tree_sitter::Language,
    /// The grammar's highlights query source.
    pub highlights: &'static str,
    /// The grammar's bundled injections query source, if it ships one.
    pub injections: Option<&'static str>,
    /// karet-authored injection patterns appended to [`injections`](Self::injections).
    /// Kept in a separate field so the grammar's own query stays pristine and the
    /// delta we add is auditable in one place.
    pub injections_extra: Option<&'static str>,
    /// The karet-authored semantic-block query, if one ships for this grammar.
    pub semantic: Option<&'static str>,
    /// The karet-authored fold query (`queries/folds/*.scm`), if one ships.
    pub folds: Option<&'static str>,
    /// The karet-authored indent query (`queries/indents/*.scm`), if one ships.
    pub indents: Option<&'static str>,
}

// The stable ids live in [`crate::languages`], the published home of the
// numbering; re-exported here so the grammar tables below and the shell/outline
// submodules name them unqualified.
#[allow(
    unused_imports,
    reason = "a build with no `lang-*` feature compiles no constant to import"
)]
pub(crate) use crate::languages::*;
#[cfg(feature = "lang-graphql")]
const GRAPHQL_HIGHLIGHTS: &str = r#"
(comment) @comment
(string_value) @string
[(int_value) (float_value)] @number
[(boolean_value) (null_value)] @constant.builtin
[(object_type_definition) (interface_type_definition) (enum_type_definition)
 (input_object_type_definition) (union_type_definition) (scalar_type_definition)] @type
(operation_type) @keyword
[(field) (field_definition)] @property
(directive) @attribute
"#;

#[cfg(feature = "lang-protobuf")]
const PROTOBUF_HIGHLIGHTS: &str = r#"
[
  "syntax" "edition" "package" "option" "import" "service" "rpc" "returns"
  "message" "enum" "oneof" "repeated" "reserved" "to"
] @keyword
[(key_type) (type) (message_name) (enum_name) (service_name) (rpc_name)] @type
(string) @string
[(int_lit) (float_lit)] @number
[(true) (false)] @constant.builtin
(comment) @comment
["(" ")" "[" "]" "{" "}"] @punctuation.bracket
"#;

#[cfg(feature = "lang-latex")]
const LATEX_HIGHLIGHTS: &str = r#"
[(line_comment) (block_comment) (comment) (comment_environment)] @comment
(todo) @comment.mark
[(part) (chapter) (section) (subsection) (subsubsection) (paragraph) (subparagraph)] @markup.heading
[(inline_formula) (displayed_equation) (math_environment)] @markup.raw
[(generic_command) (begin) (end) (class_include) (package_include)] @function
[(citation) (label_definition) (label_reference) (label_reference_range)] @variable
[(hyperlink) (curly_group_uri)] @markup.link
[(curly_group_path) (curly_group_path_list) (glob_pattern)] @string
[(operator) (math_delimiter)] @operator
[(subscript) (superscript)] @punctuation.special
"#;

#[cfg(feature = "lang-rst")]
const RST_HIGHLIGHTS: &str = r#"
(title) @markup.heading
[(emphasis) (strong)] @markup.strong
[(literal) (literal_block)] @markup.raw
[(link) (standalone_hyperlink)] @markup.link
(comment) @comment
"#;

#[cfg(feature = "lang-asciidoc")]
const ASCIIDOC_HIGHLIGHTS: &str = r#"
[(document_title) (title1) (title2) (title3) (title4) (title5)] @markup.heading
[(line_comment) (block_comment)] @comment
(email) @markup.link.url
(block_macro (target) @markup.link)
[(listing_block) (literal_block) (ident_block)] @markup.raw.block
"#;

/// karet's own addition to Rust's injections query: a doc comment is markdown.
///
/// tree-sitter-rust ships injections only for macro token trees, so `///` and `//!`
/// bodies would otherwise render as flat comment text. The `doc:` field yields the
/// comment's *content*, with the `///` marker excluded — exactly the markdown source
/// rustdoc sees.
///
/// `injection.combined` is essential rather than an optimization: each `///` line is
/// its own `line_comment` node, so without combining them into a single markdown
/// parse a fenced ` ```rust ` block could never span the lines it always spans. Once
/// combined, markdown's own fence injection recursively lights up the doctest as Rust.
#[cfg(feature = "lang-rust")]
const RUST_DOC_COMMENT_INJECTION: &str = r#"
((line_comment doc: (doc_comment) @injection.content)
 (#set! injection.language "markdown")
 (#set! injection.combined))

((block_comment doc: (doc_comment) @injection.content)
 (#set! injection.language "markdown")
 (#set! injection.combined))
"#;

/// All grammars compiled into this build, in registration order (the order of the
/// pushes below), which is not id order: `MARKDOWN_INLINE` is registered last.
/// Alias lookups that take the first match depend on this order.
// The pushes are `#[cfg]`-gated per grammar, which `vec![]` cannot express.
#[allow(clippy::vec_init_then_push)]
pub(crate) fn all() -> &'static [GrammarInfo] {
    static REG: OnceLock<Vec<GrammarInfo>> = OnceLock::new();
    REG.get_or_init(|| {
        // With no `lang-*` feature enabled nothing pushes, and `mut` goes unused.
        #[allow(unused_mut)]
        let mut v: Vec<GrammarInfo> = Vec::new();

        #[cfg(feature = "lang-rust")]
        v.push(GrammarInfo {
            id: RUST,
            name: "Rust",
            extensions: &["rs"],
            names: &["rust", "rs"],
            language: || tree_sitter_rust::LANGUAGE.into(),
            highlights: tree_sitter_rust::HIGHLIGHTS_QUERY,
            injections: Some(tree_sitter_rust::INJECTIONS_QUERY),
            injections_extra: Some(RUST_DOC_COMMENT_INJECTION),
            semantic: Some(semantic::RUST),
            folds: Some(include_str!("../queries/folds/rust.scm")),
            indents: Some(include_str!("../queries/indents/rust.scm")),
        });
        #[cfg(feature = "lang-python")]
        v.push(GrammarInfo {
            id: PYTHON,
            name: "Python",
            extensions: &["py", "pyi"],
            names: &["python", "py"],
            language: || tree_sitter_python::LANGUAGE.into(),
            highlights: tree_sitter_python::HIGHLIGHTS_QUERY,
            injections: None,
            injections_extra: None,
            semantic: Some(semantic::PYTHON),
            folds: Some(include_str!("../queries/folds/python.scm")),
            indents: Some(include_str!("../queries/indents/python.scm")),
        });
        #[cfg(feature = "lang-javascript")]
        v.push(GrammarInfo {
            id: JAVASCRIPT,
            name: "JavaScript",
            extensions: &["js", "mjs", "cjs", "jsx"],
            names: &["javascript", "js", "jsx", "mjs", "cjs"],
            language: || tree_sitter_javascript::LANGUAGE.into(),
            highlights: tree_sitter_javascript::HIGHLIGHT_QUERY, // singular
            injections: Some(tree_sitter_javascript::INJECTIONS_QUERY),
            injections_extra: Some(web::JS_GRAPHQL_MARKER_INJECTION),
            semantic: Some(semantic::JAVASCRIPT),
            folds: Some(include_str!("../queries/folds/javascript.scm")),
            indents: Some(include_str!("../queries/indents/javascript.scm")),
        });
        #[cfg(feature = "lang-typescript")]
        v.push(GrammarInfo {
            id: TYPESCRIPT,
            name: "TypeScript",
            extensions: &["ts", "mts", "cts"],
            names: &["typescript", "ts"],
            language: || tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            highlights: tree_sitter_typescript::HIGHLIGHTS_QUERY,
            injections: None,
            injections_extra: Some(web::TS_INJECTION_EXTRA),
            semantic: Some(semantic::TYPESCRIPT),
            folds: Some(include_str!("../queries/folds/typescript.scm")),
            indents: Some(include_str!("../queries/indents/typescript.scm")),
        });
        #[cfg(feature = "lang-typescript")]
        v.push(GrammarInfo {
            id: TSX,
            name: "TSX",
            extensions: &["tsx"],
            names: &["tsx"],
            language: || tree_sitter_typescript::LANGUAGE_TSX.into(),
            highlights: tree_sitter_typescript::HIGHLIGHTS_QUERY,
            injections: None,
            injections_extra: Some(web::TS_INJECTION_EXTRA),
            semantic: Some(semantic::TYPESCRIPT),
            folds: Some(include_str!("../queries/folds/typescript.scm")),
            indents: Some(include_str!("../queries/indents/typescript.scm")),
        });
        #[cfg(feature = "lang-json")]
        v.push(GrammarInfo {
            id: JSON,
            name: "JSON",
            extensions: &["json", "jsonc"],
            names: &["json", "jsonc"],
            language: || tree_sitter_json::LANGUAGE.into(),
            highlights: tree_sitter_json::HIGHLIGHTS_QUERY,
            injections: None,
            injections_extra: None,
            semantic: Some(semantic::JSON),
            folds: Some(include_str!("../queries/folds/json.scm")),
            indents: Some(include_str!("../queries/indents/json.scm")),
        });
        #[cfg(feature = "lang-go")]
        v.push(GrammarInfo {
            id: GO,
            name: "Go",
            extensions: &["go"],
            names: &["go", "golang"],
            language: || tree_sitter_go::LANGUAGE.into(),
            highlights: tree_sitter_go::HIGHLIGHTS_QUERY,
            injections: None,
            injections_extra: None,
            semantic: Some(semantic::GO),
            folds: Some(include_str!("../queries/folds/go.scm")),
            indents: Some(include_str!("../queries/indents/go.scm")),
        });
        #[cfg(feature = "lang-c")]
        v.push(GrammarInfo {
            id: C,
            name: "C",
            extensions: &["c", "h"],
            names: &["c"],
            language: || tree_sitter_c::LANGUAGE.into(),
            highlights: tree_sitter_c::HIGHLIGHT_QUERY, // singular
            injections: None,
            injections_extra: None,
            semantic: Some(semantic::C),
            folds: Some(include_str!("../queries/folds/c.scm")),
            indents: Some(include_str!("../queries/indents/c.scm")),
        });
        #[cfg(feature = "lang-cpp")]
        v.push(GrammarInfo {
            id: CPP,
            name: "C++",
            extensions: &["cc", "cpp", "cxx", "hpp", "hh", "hxx"],
            names: &["cpp", "c++", "cxx", "cc"],
            language: || tree_sitter_cpp::LANGUAGE.into(),
            highlights: tree_sitter_cpp::HIGHLIGHT_QUERY, // singular
            injections: None,
            injections_extra: None,
            semantic: Some(semantic::CPP),
            folds: Some(include_str!("../queries/folds/cpp.scm")),
            indents: Some(include_str!("../queries/indents/cpp.scm")),
        });
        #[cfg(feature = "lang-csharp")]
        v.push(GrammarInfo {
            id: CSHARP,
            name: "C#",
            extensions: &["cs"],
            names: &["csharp", "c_sharp", "c#", "cs"],
            language: || tree_sitter_c_sharp::LANGUAGE.into(),
            highlights: tree_sitter_c_sharp::HIGHLIGHTS_QUERY,
            // tree-sitter-c-sharp declares `INJECTIONS_QUERY` behind a build-script cfg
            // but ships no `queries/injections.scm`, so the constant is gated out.
            injections: None,
            injections_extra: None,
            semantic: Some(semantic::CSHARP),
            folds: None,
            indents: None,
        });
        #[cfg(feature = "lang-java")]
        v.push(GrammarInfo {
            id: JAVA,
            name: "Java",
            extensions: &["java"],
            names: &["java"],
            language: || tree_sitter_java::LANGUAGE.into(),
            highlights: tree_sitter_java::HIGHLIGHTS_QUERY,
            injections: None,
            injections_extra: None,
            semantic: Some(semantic::JAVA),
            folds: None,
            indents: None,
        });
        #[cfg(feature = "lang-ruby")]
        v.push(GrammarInfo {
            id: RUBY,
            name: "Ruby",
            extensions: &["rb"],
            names: &["ruby", "rb"],
            language: || tree_sitter_ruby::LANGUAGE.into(),
            highlights: tree_sitter_ruby::HIGHLIGHTS_QUERY,
            injections: None,
            injections_extra: None,
            semantic: Some(semantic::RUBY),
            folds: None,
            indents: None,
        });
        #[cfg(feature = "lang-php")]
        v.push(GrammarInfo {
            id: PHP,
            name: "PHP",
            extensions: &["php"],
            names: &["php"],
            language: || tree_sitter_php::LANGUAGE_PHP.into(),
            highlights: tree_sitter_php::HIGHLIGHTS_QUERY,
            injections: Some(tree_sitter_php::INJECTIONS_QUERY),
            injections_extra: Some(web::PHP_HTML_INJECTION),
            semantic: Some(semantic::PHP),
            folds: None,
            indents: None,
        });
        #[cfg(feature = "lang-bash")]
        v.push(GrammarInfo {
            id: BASH,
            name: "Bash",
            extensions: &["sh", "bash"],
            // NOTE: no "zsh" here — the Zsh grammar owns that name; a name must
            // belong to exactly one grammar or resolution depends on table order
            // (the consistency test in detect.rs enforces uniqueness).
            names: &["bash", "sh", "shell", "console"],
            language: || tree_sitter_bash::LANGUAGE.into(),
            highlights: tree_sitter_bash::HIGHLIGHT_QUERY, // singular
            injections: None,
            injections_extra: None,
            semantic: Some(semantic::BASH),
            folds: None,
            indents: None,
        });
        #[cfg(feature = "lang-toml")]
        v.push(GrammarInfo {
            id: TOML,
            name: "TOML",
            extensions: &["toml"],
            names: &["toml"],
            language: || tree_sitter_toml_ng::LANGUAGE.into(),
            highlights: tree_sitter_toml_ng::HIGHLIGHTS_QUERY,
            injections: None,
            injections_extra: None,
            semantic: Some(semantic::TOML),
            folds: None,
            indents: None,
        });
        #[cfg(feature = "lang-html")]
        v.push(GrammarInfo {
            id: HTML,
            name: "HTML",
            extensions: &["html", "htm"],
            names: &["html", "htm"],
            language: || tree_sitter_html::LANGUAGE.into(),
            highlights: tree_sitter_html::HIGHLIGHTS_QUERY,
            injections: Some(tree_sitter_html::INJECTIONS_QUERY),
            injections_extra: None,
            semantic: Some(semantic::HTML),
            folds: None,
            indents: None,
        });
        #[cfg(feature = "lang-css")]
        v.push(GrammarInfo {
            id: CSS,
            name: "CSS",
            extensions: &["css"],
            names: &["css"],
            language: || tree_sitter_css::LANGUAGE.into(),
            highlights: tree_sitter_css::HIGHLIGHTS_QUERY,
            injections: None,
            injections_extra: None,
            semantic: Some(semantic::CSS),
            folds: None,
            indents: None,
        });
        #[cfg(feature = "lang-yaml")]
        v.push(GrammarInfo {
            id: YAML,
            name: "YAML",
            extensions: &["yml", "yaml"],
            names: &["yaml", "yml"],
            language: || tree_sitter_yaml::LANGUAGE.into(),
            highlights: tree_sitter_yaml::HIGHLIGHTS_QUERY,
            injections: None,
            injections_extra: None,
            semantic: Some(semantic::YAML),
            folds: Some(include_str!("../queries/folds/yaml.scm")),
            indents: Some(include_str!("../queries/indents/yaml.scm")),
        });
        // Markdown is two grammars. The *block* grammar is the entry point (headings,
        // fences, list markers); its injections query hands `(inline)` nodes to the
        // companion inline grammar and each fence's content to the language its info
        // string names — so emphasis, links and embedded code all arrive as layers.
        #[cfg(feature = "lang-markdown")]
        v.push(GrammarInfo {
            id: MARKDOWN,
            name: "Markdown",
            extensions: &["md", "markdown", "mdown", "mkd"],
            names: &["markdown", "md"],
            language: || tree_sitter_md::LANGUAGE.into(),
            highlights: tree_sitter_md::HIGHLIGHT_QUERY_BLOCK,
            injections: Some(tree_sitter_md::INJECTION_QUERY_BLOCK),
            injections_extra: None,
            semantic: Some(semantic::MARKDOWN),
            folds: None,
            indents: None,
        });
        #[cfg(feature = "lang-latex")]
        v.push(GrammarInfo {
            id: LATEX,
            name: "TeX",
            extensions: &["tex", "sty", "cls"],
            names: &["tex", "latex"],
            language: || codebook_tree_sitter_latex::LANGUAGE.into(),
            highlights: LATEX_HIGHLIGHTS,
            injections: None,
            injections_extra: None,
            semantic: Some(semantic::LATEX),
            folds: None,
            indents: None,
        });
        #[cfg(feature = "lang-zig")]
        v.push(GrammarInfo {
            id: ZIG,
            name: "Zig",
            extensions: &["zig"],
            names: &["zig"],
            language: || tree_sitter_zig::LANGUAGE.into(),
            highlights: tree_sitter_zig::HIGHLIGHTS_QUERY,
            injections: Some(tree_sitter_zig::INJECTIONS_QUERY),
            injections_extra: None,
            semantic: None,
            folds: None,
            indents: None,
        });
        #[cfg(feature = "lang-xml")]
        v.push(GrammarInfo {
            id: XML,
            name: "XML",
            extensions: &["xml", "svg"],
            names: &["xml", "svg"],
            language: || tree_sitter_xml::LANGUAGE_XML.into(),
            highlights: tree_sitter_xml::XML_HIGHLIGHT_QUERY,
            injections: None,
            injections_extra: None,
            semantic: None,
            folds: None,
            indents: None,
        });
        #[cfg(feature = "lang-svelte")]
        v.push(GrammarInfo {
            id: SVELTE,
            name: "Svelte",
            extensions: &["svelte"],
            names: &["svelte"],
            language: || tree_sitter_svelte_next::LANGUAGE.into(),
            highlights: tree_sitter_svelte_next::HIGHLIGHTS_QUERY,
            injections: Some(tree_sitter_svelte_next::INJECTIONS_QUERY),
            injections_extra: None,
            semantic: None,
            folds: None,
            indents: None,
        });
        #[cfg(feature = "lang-astro")]
        v.push(GrammarInfo {
            id: ASTRO,
            name: "Astro",
            extensions: &["astro"],
            names: &["astro"],
            language: || tree_sitter_astro_next::LANGUAGE.into(),
            highlights: tree_sitter_astro_next::HIGHLIGHTS_QUERY,
            injections: Some(tree_sitter_astro_next::INJECTIONS_QUERY),
            injections_extra: None,
            semantic: None,
            folds: None,
            indents: None,
        });
        #[cfg(feature = "lang-vue")]
        v.push(GrammarInfo {
            id: VUE,
            name: "Vue",
            extensions: &["vue"],
            names: &["vue"],
            language: || tree_sitter_vue_next::LANGUAGE.into(),
            highlights: web::VUE_HIGHLIGHTS,
            injections: Some(web::VUE_INJECTIONS),
            injections_extra: None,
            semantic: None,
            folds: None,
            indents: None,
        });
        #[cfg(feature = "lang-sql")]
        v.push(GrammarInfo {
            id: SQL,
            name: "SQL",
            extensions: &["sql"],
            names: &["sql"],
            language: || tree_sitter_sequel::LANGUAGE.into(),
            highlights: tree_sitter_sequel::HIGHLIGHTS_QUERY,
            injections: None,
            injections_extra: None,
            semantic: None,
            folds: None,
            indents: None,
        });
        #[cfg(feature = "lang-graphql")]
        v.push(GrammarInfo {
            id: GRAPHQL,
            name: "GraphQL",
            extensions: &["graphql", "gql", "graphqls"],
            names: &["graphql", "gql"],
            language: || tree_sitter_graphql::LANGUAGE.into(),
            highlights: GRAPHQL_HIGHLIGHTS,
            injections: None,
            injections_extra: None,
            semantic: None,
            folds: None,
            indents: None,
        });
        #[cfg(feature = "lang-protobuf")]
        v.push(GrammarInfo {
            id: PROTOBUF,
            name: "Protobuf",
            extensions: &["proto"],
            names: &["protobuf", "proto"],
            language: || tree_sitter_proto::LANGUAGE.into(),
            highlights: PROTOBUF_HIGHLIGHTS,
            injections: None,
            injections_extra: None,
            semantic: None,
            folds: None,
            indents: None,
        });
        #[cfg(feature = "lang-containerfile")]
        v.push(GrammarInfo {
            id: CONTAINERFILE,
            name: "Dockerfile",
            extensions: &[],
            names: &["dockerfile", "containerfile", "docker"],
            language: || tree_sitter_containerfile::LANGUAGE.into(),
            highlights: tree_sitter_containerfile::HIGHLIGHTS_QUERY,
            injections: Some(tree_sitter_containerfile::INJECTIONS_QUERY),
            injections_extra: None,
            semantic: None,
            folds: None,
            indents: None,
        });
        #[cfg(feature = "lang-make")]
        v.push(GrammarInfo {
            id: MAKE,
            name: "Makefile",
            extensions: &["mk"],
            names: &["make", "makefile"],
            language: || tree_sitter_make::LANGUAGE.into(),
            highlights: tree_sitter_make::HIGHLIGHTS_QUERY,
            injections: None,
            injections_extra: None,
            semantic: None,
            folds: None,
            indents: None,
        });
        #[cfg(feature = "lang-cmake")]
        v.push(GrammarInfo {
            id: CMAKE,
            name: "CMake",
            extensions: &["cmake"],
            names: &["cmake"],
            language: || tree_sitter_cmake::LANGUAGE.into(),
            highlights: tree_sitter_cmake::HIGHLIGHTS_QUERY,
            injections: None,
            injections_extra: None,
            semantic: None,
            folds: None,
            indents: None,
        });
        #[cfg(feature = "lang-rst")]
        v.push(GrammarInfo {
            id: RST,
            name: "reStructuredText",
            extensions: &["rst"],
            names: &["restructuredtext", "rst"],
            language: || tree_sitter_rst::LANGUAGE.into(),
            highlights: RST_HIGHLIGHTS,
            injections: None,
            injections_extra: None,
            semantic: None,
            folds: None,
            indents: None,
        });
        #[cfg(feature = "lang-asciidoc")]
        v.push(GrammarInfo {
            id: ASCIIDOC,
            name: "AsciiDoc",
            extensions: &["adoc", "asciidoc"],
            names: &["asciidoc", "adoc"],
            language: tree_sitter_asciidoc::language,
            highlights: ASCIIDOC_HIGHLIGHTS,
            injections: None,
            injections_extra: None,
            semantic: None,
            folds: None,
            indents: None,
        });
        shells::push(&mut v);
        programming::push(&mut v);
        structured::push(&mut v);
        web::push(&mut v);
        #[cfg(feature = "lang-markdown")]
        v.push(GrammarInfo {
            id: MARKDOWN_INLINE,
            name: "Markdown (inline)",
            // Never resolved from a path: reached only via the block grammar's
            // `markdown_inline` injection.
            extensions: &[],
            names: &["markdown_inline", "markdown-inline"],
            language: || tree_sitter_md::INLINE_LANGUAGE.into(),
            highlights: tree_sitter_md::HIGHLIGHT_QUERY_INLINE,
            injections: Some(tree_sitter_md::INJECTION_QUERY_INLINE),
            injections_extra: None,
            semantic: None,
            folds: None,
            indents: None,
        });

        v
    })
}

/// The grammar registered under `id`, if compiled in.
pub(crate) fn grammar(id: LanguageId) -> Option<&'static GrammarInfo> {
    all().iter().find(|g| g.id == id)
}
