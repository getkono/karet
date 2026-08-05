//! Declaration queries for the bundled grammar registry.

use std::borrow::Cow;

use crate::LanguageId;

#[cfg(feature = "lang-bash")]
const BASH: &str = r#"
(function_definition name: (word) @name) @definition.function
"#;

#[cfg(feature = "lang-json")]
const JSON: &str = r#"
(pair key: (string) @name value: [(object) (array)]) @definition.object
"#;

#[cfg(feature = "lang-yaml")]
const YAML: &str = r#"
(block_mapping_pair
  key: (_) @name
  value: (block_node [(block_mapping) (block_sequence)])) @definition.object
"#;

#[cfg(feature = "lang-toml")]
const TOML: &str = r#"
(table [(bare_key) (dotted_key) (quoted_key)] @name) @definition.object
(table_array_element [(bare_key) (dotted_key) (quoted_key)] @name) @definition.array
"#;

#[cfg(feature = "lang-html")]
const HTML: &str = r#"
((element (start_tag (tag_name) @name)) @definition.object
 (#match? @name "^(main|nav|section|article|aside|header|footer|h[1-6])$"))
"#;

#[cfg(feature = "lang-css")]
const CSS: &str = r#"
(rule_set (selectors) @name) @definition.class
(media_statement) @name @definition.namespace
(supports_statement) @name @definition.namespace
(keyframes_statement (keyframes_name) @name) @definition.class
"#;

#[cfg(feature = "lang-sql")]
const SQL: &str = r#"
(create_schema (identifier) @name) @definition.namespace
(create_table (object_reference) @name) @definition.class
(create_view (object_reference) @name) @definition.class
(create_materialized_view (object_reference) @name) @definition.class
(create_function (object_reference) @name) @definition.function
(create_query name: (identifier) @name) @definition.function
(cte . (identifier) @name) @definition.variable
"#;

#[cfg(feature = "lang-graphql")]
const GRAPHQL: &str = r#"
(schema_definition "schema" @name) @definition.module
(object_type_definition (name) @name) @definition.class
(interface_type_definition (name) @name) @definition.interface
(enum_type_definition (name) @name) @definition.class
(input_object_type_definition (name) @name) @definition.class
(union_type_definition (name) @name) @definition.class
(scalar_type_definition (name) @name) @definition.type
(operation_definition (name) @name) @definition.function
(fragment_definition (fragment_name) @name) @definition.function
(field_definition (name) @name) @definition.field
(input_value_definition (name) @name) @definition.field
(enum_value_definition (enum_value) @name) @definition.constant
"#;

#[cfg(feature = "lang-protobuf")]
const PROTOBUF: &str = r#"
(package (full_ident) @name) @definition.namespace
(message (message_name) @name) @definition.class
(enum (enum_name) @name) @definition.class
(service (service_name) @name) @definition.interface
(rpc (rpc_name) @name) @definition.method
"#;

#[cfg(feature = "lang-containerfile")]
const CONTAINERFILE: &str = r#"
(from_instruction as: (image_alias) @name) @definition.class
"#;

#[cfg(feature = "lang-make")]
const MAKE: &str = r#"
(conditional condition: (_) @name) @definition.namespace
(rule (targets) @name) @definition.class
(variable_assignment name: (word) @name) @definition.constant
(define_directive name: (word) @name) @definition.constant
"#;

#[cfg(feature = "lang-cmake")]
const CMAKE: &str = r#"
(function_def
  (function_command (argument_list . (argument) @name))) @definition.function
(macro_def
  (macro_command (argument_list . (argument) @name))) @definition.macro
((normal_command
  (identifier) @_command
  (argument_list . (argument) @name)) @definition.class
 (#match? @_command "^(add_executable|add_library|add_custom_target)$"))
(if_condition (if_command (argument_list) @name)) @definition.namespace
(foreach_loop (foreach_command (argument_list) @name)) @definition.namespace
(while_loop (while_command (argument_list) @name)) @definition.namespace
"#;

#[cfg(feature = "lang-rst")]
const RST: &str = r#"
(section (title) @name) @definition.heading
"#;

#[cfg(feature = "lang-asciidoc")]
const ASCIIDOC: &str = r#"
(document_title (line) @name) @definition.heading.1
(section_block (title1 (line) @name)) @definition.heading.2
(section_block (title2 (line) @name)) @definition.heading.3
(section_block (title3 (line) @name)) @definition.heading.4
(section_block (title4 (line) @name)) @definition.heading.5
(section_block (title5 (line) @name)) @definition.heading.6
"#;

#[cfg(feature = "lang-latex")]
const LATEX: &str = r#"
(part text: (curly_group (text) @name)) @definition.heading.1
(chapter text: (curly_group (text) @name)) @definition.heading.2
(section text: (curly_group (text) @name)) @definition.heading.3
(subsection text: (curly_group (text) @name)) @definition.heading.4
(subsubsection text: (curly_group (text) @name)) @definition.heading.5
(paragraph text: (curly_group (text) @name)) @definition.heading.6
(subparagraph text: (curly_group (text) @name)) @definition.heading.7
"#;

#[cfg(feature = "lang-zsh")]
const ZSH: &str = r#"
(function_definition name: (word) @name) @definition.function
"#;

#[cfg(feature = "lang-fish")]
const FISH: &str = r#"
(function_definition name: (_) @name) @definition.function
"#;

#[cfg(feature = "lang-powershell")]
const POWERSHELL: &str = r#"
(function_statement (function_name) @name) @definition.function
(class_statement . (simple_name) @name) @definition.class
(class_method_definition (simple_name) @name) @definition.method
(enum_statement . (simple_name) @name) @definition.class
"#;

#[cfg(feature = "lang-batch")]
const BATCH: &str = r#"
(label) @name @definition.subroutine
"#;

pub(crate) fn outline_query(_lang: LanguageId) -> Option<Cow<'static, str>> {
    #[cfg(feature = "lang-rust")]
    if _lang == super::RUST {
        return Some(Cow::Borrowed(tree_sitter_rust::TAGS_QUERY));
    }
    #[cfg(feature = "lang-python")]
    if _lang == super::PYTHON {
        return Some(Cow::Borrowed(tree_sitter_python::TAGS_QUERY));
    }
    #[cfg(feature = "lang-javascript")]
    if _lang == super::JAVASCRIPT {
        return Some(Cow::Borrowed(tree_sitter_javascript::TAGS_QUERY));
    }
    #[cfg(feature = "lang-typescript")]
    if _lang == super::TYPESCRIPT || _lang == super::TSX {
        return Some(Cow::Owned(format!(
            "{}\n{}",
            tree_sitter_javascript::TAGS_QUERY,
            tree_sitter_typescript::TAGS_QUERY
        )));
    }
    #[cfg(feature = "lang-go")]
    if _lang == super::GO {
        return Some(Cow::Borrowed(tree_sitter_go::TAGS_QUERY));
    }
    #[cfg(feature = "lang-c")]
    if _lang == super::C {
        return Some(Cow::Borrowed(tree_sitter_c::TAGS_QUERY));
    }
    #[cfg(feature = "lang-cpp")]
    if _lang == super::CPP {
        return Some(Cow::Borrowed(tree_sitter_cpp::TAGS_QUERY));
    }
    #[cfg(feature = "lang-csharp")]
    if _lang == super::CSHARP {
        return Some(Cow::Borrowed(tree_sitter_c_sharp::TAGS_QUERY));
    }
    #[cfg(feature = "lang-java")]
    if _lang == super::JAVA {
        return Some(Cow::Borrowed(tree_sitter_java::TAGS_QUERY));
    }
    #[cfg(feature = "lang-ruby")]
    if _lang == super::RUBY {
        return Some(Cow::Borrowed(tree_sitter_ruby::TAGS_QUERY));
    }
    #[cfg(feature = "lang-php")]
    if _lang == super::PHP {
        return Some(Cow::Borrowed(tree_sitter_php::TAGS_QUERY));
    }
    #[cfg(feature = "lang-bash")]
    if _lang == super::BASH {
        return Some(Cow::Borrowed(BASH));
    }
    #[cfg(feature = "lang-json")]
    if _lang == super::JSON {
        return Some(Cow::Borrowed(JSON));
    }
    #[cfg(feature = "lang-yaml")]
    if _lang == super::YAML {
        return Some(Cow::Borrowed(YAML));
    }
    #[cfg(feature = "lang-toml")]
    if _lang == super::TOML {
        return Some(Cow::Borrowed(TOML));
    }
    #[cfg(feature = "lang-html")]
    if _lang == super::HTML {
        return Some(Cow::Borrowed(HTML));
    }
    #[cfg(feature = "lang-css")]
    if _lang == super::CSS {
        return Some(Cow::Borrowed(CSS));
    }
    #[cfg(feature = "lang-sql")]
    if _lang == super::SQL {
        return Some(Cow::Borrowed(SQL));
    }
    #[cfg(feature = "lang-graphql")]
    if _lang == super::GRAPHQL {
        return Some(Cow::Borrowed(GRAPHQL));
    }
    #[cfg(feature = "lang-protobuf")]
    if _lang == super::PROTOBUF {
        return Some(Cow::Borrowed(PROTOBUF));
    }
    #[cfg(feature = "lang-containerfile")]
    if _lang == super::CONTAINERFILE {
        return Some(Cow::Borrowed(CONTAINERFILE));
    }
    #[cfg(feature = "lang-make")]
    if _lang == super::MAKE {
        return Some(Cow::Borrowed(MAKE));
    }
    #[cfg(feature = "lang-cmake")]
    if _lang == super::CMAKE {
        return Some(Cow::Borrowed(CMAKE));
    }
    #[cfg(feature = "lang-rst")]
    if _lang == super::RST {
        return Some(Cow::Borrowed(RST));
    }
    #[cfg(feature = "lang-asciidoc")]
    if _lang == super::ASCIIDOC {
        return Some(Cow::Borrowed(ASCIIDOC));
    }
    #[cfg(feature = "lang-latex")]
    if _lang == super::LATEX {
        return Some(Cow::Borrowed(LATEX));
    }
    #[cfg(feature = "lang-zsh")]
    if _lang == super::ZSH {
        return Some(Cow::Borrowed(ZSH));
    }
    #[cfg(feature = "lang-fish")]
    if _lang == super::FISH {
        return Some(Cow::Borrowed(FISH));
    }
    #[cfg(feature = "lang-powershell")]
    if _lang == super::POWERSHELL {
        return Some(Cow::Borrowed(POWERSHELL));
    }
    #[cfg(feature = "lang-batch")]
    if _lang == super::BATCH {
        return Some(Cow::Borrowed(BATCH));
    }
    None
}
