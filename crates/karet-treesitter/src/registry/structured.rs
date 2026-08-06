//! Structured-data and configuration grammar entries.

use super::GrammarInfo;
#[cfg(any(
    feature = "lang-json5",
    feature = "lang-ini",
    feature = "lang-properties",
    feature = "lang-edn",
    feature = "lang-cbor",
    feature = "lang-lockfile"
))]
use crate::LanguageId;

#[cfg(feature = "lang-json5")]
pub(super) const JSON5: LanguageId = LanguageId(59);
#[cfg(feature = "lang-ini")]
pub(super) const INI: LanguageId = LanguageId(60);
#[cfg(feature = "lang-properties")]
pub(super) const PROPERTIES: LanguageId = LanguageId(61);
#[cfg(feature = "lang-edn")]
pub(super) const EDN: LanguageId = LanguageId(62);
#[cfg(feature = "lang-cbor")]
pub(super) const CBOR: LanguageId = LanguageId(63);
#[cfg(feature = "lang-lockfile")]
pub(super) const LOCKFILE: LanguageId = LanguageId(64);

// Pkl deliberately has no entry: no suitable published pure-Rust binding is
// available, and issue #137 explicitly prefers deferral to a policy violation.

#[allow(clippy::vec_init_then_push)]
pub(super) fn push(grammars: &mut Vec<GrammarInfo>) {
    #[cfg(not(any(
        feature = "lang-json5",
        feature = "lang-ini",
        feature = "lang-properties",
        feature = "lang-edn",
        feature = "lang-cbor",
        feature = "lang-lockfile"
    )))]
    let _ = grammars;

    // The only published JSON5 Rust binding is pinned to tree-sitter 0.20.
    // Keep a distinct identity over the compatible JSON parser; karet-syntax's
    // format-aware outline scanner handles JSON5 keys and recovery directly.
    #[cfg(feature = "lang-json5")]
    grammars.push(GrammarInfo {
        id: JSON5,
        name: "JSON5",
        extensions: &["json5"],
        names: &["json5"],
        language: || tree_sitter_json::LANGUAGE.into(),
        highlights: tree_sitter_json::HIGHLIGHTS_QUERY,
        injections: None,
        injections_extra: None,
    });
    #[cfg(feature = "lang-ini")]
    grammars.push(GrammarInfo {
        id: INI,
        name: "INI",
        extensions: &["ini", "cfg", "conf"],
        names: &["ini", "editorconfig", "gitconfig"],
        language: || tree_sitter_ini::LANGUAGE.into(),
        highlights: tree_sitter_ini::HIGHLIGHTS_QUERY,
        injections: None,
        injections_extra: None,
    });
    #[cfg(feature = "lang-properties")]
    grammars.push(GrammarInfo {
        id: PROPERTIES,
        name: "Properties",
        extensions: &["properties"],
        names: &["properties", "dotenv"],
        language: || tree_sitter_properties::LANGUAGE.into(),
        highlights: tree_sitter_properties::HIGHLIGHTS_QUERY,
        injections: None,
        injections_extra: None,
    });
    #[cfg(feature = "lang-edn")]
    grammars.push(GrammarInfo {
        id: EDN,
        name: "EDN",
        extensions: &["edn"],
        names: &["edn"],
        language: || tree_sitter_clojure_orchard::LANGUAGE.into(),
        highlights: tree_sitter_clojure_orchard::HIGHLIGHTS_QUERY,
        injections: None,
        injections_extra: None,
    });
    #[cfg(feature = "lang-cbor")]
    grammars.push(GrammarInfo {
        id: CBOR,
        name: "CBOR diagnostic notation",
        extensions: &["cbor"],
        names: &["cbor", "cbor-diag"],
        language: || tree_sitter_json::LANGUAGE.into(),
        highlights: tree_sitter_json::HIGHLIGHTS_QUERY,
        injections: None,
        injections_extra: None,
    });
    #[cfg(feature = "lang-lockfile")]
    grammars.push(GrammarInfo {
        id: LOCKFILE,
        name: "Yarn lockfile",
        extensions: &[],
        names: &["lockfile", "yarn-lock"],
        language: || tree_sitter_yaml::LANGUAGE.into(),
        highlights: tree_sitter_yaml::HIGHLIGHTS_QUERY,
        injections: None,
        injections_extra: None,
    });
}
