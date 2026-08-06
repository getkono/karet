//! Declaration queries for additional programming languages.

use std::borrow::Cow;

use crate::LanguageId;

#[cfg(feature = "lang-kotlin")]
const KOTLIN: &str = r#"
(package_header (_) @name) @definition.module
(class_declaration name: (identifier) @name) @definition.class
(object_declaration name: (identifier) @name) @definition.object
(function_declaration name: (identifier) @name) @definition.function
(type_alias type: (identifier) @name) @definition.type
"#;

#[cfg(feature = "lang-swift")]
const SWIFT: &str = r#"
(class_declaration name: (type_identifier) @name) @definition.class
(protocol_declaration name: (type_identifier) @name) @definition.interface
(function_declaration name: (simple_identifier) @name) @definition.function
(init_declaration "init" @name) @definition.constructor
(deinit_declaration "deinit" @name) @definition.method
"#;

#[cfg(feature = "lang-scala")]
const SCALA: &str = r#"
(package_clause name: (package_identifier) @name) @definition.module
(trait_definition name: (identifier) @name) @definition.interface
(enum_definition name: (identifier) @name) @definition.enum
(class_definition name: (identifier) @name) @definition.class
(object_definition name: (identifier) @name) @definition.object
(function_definition name: (identifier) @name) @definition.function
(type_definition name: (type_identifier) @name) @definition.type
"#;

#[cfg(feature = "lang-haskell")]
const HASKELL: &str = r#"
(module (module_id) @name) @definition.module
(class name: (_) @name) @definition.interface
(data_type name: (_) @name) @definition.type
(newtype name: (_) @name) @definition.type
(type_synomym name: (_) @name) @definition.type
(function name: [(prefix_id) (variable)] @name) @definition.function
"#;

#[cfg(feature = "lang-erlang")]
const ERLANG: &str = r#"
(module_attribute name: (_) @name) @definition.module
(record_decl name: (_) @name) @definition.type
(type_alias name: (type_name) @name) @definition.type
(function_clause name: (_) @name) @definition.function
"#;

#[cfg(feature = "lang-zig")]
const ZIG: &str = r#"
(function_declaration name: (identifier) @name) @definition.function
(variable_declaration (identifier) @name (struct_declaration)) @definition.type
(variable_declaration (identifier) @name (enum_declaration)) @definition.enum
(variable_declaration (identifier) @name (union_declaration)) @definition.type
(variable_declaration (identifier) @name (opaque_declaration)) @definition.type
(test_declaration [(identifier) (string)] @name) @definition.function
"#;

#[cfg(feature = "lang-perl")]
const PERL: &str = r#"
(package_statement (package_name) @name) @definition.module
(function_definition name: (identifier) @name) @definition.function
"#;

#[cfg(feature = "lang-clojure")]
const CLOJURE: &str = r#"
((list_lit
  value: (sym_lit) @_form
  . value: (sym_lit) @name) @definition.module
 (#eq? @_form "ns"))
((list_lit
  value: (sym_lit) @_form
  . value: (sym_lit) @name) @definition.interface
 (#any-of? @_form "defprotocol" "definterface"))
((list_lit
  value: (sym_lit) @_form
  . value: (sym_lit) @name) @definition.class
 (#any-of? @_form "defrecord" "deftype"))
((list_lit
  value: (sym_lit) @_form
  . value: (sym_lit) @name) @definition.function
 (#any-of? @_form "defn" "defn-" "defmulti" "defmethod"))
((list_lit
  value: (sym_lit) @_form
  . value: (sym_lit) @name) @definition.macro
 (#eq? @_form "defmacro"))
"#;

#[cfg(feature = "lang-vim")]
const VIM: &str = r#"
(function_definition
  (function_declaration name: (_) @name)) @definition.function
"#;

pub(super) fn query(_lang: LanguageId) -> Option<Cow<'static, str>> {
    #[cfg(feature = "lang-kotlin")]
    if _lang == super::super::programming::KOTLIN {
        return Some(Cow::Borrowed(KOTLIN));
    }
    #[cfg(feature = "lang-swift")]
    if _lang == super::super::programming::SWIFT {
        return Some(Cow::Borrowed(SWIFT));
    }
    #[cfg(feature = "lang-scala")]
    if _lang == super::super::programming::SCALA {
        return Some(Cow::Borrowed(SCALA));
    }
    #[cfg(feature = "lang-lua")]
    if _lang == super::super::programming::LUA {
        return Some(Cow::Borrowed(tree_sitter_lua::TAGS_QUERY));
    }
    #[cfg(feature = "lang-haskell")]
    if _lang == super::super::programming::HASKELL {
        return Some(Cow::Borrowed(HASKELL));
    }
    #[cfg(feature = "lang-ocaml")]
    if _lang == super::super::programming::OCAML
        || _lang == super::super::programming::OCAML_INTERFACE
    {
        return Some(Cow::Borrowed(tree_sitter_ocaml::TAGS_QUERY));
    }
    #[cfg(feature = "lang-elixir")]
    if _lang == super::super::programming::ELIXIR {
        return Some(Cow::Borrowed(tree_sitter_elixir::TAGS_QUERY));
    }
    #[cfg(feature = "lang-erlang")]
    if _lang == super::super::programming::ERLANG {
        return Some(Cow::Borrowed(ERLANG));
    }
    #[cfg(feature = "lang-dart")]
    if _lang == super::super::programming::DART {
        return Some(Cow::Borrowed(tree_sitter_dart::TAGS_QUERY));
    }
    #[cfg(feature = "lang-r")]
    if _lang == super::super::programming::R {
        return Some(Cow::Borrowed(tree_sitter_r::TAGS_QUERY));
    }
    #[cfg(feature = "lang-zig")]
    if _lang == super::super::ZIG {
        return Some(Cow::Borrowed(ZIG));
    }
    #[cfg(feature = "lang-perl")]
    if _lang == super::super::programming::PERL {
        return Some(Cow::Borrowed(PERL));
    }
    #[cfg(feature = "lang-clojure")]
    if _lang == super::super::programming::CLOJURE {
        return Some(Cow::Borrowed(CLOJURE));
    }
    #[cfg(feature = "lang-elisp")]
    if _lang == super::super::programming::ELISP {
        return Some(Cow::Borrowed(tree_sitter_elisp::TAGS_QUERY));
    }
    #[cfg(feature = "lang-vim")]
    if _lang == super::super::programming::VIM {
        return Some(Cow::Borrowed(VIM));
    }
    None
}
