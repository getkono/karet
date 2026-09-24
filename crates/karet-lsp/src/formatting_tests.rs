//! What the handshake says about `textDocument/formatting`, and what a
//! formatting request says about the buffer's indentation.
//!
//! Split from `lib_tests`, which owns the wire harness and the per-request
//! round trips.

use serde_json::Value;
use serde_json::json;

use super::TestResult;
use super::wire;
use crate::Indentation;
use crate::LspClient;
use crate::Path;
use crate::ServerFeature;
use crate::capability;
use crate::formatting_options;

/// LSP types `documentFormattingProvider` as `boolean | DocumentFormattingOptions`,
/// and treats an unadvertised capability as absent. Getting any of these wrong
/// costs a user their formatter, or costs every save a pointless round trip.
#[test]
fn formatting_capability_reads_every_shape_the_spec_allows() {
    let reads = |capabilities: Value| {
        capability::parse(&json!({"capabilities": capabilities}))
            .supports(ServerFeature::Formatting)
    };

    assert!(reads(json!({"documentFormattingProvider": true})));
    assert!(
        reads(json!({"documentFormattingProvider": {}})),
        "an options object is how a server with work-done support answers"
    );
    assert!(reads(
        json!({"documentFormattingProvider": {"workDoneProgress": true}})
    ));

    assert!(!reads(json!({"documentFormattingProvider": false})));
    assert!(!reads(json!({"documentFormattingProvider": null})));
    assert!(
        !reads(json!({})),
        "an unadvertised capability is not a supported one"
    );
    assert!(
        !capability::parse(&json!({})).supports(ServerFeature::Formatting),
        "a result with no capabilities at all supports nothing"
    );
}

/// The handshake is the first chance to learn this, and for a server that never
/// registers the method later, the only one: a client that drops it has to
/// guess for the rest of the session.
#[tokio::test]
async fn the_handshake_records_whether_the_server_formats() -> TestResult {
    async fn connect_advertising(
        provider: Value,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        let ((read, write), mut server) = wire();
        let server_task = tokio::spawn(async move {
            let init = server.recv().await;
            let id = init["id"].clone();
            server
                .respond(
                    &id,
                    json!({"capabilities": {"documentFormattingProvider": provider}}),
                )
                .await;
            let _initialized = server.recv().await;
        });
        let client = LspClient::connect(read, write, Path::new("/tmp")).await?;
        server_task.await?;
        Ok(client.supports_formatting())
    }

    assert!(connect_advertising(json!(true)).await?);
    assert!(connect_advertising(json!({})).await?);
    assert!(!connect_advertising(json!(false)).await?);
    Ok(())
}

/// `FormattingOptions` is the only place the request states how the buffer is
/// indented, and a server that honours it reindents the whole file to match.
/// Building it from a constant therefore rewrote every formatted file to four
/// spaces regardless of `editor.tabSize` / `editor.insertSpaces`; these values
/// must be the caller's, passed through unchanged.
#[test]
fn formatting_options_state_the_caller_s_indentation() {
    let tabs = formatting_options(Indentation {
        tab_size: 2,
        insert_spaces: false,
    });
    assert_eq!(tabs.tab_size, 2);
    assert!(!tabs.insert_spaces);

    let fallback = formatting_options(Indentation::default());
    assert_eq!(fallback.tab_size, 4);
    assert!(fallback.insert_spaces);
}
