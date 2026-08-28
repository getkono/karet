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

/// Classify a handshake that failed against a server the shared broker owns.
///
/// The broker sits between karet and the process, so the two ways the
/// handshake can end without an answer mean different things and must not be
/// collapsed:
///
/// - `Closed` is the broker's report of a dead child. `Exited`, not `Host`:
///   classifying it as a host problem told the restart policy that a retry
///   might help. Since the app always has a registry directory, and therefore
///   always takes this branch, that made "stop retrying what can never start"
///   unreachable in production.
/// - `Timeout` is the 30s request deadline expiring, which says nothing about
///   the child being dead — a server still indexing a large workspace looks
///   exactly like this. Reporting it as `Exited` disabled such a server for
///   the rest of the session, so it maps to the non-permanent `Timeout`.
fn brokered_handshake_failure(spec: &LspSpec, error: LspError) -> LspError {
    let cause = match error {
        LspError::Closed => karet_lsp::LaunchCause::Exited,
        LspError::Timeout => karet_lsp::LaunchCause::Timeout,
        other => return other,
    };
    LspError::Launch(Box::new(
        LaunchFailure::new(spec.command.clone(), spec.args.clone(), cause)
            .with_stderr(vec![error.to_string()]),
    ))
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
                .map_err(|error| brokered_handshake_failure(&spec, error));
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

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> LspSpec {
        LspSpec::new("gopls", vec!["serve".into()], vec!["go".into()])
    }

    /// The failure a brokered handshake error is classified as, or `None` when
    /// it was not turned into a launch failure at all.
    fn failure(error: LspError) -> Option<LaunchFailure> {
        match brokered_handshake_failure(&spec(), error) {
            LspError::Launch(failure) => Some(*failure),
            _ => None,
        }
    }

    /// The broker watched the process, so a handshake that closes means the
    /// server behind it never came up: permanent, and the runtime stops.
    #[test]
    fn a_closed_brokered_handshake_is_a_dead_child() {
        let cause = failure(LspError::Closed).map(|failure| failure.cause);
        assert_eq!(cause, Some(karet_lsp::LaunchCause::Exited));
        assert!(cause.is_some_and(karet_lsp::LaunchCause::is_permanent));
    }

    /// The 30s request deadline is not evidence of a dead child. A server that
    /// is merely slow to answer `initialize` -- a large workspace still being
    /// indexed -- must keep its retries instead of being disabled for the rest
    /// of the session.
    #[test]
    fn a_brokered_handshake_that_timed_out_keeps_its_retries() {
        let reported = failure(LspError::Timeout);
        assert_eq!(
            reported.as_ref().map(|failure| failure.cause),
            Some(karet_lsp::LaunchCause::Timeout)
        );
        assert_eq!(
            reported
                .as_ref()
                .map(|failure| failure.cause.is_permanent()),
            Some(false),
            "a slow server is not a dead one"
        );
        assert_eq!(
            reported.as_ref().map(LaunchFailure::command_line),
            Some("gopls serve".to_owned())
        );
        assert_eq!(
            reported.as_ref().map(LaunchFailure::diagnosis),
            Some("request timed out".to_owned())
        );
    }

    /// Anything the server itself answered is a protocol failure, not a launch
    /// one, and travels unchanged.
    #[test]
    fn any_other_handshake_error_passes_through() {
        assert!(failure(LspError::Server("boom".into())).is_none());
    }
}
