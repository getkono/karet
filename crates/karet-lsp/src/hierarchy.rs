//! Call and type hierarchies: `textDocument/prepareCallHierarchy` and
//! `textDocument/prepareTypeHierarchy` find the item at a position, and
//! `callHierarchy/incomingCalls`, `callHierarchy/outgoingCalls`,
//! `typeHierarchy/supertypes` and `typeHierarchy/subtypes` grow the tree one
//! level from an item.
//!
//! An item is kept in its wire form as the [`HierarchyItem::payload`], and that
//! form -- not one rebuilt from the neutral fields -- is what a follow-up hands
//! back: the protocol requires the item exactly as the server gave it, `data`
//! and every field this crate does not model included.

use std::path::Path;

use karet_core::HierarchyDirection;
use karet_core::HierarchyItem;
use karet_core::LineCol;
use karet_core::Location;
use serde_json::Value;
use serde_json::json;

use crate::LspClient;
use crate::LspError;
use crate::convert;
use crate::text_document_position;
use crate::uri;

/// The request that finds the item at a position, for a direction.
fn prepare_method(direction: HierarchyDirection) -> &'static str {
    if direction.is_calls() {
        "textDocument/prepareCallHierarchy"
    } else {
        "textDocument/prepareTypeHierarchy"
    }
}

/// The request that grows the tree one level in `direction`, and the field of
/// each answer entry that holds the item (`None`: the entry is the item).
fn children_method(direction: HierarchyDirection) -> (&'static str, Option<&'static str>) {
    match direction {
        HierarchyDirection::IncomingCalls => ("callHierarchy/incomingCalls", Some("from")),
        HierarchyDirection::OutgoingCalls => ("callHierarchy/outgoingCalls", Some("to")),
        HierarchyDirection::Supertypes => ("typeHierarchy/supertypes", None),
        // `HierarchyDirection` is non-exhaustive; a direction added later is a
        // type walk until it says otherwise.
        _ => ("typeHierarchy/subtypes", None),
    }
}

impl LspClient {
    /// The hierarchy items at `pos` in `doc`, the roots a hierarchy in
    /// `direction` grows from. Usually one; empty when there is no symbol
    /// there the server can place in a hierarchy.
    ///
    /// # Errors
    /// Returns [`LspError::Unsupported`] **without issuing a request** when the
    /// server did not advertise the hierarchy `direction` walks, else
    /// [`LspError::Server`] or [`LspError::Timeout`].
    pub async fn prepare_hierarchy(
        &self,
        direction: HierarchyDirection,
        doc: &Path,
        pos: LineCol,
    ) -> Result<Vec<HierarchyItem>, LspError> {
        let method = prepare_method(direction);
        self.require_for(direction.feature(), method, doc)?;
        let params = text_document_position(doc, pos)?;
        let items: Option<Vec<Value>> = self.conn.request(method, params).await?;
        Ok(items
            .unwrap_or_default()
            .into_iter()
            .filter_map(item_from_wire)
            .collect())
    }

    /// The items one level from `item` in `direction`: its callers, its
    /// callees, its supertypes or its subtypes, in the server's order.
    ///
    /// `item` must be one this connection's server produced, by
    /// [`Self::prepare_hierarchy`] or an earlier call to this.
    ///
    /// # Errors
    /// Returns [`LspError::Unsupported`] **without issuing a request** when the
    /// server neither advertised nor registered the hierarchy for any
    /// document, [`LspError::Protocol`] when
    /// `item`'s payload is not an item's wire form, else [`LspError::Server`]
    /// or [`LspError::Timeout`].
    pub async fn hierarchy_children(
        &self,
        direction: HierarchyDirection,
        item: &HierarchyItem,
    ) -> Result<Vec<HierarchyItem>, LspError> {
        let (method, field) = children_method(direction);
        // Gated on the feature alone, not on `item`'s document: a follow-up
        // carries no text document, and the item's file is often one never
        // opened, whose language a selector-scoped registration cannot match.
        self.require(direction.feature(), method)?;
        let wire: Value = serde_json::from_str(&item.payload)
            .map_err(|error| LspError::Protocol(format!("hierarchy item payload: {error}")))?;
        let entries: Option<Vec<Value>> =
            self.conn.request(method, json!({ "item": wire })).await?;
        Ok(entries
            .unwrap_or_default()
            .into_iter()
            .filter_map(|mut entry| match field {
                Some(field) => entry.get_mut(field).map(Value::take),
                None => Some(entry),
            })
            .filter_map(item_from_wire)
            .collect())
    }

    /// Resolve the supertypes of the type at `pos` — what it derives from or implements.
    ///
    /// Two round trips, as the protocol requires: `prepare` establishes the item, and
    /// `supertypes` walks upward from it. A server that declines the first returns no
    /// supertypes rather than an error, since not supporting the request is not a failure.
    ///
    /// # Errors
    /// Returns [`LspError::Unsupported`] **without issuing a request** when the
    /// server did not advertise the capability, else [`LspError::Server`] or
    /// [`LspError::Timeout`].
    pub async fn supertypes(&self, doc: &Path, pos: LineCol) -> Result<Vec<Location>, LspError> {
        let direction = HierarchyDirection::Supertypes;
        let roots = self.prepare_hierarchy(direction, doc, pos).await?;
        let Some(item) = roots.first() else {
            return Ok(Vec::new());
        };
        let supertypes = self.hierarchy_children(direction, item).await?;
        Ok(supertypes.into_iter().map(|item| item.location).collect())
    }
}

/// A call or type hierarchy item -- the two share one shape on the wire --
/// with its wire form kept as the payload. `None` for an entry that is not an
/// item, or that names a file by a non-file URI.
fn item_from_wire(wire: Value) -> Option<HierarchyItem> {
    let item: lsp_types::CallHierarchyItem = serde_json::from_value(wire.clone()).ok()?;
    Some(HierarchyItem {
        name: item.name,
        kind: convert::symbol_kind_from_lsp(item.kind),
        detail: item.detail.filter(|detail| !detail.is_empty()),
        location: Location {
            path: uri::uri_to_path(&item.uri)?,
            range: convert::range_from_lsp(item.selection_range),
        },
        payload: wire.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use karet_core::SymbolKind;

    use super::*;

    fn wire(name: &str) -> Value {
        json!({
            "name": name,
            "kind": 12,
            "detail": "fn()",
            "uri": "file:///tmp/ws/lib.rs",
            "range": {"start": {"line": 1, "character": 0}, "end": {"line": 3, "character": 1}},
            "selectionRange": {"start": {"line": 1, "character": 3}, "end": {"line": 1, "character": 7}},
            "data": {"opaque": [1, 2]},
        })
    }

    /// The neutral fields come from the item, the location is its selection
    /// range, and the payload is the item verbatim, `data` included: rebuilt
    /// from the neutral fields it would lose `data`, which servers require
    /// back to answer a follow-up.
    #[test]
    fn an_item_keeps_its_selection_range_and_its_wire_form() -> Result<(), serde_json::Error> {
        let item = item_from_wire(wire("main"));
        assert!(item.is_some(), "a well-formed item is kept");
        let Some(item) = item else { return Ok(()) };
        assert_eq!(item.name, "main");
        assert_eq!(item.kind, SymbolKind::Function);
        assert_eq!(item.detail.as_deref(), Some("fn()"));
        assert_eq!(item.location.path, Path::new("/tmp/ws/lib.rs"));
        assert_eq!(item.location.range.start, LineCol::new(1, 3));
        let payload: Value = serde_json::from_str(&item.payload)?;
        assert_eq!(payload, wire("main"));
        Ok(())
    }

    /// An entry that is not an item, or names a non-file URI, is dropped
    /// rather than failing the whole level.
    #[test]
    fn malformed_and_non_file_entries_are_dropped() {
        assert!(item_from_wire(json!({"name": "x"})).is_none());
        let mut remote = wire("x");
        remote["uri"] = json!("untitled:Untitled-1");
        assert!(item_from_wire(remote).is_none());
    }

    /// Each direction asks its own method, and a call answer's item is read
    /// from the field that holds the *other* end of the call.
    #[test]
    fn each_direction_asks_its_own_method() {
        assert_eq!(
            prepare_method(HierarchyDirection::OutgoingCalls),
            "textDocument/prepareCallHierarchy"
        );
        assert_eq!(
            prepare_method(HierarchyDirection::Subtypes),
            "textDocument/prepareTypeHierarchy"
        );
        assert_eq!(
            [
                HierarchyDirection::IncomingCalls,
                HierarchyDirection::OutgoingCalls,
                HierarchyDirection::Supertypes,
                HierarchyDirection::Subtypes,
            ]
            .map(children_method),
            [
                ("callHierarchy/incomingCalls", Some("from")),
                ("callHierarchy/outgoingCalls", Some("to")),
                ("typeHierarchy/supertypes", None),
                ("typeHierarchy/subtypes", None),
            ]
        );
    }
}
