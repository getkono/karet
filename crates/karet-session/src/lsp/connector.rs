//! Production and test seams for establishing language-server connections.

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use karet_lsp::LaunchFailure;
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

/// Turn a host-side failure into one that still names the launch it belonged to.
///
/// Every failure on this path used to collapse into a bare `LspError::Spawn`,
/// whose message was the same four words whether the supervisor was missing,
/// the broker was unreachable, or the server itself had died. The spec is the
/// one piece of context the host always has, so it is always attached.
fn host_failure(spec: &LspSpec, detail: impl std::fmt::Display) -> LspError {
    LspError::Launch(Box::new(LaunchFailure::host(
        spec.command.clone(),
        spec.args.clone(),
        detail.to_string(),
    )))
}

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
            let Some(supervisor) = supervisor else {
                return Err(host_failure(
                    &spec,
                    "this karet build has no process supervisor, so it cannot run language servers",
                ));
            };
            if let Some(registry_root) = registry_root {
                let stream =
                    karet_supervisor::broker::connect(&supervisor, &registry_root, &spec, &root)
                        .await
                        .map_err(|error| {
                            tracing::warn!(error = %error, "shared LSP broker connection failed");
                            match &error {
                                // The broker watched the process itself, so its
                                // verdict beats any guess made out here.
                                karet_supervisor::broker::BrokerError::Launch(reported)
                                    if reported.ran =>
                                {
                                    LspError::Launch(Box::new(
                                        LaunchFailure::new(
                                            spec.command.clone(),
                                            spec.args.clone(),
                                            karet_lsp::LaunchCause::Exited,
                                        )
                                        .with_stderr(vec![reported.message.clone()]),
                                    ))
                                },
                                _ => host_failure(&spec, error),
                            }
                        })?;
                let (read, write) = tokio::io::split(stream);
                return LspClient::connect_with(
                    read,
                    write,
                    &root,
                    spec.initialization_options.clone(),
                )
                .await
                .map_err(|error| match error {
                    // The broker owns the process, so a handshake that closes
                    // means the server behind it never came up. `Exited`, not
                    // `Host`: this is the broker's report of a dead child, and
                    // classifying it as a host problem told the restart policy
                    // that a retry might help. Since the app always has a
                    // registry directory, and therefore always takes this
                    // branch, that made "stop retrying what can never start"
                    // unreachable in production.
                    LspError::Closed | LspError::Timeout => LspError::Launch(Box::new(
                        LaunchFailure::new(
                            spec.command.clone(),
                            spec.args.clone(),
                            karet_lsp::LaunchCause::Exited,
                        )
                        .with_stderr(vec![error.to_string()]),
                    )),
                    other => other,
                });
            }
            let command = karet_supervisor::supervisor::command(
                &supervisor,
                spec.command.clone(),
                spec.args.clone(),
                &root,
            )
            .map_err(|error| host_failure(&spec, error))?;
            LspClient::spawn_command_with(
                command,
                &spec.command,
                &root,
                spec.initialization_options.clone(),
            )
            .await
        })
    })
}
