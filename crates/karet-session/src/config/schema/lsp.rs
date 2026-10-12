//! `lsp.*` — language-server integration.
//!
//! In its own module because a server entry carries more than a launch: the
//! `settings` a server pulls with `workspace/configuration` and the
//! `initializationOptions` it reads at the handshake are free-form JSON owned
//! by each server, not by karet.

use std::collections::BTreeMap;

use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;

/// `lsp.*` — language-server integration.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct Lsp {
    /// Run language servers for open documents (powers completions).
    pub enabled: bool,
    /// Whether Karet may install a missing managed fallback.
    pub managed_downloads: ManagedDownloads,
    /// Per-language server launch configurations, keyed by the lowercase
    /// language name (e.g. `"rust"`, `"typescript"`, `"python"`). Entries are
    /// merged *over* the built-in defaults (rust → `rust-analyzer`,
    /// typescript/javascript → `typescript-language-server --stdio`,
    /// python → `pyright-langserver --stdio`), so setting a language here
    /// overrides its default and unlisted languages keep theirs.
    ///
    /// An entry with no `command` keeps karet's own launch and only adds its
    /// `settings` and `initializationOptions` -- keyed by a language, to the
    /// language's server; keyed by a provider id (`"rust-analyzer"`,
    /// `"pyright"`), to that provider wherever it runs.
    pub servers: BTreeMap<String, LspServer>,
    /// Per-language provider order and exclusive capability owners.
    pub languages: BTreeMap<String, LspLanguage>,
}

impl Default for Lsp {
    fn default() -> Self {
        Self {
            enabled: true,
            managed_downloads: ManagedDownloads::Prompt,
            servers: BTreeMap::new(),
            languages: BTreeMap::new(),
        }
    }
}

impl Lsp {
    /// Whether `self` and `other` differ only in their servers' `settings`.
    ///
    /// The one change a live reload can hand to running servers
    /// (`workspace/didChangeConfiguration`) instead of restarting them: every
    /// other field decides what is launched, or what it is told at the
    /// handshake.
    #[must_use]
    pub fn differs_only_in_settings(&self, other: &Self) -> bool {
        let launch = |lsp: &Self| {
            let mut lsp = lsp.clone();
            for server in lsp.servers.values_mut() {
                server.settings = None;
            }
            lsp
        };
        self != other && launch(self) == launch(other)
    }
}

/// Managed language-server download policy.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub enum ManagedDownloads {
    /// Show exact release metadata and require confirmation.
    #[default]
    Prompt,
    /// Install an approved managed fallback automatically.
    Auto,
    /// Never discover or download managed language servers.
    Off,
}

/// How to launch one language server (see [`Lsp::servers`]).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct LspServer {
    /// Whether this provider participates in resolution.
    pub enabled: bool,
    /// The server executable, looked up on `PATH` (or an absolute path).
    /// Empty keeps karet's own launch.
    pub command: String,
    /// Command-line arguments.
    #[serde(default)]
    pub args: Vec<String>,
    /// The server's settings, keyed by section the way VS Code writes them
    /// (`{ "rust-analyzer": { "cargo": { "features": "all" } } }`). Answered to
    /// the server's `workspace/configuration` requests and pushed with
    /// `workspace/didChangeConfiguration` at startup and on every reload that
    /// changes them -- without restarting the server.
    pub settings: Option<Value>,
    /// Sent verbatim as `initializationOptions` with `initialize`. Read only
    /// at the handshake, so a reload that changes it restarts the server.
    pub initialization_options: Option<Value>,
}

impl Default for LspServer {
    fn default() -> Self {
        Self {
            enabled: true,
            command: String::new(),
            args: Vec::new(),
            settings: None,
            initialization_options: None,
        }
    }
}

/// Provider selection and exclusive capability owners for one language.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct LspLanguage {
    /// Ordered provider IDs. The first capable provider owns intelligence.
    pub servers: Vec<String>,
    /// Explicit formatting owner, when repository defaults should be overridden.
    pub formatter: Option<String>,
    /// Explicit semantic-token owner.
    pub semantic_tokens: Option<String>,
    /// Additional providers whose diagnostics should be merged.
    pub diagnostics: Vec<String>,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn lsp_server_overrides_deserialize_camel_case() {
        let parsed: Lsp = serde_json::from_str(
            r#"{ "enabled": false,
                 "servers": { "rust": { "command": "ra-custom", "args": ["--log"] } } }"#,
        )
        .unwrap_or_default();
        assert!(!parsed.enabled);
        let rust = parsed.servers.get("rust");
        assert_eq!(rust.map(|s| s.command.as_str()), Some("ra-custom"));
        assert_eq!(rust.map(|s| s.args.clone()), Some(vec!["--log".to_owned()]));
        // `args` is optional.
        let parsed: LspServer =
            serde_json::from_str(r#"{ "command": "pylsp" }"#).unwrap_or(LspServer {
                command: String::new(),
                args: vec!["sentinel".into()],
                ..LspServer::default()
            });
        assert_eq!(parsed.command, "pylsp");
        assert!(parsed.args.is_empty());
    }

    #[test]
    fn settings_and_initialization_options_deserialize_verbatim() {
        let parsed: LspServer = serde_json::from_str(
            r#"{ "settings": { "rust-analyzer": { "cargo": { "features": "all" } } },
                 "initializationOptions": { "lens": false } }"#,
        )
        .unwrap_or_default();
        assert!(parsed.enabled, "an entry with no `enabled` stays on");
        assert!(parsed.command.is_empty());
        assert_eq!(
            parsed.settings,
            Some(json!({ "rust-analyzer": { "cargo": { "features": "all" } } }))
        );
        assert_eq!(
            parsed.initialization_options,
            Some(json!({ "lens": false }))
        );
    }

    fn with_server(server: LspServer) -> Lsp {
        let mut lsp = Lsp::default();
        lsp.servers.insert("gopls".to_owned(), server);
        lsp
    }

    #[test]
    fn only_a_settings_change_is_a_settings_only_change() {
        let base = with_server(LspServer {
            settings: Some(json!({ "gopls": { "gofumpt": false } })),
            ..LspServer::default()
        });
        let settings = with_server(LspServer {
            settings: Some(json!({ "gopls": { "gofumpt": true } })),
            ..LspServer::default()
        });
        let options = with_server(LspServer {
            settings: Some(json!({ "gopls": { "gofumpt": true } })),
            initialization_options: Some(json!({ "x": 1 })),
            ..LspServer::default()
        });
        assert!(base.differs_only_in_settings(&settings));
        assert!(
            !base.differs_only_in_settings(&base),
            "no change is not one"
        );
        assert!(!base.differs_only_in_settings(&options));
        let mut disabled = settings.clone();
        disabled.enabled = false;
        assert!(!base.differs_only_in_settings(&disabled));
    }
}
