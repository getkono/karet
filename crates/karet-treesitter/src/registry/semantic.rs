//! The karet-authored semantic-block queries, each wired into its grammar's
//! [`GrammarInfo::semantic`](super::GrammarInfo::semantic).

#[cfg(feature = "lang-rust")]
pub(super) const RUST: &str = r#"
(function_item (block) @semantic.body) @semantic.scope
(struct_item (field_declaration_list) @semantic.body) @semantic.scope
(enum_item (enum_variant_list) @semantic.body) @semantic.scope
(union_item (field_declaration_list) @semantic.body) @semantic.scope
(trait_item (declaration_list) @semantic.body) @semantic.scope
(impl_item (declaration_list) @semantic.body) @semantic.scope
(mod_item (declaration_list) @semantic.body) @semantic.scope
"#;

#[cfg(feature = "lang-python")]
pub(super) const PYTHON: &str = r#"
(class_definition body: (block) @semantic.body) @semantic.scope
(function_definition body: (block) @semantic.body) @semantic.scope
"#;

#[cfg(feature = "lang-javascript")]
pub(super) const JAVASCRIPT: &str = r#"
(class_declaration body: (class_body) @semantic.body) @semantic.scope
(class body: (class_body) @semantic.body) @semantic.scope
(function_declaration body: (statement_block) @semantic.body) @semantic.scope
(generator_function_declaration body: (statement_block) @semantic.body) @semantic.scope
(method_definition body: (statement_block) @semantic.body) @semantic.scope
"#;

#[cfg(feature = "lang-typescript")]
pub(super) const TYPESCRIPT: &str = r#"
(class_declaration body: (class_body) @semantic.body) @semantic.scope
(abstract_class_declaration body: (class_body) @semantic.body) @semantic.scope
(function_declaration body: (statement_block) @semantic.body) @semantic.scope
(generator_function_declaration body: (statement_block) @semantic.body) @semantic.scope
(method_definition body: (statement_block) @semantic.body) @semantic.scope
(interface_declaration body: (interface_body) @semantic.body) @semantic.scope
"#;

#[cfg(feature = "lang-json")]
pub(super) const JSON: &str = r#"
(pair key: (string) @semantic.header value: [(object) (array)] @semantic.body) @semantic.scope
"#;

#[cfg(feature = "lang-go")]
pub(super) const GO: &str = r#"
(function_declaration body: (block) @semantic.body) @semantic.scope
(method_declaration body: (block) @semantic.body) @semantic.scope
(type_declaration (type_spec type: [(struct_type) (interface_type)] @semantic.body)) @semantic.scope
"#;

#[cfg(feature = "lang-c")]
pub(super) const C: &str = r#"
(function_definition body: (compound_statement) @semantic.body) @semantic.scope
(struct_specifier body: (field_declaration_list) @semantic.body) @semantic.scope
(union_specifier body: (field_declaration_list) @semantic.body) @semantic.scope
(enum_specifier body: (enumerator_list) @semantic.body) @semantic.scope
"#;

#[cfg(feature = "lang-cpp")]
pub(super) const CPP: &str = r#"
(function_definition body: (compound_statement) @semantic.body) @semantic.scope
(class_specifier body: (field_declaration_list) @semantic.body) @semantic.scope
(struct_specifier body: (field_declaration_list) @semantic.body) @semantic.scope
(namespace_definition body: (declaration_list) @semantic.body) @semantic.scope
(enum_specifier body: (enumerator_list) @semantic.body) @semantic.scope
"#;

#[cfg(feature = "lang-csharp")]
pub(super) const CSHARP: &str = r#"
(namespace_declaration body: (declaration_list) @semantic.body) @semantic.scope
(class_declaration body: (declaration_list) @semantic.body) @semantic.scope
(interface_declaration body: (declaration_list) @semantic.body) @semantic.scope
(struct_declaration body: (declaration_list) @semantic.body) @semantic.scope
(record_declaration body: (declaration_list) @semantic.body) @semantic.scope
(method_declaration body: (block) @semantic.body) @semantic.scope
(constructor_declaration body: (block) @semantic.body) @semantic.scope
"#;

#[cfg(feature = "lang-java")]
pub(super) const JAVA: &str = r#"
(class_declaration body: (class_body) @semantic.body) @semantic.scope
(interface_declaration body: (interface_body) @semantic.body) @semantic.scope
(enum_declaration body: (enum_body) @semantic.body) @semantic.scope
(record_declaration body: (class_body) @semantic.body) @semantic.scope
(method_declaration body: (block) @semantic.body) @semantic.scope
(constructor_declaration body: (constructor_body) @semantic.body) @semantic.scope
"#;

#[cfg(feature = "lang-ruby")]
pub(super) const RUBY: &str = r#"
(class) @semantic.scope
(singleton_class) @semantic.scope
(module) @semantic.scope
(method) @semantic.scope
(singleton_method) @semantic.scope
"#;

#[cfg(feature = "lang-php")]
pub(super) const PHP: &str = r#"
(namespace_definition body: (compound_statement) @semantic.body) @semantic.scope
(class_declaration body: (declaration_list) @semantic.body) @semantic.scope
(interface_declaration body: (declaration_list) @semantic.body) @semantic.scope
(trait_declaration body: (declaration_list) @semantic.body) @semantic.scope
(function_definition body: (compound_statement) @semantic.body) @semantic.scope
(method_declaration body: (compound_statement) @semantic.body) @semantic.scope
"#;

#[cfg(feature = "lang-bash")]
pub(super) const BASH: &str = r#"
(function_definition body: (_) @semantic.body) @semantic.scope
(if_statement) @semantic.scope
(for_statement) @semantic.scope
(c_style_for_statement) @semantic.scope
(while_statement) @semantic.scope
(case_statement) @semantic.scope
"#;

#[cfg(feature = "lang-toml")]
pub(super) const TOML: &str = r#"
(table) @semantic.scope
(table_array_element) @semantic.scope
"#;

#[cfg(feature = "lang-html")]
pub(super) const HTML: &str = r#"
(element (start_tag) @semantic.header) @semantic.scope
(script_element (start_tag) @semantic.header) @semantic.scope
(style_element (start_tag) @semantic.header) @semantic.scope
"#;

#[cfg(feature = "lang-css")]
pub(super) const CSS: &str = r#"
(rule_set (selectors) @semantic.header (block) @semantic.body) @semantic.scope
(media_statement (block) @semantic.body) @semantic.scope
(supports_statement (block) @semantic.body) @semantic.scope
(keyframes_statement (keyframe_block_list) @semantic.body) @semantic.scope
"#;

#[cfg(feature = "lang-yaml")]
pub(super) const YAML: &str = r#"
(block_mapping_pair
  key: (_) @semantic.header
  value: (block_node [(block_mapping) (block_sequence)] @semantic.body)) @semantic.scope
"#;

#[cfg(feature = "lang-markdown")]
pub(super) const MARKDOWN: &str = r#"
(atx_heading (atx_h1_marker)) @semantic.heading.1
(atx_heading (atx_h2_marker)) @semantic.heading.2
(atx_heading (atx_h3_marker)) @semantic.heading.3
(atx_heading (atx_h4_marker)) @semantic.heading.4
(atx_heading (atx_h5_marker)) @semantic.heading.5
(atx_heading (atx_h6_marker)) @semantic.heading.6
(setext_heading (setext_h1_underline)) @semantic.heading.1
(setext_heading (setext_h2_underline)) @semantic.heading.2
"#;

#[cfg(feature = "lang-latex")]
pub(super) const LATEX: &str = r#"
[(part) (chapter) (section) (subsection) (subsubsection)] @semantic.scope
"#;
