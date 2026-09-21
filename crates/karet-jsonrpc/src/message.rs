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

/// JSON-RPC error code for a payload that is not valid JSON.
pub const PARSE_ERROR: i64 = -32700;

/// JSON-RPC error code for a well-formed payload that is not a valid request.
pub const INVALID_REQUEST: i64 = -32600;

/// JSON-RPC error code for a method the receiving side does not implement.
pub const METHOD_NOT_FOUND: i64 = -32601;

/// JSON-RPC error code for parameters a known method cannot accept.
pub const INVALID_PARAMS: i64 = -32602;

/// JSON-RPC error code for a failure internal to the answering side.
///
/// This is the honest answer when a request cannot be routed to whatever would
/// have handled it — no consumer is listening, or its queue is full. Answering
/// is mandatory: a peer that receives no reply waits indefinitely, because
/// JSON-RPC gives the *requester* no timeout obligation.
pub const INTERNAL_ERROR: i64 = -32603;

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
    /// Optional structured detail the peer may attach to the failure.
    ///
    /// Omitted from the wire when absent, so a response carrying no detail is
    /// byte-identical to one produced before this field existed.
    ///
    /// Boxed because it is the rare case and `Value` is not small — larger
    /// still where a workspace enables `serde_json/preserve_order`. Inline, it
    /// pushed every `Result<_, ResponseError>` in the crate over the
    /// `result_large_err` threshold. Read it through [`data`](Self::data).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Box<Value>>,
}

impl ResponseError {
    /// A failure with `code` and `message` and no structured detail.
    ///
    /// Prefer this over a struct literal: it keeps call sites compiling when a
    /// future field is added, which a literal cannot do.
    #[must_use]
    pub fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }

    /// Attach structured detail to the failure.
    #[must_use]
    pub fn with_data(mut self, data: Value) -> Self {
        self.data = Some(Box::new(data));
        self
    }

    /// The structured detail the peer attached, if any.
    #[must_use]
    pub fn data(&self) -> Option<&Value> {
        self.data.as_deref()
    }

    /// The standard "no such method" failure for `method`.
    #[must_use]
    pub fn method_not_found(method: &str) -> Self {
        Self::new(METHOD_NOT_FOUND, format!("method not found: {method}"))
    }

    /// The standard "could not be answered" failure for `method`.
    #[must_use]
    pub fn internal_error(method: &str, detail: impl std::fmt::Display) -> Self {
        Self::new(
            INTERNAL_ERROR,
            format!("{method} could not be answered: {detail}"),
        )
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
    /// A peer error that names no request.
    ///
    /// JSON-RPC reserves a **null** id for a failure the peer detected *before*
    /// it could identify which request was at fault — it rejecting something we
    /// sent as unparseable or invalid. It correlates to nothing, so no pending
    /// request can be completed with it, but dropping it silently leaves the
    /// request it refers to waiting out its entire timeout undiagnosed.
    ProtocolError {
        /// The peer's complaint.
        error: ResponseError,
    },
}

/// Classify one incoming message; `None` when the value has no JSON-RPC shape.
///
/// Response ids are accepted in both spec-legal shapes — a number that fits an
/// `i64`, or a string — and a **null** id yields [`Incoming::ProtocolError`]
/// rather than being discarded. Any other id shape (a float, an `i64`
/// overflow, an object) still yields `None`, exactly as a shapeless value does.
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
    let id = id?;
    let result = match obj.remove("error") {
        Some(err) => Err(ResponseError {
            code: err.get("code").and_then(Value::as_i64).unwrap_or_default(),
            message: err
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("malformed error response")
                .to_owned(),
            data: err.get("data").cloned().map(Box::new),
        }),
        None => Ok(obj.remove("result").unwrap_or(Value::Null)),
    };
    let id = match id {
        Value::Number(number) => RequestId::Number(number.as_i64()?),
        Value::String(text) => RequestId::Text(text),
        Value::Null => {
            return Some(Incoming::ProtocolError {
                error: result.err().unwrap_or_else(|| {
                    ResponseError::new(
                        INVALID_REQUEST,
                        "peer sent a null-id response carrying no error",
                    )
                }),
            });
        },
        _ => return None,
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
            Err(ResponseError::new(METHOD_NOT_FOUND, "nope")),
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
        assert!(
            classify(json!({"jsonrpc": "2.0", "id": 18_446_744_073_709_551_615_u64, "result": 1}))
                .is_none()
        );
        assert!(classify(json!({"jsonrpc": "2.0", "id": {"n": 1}, "result": 1})).is_none());
    }

    #[test]
    fn request_ids_round_trip_untagged() -> TestResult {
        for (id, wire) in [
            (RequestId::Number(7), json!(7)),
            (RequestId::Text("call-1".to_owned()), json!("call-1")),
        ] {
            // Untagged: a bare number or a bare string, with no wrapper object.
            let encoded = serde_json::to_value(&id)?;
            assert_eq!(encoded, wire);
            let decoded: RequestId = serde_json::from_value(encoded)?;
            assert_eq!(decoded, id);
        }
        Ok(())
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

    #[test]
    fn a_null_id_error_is_reported_rather_than_dropped() -> TestResult {
        // How a peer says "the thing you sent was unparseable": it cannot name
        // the request, so the id is null. Dropping this as shapeless left the
        // request it refers to waiting out its whole timeout undiagnosed.
        let Some(Incoming::ProtocolError { error }) = classify(
            json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32700, "message": "Parse error"}}),
        ) else {
            return Err("expected a protocol error".into());
        };
        assert_eq!(
            (error.code, error.message.as_str()),
            (PARSE_ERROR, "Parse error")
        );
        Ok(())
    }

    #[test]
    fn a_null_id_response_carrying_no_error_is_still_reported() -> TestResult {
        // Malformed — the spec pairs a null id with an error — but reporting it
        // beats dropping it, for exactly the same reason.
        let Some(Incoming::ProtocolError { error }) =
            classify(json!({"jsonrpc": "2.0", "id": null, "result": 1}))
        else {
            return Err("expected a protocol error".into());
        };
        assert_eq!(error.code, INVALID_REQUEST);
        Ok(())
    }

    #[test]
    fn a_null_id_request_is_still_a_request() -> TestResult {
        // The boundary: `method` decides first, so a null-id *request* keeps
        // classifying as one and is answered with `"id": null`, as the spec's
        // degenerate case requires.
        let Some(Incoming::Request { id, method, .. }) =
            classify(json!({"jsonrpc": "2.0", "id": null, "method": "window/showDocument"}))
        else {
            return Err("expected a request".into());
        };
        assert_eq!(id, Value::Null);
        assert_eq!(method, "window/showDocument");
        Ok(())
    }

    #[test]
    fn error_data_survives_a_round_trip_and_is_omitted_when_absent() -> TestResult {
        // `data` is how a server explains a refusal it wants acted on — an
        // `applyEdit` that failed, say — so it must reach the caller intact.
        let Some(Incoming::Response { result, .. }) = classify(
            json!({"jsonrpc": "2.0", "id": 1, "error": {"code": -32803, "message": "no", "data": {"why": "stale"}}}),
        ) else {
            return Err("expected a response".into());
        };
        let Err(error) = result else {
            return Err("expected an error result".into());
        };
        assert_eq!(error.data(), Some(&json!({"why": "stale"})));

        // Absent `data` stays absent on the wire, so an untouched response is
        // byte-identical to one produced before the field existed.
        let plain = serde_json::to_value(OutgoingResponse::new(
            json!(1),
            Err(ResponseError::new(METHOD_NOT_FOUND, "nope")),
        ))?;
        assert_eq!(
            plain,
            json!({"jsonrpc": "2.0", "id": 1, "error": {"code": METHOD_NOT_FOUND, "message": "nope"}})
        );

        let detailed = serde_json::to_value(OutgoingResponse::new(
            json!(2),
            Err(ResponseError::new(INTERNAL_ERROR, "busy").with_data(json!([1]))),
        ))?;
        assert_eq!(detailed["error"]["data"], json!([1]));
        Ok(())
    }
}
