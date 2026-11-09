//! Web and template grammar entries.

#[cfg(any(feature = "lang-sass", feature = "lang-mdx"))]
use tree_sitter_language::LanguageFn;

use super::GrammarInfo;
#[allow(
    unused_imports,
    reason = "a build with none of these grammars compiles no constant to import"
)]
use crate::languages::ids::*;

#[cfg(feature = "lang-sass")]
unsafe extern "C" {
    fn tree_sitter_sass() -> *const ();
}

#[cfg(feature = "lang-sass")]
const SASS_LANGUAGE: LanguageFn = unsafe { LanguageFn::from_raw(tree_sitter_sass) };

#[cfg(feature = "lang-mdx")]
unsafe extern "C" {
    fn tree_sitter_mdx() -> *const ();
}

#[cfg(feature = "lang-mdx")]
const MDX_LANGUAGE: LanguageFn = unsafe { LanguageFn::from_raw(tree_sitter_mdx) };

/// GraphQL marker injections for JavaScript: a `/* GraphQL */` comment before
/// a template string, or a leading `#graphql` comment inside one. The tagged
/// ``gql`…` `` form is already covered by the grammar's bundled query.
#[cfg(feature = "lang-javascript")]
pub(super) const JS_GRAPHQL_MARKER_INJECTION: &str = r#"
((comment) @_graphql_marker
 .
 (template_string (string_fragment) @injection.content)
 (#any-of? @_graphql_marker "/* GraphQL */" "/* graphql */")
 (#set! injection.language "graphql")
 (#set! injection.combined)
 (#set! injection.include-children))

((template_string (string_fragment) @injection.content)
 (#match? @injection.content "^\\s*#graphql")
 (#set! injection.language "graphql")
 (#set! injection.combined)
 (#set! injection.include-children))
"#;

/// Injections for TypeScript/TSX, whose grammars ship no query of their own:
/// the tag-inferred template-literal pattern mirrored from
/// tree-sitter-javascript's bundled query, plus the same GraphQL markers as
/// [`JS_GRAPHQL_MARKER_INJECTION`] (duplicated because query sources are
/// `&'static str` — keep the marker patterns in sync).
#[cfg(feature = "lang-typescript")]
pub(super) const TS_INJECTION_EXTRA: &str = r#"
(call_expression
  function: [
    (identifier) @injection.language
    (member_expression
      property: (property_identifier) @injection.language)
  ]
  arguments: (template_string (string_fragment) @injection.content)
  (#set! injection.combined)
  (#set! injection.include-children))

((comment) @_graphql_marker
 .
 (template_string (string_fragment) @injection.content)
 (#any-of? @_graphql_marker "/* GraphQL */" "/* graphql */")
 (#set! injection.language "graphql")
 (#set! injection.combined)
 (#set! injection.include-children))

((template_string (string_fragment) @injection.content)
 (#match? @injection.content "^\\s*#graphql")
 (#set! injection.language "graphql")
 (#set! injection.combined)
 (#set! injection.include-children))
"#;

/// Parse non-PHP `text` ranges as one HTML layer. The upstream PHP query only
/// covers doc comments and heredocs, despite the grammar accepting mixed files.
#[cfg(feature = "lang-php")]
pub(super) const PHP_HTML_INJECTION: &str = r#"
((text) @injection.content
 (#set! injection.language "html")
 (#set! injection.combined))
"#;

// tree-sitter-vue-next 0.1 publishes its queries under `queries/vue/`, while
// its Rust binding currently looks only in `queries/`. Keep the small Karet
// query here until the crate exposes those constants correctly.
#[cfg(feature = "lang-vue")]
pub(super) const VUE_HIGHLIGHTS: &str = r#"
(comment) @comment
(doctype) @keyword
(tag_name) @tag
(attribute_name) @tag.attribute
(directive_name) @tag.attribute
(directive_value) @variable
(attribute_value) @string
(quoted_attribute_value) @string
["<" ">" "</" "/>" "="] @punctuation.delimiter
"#;

#[cfg(feature = "lang-vue")]
pub(super) const VUE_INJECTIONS: &str = r#"
((style_element
  (start_tag
    (attribute
      (attribute_name) @_attr
      (quoted_attribute_value (attribute_value) @injection.language)))
  (raw_text) @injection.content)
 (#eq? @_attr "lang"))
((script_element
  (start_tag
    (attribute
      (attribute_name) @_attr
      (quoted_attribute_value (attribute_value) @injection.language)))
  (raw_text) @injection.content)
 (#eq? @_attr "lang"))
((interpolation (raw_text) @injection.content)
 (#set! injection.language "typescript"))
"#;

// The pushes are `#[cfg]`-gated per grammar, which `vec![]` cannot express.
#[allow(clippy::vec_init_then_push)]
pub(super) fn push(grammars: &mut Vec<GrammarInfo>) {
    #[cfg(not(any(
        feature = "lang-scss",
        feature = "lang-sass",
        feature = "lang-less",
        feature = "lang-erb",
        feature = "lang-mdx"
    )))]
    let _ = grammars;

    #[cfg(feature = "lang-scss")]
    grammars.push(GrammarInfo {
        id: SCSS,
        name: "SCSS",
        extensions: &["scss"],
        names: &["scss"],
        language: tree_sitter_scss::language,
        highlights: tree_sitter_scss::HIGHLIGHTS_QUERY,
        injections: None,
        injections_extra: None,
        semantic: None,
        folds: None,
        indents: None,
    });
    #[cfg(feature = "lang-sass")]
    grammars.push(GrammarInfo {
        id: SASS,
        name: "Sass",
        extensions: &["sass"],
        names: &["sass"],
        language: || SASS_LANGUAGE.into(),
        highlights: include_str!("../../vendor/tree-sitter-sass/queries/highlights.scm"),
        injections: Some(include_str!(
            "../../vendor/tree-sitter-sass/queries/injections.scm"
        )),
        injections_extra: None,
        semantic: None,
        folds: None,
        indents: None,
    });
    #[cfg(feature = "lang-less")]
    grammars.push(GrammarInfo {
        id: LESS,
        name: "Less",
        extensions: &["less"],
        names: &["less"],
        language: tree_sitter_less::language,
        highlights: tree_sitter_less::HIGHLIGHTS_QUERY,
        injections: None,
        injections_extra: None,
        semantic: None,
        folds: None,
        indents: None,
    });
    #[cfg(feature = "lang-erb")]
    grammars.push(GrammarInfo {
        id: ERB,
        name: "ERB",
        extensions: &["erb"],
        names: &["erb", "embedded-template"],
        language: || tree_sitter_embedded_template::LANGUAGE.into(),
        highlights: tree_sitter_embedded_template::HIGHLIGHTS_QUERY,
        injections: Some(tree_sitter_embedded_template::INJECTIONS_ERB_QUERY),
        injections_extra: None,
        semantic: None,
        folds: None,
        indents: None,
    });
    #[cfg(feature = "lang-mdx")]
    grammars.push(GrammarInfo {
        id: MDX,
        name: "MDX",
        extensions: &["mdx"],
        names: &["mdx"],
        language: || MDX_LANGUAGE.into(),
        highlights: include_str!("../../vendor/tree-sitter-mdx/queries/highlights.scm"),
        injections: Some(include_str!(
            "../../vendor/tree-sitter-mdx/queries/injections.scm"
        )),
        injections_extra: None,
        semantic: None,
        folds: None,
        indents: None,
    });
}
