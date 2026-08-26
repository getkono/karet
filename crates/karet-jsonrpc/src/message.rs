//! JSON-RPC 2.0 message envelopes.
//!
//! Outgoing messages are strongly typed serialize-only structs; incoming messages
//! are classified from a parsed [`Value`] by shape — a `method` marks a request or
//! notification (split on the presence of `id`), anything else with an `id` is a
//! response to one of our requests.

use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;

/// The protocol version stamped on every message.
pub const JSONRPC_VERSION: &str = "2.0";

/// JSON-RPC error code for a method the receiving side does not implement.
pub const METHOD_NOT_FOUND: i64 = -32601;

/// A JSON-RPC request identifier: a number or a string, per the spec.
///
/// This crate only ever *allocates* [`RequestId::Number`]s, but a nonconforming
/// peer may answer with the id stringified, so both shapes are correlated.
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(untagged)]
pub enum RequestId {
    /// A numeric id.
    Number(i64),
    /// A string id.
    Text(String),
}

impl std::fmt::Display for RequestId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Number(id) => write!(f, "{id}"),
            Self::Text(id) => f.write_str(id),
        }
    }
}

/// A request we send to the peer.
#[derive(Serialize)]
pub struct OutgoingRequest<'a, P: Serialize> {
    jsonrpc: &'static str,
    /// The id the peer must echo in its response.
    pub id: RequestId,
    /// The method being invoked.
    pub method: &'a str,
    /// The request parameters.
    pub params: P,
}

impl<'a, P: Serialize> OutgoingRequest<'a, P> {
    /// Build a request envelope.
    #[must_use]
    pub fn new(id: RequestId, method: &'a str, params: P) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION,
            id,
            method,
            params,
        }
    }
}

/// A notification we send to the peer.
#[derive(Serialize)]
pub struct OutgoingNotification<'a, P: Serialize> {
    jsonrpc: &'static str,
    /// The method being notified.
    pub method: &'a str,
    /// The notification parameters.
    pub params: P,
}

impl<'a, P: Serialize> OutgoingNotification<'a, P> {
    /// Build a notification envelope.
    #[must_use]
    pub fn new(method: &'a str, params: P) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION,
            method,
            params,
        }
    }
}

/// Our response to a peer-initiated request. The `id` is echoed verbatim
/// (peers may use string ids).
///
/// The asymmetry with [`OutgoingRequest::id`] is deliberate: ids the peer chose
/// must be echoed **byte-identically**, whereas ids we allocate must be
/// **matched**, which is what [`RequestId`] is for.
#[derive(Serialize)]
pub struct OutgoingResponse {
    jsonrpc: &'static str,
    /// The peer's request id, echoed verbatim.
    pub id: Value,
    /// The successful result, when the request succeeded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    /// The failure, when the request failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ResponseError>,
}

impl OutgoingResponse {
    /// A response carrying `outcome` for the request identified by `id`.
    #[must_use]
    pub fn new(id: Value, outcome: Result<Value, ResponseError>) -> Self {
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
pub struct ResponseError {
    /// The JSON-RPC error code.
    pub code: i64,
    /// A human-readable message.
    pub message: String,
}

impl ResponseError {
    /// The standard "no such method" failure for `method`.
    #[must_use]
    pub fn method_not_found(method: &str) -> Self {
        Self {
            code: METHOD_NOT_FOUND,
            message: format!("method not found: {method}"),
        }
    }
}

/// A parsed incoming message.
#[derive(Debug)]
pub enum Incoming {
    /// A response to a request we issued.
    Response {
        /// The request id being answered.
        id: RequestId,
        /// The result, or the peer's error.
        result: Result<Value, ResponseError>,
    },
    /// A peer-initiated request expecting a response.
    Request {
        /// The peer's id, echoed back verbatim in our response.
        id: Value,
        /// The request method.
        method: String,
        /// The request params (or `Null`).
        params: Value,
    },
    /// A peer notification.
    Notification {
        /// The notification method.
        method: String,
        /// The notification params (or `Null`).
        params: Value,
    },
}

/// Classify one incoming message; `None` when the value has no JSON-RPC shape.
///
/// Response ids are accepted in both spec-legal shapes — a number that fits an
/// `i64`, or a string. Any other id shape (a float, an object) still yields
/// `None`, exactly as a shapeless value does.
#[must_use]
pub fn classify(mut value: Value) -> Option<Incoming> {
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
    let id = match id? {
        Value::Number(number) => RequestId::Number(number.as_i64()?),
        Value::String(text) => RequestId::Text(text),
        _ => return None,
    };
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

    type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

    #[test]
    fn serializes_request_notification_and_response() {
        let req = serde_json::to_value(OutgoingRequest::new(
            RequestId::Number(7),
            "initialize",
            json!({"a": 1}),
        ))
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

    #[test]
    fn serializes_a_string_request_id() {
        let req = serde_json::to_value(OutgoingRequest::new(
            RequestId::Text("call-1".to_owned()),
            "session/prompt",
            Value::Null,
        ))
        .unwrap_or_default();
        assert_eq!(
            req,
            json!({"jsonrpc": "2.0", "id": "call-1", "method": "session/prompt", "params": null})
        );
    }

    #[test]
    fn displays_both_id_shapes() {
        assert_eq!(RequestId::Number(12).to_string(), "12");
        assert_eq!(RequestId::Text("abc".to_owned()).to_string(), "abc");
    }

    #[test]
    fn method_not_found_names_the_method() {
        let error = ResponseError::method_not_found("window/showMessageRequest");
        assert_eq!(error.code, METHOD_NOT_FOUND);
        assert!(error.message.contains("window/showMessageRequest"));
    }

    #[test]
    fn classifies_responses() -> TestResult {
        let Some(Incoming::Response { id, result }) =
            classify(json!({"jsonrpc": "2.0", "id": 4, "result": {"ok": true}}))
        else {
            return Err("expected a response".into());
        };
        assert_eq!(id, RequestId::Number(4));
        assert_eq!(result.ok(), Some(json!({"ok": true})));

        let Some(Incoming::Response { id, result }) = classify(
            json!({"jsonrpc": "2.0", "id": 5, "error": {"code": -32600, "message": "bad"}}),
        ) else {
            return Err("expected a response".into());
        };
        assert_eq!(id, RequestId::Number(5));
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
        // Neither a number that overflows `i64` nor a structured id is legal.
        assert!(classify(json!({"jsonrpc": "2.0", "id": 1.5, "result": 1})).is_none());
        assert!(classify(json!({"jsonrpc": "2.0", "id": {"n": 1}, "result": 1})).is_none());
    }

    #[test]
    fn classifies_a_string_id_response() -> TestResult {
        // The id widening: a string-id response is a response, not a shapeless
        // value. Correlation is exact `RequestId` equality, so a peer answering
        // our numeric id as a string still goes unmatched — but it is now
        // *classified*, which is what a string-id protocol (ACP) needs.
        let Some(Incoming::Response { id, result }) =
            classify(json!({"jsonrpc": "2.0", "id": "x", "result": 1}))
        else {
            return Err("expected a response".into());
        };
        assert_eq!(id, RequestId::Text("x".to_owned()));
        assert_eq!(result.ok(), Some(json!(1)));
        Ok(())
    }
}
