//! Additional programming-language grammar entries.

use super::GrammarInfo;
#[cfg(any(
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
    feature = "lang-perl",
    feature = "lang-clojure",
    feature = "lang-elisp",
    feature = "lang-vim"
))]
use crate::LanguageId;

#[cfg(feature = "lang-kotlin")]
pub(super) const KOTLIN: LanguageId = LanguageId(39);
#[cfg(feature = "lang-swift")]
pub(super) const SWIFT: LanguageId = LanguageId(40);
#[cfg(feature = "lang-scala")]
pub(super) const SCALA: LanguageId = LanguageId(41);
#[cfg(feature = "lang-lua")]
pub(super) const LUA: LanguageId = LanguageId(42);
#[cfg(feature = "lang-haskell")]
pub(super) const HASKELL: LanguageId = LanguageId(43);
#[cfg(feature = "lang-ocaml")]
pub(super) const OCAML: LanguageId = LanguageId(44);
#[cfg(feature = "lang-ocaml")]
pub(super) const OCAML_INTERFACE: LanguageId = LanguageId(45);
#[cfg(feature = "lang-elixir")]
pub(super) const ELIXIR: LanguageId = LanguageId(46);
#[cfg(feature = "lang-erlang")]
pub(super) const ERLANG: LanguageId = LanguageId(47);
#[cfg(feature = "lang-dart")]
pub(super) const DART: LanguageId = LanguageId(48);
#[cfg(feature = "lang-r")]
pub(super) const R: LanguageId = LanguageId(49);
#[cfg(feature = "lang-perl")]
pub(super) const PERL: LanguageId = LanguageId(50);
#[cfg(feature = "lang-clojure")]
pub(super) const CLOJURE: LanguageId = LanguageId(51);
#[cfg(feature = "lang-elisp")]
pub(super) const ELISP: LanguageId = LanguageId(52);
#[cfg(feature = "lang-vim")]
pub(super) const VIM: LanguageId = LanguageId(53);

#[cfg(feature = "lang-kotlin")]
const KOTLIN_HIGHLIGHTS: &str = r#"
[(line_comment) (block_comment)] @comment
[(string_literal) (multiline_string_literal)] @string
(character_literal) @string.special
[(number_literal) (float_literal)] @number
(class_declaration name: (identifier) @type)
(object_declaration name: (identifier) @type)
(function_declaration name: (identifier) @function)
[
  "class" "interface" "object" "fun" "val" "var" "typealias"
  "package" "import" "enum"
] @keyword
"#;

#[cfg(feature = "lang-perl")]
const PERL_HIGHLIGHTS: &str = r#"
(comments) @comment
[
  (string_double_quoted) (string_single_quoted) (string_q_quoted)
  (string_qq_quoted)
] @string
[(integer) (floating_point)] @number
(package_name) @type
(function_definition name: (identifier) @function)
"#;

#[cfg(feature = "lang-ocaml")]
const OCAML_INTERFACE_HIGHLIGHTS: &str = r#"
(comment) @comment
[(string) (quoted_string)] @string
(character) @string.special
[(value_name) (method_name)] @function
[(type_constructor) (module_name) (module_type_name) (class_name)] @type
"#;

// Each entry is deliberately explicit so feature-subset builds can discard every
// unused grammar and its query strings; splitting the table would obscure that audit.
#[allow(clippy::too_many_lines)]
pub(super) fn push(grammars: &mut Vec<GrammarInfo>) {
    #[cfg(not(any(
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
        feature = "lang-perl",
        feature = "lang-clojure",
        feature = "lang-elisp",
        feature = "lang-vim"
    )))]
    let _ = grammars;
    #[cfg(feature = "lang-kotlin")]
    grammars.push(GrammarInfo {
        id: KOTLIN,
        name: "Kotlin",
        extensions: &["kt", "kts"],
        names: &["kotlin", "kt", "kts"],
        language: || tree_sitter_kotlin_ng::LANGUAGE.into(),
        highlights: KOTLIN_HIGHLIGHTS,
        injections: None,
        injections_extra: None,
    });
    #[cfg(feature = "lang-swift")]
    grammars.push(GrammarInfo {
        id: SWIFT,
        name: "Swift",
        extensions: &["swift"],
        names: &["swift"],
        language: || tree_sitter_swift::LANGUAGE.into(),
        highlights: tree_sitter_swift::HIGHLIGHTS_QUERY,
        injections: Some(tree_sitter_swift::INJECTIONS_QUERY),
        injections_extra: None,
    });
    #[cfg(feature = "lang-scala")]
    grammars.push(GrammarInfo {
        id: SCALA,
        name: "Scala",
        extensions: &["scala", "sbt", "sc"],
        names: &["scala", "sbt"],
        language: || tree_sitter_scala::LANGUAGE.into(),
        highlights: tree_sitter_scala::HIGHLIGHTS_QUERY,
        injections: None,
        injections_extra: None,
    });
    #[cfg(feature = "lang-lua")]
    grammars.push(GrammarInfo {
        id: LUA,
        name: "Lua",
        extensions: &["lua"],
        names: &["lua"],
        language: || tree_sitter_lua::LANGUAGE.into(),
        highlights: tree_sitter_lua::HIGHLIGHTS_QUERY,
        injections: Some(tree_sitter_lua::INJECTIONS_QUERY),
        injections_extra: None,
    });
    #[cfg(feature = "lang-haskell")]
    grammars.push(GrammarInfo {
        id: HASKELL,
        name: "Haskell",
        extensions: &["hs", "lhs"],
        names: &["haskell", "hs"],
        language: || tree_sitter_haskell::LANGUAGE.into(),
        highlights: tree_sitter_haskell::HIGHLIGHTS_QUERY,
        injections: Some(tree_sitter_haskell::INJECTIONS_QUERY),
        injections_extra: None,
    });
    #[cfg(feature = "lang-ocaml")]
    grammars.push(GrammarInfo {
        id: OCAML,
        name: "OCaml",
        extensions: &["ml"],
        names: &["ocaml", "ml"],
        language: || tree_sitter_ocaml::LANGUAGE_OCAML.into(),
        highlights: tree_sitter_ocaml::HIGHLIGHTS_QUERY,
        injections: None,
        injections_extra: None,
    });
    #[cfg(feature = "lang-ocaml")]
    grammars.push(GrammarInfo {
        id: OCAML_INTERFACE,
        name: "OCaml",
        extensions: &["mli"],
        names: &["ocaml-interface", "mli"],
        language: || tree_sitter_ocaml::LANGUAGE_OCAML_INTERFACE.into(),
        highlights: OCAML_INTERFACE_HIGHLIGHTS,
        injections: None,
        injections_extra: None,
    });
    #[cfg(feature = "lang-elixir")]
    grammars.push(GrammarInfo {
        id: ELIXIR,
        name: "Elixir",
        extensions: &["ex", "exs"],
        names: &["elixir", "ex"],
        language: || tree_sitter_elixir::LANGUAGE.into(),
        highlights: tree_sitter_elixir::HIGHLIGHTS_QUERY,
        injections: Some(tree_sitter_elixir::INJECTIONS_QUERY),
        injections_extra: None,
    });
    #[cfg(feature = "lang-erlang")]
    grammars.push(GrammarInfo {
        id: ERLANG,
        name: "Erlang",
        extensions: &["erl", "hrl"],
        names: &["erlang", "erl"],
        language: || tree_sitter_erlang::LANGUAGE.into(),
        highlights: tree_sitter_erlang::HIGHLIGHTS_QUERY,
        injections: None,
        injections_extra: None,
    });
    #[cfg(feature = "lang-dart")]
    grammars.push(GrammarInfo {
        id: DART,
        name: "Dart",
        extensions: &["dart"],
        names: &["dart"],
        language: || tree_sitter_dart::LANGUAGE.into(),
        highlights: tree_sitter_dart::HIGHLIGHTS_QUERY,
        injections: None,
        injections_extra: None,
    });
    #[cfg(feature = "lang-r")]
    grammars.push(GrammarInfo {
        id: R,
        name: "R",
        extensions: &["r"],
        names: &["r"],
        language: || tree_sitter_r::LANGUAGE.into(),
        highlights: tree_sitter_r::HIGHLIGHTS_QUERY,
        injections: None,
        injections_extra: None,
    });
    #[cfg(feature = "lang-perl")]
    grammars.push(GrammarInfo {
        id: PERL,
        name: "Perl",
        extensions: &["pl", "pm"],
        names: &["perl", "pl"],
        language: || tree_sitter_perl::LANGUAGE.into(),
        highlights: PERL_HIGHLIGHTS,
        injections: None,
        injections_extra: None,
    });
    #[cfg(feature = "lang-clojure")]
    grammars.push(GrammarInfo {
        id: CLOJURE,
        name: "Clojure",
        extensions: &["clj", "cljs", "cljc"],
        names: &["clojure", "clj", "cljs"],
        language: || tree_sitter_clojure_orchard::LANGUAGE.into(),
        highlights: tree_sitter_clojure_orchard::HIGHLIGHTS_QUERY,
        injections: None,
        injections_extra: None,
    });
    #[cfg(feature = "lang-elisp")]
    grammars.push(GrammarInfo {
        id: ELISP,
        name: "Emacs Lisp",
        extensions: &["el"],
        names: &["elisp", "emacs-lisp"],
        language: || tree_sitter_elisp::LANGUAGE.into(),
        highlights: tree_sitter_elisp::HIGHLIGHTS_QUERY,
        injections: None,
        injections_extra: None,
    });
    #[cfg(feature = "lang-vim")]
    grammars.push(GrammarInfo {
        id: VIM,
        name: "Vim script",
        extensions: &["vim"],
        names: &["vim", "vimscript"],
        language: tree_sitter_vim::language,
        highlights: tree_sitter_vim::HIGHLIGHTS_QUERY,
        injections: Some(tree_sitter_vim::INJECTIONS_QUERY),
        injections_extra: None,
    });
}
