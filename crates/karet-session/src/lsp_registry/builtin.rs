//! Providers compiled into karet: the registry's answer for a server that is
//! never installed, because it ships inside the binary.

use karet_lsp::LspSpec;

use crate::api::LanguageServerId;

/// Whether `server` is compiled into this karet and run in-process, so it never
/// needs installing (feature `toml-lsp`, for taplo).
pub(crate) fn builtin_provider(server: &LanguageServerId) -> bool {
    #[cfg(feature = "toml-lsp")]
    {
        crate::toml_lsp::bundles(server)
    }
    #[cfg(not(feature = "toml-lsp"))]
    {
        let _ = server;
        false
    }
}

/// The in-process launch for a [built-in](builtin_provider) provider: the last
/// resolution step, after configuration, the project, `PATH`, and a managed
/// installation have all come up empty.
pub(crate) fn builtin_spec(server: &LanguageServerId, language: &str) -> Option<LspSpec> {
    #[cfg(feature = "toml-lsp")]
    {
        crate::toml_lsp::spec(server, language)
    }
    #[cfg(not(feature = "toml-lsp"))]
    {
        let _ = (server, language);
        None
    }
}
