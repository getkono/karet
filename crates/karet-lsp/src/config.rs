//! Server configuration: the settings a server pulls with
//! `workspace/configuration`, and the push that says they changed.
//!
//! The consumer supplies one settings object per connection, shaped the way
//! VS Code's are: keyed by section (`{"rust-analyzer": {"cargo": {...}}}`). A
//! `workspace/configuration` item naming a section gets the value at that
//! dotted path -- a literal key spelled with the dots is tried first, since
//! flattened VS Code settings write `"python.analysis"` as one key -- and an
//! item naming none gets the whole object. A section that is not there is
//! answered `null`, which every server reads as "use your default", exactly
//! what this client answered before it held any settings.

use std::sync::Arc;
use std::sync::RwLock;

use serde_json::Value;

use crate::LspClient;
use crate::LspError;

/// The settings one connection answers `workspace/configuration` from.
pub(crate) type Settings = Arc<RwLock<Value>>;

/// What [`LspClient::connect_configured`] sends at the handshake and answers
/// with afterwards.
///
/// `#[non_exhaustive]` and built through [`ConnectOptions::default`] and the
/// `with_*` methods, so a later knob does not break a consumer.
#[derive(Clone, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct ConnectOptions {
    /// Server-specific `initializationOptions`, sent verbatim with
    /// `initialize`.
    pub initialization_options: Option<Value>,
    /// The settings `workspace/configuration` is answered from, keyed by
    /// section. When present they are also pushed once with
    /// `workspace/didChangeConfiguration` right after `initialized`, for the
    /// servers that read their settings from the push rather than pulling.
    pub settings: Option<Value>,
}

impl ConnectOptions {
    /// Attach the `initializationOptions` sent with `initialize`.
    #[must_use]
    pub fn with_initialization_options(mut self, options: Option<Value>) -> Self {
        self.initialization_options = options;
        self
    }

    /// Attach the settings `workspace/configuration` is answered from.
    #[must_use]
    pub fn with_settings(mut self, settings: Option<Value>) -> Self {
        self.settings = settings;
        self
    }
}

/// The value at `section` in `settings`, or `null`.
pub(crate) fn section(settings: &Value, section: Option<&str>) -> Value {
    let Some(section) = section.filter(|section| !section.is_empty()) else {
        return settings.clone();
    };
    if let Some(value) = settings.get(section) {
        return value.clone();
    }
    section
        .split('.')
        .try_fold(settings, |value, key| value.get(key))
        .cloned()
        .unwrap_or(Value::Null)
}

/// Answer one `workspace/configuration` request: one value per item, in order.
pub(crate) fn answer(settings: &Settings, params: &Value) -> Value {
    let items = params
        .get("items")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let Ok(settings) = settings.read() else {
        return Value::Array(vec![Value::Null; items.len()]);
    };
    Value::Array(
        items
            .iter()
            .map(|item| section(&settings, item.get("section").and_then(Value::as_str)))
            .collect(),
    )
}

/// Replace what `settings` holds.
pub(crate) fn replace(settings: &Settings, value: Value) {
    if let Ok(mut held) = settings.write() {
        *held = value;
    }
}

impl LspClient {
    /// Replace the settings this connection answers `workspace/configuration`
    /// from, and tell the server with `workspace/didChangeConfiguration`.
    ///
    /// The notification carries the whole new object as `settings`: a server
    /// that pulls re-asks for its sections on receipt, and one that reads the
    /// push takes it from there.
    ///
    /// # Errors
    ///
    /// [`LspError::Closed`] when the connection is gone, or
    /// [`LspError::Protocol`] when the outbound queue is full. The stored
    /// settings are replaced either way.
    pub fn did_change_configuration(&self, settings: Value) -> Result<(), LspError> {
        self.conn.set_settings(settings.clone());
        self.conn.notify(
            "workspace/didChangeConfiguration",
            serde_json::json!({ "settings": settings }),
        )
    }
}
