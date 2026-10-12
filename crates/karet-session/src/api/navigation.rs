//! Code navigation beyond Go to Definition, asked with
//! [`Command::FindLocations`](super::Command::FindLocations) and answered by
//! [`Event::Locations`](super::Event::Locations).

use karet_core::ServerFeature;

/// What a [`Command::FindLocations`](super::Command::FindLocations) asks the
/// language server for: every one is a position in, file locations out.
///
/// Go to Definition is not among them: it keeps its own
/// [`Command::Definition`](super::Command::Definition) /
/// [`Event::Definitions`](super::Event::Definitions) pair, which predates these.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
#[derive(serde::Serialize, serde::Deserialize)]
pub enum LocationKind {
    /// Where the symbol is declared (a C prototype, a forward declaration),
    /// for languages that separate declaring from defining.
    Declaration,
    /// Where the symbol's *type* is defined.
    TypeDefinition,
    /// The implementations of an interface, trait or abstract member.
    Implementation,
    /// Every reference to the symbol, its declaration included.
    References,
}

impl LocationKind {
    /// The server capability this request is gated on, and reported under in
    /// [`Event::FeatureUnsupported`](super::Event::FeatureUnsupported).
    #[must_use]
    pub fn feature(self) -> ServerFeature {
        match self {
            Self::Declaration => ServerFeature::Declaration,
            Self::TypeDefinition => ServerFeature::TypeDefinition,
            Self::Implementation => ServerFeature::Implementation,
            Self::References => ServerFeature::References,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each kind is gated on its own capability: a shared or swapped one would
    /// refuse a request the server offers, or issue one it never advertised.
    #[test]
    fn each_kind_is_gated_on_its_own_capability() {
        let kinds = [
            LocationKind::Declaration,
            LocationKind::TypeDefinition,
            LocationKind::Implementation,
            LocationKind::References,
        ];
        let features: Vec<_> = kinds.iter().map(|kind| kind.feature()).collect();
        assert_eq!(
            features,
            vec![
                ServerFeature::Declaration,
                ServerFeature::TypeDefinition,
                ServerFeature::Implementation,
                ServerFeature::References,
            ]
        );
    }
}
