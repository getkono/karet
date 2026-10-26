//! Call and type hierarchies: a tree of symbols grown one level at a time,
//! each level asked of the producer from the item above it.

use super::Location;
use super::SymbolKind;
use crate::capability::ServerFeature;

/// Which way a hierarchy grows from an item.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub enum HierarchyDirection {
    /// The functions that call the item.
    IncomingCalls,
    /// The functions the item calls.
    OutgoingCalls,
    /// The types the item derives from or implements.
    Supertypes,
    /// The types that derive from or implement the item.
    Subtypes,
}

impl HierarchyDirection {
    /// The server capability every request in this direction is gated on:
    /// the call hierarchy for calls, the type hierarchy for types.
    #[must_use]
    pub fn feature(self) -> ServerFeature {
        if self.is_calls() {
            ServerFeature::CallHierarchy
        } else {
            ServerFeature::TypeHierarchy
        }
    }

    /// Whether this direction walks calls rather than types.
    #[must_use]
    pub fn is_calls(self) -> bool {
        matches!(self, Self::IncomingCalls | Self::OutgoingCalls)
    }
}

/// One symbol in a call or type hierarchy.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct HierarchyItem {
    /// The symbol's name.
    pub name: String,
    /// What kind of symbol it is.
    pub kind: SymbolKind,
    /// More about it, such as a signature or the module it is in.
    pub detail: Option<String>,
    /// Where it is named: the range worth navigating to, not its whole body.
    pub location: Location,
    /// The producer's own encoding of the item, opaque to everyone else.
    ///
    /// Handed back to the producer to grow the hierarchy from this item: a
    /// language server needs the item exactly as it gave it, including data
    /// a consumer never sees.
    pub payload: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Calls are gated on the call hierarchy and types on the type hierarchy:
    /// a swapped gate would refuse a request a server offers, or issue one it
    /// never advertised.
    #[test]
    fn each_direction_is_gated_on_its_own_hierarchy() {
        let gates: Vec<_> = [
            HierarchyDirection::IncomingCalls,
            HierarchyDirection::OutgoingCalls,
            HierarchyDirection::Supertypes,
            HierarchyDirection::Subtypes,
        ]
        .into_iter()
        .map(|direction| (direction.is_calls(), direction.feature()))
        .collect();
        assert_eq!(
            gates,
            vec![
                (true, ServerFeature::CallHierarchy),
                (true, ServerFeature::CallHierarchy),
                (false, ServerFeature::TypeHierarchy),
                (false, ServerFeature::TypeHierarchy),
            ]
        );
    }
}
