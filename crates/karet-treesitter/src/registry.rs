//! The compile-time registry of bundled tree-sitter grammars.
//!
//! Each entry is wired by a `lang-*` cargo feature, so [`all`] returns exactly the
//! grammars compiled in. [`LanguageId`] values are stable and must **never** be
//! renumbered once shipped.

use std::sync::OnceLock;

use crate::LanguageId;

#[cfg(feature = "lang-rust")]
const RUST_SEMANTIC: &str = r#"
(function_item (block) @semantic.body) @semantic.scope
(struct_item (field_declaration_list) @semantic.body) @semantic.scope
(enum_item (enum_variant_list) @semantic.body) @semantic.scope
(union_item (field_declaration_list) @semantic.body) @semantic.scope
(trait_item (declaration_list) @semantic.body) @semantic.scope
(impl_item (declaration_list) @semantic.body) @semantic.scope
(mod_item (declaration_list) @semantic.body) @semantic.scope
"#;

#[cfg(feature = "lang-python")]
const PYTHON_SEMANTIC: &str = r#"
(class_definition body: (block) @semantic.body) @semantic.scope
(function_definition body: (block) @semantic.body) @semantic.scope
"#;

#[cfg(feature = "lang-javascript")]
const JAVASCRIPT_SEMANTIC: &str = r#"
(class_declaration body: (class_body) @semantic.body) @semantic.scope
(class body: (class_body) @semantic.body) @semantic.scope
(function_declaration body: (statement_block) @semantic.body) @semantic.scope
(generator_function_declaration body: (statement_block) @semantic.body) @semantic.scope
(method_definition body: (statement_block) @semantic.body) @semantic.scope
"#;

#[cfg(feature = "lang-typescript")]
const TYPESCRIPT_SEMANTIC: &str = r#"
(class_declaration body: (class_body) @semantic.body) @semantic.scope
(abstract_class_declaration body: (class_body) @semantic.body) @semantic.scope
(function_declaration body: (statement_block) @semantic.body) @semantic.scope
(generator_function_declaration body: (statement_block) @semantic.body) @semantic.scope
(method_definition body: (statement_block) @semantic.body) @semantic.scope
(interface_declaration body: (interface_body) @semantic.body) @semantic.scope
"#;

#[cfg(feature = "lang-json")]
const JSON_SEMANTIC: &str = r#"
(pair key: (string) @semantic.header value: [(object) (array)] @semantic.body) @semantic.scope
"#;

#[cfg(feature = "lang-go")]
const GO_SEMANTIC: &str = r#"
(function_declaration body: (block) @semantic.body) @semantic.scope
(method_declaration body: (block) @semantic.body) @semantic.scope
(type_declaration (type_spec type: [(struct_type) (interface_type)] @semantic.body)) @semantic.scope
"#;

#[cfg(feature = "lang-c")]
const C_SEMANTIC: &str = r#"
(function_definition body: (compound_statement) @semantic.body) @semantic.scope
(struct_specifier body: (field_declaration_list) @semantic.body) @semantic.scope
(union_specifier body: (field_declaration_list) @semantic.body) @semantic.scope
(enum_specifier body: (enumerator_list) @semantic.body) @semantic.scope
"#;

#[cfg(feature = "lang-cpp")]
const CPP_SEMANTIC: &str = r#"
(function_definition body: (compound_statement) @semantic.body) @semantic.scope
(class_specifier body: (field_declaration_list) @semantic.body) @semantic.scope
(struct_specifier body: (field_declaration_list) @semantic.body) @semantic.scope
(namespace_definition body: (declaration_list) @semantic.body) @semantic.scope
(enum_specifier body: (enumerator_list) @semantic.body) @semantic.scope
"#;

#[cfg(feature = "lang-csharp")]
const CSHARP_SEMANTIC: &str = r#"
(namespace_declaration body: (declaration_list) @semantic.body) @semantic.scope
(class_declaration body: (declaration_list) @semantic.body) @semantic.scope
(interface_declaration body: (declaration_list) @semantic.body) @semantic.scope
(struct_declaration body: (declaration_list) @semantic.body) @semantic.scope
(record_declaration body: (declaration_list) @semantic.body) @semantic.scope
(method_declaration body: (block) @semantic.body) @semantic.scope
(constructor_declaration body: (block) @semantic.body) @semantic.scope
"#;

#[cfg(feature = "lang-java")]
const JAVA_SEMANTIC: &str = r#"
(class_declaration body: (class_body) @semantic.body) @semantic.scope
(interface_declaration body: (interface_body) @semantic.body) @semantic.scope
(enum_declaration body: (enum_body) @semantic.body) @semantic.scope
(record_declaration body: (class_body) @semantic.body) @semantic.scope
(method_declaration body: (block) @semantic.body) @semantic.scope
(constructor_declaration body: (constructor_body) @semantic.body) @semantic.scope
"#;

#[cfg(feature = "lang-ruby")]
const RUBY_SEMANTIC: &str = r#"
(class) @semantic.scope
(singleton_class) @semantic.scope
(module) @semantic.scope
(method) @semantic.scope
(singleton_method) @semantic.scope
"#;

#[cfg(feature = "lang-php")]
const PHP_SEMANTIC: &str = r#"
(namespace_definition body: (compound_statement) @semantic.body) @semantic.scope
(class_declaration body: (declaration_list) @semantic.body) @semantic.scope
(interface_declaration body: (declaration_list) @semantic.body) @semantic.scope
(trait_declaration body: (declaration_list) @semantic.body) @semantic.scope
(function_definition body: (compound_statement) @semantic.body) @semantic.scope
(method_declaration body: (compound_statement) @semantic.body) @semantic.scope
"#;

#[cfg(feature = "lang-bash")]
const BASH_SEMANTIC: &str = r#"
(function_definition body: (_) @semantic.body) @semantic.scope
(if_statement) @semantic.scope
(for_statement) @semantic.scope
(c_style_for_statement) @semantic.scope
(while_statement) @semantic.scope
(case_statement) @semantic.scope
"#;

#[cfg(feature = "lang-toml")]
const TOML_SEMANTIC: &str = r#"
(table) @semantic.scope
(table_array_element) @semantic.scope
"#;

#[cfg(feature = "lang-html")]
const HTML_SEMANTIC: &str = r#"
(element (start_tag) @semantic.header) @semantic.scope
(script_element (start_tag) @semantic.header) @semantic.scope
(style_element (start_tag) @semantic.header) @semantic.scope
"#;

#[cfg(feature = "lang-css")]
const CSS_SEMANTIC: &str = r#"
(rule_set (selectors) @semantic.header (block) @semantic.body) @semantic.scope
(media_statement (block) @semantic.body) @semantic.scope
(supports_statement (block) @semantic.body) @semantic.scope
(keyframes_statement (keyframe_block_list) @semantic.body) @semantic.scope
"#;

#[cfg(feature = "lang-yaml")]
const YAML_SEMANTIC: &str = r#"
(block_mapping_pair
  key: (_) @semantic.header
  value: (block_node [(block_mapping) (block_sequence)] @semantic.body)) @semantic.scope
"#;

#[cfg(feature = "lang-markdown")]
const MARKDOWN_SEMANTIC: &str = r#"
(atx_heading (atx_h1_marker)) @semantic.heading.1
(atx_heading (atx_h2_marker)) @semantic.heading.2
(atx_heading (atx_h3_marker)) @semantic.heading.3
(atx_heading (atx_h4_marker)) @semantic.heading.4
(atx_heading (atx_h5_marker)) @semantic.heading.5
(atx_heading (atx_h6_marker)) @semantic.heading.6
(setext_heading (setext_h1_underline)) @semantic.heading.1
(setext_heading (setext_h2_underline)) @semantic.heading.2
"#;

pub(crate) fn semantic_query(_lang: LanguageId) -> Option<&'static str> {
    #[cfg(feature = "lang-rust")]
    if _lang == RUST {
        return Some(RUST_SEMANTIC);
    }
    #[cfg(feature = "lang-python")]
    if _lang == PYTHON {
        return Some(PYTHON_SEMANTIC);
    }
    #[cfg(feature = "lang-javascript")]
    if _lang == JAVASCRIPT {
        return Some(JAVASCRIPT_SEMANTIC);
    }
    #[cfg(feature = "lang-typescript")]
    if _lang == TYPESCRIPT || _lang == TSX {
        return Some(TYPESCRIPT_SEMANTIC);
    }
    #[cfg(feature = "lang-json")]
    if _lang == JSON {
        return Some(JSON_SEMANTIC);
    }
    #[cfg(feature = "lang-go")]
    if _lang == GO {
        return Some(GO_SEMANTIC);
    }
    #[cfg(feature = "lang-c")]
    if _lang == C {
        return Some(C_SEMANTIC);
    }
    #[cfg(feature = "lang-cpp")]
    if _lang == CPP {
        return Some(CPP_SEMANTIC);
    }
    #[cfg(feature = "lang-csharp")]
    if _lang == CSHARP {
        return Some(CSHARP_SEMANTIC);
    }
    #[cfg(feature = "lang-java")]
    if _lang == JAVA {
        return Some(JAVA_SEMANTIC);
    }
    #[cfg(feature = "lang-ruby")]
    if _lang == RUBY {
        return Some(RUBY_SEMANTIC);
    }
    #[cfg(feature = "lang-php")]
    if _lang == PHP {
        return Some(PHP_SEMANTIC);
    }
    #[cfg(feature = "lang-bash")]
    if _lang == BASH {
        return Some(BASH_SEMANTIC);
    }
    #[cfg(feature = "lang-toml")]
    if _lang == TOML {
        return Some(TOML_SEMANTIC);
    }
    #[cfg(feature = "lang-html")]
    if _lang == HTML {
        return Some(HTML_SEMANTIC);
    }
    #[cfg(feature = "lang-css")]
    if _lang == CSS {
        return Some(CSS_SEMANTIC);
    }
    #[cfg(feature = "lang-yaml")]
    if _lang == YAML {
        return Some(YAML_SEMANTIC);
    }
    #[cfg(feature = "lang-markdown")]
    if _lang == MARKDOWN {
        return Some(MARKDOWN_SEMANTIC);
    }
    #[cfg(feature = "lang-latex")]
    if _lang == LATEX {
        return Some(LATEX_SEMANTIC);
    }
    None
}

/// Static metadata for one registered grammar.
pub(crate) struct GrammarInfo {
    /// The grammar's stable identifier.
    pub id: LanguageId,
    /// A human-readable display name (e.g. `"Rust"`).
    pub name: &'static str,
    /// File extensions (lowercase, without the dot) handled by this grammar.
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
/// tree-sitter-md's companion *inline* grammar. Never resolved from a path — the
/// block grammar reaches it only through an `markdown_inline` injection.
#[cfg(feature = "lang-markdown")]
pub(crate) const MARKDOWN_INLINE: LanguageId = LanguageId(20);
#[cfg(feature = "lang-latex")]
pub(crate) const LATEX: LanguageId = LanguageId(21);

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

#[cfg(feature = "lang-latex")]
const LATEX_SEMANTIC: &str = r#"
[(part) (chapter) (section) (subsection) (subsubsection)] @semantic.scope
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
            names: &["rust", "rs"],
            language: || tree_sitter_rust::LANGUAGE.into(),
            highlights: tree_sitter_rust::HIGHLIGHTS_QUERY,
            injections: Some(tree_sitter_rust::INJECTIONS_QUERY),
            injections_extra: Some(RUST_DOC_COMMENT_INJECTION),
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
            injections_extra: None,
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
            injections_extra: None,
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
            injections_extra: None,
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
            injections_extra: None,
        });
        #[cfg(feature = "lang-bash")]
        v.push(GrammarInfo {
            id: BASH,
            name: "Bash",
            extensions: &["sh", "bash"],
            names: &["bash", "sh", "shell", "zsh", "console"],
            language: || tree_sitter_bash::LANGUAGE.into(),
            highlights: tree_sitter_bash::HIGHLIGHT_QUERY, // singular
            injections: None,
            injections_extra: None,
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
        });
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
        });

        v
    })
}

/// The grammar registered under `id`, if compiled in.
pub(crate) fn grammar(id: LanguageId) -> Option<&'static GrammarInfo> {
    all().iter().find(|g| g.id == id)
}
