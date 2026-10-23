//! The karet-authored fold queries (`queries/folds/*.scm`).
//!
//! A fold query names up to three captures:
//!
//! - `@fold` marks a construct that folds from the line it starts on through
//!   the line it ends on (a function, a block, a literal).
//! - `@fold.comment` marks one comment.
//! - `@fold.imports` marks one import declaration.
//!
//! Comments and imports are captured one node at a time because a grammar
//! parses `// a` / `// b` or two `use` lines as sibling nodes, never one; the
//! consumer joins a run of them on adjacent lines into one region.

use crate::LanguageId;
#[cfg(any(
    feature = "lang-rust",
    feature = "lang-python",
    feature = "lang-javascript",
    feature = "lang-typescript",
    feature = "lang-json",
    feature = "lang-go",
    feature = "lang-c",
    feature = "lang-cpp",
    feature = "lang-yaml",
    feature = "lang-lua"
))]
use crate::registry;

/// The karet-authored fold query for `lang`, if one ships for its grammar.
pub(crate) fn query_source(_lang: LanguageId) -> Option<&'static str> {
    #[cfg(feature = "lang-rust")]
    if _lang == registry::RUST {
        return Some(include_str!("../queries/folds/rust.scm"));
    }
    #[cfg(feature = "lang-python")]
    if _lang == registry::PYTHON {
        return Some(include_str!("../queries/folds/python.scm"));
    }
    #[cfg(feature = "lang-javascript")]
    if _lang == registry::JAVASCRIPT {
        return Some(include_str!("../queries/folds/javascript.scm"));
    }
    #[cfg(feature = "lang-typescript")]
    if _lang == registry::TYPESCRIPT || _lang == registry::TSX {
        return Some(include_str!("../queries/folds/typescript.scm"));
    }
    #[cfg(feature = "lang-json")]
    if _lang == registry::JSON {
        return Some(include_str!("../queries/folds/json.scm"));
    }
    #[cfg(feature = "lang-go")]
    if _lang == registry::GO {
        return Some(include_str!("../queries/folds/go.scm"));
    }
    #[cfg(feature = "lang-c")]
    if _lang == registry::C {
        return Some(include_str!("../queries/folds/c.scm"));
    }
    #[cfg(feature = "lang-cpp")]
    if _lang == registry::CPP {
        return Some(include_str!("../queries/folds/cpp.scm"));
    }
    #[cfg(feature = "lang-yaml")]
    if _lang == registry::YAML {
        return Some(include_str!("../queries/folds/yaml.scm"));
    }
    #[cfg(feature = "lang-lua")]
    if _lang == registry::programming::LUA {
        return Some(include_str!("../queries/folds/lua.scm"));
    }
    None
}

// Needs every grammar it names: without one, the lookup has nothing to find.
#[cfg(all(
    test,
    feature = "lang-rust",
    feature = "lang-python",
    feature = "lang-javascript",
    feature = "lang-typescript",
    feature = "lang-json",
    feature = "lang-go",
    feature = "lang-c",
    feature = "lang-cpp",
    feature = "lang-yaml",
    feature = "lang-lua",
    feature = "lang-toml"
))]
mod tests {
    use super::*;
    use crate::Query;
    use crate::TsError;
    use crate::language_id_from_injection_name;

    #[test]
    fn every_bundled_fold_query_compiles() -> Result<(), TsError> {
        let names = [
            "rust",
            "python",
            "javascript",
            "typescript",
            "tsx",
            "json",
            "go",
            "c",
            "cpp",
            "yaml",
            "lua",
        ];
        for name in names {
            let lang = language_id_from_injection_name(name).ok_or(TsError::UnknownLanguage)?;
            let source = query_source(lang).ok_or(TsError::UnknownLanguage)?;
            let query = Query::compile(lang, source)?;
            let captures = query.capture_names();
            assert!(captures.contains(&"fold"), "{name}");
            assert!(captures.contains(&"fold.comment"), "{name}");
            assert!(
                captures
                    .iter()
                    .all(|c| matches!(*c, "fold" | "fold.comment" | "fold.imports")),
                "{name}: {captures:?}"
            );
        }
        // A grammar karet ships no fold query for keeps the generic fallback.
        let toml = language_id_from_injection_name("toml").ok_or(TsError::UnknownLanguage)?;
        assert!(query_source(toml).is_none());
        Ok(())
    }
}
