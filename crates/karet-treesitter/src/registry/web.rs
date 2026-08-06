//! Web and template grammar entries.

#[cfg(any(feature = "lang-sass", feature = "lang-mdx"))]
use tree_sitter_language::LanguageFn;

use super::GrammarInfo;
#[cfg(any(
    feature = "lang-scss",
    feature = "lang-sass",
    feature = "lang-less",
    feature = "lang-erb",
    feature = "lang-mdx"
))]
use crate::LanguageId;

#[cfg(feature = "lang-scss")]
pub(super) const SCSS: LanguageId = LanguageId(54);
#[cfg(feature = "lang-sass")]
pub(super) const SASS: LanguageId = LanguageId(55);
#[cfg(feature = "lang-less")]
pub(super) const LESS: LanguageId = LanguageId(56);
#[cfg(feature = "lang-erb")]
pub(super) const ERB: LanguageId = LanguageId(57);
#[cfg(feature = "lang-mdx")]
pub(super) const MDX: LanguageId = LanguageId(58);

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
    });
}
