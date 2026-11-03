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
use crate::registry;

/// The karet-authored fold query for `lang`, if one ships for its grammar.
pub(crate) fn query_source(lang: LanguageId) -> Option<&'static str> {
    registry::grammar(lang).and_then(|g| g.folds)
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
