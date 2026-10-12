//! `editor.inlayHints.*` — the inferred types and parameter names a language
//! server supplies, drawn between characters.
//!
//! `enabled` takes VS Code's spelling of its four modes as well as a plain
//! boolean, so a setting copied from VS Code means here what it meant there.

use serde::Deserialize;
use serde::Serialize;

/// `editor.inlayHints.*` — the inferred types and parameter names a language
/// server supplies, drawn between characters.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct InlayHints {
    /// Whether inlay hints are drawn: `true`/`"on"` and `false`/`"off"`, or
    /// `"onUnlessPressed"`/`"offUnlessPressed"` to show them, or hide them,
    /// except for a moment after the Toggle Inlay Hints key.
    pub enabled: InlayHintsEnabled,
}

/// When inlay hints are drawn (`editor.inlayHints.enabled`).
///
/// A terminal cannot report a key being *held*, so the two `UnlessPressed`
/// modes read "pressed" as the moment after the Toggle Inlay Hints key: the
/// hints flip until the next key, then return to the mode's own state. The
/// other two modes make the key a plain toggle for the session.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum InlayHintsEnabled {
    /// Drawn (`true` or `"on"`). On by default: the annotation is the point of
    /// running a server that infers types, and a server that offers none
    /// costs nothing.
    #[default]
    On,
    /// Not drawn (`false` or `"off"`).
    Off,
    /// Drawn, and hidden for a moment by the toggle key.
    OnUnlessPressed,
    /// Hidden, and shown for a moment by the toggle key.
    OffUnlessPressed,
}

impl InlayHintsEnabled {
    /// Every mode's string spelling, as VS Code writes it.
    const NAMES: [(&'static str, Self); 4] = [
        ("on", Self::On),
        ("off", Self::Off),
        ("onUnlessPressed", Self::OnUnlessPressed),
        ("offUnlessPressed", Self::OffUnlessPressed),
    ];

    /// Whether hints are drawn when the toggle key has not been pressed.
    #[must_use]
    pub fn shown(self) -> bool {
        matches!(self, Self::On | Self::OnUnlessPressed)
    }

    /// Whether the toggle key flips the hints only until the next key.
    #[must_use]
    pub fn momentary(self) -> bool {
        matches!(self, Self::OnUnlessPressed | Self::OffUnlessPressed)
    }
}

impl From<bool> for InlayHintsEnabled {
    fn from(enabled: bool) -> Self {
        if enabled { Self::On } else { Self::Off }
    }
}

impl Serialize for InlayHintsEnabled {
    /// The two plain modes as the boolean they have always been, so a
    /// configuration written before the modes existed reads back unchanged.
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::On => serializer.serialize_bool(true),
            Self::Off => serializer.serialize_bool(false),
            Self::OnUnlessPressed => serializer.serialize_str("onUnlessPressed"),
            Self::OffUnlessPressed => serializer.serialize_str("offUnlessPressed"),
        }
    }
}

impl<'de> Deserialize<'de> for InlayHintsEnabled {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;

        impl serde::de::Visitor<'_> for Visitor {
            type Value = InlayHintsEnabled;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(
                    "a boolean, or one of \"on\", \"off\", \"onUnlessPressed\", \
                     \"offUnlessPressed\"",
                )
            }

            fn visit_bool<E: serde::de::Error>(self, enabled: bool) -> Result<Self::Value, E> {
                Ok(enabled.into())
            }

            fn visit_str<E: serde::de::Error>(self, name: &str) -> Result<Self::Value, E> {
                InlayHintsEnabled::NAMES
                    .iter()
                    .find_map(|&(spelled, mode)| (spelled == name).then_some(mode))
                    .ok_or_else(|| E::invalid_value(serde::de::Unexpected::Str(name), &self))
            }
        }

        deserializer.deserialize_any(Visitor)
    }
}

#[cfg(feature = "schema")]
impl schemars::JsonSchema for InlayHintsEnabled {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        std::borrow::Cow::Borrowed("InlayHintsEnabled")
    }

    fn json_schema(_generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "anyOf": [
                {"type": "boolean"},
                {"type": "string", "enum": ["on", "off", "onUnlessPressed", "offUnlessPressed"]}
            ]
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(json: &str) -> Result<InlayHintsEnabled, serde_json::Error> {
        serde_json::from_str(json)
    }

    #[test]
    fn booleans_and_every_vs_code_spelling_read() -> Result<(), serde_json::Error> {
        assert_eq!(read("true")?, InlayHintsEnabled::On);
        assert_eq!(read("false")?, InlayHintsEnabled::Off);
        assert_eq!(read("\"on\"")?, InlayHintsEnabled::On);
        assert_eq!(read("\"off\"")?, InlayHintsEnabled::Off);
        assert_eq!(
            read("\"onUnlessPressed\"")?,
            InlayHintsEnabled::OnUnlessPressed
        );
        assert_eq!(
            read("\"offUnlessPressed\"")?,
            InlayHintsEnabled::OffUnlessPressed
        );
        Ok(())
    }

    #[test]
    fn anything_else_is_rejected_with_the_choices() {
        let error = read("\"sometimes\"").err().map(|e| e.to_string());
        assert!(
            error
                .as_deref()
                .is_some_and(|e| e.contains("onUnlessPressed")),
            "{error:?}"
        );
        assert!(read("1").is_err());
    }

    /// The plain modes write back as booleans, so an existing configuration
    /// round-trips byte for byte; the others as their VS Code names.
    #[test]
    fn modes_round_trip() -> Result<(), serde_json::Error> {
        assert_eq!(serde_json::to_string(&InlayHintsEnabled::On)?, "true");
        assert_eq!(serde_json::to_string(&InlayHintsEnabled::Off)?, "false");
        for (_, mode) in InlayHintsEnabled::NAMES {
            assert_eq!(read(&serde_json::to_string(&mode)?)?, mode);
        }
        Ok(())
    }

    #[test]
    fn shown_and_momentary_follow_the_mode() {
        let cases = [
            (InlayHintsEnabled::On, true, false),
            (InlayHintsEnabled::Off, false, false),
            (InlayHintsEnabled::OnUnlessPressed, true, true),
            (InlayHintsEnabled::OffUnlessPressed, false, true),
        ];
        for (mode, shown, momentary) in cases {
            assert_eq!(
                (mode.shown(), mode.momentary()),
                (shown, momentary),
                "{mode:?}"
            );
        }
        assert_eq!(InlayHintsEnabled::default(), InlayHintsEnabled::On);
        assert_eq!(InlayHints::default().enabled, InlayHintsEnabled::On);
    }
}
