//! Production and test seams for establishing language-server connections.

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use karet_lsp::LspClient;
use karet_lsp::LspError;
use karet_lsp::LspSpec;

/// How the manager establishes a client for a spec — [`LspClient::spawn`] in
/// production; tests inject an in-memory duplex connection instead.
pub(crate) type Connector = Arc<
    dyn Fn(LspSpec, PathBuf) -> Pin<Box<dyn Future<Output = Result<LspClient, LspError>> + Send>>
        + Send
        + Sync,
>;

/// Run the server through karet's crash-safe process supervisor.
/// A headless host that supplied no supervisor fails closed.
pub(super) fn spawn_connector(
    supervisor: Option<PathBuf>,
    registry_root: Option<PathBuf>,
) -> Connector {
    Arc::new(move |spec, root| {
        let supervisor = supervisor.clone();
        let registry_root = registry_root.clone();
        Box::pin(async move {
            let supervisor = supervisor.ok_or(LspError::Spawn)?;
            if let Some(registry_root) = registry_root {
                let stream = crate::lsp_broker::connect(&supervisor, &registry_root, &spec, &root)
                    .await
                    .map_err(|error| {
                        tracing::warn!(error = %error, "shared LSP broker connection failed");
                        LspError::Spawn
                    })?;
                let (read, write) = tokio::io::split(stream);
                return LspClient::connect(read, write, &root).await;
            }
            let command = crate::process_supervisor::command(
                &supervisor,
                spec.command.clone(),
                spec.args.clone(),
                &root,
            )
            .map_err(|_| LspError::Spawn)?;
            LspClient::spawn_command(command, &spec.command, &root).await
        })
    })
}
