//! Call and type hierarchy round trips over the in-memory wire `lib_tests`
//! owns.

use std::path::Path;

use karet_core::HierarchyDirection;
use karet_core::HierarchyItem;
use karet_core::LineCol;
use karet_core::Location;
use karet_core::Range;
use serde_json::Value;
use serde_json::json;

use super::TestResult;
use super::wire;
use crate::LspClient;
use crate::LspError;

#[tokio::test]
async fn supertypes_takes_two_round_trips_and_keeps_selection_ranges() -> TestResult {
    let ((read, write), mut server) = wire();
    let server_task = tokio::spawn(async move {
        server.handshake().await;
        let prepare = server.recv().await;
        assert_eq!(prepare["method"], "textDocument/prepareTypeHierarchy");
        server
            .respond(
                &prepare["id"],
                json!([{
                    "name": "Widget", "kind": 23, "uri": "file:///tmp/ws/lib.rs",
                    "range": {"start": {"line": 0, "character": 0},
                              "end": {"line": 5, "character": 1}},
                    "selectionRange": {"start": {"line": 0, "character": 7},
                                       "end": {"line": 0, "character": 13}}
                }]),
            )
            .await;
        let supertypes = server.recv().await;
        assert_eq!(supertypes["method"], "typeHierarchy/supertypes");
        server
            .respond(
                &supertypes["id"],
                json!([{
                    "name": "Render", "kind": 11, "uri": "file:///tmp/ws/render.rs",
                    "range": {"start": {"line": 1, "character": 0},
                              "end": {"line": 4, "character": 1}},
                    "selectionRange": {"start": {"line": 1, "character": 6},
                                       "end": {"line": 1, "character": 12}}
                }]),
            )
            .await;
    });
    let client = LspClient::connect(read, write, Path::new("/tmp/ws")).await?;
    let found = client
        .supertypes(Path::new("/tmp/ws/lib.rs"), LineCol::new(0, 7))
        .await?;
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].path, Path::new("/tmp/ws/render.rs"));
    // The selection range is the navigable one, not the whole body.
    assert_eq!(found[0].range.start, LineCol::new(1, 6));
    server_task.await?;
    Ok(())
}

/// A call hierarchy item as a server sends it, with `data` it needs back.
fn call_item(name: &str, line: u32) -> Value {
    json!({
        "name": name, "kind": 12, "uri": "file:///tmp/ws/lib.rs",
        "range": {"start": {"line": line, "character": 0},
                  "end": {"line": line + 2, "character": 1}},
        "selectionRange": {"start": {"line": line, "character": 3},
                           "end": {"line": line, "character": 7}},
        "data": {"id": name}
    })
}

/// Incoming calls hand the prepared item back verbatim -- `data` and all --
/// and read each caller from `from`; outgoing calls read each callee from
/// `to`. A follow-up that rebuilt the item from the neutral fields would drop
/// `data`, and servers that key their answer on it would find nothing.
#[tokio::test]
async fn call_hierarchy_hands_the_item_back_and_reads_the_other_end() -> TestResult {
    let ((read, write), mut server) = wire();
    let server_task = tokio::spawn(async move {
        server.handshake().await;
        let prepare = server.recv().await;
        assert_eq!(prepare["method"], "textDocument/prepareCallHierarchy");
        assert_eq!(prepare["params"]["position"]["line"], 4);
        server
            .respond(&prepare["id"], json!([call_item("main", 4)]))
            .await;
        let incoming = server.recv().await;
        assert_eq!(incoming["method"], "callHierarchy/incomingCalls");
        assert_eq!(incoming["params"]["item"], call_item("main", 4));
        let from = json!([{"from": call_item("caller", 9), "fromRanges": []}]);
        server.respond(&incoming["id"], from).await;
        let outgoing = server.recv().await;
        assert_eq!(outgoing["method"], "callHierarchy/outgoingCalls");
        let to = json!([{"to": call_item("callee", 20), "fromRanges": []}, {"bogus": 1}]);
        server.respond(&outgoing["id"], to).await;
    });
    let client = LspClient::connect(read, write, Path::new("/tmp/ws")).await?;
    let doc = Path::new("/tmp/ws/lib.rs");
    let roots = client
        .prepare_hierarchy(HierarchyDirection::IncomingCalls, doc, LineCol::new(4, 3))
        .await?;
    assert_eq!(roots.len(), 1);
    let callers = client
        .hierarchy_children(HierarchyDirection::IncomingCalls, &roots[0])
        .await?;
    assert_eq!(callers.len(), 1);
    assert_eq!(callers[0].name, "caller");
    assert_eq!(callers[0].location.range.start, LineCol::new(9, 3));
    let callees = client
        .hierarchy_children(HierarchyDirection::OutgoingCalls, &roots[0])
        .await?;
    // The entry that names no callee is dropped, not the whole level.
    assert_eq!(callees.len(), 1);
    assert_eq!(callees[0].name, "callee");
    server_task.await?;
    Ok(())
}

/// Subtypes walk down from a prepared type item with their own method.
#[tokio::test]
async fn subtypes_walk_down_from_the_prepared_item() -> TestResult {
    let ((read, write), mut server) = wire();
    let server_task = tokio::spawn(async move {
        server.handshake().await;
        let prepare = server.recv().await;
        assert_eq!(prepare["method"], "textDocument/prepareTypeHierarchy");
        server
            .respond(&prepare["id"], json!([call_item("Shape", 0)]))
            .await;
        let subtypes = server.recv().await;
        assert_eq!(subtypes["method"], "typeHierarchy/subtypes");
        assert_eq!(subtypes["params"]["item"]["data"], json!({"id": "Shape"}));
        server
            .respond(&subtypes["id"], json!([call_item("Circle", 30)]))
            .await;
    });
    let client = LspClient::connect(read, write, Path::new("/tmp/ws")).await?;
    let doc = Path::new("/tmp/ws/lib.rs");
    let roots = client
        .prepare_hierarchy(HierarchyDirection::Subtypes, doc, LineCol::new(0, 3))
        .await?;
    let subtypes = client
        .hierarchy_children(HierarchyDirection::Subtypes, &roots[0])
        .await?;
    assert_eq!(subtypes.len(), 1);
    assert_eq!(subtypes[0].name, "Circle");
    server_task.await?;
    Ok(())
}

/// A payload that is not an item's wire form is a protocol error raised
/// before anything is sent, not a malformed request the server must reject.
#[tokio::test]
async fn a_corrupt_payload_is_refused_without_a_request() -> TestResult {
    let ((read, write), mut server) = wire();
    let server_task = tokio::spawn(async move { server.handshake().await });
    let client = LspClient::connect(read, write, Path::new("/tmp/ws")).await?;
    server_task.await?;
    let item = HierarchyItem {
        name: "x".into(),
        kind: karet_core::SymbolKind::Function,
        detail: None,
        location: Location {
            path: "/tmp/ws/lib.rs".into(),
            range: Range::default(),
        },
        payload: "not json".into(),
    };
    let answer = client
        .hierarchy_children(HierarchyDirection::IncomingCalls, &item)
        .await;
    assert!(matches!(answer, Err(LspError::Protocol(_))), "{answer:?}");
    Ok(())
}

/// A follow-up carries no text document, so a hierarchy registered for one
/// language only still expands an item whose file was never opened -- its
/// language is unknown, and a per-document gate would refuse it as
/// unsupported. Refused without a request before any registration, sent once
/// one exists.
#[tokio::test]
async fn a_selector_scoped_registration_expands_items_in_unopened_files() -> TestResult {
    let ((read, write), mut server) = wire();
    let server_task = tokio::spawn(async move {
        server.handshake_with(json!({})).await;
        server
    });
    let client = LspClient::connect(read, write, Path::new("/tmp/ws")).await?;
    let mut server = server_task.await?;
    let item = HierarchyItem {
        name: "caller".into(),
        kind: karet_core::SymbolKind::Function,
        detail: None,
        location: Location {
            path: "/tmp/ws/unopened.rs".into(),
            range: Range::default(),
        },
        payload: call_item("caller", 9).to_string(),
    };
    let refused = client
        .hierarchy_children(HierarchyDirection::IncomingCalls, &item)
        .await;
    assert!(
        matches!(refused, Err(LspError::Unsupported { .. })),
        "{refused:?}"
    );

    server
        .send(&json!({
            "jsonrpc": "2.0", "id": "reg-1", "method": "client/registerCapability",
            "params": {"registrations": [{
                "id": "r1", "method": "textDocument/prepareCallHierarchy",
                "registerOptions": {"documentSelector": [{"language": "rust"}]}
            }]}
        }))
        .await;
    let ack = server.recv().await;
    assert_eq!(ack["id"], json!("reg-1"));

    let server_task = tokio::spawn(async move {
        let incoming = server.recv().await;
        assert_eq!(incoming["method"], "callHierarchy/incomingCalls");
        let from = json!([{"from": call_item("outer", 30), "fromRanges": []}]);
        server.respond(&incoming["id"], from).await;
    });
    let callers = client
        .hierarchy_children(HierarchyDirection::IncomingCalls, &item)
        .await?;
    assert_eq!(callers.len(), 1);
    assert_eq!(callers[0].name, "outer");
    server_task.await?;
    Ok(())
}

#[tokio::test]
async fn a_server_that_declines_type_hierarchy_yields_no_supertypes() -> TestResult {
    // Not supporting the request is not a failure — the caller degrades to its
    // structural answer rather than surfacing an error.
    let ((read, write), mut server) = wire();
    let server_task = tokio::spawn(async move {
        server.handshake().await;
        let prepare = server.recv().await;
        assert_eq!(prepare["method"], "textDocument/prepareTypeHierarchy");
        server.respond(&prepare["id"], json!(null)).await;
    });
    let client = LspClient::connect(read, write, Path::new("/tmp/ws")).await?;
    let found = client
        .supertypes(Path::new("/tmp/ws/lib.rs"), LineCol::new(0, 7))
        .await?;
    assert!(found.is_empty());
    server_task.await?;
    Ok(())
}
