//! The per-language override mechanism: `[language]` selectors, the partial
//! [`EditorOverride`] patch, tri-state [`NullableOverride`], and the
//! `Resolved*` views that apply a patch over the merged global editor values.
//!
//! Split from the section structs in [`super`] because this is a *mechanism*
//! (selector parsing, serde plumbing, resolution precedence), not settings
//! vocabulary.

#[cfg(feature = "schema")]
use std::borrow::Cow;
use std::fmt;

#[cfg(feature = "schema")]
use schemars::Schema;
#[cfg(feature = "schema")]
use schemars::SchemaGenerator;
use serde::Deserialize;
use serde::Serialize;

use super::Completion;
use super::Editor;
use super::LineNumbers;
use super::SemanticComments;

/// A normalized `[language]` key used by [`Editor::language_overrides`].
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LanguageSelector(String);

impl LanguageSelector {
    /// Build a selector from a display language or language id.
    #[must_use]
    pub fn from_language(language: &str) -> Option<Self> {
        let language = language.trim();
        (!language.is_empty() && !language.contains(['[', ']']))
            .then(|| Self(language.to_ascii_lowercase()))
    }

    /// Return the normalized language name without surrounding brackets.
    #[must_use]
    pub fn language(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for LanguageSelector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}]", self.0)
    }
}

impl Serialize for LanguageSelector {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for LanguageSelector {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::Error as _;

        let key = String::deserialize(deserializer)?;
        let Some(language) = key.strip_prefix('[').and_then(|k| k.strip_suffix(']')) else {
            return Err(D::Error::custom(format!(
                "unknown editor setting `{key}`; expected a `[language]` selector"
            )));
        };
        Self::from_language(language)
            .ok_or_else(|| D::Error::custom(format!("invalid language selector `{key}`")))
    }
}

#[cfg(feature = "schema")]
impl schemars::JsonSchema for LanguageSelector {
    fn schema_name() -> Cow<'static, str> {
        Cow::Borrowed("LanguageSelector")
    }

    fn json_schema(_generator: &mut SchemaGenerator) -> Schema {
        schemars::json_schema!({
            "type": "string",
            "pattern": r"^\[[^\[\]]+\]$"
        })
    }
}

/// A partial per-language patch for [`Editor`].
///
/// Every field is optional: omitted fields inherit the merged global editor value.
/// Arrays replace the global value when present, and nested objects merge field by
/// field through their own partial patch types.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct EditorOverride {
    /// Override columns per indent level.
    pub tab_size: Option<u8>,
    /// Override spaces-versus-tabs indentation.
    pub insert_spaces: Option<bool>,
    /// Override line-number gutter mode.
    pub line_numbers: Option<LineNumbers>,
    /// Override current-line highlighting.
    pub cursor_line: Option<bool>,
    /// Override graphical-cursor behavior; explicit `null` restores auto mode.
    #[serde(default, skip_serializing_if = "NullableOverride::is_unset")]
    #[cfg_attr(feature = "schema", schemars(with = "Option<bool>"))]
    pub graphical_cursor: NullableOverride<bool>,
    /// Override the caret scroll margin.
    pub scroll_off: Option<u16>,
    /// Replace the global ruler columns.
    pub rulers: Option<Vec<u16>>,
    /// Override wrapping; explicit `null` restores the file-type default.
    #[serde(default, skip_serializing_if = "NullableOverride::is_unset")]
    #[cfg_attr(feature = "schema", schemars(with = "Option<bool>"))]
    pub word_wrap: NullableOverride<bool>,
    /// Override semantic sticky-scroll rendering.
    pub sticky_scroll: Option<bool>,
    /// Override trailing-whitespace trimming.
    pub trim_trailing_whitespace: Option<bool>,
    /// Override final-newline insertion.
    pub insert_final_newline: Option<bool>,
    /// Override format-on-save.
    pub format_on_save: Option<bool>,
    /// Partially override semantic-comment behavior.
    pub semantic_comments: Option<SemanticCommentsOverride>,
    /// Partially override completion behavior.
    pub completion: Option<CompletionOverride>,
}

/// A partial per-language patch for [`Completion`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct CompletionOverride {
    /// Override whether completion is enabled.
    pub enabled: Option<bool>,
    /// Override automatic completion triggering.
    pub auto_trigger: Option<bool>,
}

/// A partial per-language patch for [`SemanticComments`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct SemanticCommentsOverride {
    /// Override whether semantic comments are highlighted.
    pub enabled: Option<bool>,
    /// Replace the global semantic-comment tag list.
    pub tags: Option<Vec<String>>,
}

/// An optional override that distinguishes an omitted field from explicit `null`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum NullableOverride<T> {
    /// The language patch inherits the global value.
    #[default]
    Unset,
    /// The language patch explicitly supplies a nullable value.
    Set(Option<T>),
}

impl<T> NullableOverride<T> {
    fn is_unset(&self) -> bool {
        matches!(self, Self::Unset)
    }
}

impl<T: Serialize> Serialize for NullableOverride<T> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            Self::Unset | Self::Set(None) => serializer.serialize_none(),
            Self::Set(Some(value)) => serializer.serialize_some(value),
        }
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for NullableOverride<T> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Option::<T>::deserialize(deserializer).map(Self::Set)
    }
}

/// A zero-copy view of the final editor settings for one language.
#[derive(Clone, Copy, Debug)]
pub struct ResolvedEditor<'a> {
    base: &'a Editor,
    override_: Option<&'a EditorOverride>,
}

impl<'a> ResolvedEditor<'a> {
    /// A resolved view applying `override_` (when present) over `base`.
    #[must_use]
    pub(crate) fn new(base: &'a Editor, override_: Option<&'a EditorOverride>) -> Self {
        Self { base, override_ }
    }

    /// Final columns per indent level.
    #[must_use]
    pub fn tab_size(self) -> u8 {
        self.override_
            .and_then(|o| o.tab_size)
            .unwrap_or(self.base.tab_size)
    }

    /// Final spaces-versus-tabs indentation setting.
    #[must_use]
    pub fn insert_spaces(self) -> bool {
        self.override_
            .and_then(|o| o.insert_spaces)
            .unwrap_or(self.base.insert_spaces)
    }

    /// Final line-number gutter mode.
    #[must_use]
    pub fn line_numbers(self) -> LineNumbers {
        self.override_
            .and_then(|o| o.line_numbers)
            .unwrap_or(self.base.line_numbers)
    }

    /// Final current-line highlighting setting.
    #[must_use]
    pub fn cursor_line(self) -> bool {
        self.override_
            .and_then(|o| o.cursor_line)
            .unwrap_or(self.base.cursor_line)
    }

    /// Final graphical-cursor setting.
    #[must_use]
    pub fn graphical_cursor(self) -> Option<bool> {
        match self.override_.map(|o| &o.graphical_cursor) {
            Some(NullableOverride::Set(value)) => *value,
            _ => self.base.graphical_cursor,
        }
    }

    /// Final caret scroll margin.
    #[must_use]
    pub fn scroll_off(self) -> u16 {
        self.override_
            .and_then(|o| o.scroll_off)
            .unwrap_or(self.base.scroll_off)
    }

    /// Final ruler columns.
    #[must_use]
    pub fn rulers(self) -> &'a [u16] {
        self.override_
            .and_then(|o| o.rulers.as_deref())
            .unwrap_or(&self.base.rulers)
    }

    /// Final wrapping override.
    #[must_use]
    pub fn word_wrap(self) -> Option<bool> {
        match self.override_.map(|o| &o.word_wrap) {
            Some(NullableOverride::Set(value)) => *value,
            _ => self.base.word_wrap,
        }
    }

    /// Final semantic sticky-scroll setting.
    #[must_use]
    pub fn sticky_scroll(self) -> bool {
        self.override_
            .and_then(|o| o.sticky_scroll)
            .unwrap_or(self.base.sticky_scroll)
    }

    /// Final trailing-whitespace trimming setting.
    #[must_use]
    pub fn trim_trailing_whitespace(self) -> bool {
        self.override_
            .and_then(|o| o.trim_trailing_whitespace)
            .unwrap_or(self.base.trim_trailing_whitespace)
    }

    /// Final final-newline insertion setting.
    #[must_use]
    pub fn insert_final_newline(self) -> bool {
        self.override_
            .and_then(|o| o.insert_final_newline)
            .unwrap_or(self.base.insert_final_newline)
    }

    /// Final format-on-save setting.
    #[must_use]
    pub fn format_on_save(self) -> bool {
        self.override_
            .and_then(|o| o.format_on_save)
            .unwrap_or(self.base.format_on_save)
    }

    /// Final semantic-comment settings.
    #[must_use]
    pub fn semantic_comments(self) -> ResolvedSemanticComments<'a> {
        ResolvedSemanticComments {
            base: &self.base.semantic_comments,
            override_: self.override_.and_then(|o| o.semantic_comments.as_ref()),
        }
    }

    /// Final completion settings.
    #[must_use]
    pub fn completion(self) -> ResolvedCompletion<'a> {
        ResolvedCompletion {
            base: &self.base.completion,
            override_: self.override_.and_then(|o| o.completion.as_ref()),
        }
    }
}

/// A zero-copy view of resolved completion settings.
#[derive(Clone, Copy, Debug)]
pub struct ResolvedCompletion<'a> {
    base: &'a Completion,
    override_: Option<&'a CompletionOverride>,
}

impl ResolvedCompletion<'_> {
    /// Whether completion is enabled.
    #[must_use]
    pub fn enabled(self) -> bool {
        self.override_
            .and_then(|o| o.enabled)
            .unwrap_or(self.base.enabled)
    }

    /// Whether completion triggers automatically.
    #[must_use]
    pub fn auto_trigger(self) -> bool {
        self.override_
            .and_then(|o| o.auto_trigger)
            .unwrap_or(self.base.auto_trigger)
    }
}

/// A zero-copy view of resolved semantic-comment settings.
#[derive(Clone, Copy, Debug)]
pub struct ResolvedSemanticComments<'a> {
    base: &'a SemanticComments,
    override_: Option<&'a SemanticCommentsOverride>,
}

impl<'a> ResolvedSemanticComments<'a> {
    /// Whether semantic-comment highlighting is enabled.
    #[must_use]
    pub fn enabled(self) -> bool {
        self.override_
            .and_then(|o| o.enabled)
            .unwrap_or(self.base.enabled)
    }

    /// The semantic-comment tags to recognize.
    #[must_use]
    pub fn tags(self) -> &'a [String] {
        self.override_
            .and_then(|o| o.tags.as_deref())
            .unwrap_or(&self.base.tags)
    }
}
