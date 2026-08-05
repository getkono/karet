//! Declaration queries for web and template languages.

use std::borrow::Cow;

use crate::LanguageId;

#[cfg(any(feature = "lang-scss", feature = "lang-less"))]
const CSS_LIKE: &str = r#"
(rule_set (selectors) @name) @definition.class
(media_statement) @name @definition.namespace
(supports_statement) @name @definition.namespace
(keyframes_statement (keyframes_name) @name) @definition.class
"#;

#[cfg(feature = "lang-scss")]
const SCSS: &str = r#"
(mixin_statement name: (identifier) @name) @definition.function
(function_statement name: (identifier) @name) @definition.function
"#;

#[cfg(feature = "lang-sass")]
const SASS: &str = r#"
(rule_set (selectors) @name) @definition.class
(media_statement) @name @definition.namespace
(supports_statement) @name @definition.namespace
(keyframes_statement name: (keyframes_name) @name) @definition.class
(mixin_statement (name) @name) @definition.function
(function_statement (name) @name) @definition.function
"#;

#[cfg(feature = "lang-less")]
const LESS: &str = r#"
(mixin_definition [(class_name) (id_name)] @name) @definition.function
"#;

#[cfg(any(feature = "lang-vue", feature = "lang-svelte"))]
const COMPONENT: &str = r#"
(script_element (start_tag (tag_name) @name)) @definition.module
(style_element (start_tag (tag_name) @name)) @definition.module
((element (start_tag (tag_name) @name)) @definition.object
 (#match? @name "^(main|nav|section|article|aside|header|footer|h[1-6])$"))
"#;

#[cfg(feature = "lang-vue")]
const VUE: &str = r#"
(template_element (start_tag (tag_name) @name)) @definition.module
"#;

#[cfg(feature = "lang-mdx")]
const MDX: &str = r#"
(atx_heading (atx_h1_marker) heading_content: (_) @name) @definition.heading.1
(atx_heading (atx_h2_marker) heading_content: (_) @name) @definition.heading.2
(atx_heading (atx_h3_marker) heading_content: (_) @name) @definition.heading.3
(atx_heading (atx_h4_marker) heading_content: (_) @name) @definition.heading.4
(atx_heading (atx_h5_marker) heading_content: (_) @name) @definition.heading.5
(atx_heading (atx_h6_marker) heading_content: (_) @name) @definition.heading.6
(setext_heading heading_content: (_) @name (setext_h1_underline)) @definition.heading.1
(setext_heading heading_content: (_) @name (setext_h2_underline)) @definition.heading.2
(class_declaration name: (identifier) @name) @definition.class
(function_declaration name: (identifier) @name) @definition.function
(lexical_declaration
  (variable_declarator
    name: (identifier) @name
    value: [(arrow_function) (function_expression)])) @definition.function
"#;

pub(super) fn query(_lang: LanguageId) -> Option<Cow<'static, str>> {
    #[cfg(feature = "lang-scss")]
    if _lang == super::super::web::SCSS {
        return Some(Cow::Owned(format!("{CSS_LIKE}\n{SCSS}")));
    }
    #[cfg(feature = "lang-sass")]
    if _lang == super::super::web::SASS {
        return Some(Cow::Borrowed(SASS));
    }
    #[cfg(feature = "lang-less")]
    if _lang == super::super::web::LESS {
        return Some(Cow::Owned(format!("{CSS_LIKE}\n{LESS}")));
    }
    #[cfg(feature = "lang-vue")]
    if _lang == super::super::VUE {
        return Some(Cow::Owned(format!("{COMPONENT}\n{VUE}")));
    }
    #[cfg(feature = "lang-svelte")]
    if _lang == super::super::SVELTE {
        return Some(Cow::Borrowed(COMPONENT));
    }
    #[cfg(feature = "lang-mdx")]
    if _lang == super::super::web::MDX {
        return Some(Cow::Borrowed(MDX));
    }
    None
}
