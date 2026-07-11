//! JSON-RPC 2.0 message envelopes, as LSP uses them.
//!
//! Outgoing messages are strongly typed serialize-only structs; incoming messages
//! are classified from a parsed [`Value`] by shape — a `method` marks a request or
//! notification (split on the presence of `id`), anything else with an `id` is a
//! response to one of our requests.

use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;

/// The protocol version stamped on every message.
pub(crate) const JSONRPC_VERSION: &str = "2.0";

/// JSON-RPC error code for a method the receiving side does not implement.
pub(crate) const METHOD_NOT_FOUND: i64 = -32601;

/// A request we send to the server.
#[derive(Serialize)]
pub(crate) struct OutgoingRequest<'a, P: Serialize> {
    jsonrpc: &'static str,
    pub(crate) id: i64,
    pub(crate) method: &'a str,
    pub(crate) params: P,
}

impl<'a, P: Serialize> OutgoingRequest<'a, P> {
    /// Build a request envelope.
    pub(crate) fn new(id: i64, method: &'a str, params: P) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION,
            id,
            method,
            params,
        }
    }
}

/// A notification we send to the server.
#[derive(Serialize)]
pub(crate) struct OutgoingNotification<'a, P: Serialize> {
    jsonrpc: &'static str,
    pub(crate) method: &'a str,
    pub(crate) params: P,
}

impl<'a, P: Serialize> OutgoingNotification<'a, P> {
    /// Build a notification envelope.
    pub(crate) fn new(method: &'a str, params: P) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION,
            method,
            params,
        }
    }
}

/// Our response to a server-initiated request. The `id` is echoed verbatim
/// (servers may use string ids).
#[derive(Serialize)]
pub(crate) struct OutgoingResponse {
    jsonrpc: &'static str,
    pub(crate) id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) error: Option<ResponseError>,
}

impl OutgoingResponse {
    /// A response carrying `outcome` for the request identified by `id`.
    pub(crate) fn new(id: Value, outcome: Result<Value, ResponseError>) -> Self {
        let (result, error) = match outcome {
            Ok(v) => (Some(v), None),
            Err(e) => (None, Some(e)),
        };
        Self {
            jsonrpc: JSONRPC_VERSION,
            id,
            result,
            error,
        }
    }
}

/// The `error` member of a response.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct ResponseError {
    /// The JSON-RPC error code.
    pub(crate) code: i64,
    /// A human-readable message.
    pub(crate) message: String,
}

/// A parsed incoming message.
#[derive(Debug)]
pub(crate) enum Incoming {
    /// A response to a request we issued (ids we issue are always numeric).
    Response {
        /// The request id being answered.
        id: i64,
        /// The result, or the server's error.
        result: Result<Value, ResponseError>,
    },
    /// A server-initiated request expecting a response.
    Request {
        /// The server's id, echoed back verbatim in our response.
        id: Value,
        /// The request method.
        method: String,
        /// The request params (or `Null`).
        params: Value,
    },
    /// A server notification.
    Notification {
        /// The notification method.
        method: String,
        /// The notification params (or `Null`).
        params: Value,
    },
}

/// Classify one incoming message; `None` when the value has no JSON-RPC shape.
pub(crate) fn classify(mut value: Value) -> Option<Incoming> {
    let obj = value.as_object_mut()?;
    let id = obj.remove("id");
    let params = obj.remove("params").unwrap_or(Value::Null);
    if let Some(method) = obj.get("method").and_then(Value::as_str) {
        let method = method.to_owned();
        return Some(match id {
            Some(id) => Incoming::Request { id, method, params },
            None => Incoming::Notification { method, params },
        });
    }
    let id = id?.as_i64()?;
    let result = match obj.remove("error") {
        Some(err) => Err(ResponseError {
            code: err.get("code").and_then(Value::as_i64).unwrap_or_default(),
            message: err
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("malformed error response")
                .to_owned(),
        }),
        None => Ok(obj.remove("result").unwrap_or(Value::Null)),
    };
    Some(Incoming::Response { id, result })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn serializes_request_notification_and_response() {
        let req = serde_json::to_value(OutgoingRequest::new(7, "initialize", json!({"a": 1})))
            .unwrap_or_default();
        assert_eq!(
            req,
            json!({"jsonrpc": "2.0", "id": 7, "method": "initialize", "params": {"a": 1}})
        );

        let note = serde_json::to_value(OutgoingNotification::new("exit", Value::Null))
            .unwrap_or_default();
        assert_eq!(
            note,
            json!({"jsonrpc": "2.0", "method": "exit", "params": null})
        );

        let ok = serde_json::to_value(OutgoingResponse::new(json!("abc"), Ok(Value::Null)))
            .unwrap_or_default();
        assert_eq!(ok, json!({"jsonrpc": "2.0", "id": "abc", "result": null}));

        let err = serde_json::to_value(OutgoingResponse::new(
            json!(3),
            Err(ResponseError {
                code: METHOD_NOT_FOUND,
                message: "nope".into(),
            }),
        ))
        .unwrap_or_default();
        assert_eq!(
            err,
            json!({"jsonrpc": "2.0", "id": 3, "error": {"code": -32601, "message": "nope"}})
        );
    }

    type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

    #[test]
    fn classifies_responses() -> TestResult {
        let Some(Incoming::Response { id, result }) =
            classify(json!({"jsonrpc": "2.0", "id": 4, "result": {"ok": true}}))
        else {
            return Err("expected a response".into());
        };
        assert_eq!(id, 4);
        assert_eq!(result.ok(), Some(json!({"ok": true})));

        let Some(Incoming::Response { id, result }) = classify(
            json!({"jsonrpc": "2.0", "id": 5, "error": {"code": -32600, "message": "bad"}}),
        ) else {
            return Err("expected a response".into());
        };
        assert_eq!(id, 5);
        let Err(e) = result else {
            return Err("expected an error result".into());
        };
        assert_eq!((e.code, e.message.as_str()), (-32600, "bad"));
        Ok(())
    }

    #[test]
    fn classifies_requests_and_notifications() -> TestResult {
        let Some(Incoming::Request { id, method, params }) = classify(
            json!({"jsonrpc": "2.0", "id": "s1", "method": "workspace/configuration", "params": {"items": []}}),
        ) else {
            return Err("expected a request".into());
        };
        assert_eq!(id, json!("s1"));
        assert_eq!(method, "workspace/configuration");
        assert_eq!(params, json!({"items": []}));

        let Some(Incoming::Notification { method, params }) =
            classify(json!({"jsonrpc": "2.0", "method": "window/logMessage"}))
        else {
            return Err("expected a notification".into());
        };
        assert_eq!(method, "window/logMessage");
        assert_eq!(params, Value::Null);
        Ok(())
    }

    #[test]
    fn rejects_shapeless_values() {
        assert!(classify(json!("just a string")).is_none());
        assert!(classify(json!({"jsonrpc": "2.0"})).is_none());
        // A string id on a *response* cannot be ours (we issue numeric ids).
        assert!(classify(json!({"jsonrpc": "2.0", "id": "x", "result": 1})).is_none());
    }
}
