//! Production and test seams for establishing language-server connections.

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use karet_lsp::LaunchFailure;
use karet_lsp::LspClient;
use karet_lsp::LspError;
use karet_lsp::LspSpec;
use karet_supervisor::broker::BrokeredLaunchFailure;
use karet_supervisor::broker::Launch;

/// How the manager establishes a client for a spec — [`LspClient::spawn`] in
/// production; tests inject an in-memory duplex connection instead.
pub(crate) type Connector = Arc<
    dyn Fn(LspSpec, PathBuf) -> Pin<Box<dyn Future<Output = Result<LspClient, LspError>> + Send>>
        + Send
        + Sync,
>;

/// Turn a host-side failure into one that still names the launch it belonged to.
///
/// Every failure on this path used to collapse into one bare spawn error whose
/// message was the same four words whether the supervisor was missing, the
/// broker was unreachable, or the server itself had died. The spec is the one
/// piece of context the host always has, so it is always attached.
fn host_failure(spec: &LspSpec, detail: impl std::fmt::Display) -> LspError {
    LspError::Launch(Box::new(LaunchFailure::host(
        spec.command.clone(),
        spec.args.clone(),
        detail.to_string(),
    )))
}

/// A failure this host will never get past, however many times it tries.
fn unsupported_host(spec: &LspSpec, detail: &str) -> LspError {
    LspError::Launch(Box::new(
        LaunchFailure::new(
            spec.command.clone(),
            spec.args.clone(),
            karet_lsp::LaunchCause::Unsupported,
        )
        .with_detail(detail),
    ))
}

/// The broker's description of this launch: what runs, and where.
fn launch_of(spec: &LspSpec, root: &std::path::Path) -> Launch {
    Launch {
        command: spec.command.clone(),
        args: spec.args.clone(),
        root: root.to_path_buf(),
    }
}

/// Turn a broker's own report of a dead child into a launch failure.
///
/// The broker watched the process, so its verdict beats any guess made out
/// here — and it is the only thing on this path that can justify a permanent
/// one. The tail it carries is the server's own last words, which is what makes
/// the failure diagnosable: without it the brokered fork could say no more than
/// "connection closed" about a server that had explained itself on stderr.
fn reported_failure(spec: &LspSpec, reported: &BrokeredLaunchFailure) -> LspError {
    let stderr = if reported.stderr.is_empty() {
        vec![reported.message.clone()]
    } else {
        reported.stderr.clone()
    };
    LspError::Launch(Box::new(
        LaunchFailure::new(
            spec.command.clone(),
            spec.args.clone(),
            karet_lsp::LaunchCause::Exited,
        )
        .with_stderr(stderr),
    ))
}

/// Classify a handshake that failed against a server the shared broker owns.
///
/// The broker sits between karet and the process, so the ways the handshake can
/// end without an answer mean different things and must not be collapsed:
///
/// - `Closed` with a matching report is the broker saying its child died.
///   `Exited`, and permanent: classifying it as a host problem told the restart
///   policy that a retry might help, and since the app always has a registry
///   directory it always takes this branch, which made "stop retrying what can
///   never start" unreachable in production.
/// - `Closed` with **no** report is a fact about a TCP socket and nothing more.
///   The connector cannot tell its own broker from any other listener that
///   inherited the address in a stale endpoint file, so a server that was
///   perfectly runnable was written off for the session on the strength of a
///   stranger hanging up. Non-permanent: the broker layer has already
///   invalidated that endpoint, so the next attempt elects a real broker.
/// - `Timeout` is the 30s request deadline expiring, which says nothing about
///   the child being dead — a server still indexing a large workspace looks
///   exactly like this. Reporting it as `Exited` disabled such a server for
///   the rest of the session, so it maps to the non-permanent `Timeout`.
fn brokered_handshake_failure(
    spec: &LspSpec,
    error: LspError,
    reported: Option<&BrokeredLaunchFailure>,
) -> LspError {
    match (&error, reported) {
        (LspError::Closed, Some(reported)) if reported.ran => reported_failure(spec, reported),
        (LspError::Closed, _) => host_failure(
            spec,
            "the shared language-server broker closed the connection without answering the \
             handshake",
        ),
        (LspError::Timeout, _) => LspError::Launch(Box::new(
            LaunchFailure::new(
                spec.command.clone(),
                spec.args.clone(),
                karet_lsp::LaunchCause::Timeout,
            )
            .with_stderr(vec![error.to_string()]),
        )),
        _ => error,
    }
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
                // Not `Host`: a host problem may be over by the next attempt,
                // and this one cannot be. The supervisor path is fixed when the
                // manager is built, so every retry runs the identical
                // impossible launch -- which is what it did, four "Retrying"
                // states in three seconds and then a five-minute circuit
                // repeating for the session.
                return Err(unsupported_host(
                    &spec,
                    "this karet build has no process supervisor, so it cannot run language servers",
                ));
            };
            if let Some(registry_root) = registry_root {
                let launch = launch_of(&spec, &root);
                let (stream, broker) = karet_supervisor::broker::connect_observed(
                    &supervisor,
                    &registry_root,
                    &launch,
                )
                .await
                .map_err(|error| {
                    tracing::warn!(error = %error, "shared LSP broker connection failed");
                    match &error {
                        // The broker watched the process itself, so its verdict
                        // beats any guess made out here.
                        karet_supervisor::broker::BrokerError::Launch(reported) if reported.ran => {
                            reported_failure(&spec, reported)
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
                .map_err(|error| {
                    // Only the broker that served this connection can convict
                    // the server, and only through the report it stamped with
                    // its own process id.
                    let reported =
                        karet_supervisor::broker::reported_failure(&registry_root, &launch, broker);
                    brokered_handshake_failure(&spec, error, reported.as_ref())
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

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> LspSpec {
        LspSpec::new("gopls", vec!["serve".into()], vec!["go".into()])
    }

    /// A report from the broker that served the connection.
    fn report(stderr: Vec<String>) -> BrokeredLaunchFailure {
        BrokeredLaunchFailure {
            command: "gopls".to_owned(),
            args: vec!["serve".to_owned()],
            message: "language server connection closed".to_owned(),
            stderr,
            ran: true,
            pid: std::process::id(),
        }
    }

    /// The failure a brokered handshake error is classified as, or `None` when
    /// it was not turned into a launch failure at all.
    fn failure(error: LspError) -> Option<LaunchFailure> {
        classify(error, None)
    }

    fn classify(
        error: LspError,
        reported: Option<&BrokeredLaunchFailure>,
    ) -> Option<LaunchFailure> {
        match brokered_handshake_failure(&spec(), error, reported) {
            LspError::Launch(failure) => Some(*failure),
            _ => None,
        }
    }

    /// The broker watched the process, so its report of a dead child is what
    /// makes this permanent — and the runtime stops.
    #[test]
    fn a_closed_handshake_the_broker_reported_is_a_dead_child() {
        let reported = report(vec!["Error: Cannot find module 'x'".to_owned()]);
        let failure = classify(LspError::Closed, Some(&reported));
        assert_eq!(
            failure.as_ref().map(|failure| failure.cause),
            Some(karet_lsp::LaunchCause::Exited)
        );
        assert!(
            failure
                .as_ref()
                .is_some_and(|failure| failure.cause.is_permanent())
        );
        assert_eq!(
            failure.as_ref().map(LaunchFailure::diagnosis),
            Some("Error: Cannot find module 'x'".to_owned()),
            "the report's tail is the whole reason the brokered fork is diagnosable"
        );
    }

    /// A broker that reported nothing about the server still names the failure
    /// it did see, rather than leaving the diagnosis empty.
    #[test]
    fn a_report_without_a_tail_falls_back_to_the_brokers_own_words() {
        let failure = classify(LspError::Closed, Some(&report(Vec::new())));
        assert_eq!(
            failure.as_ref().map(LaunchFailure::diagnosis),
            Some("language server connection closed".to_owned())
        );
    }

    /// The stale-endpoint defect. On the brokered fork a `Closed` is a fact
    /// about a TCP socket: `connect_existing` writes a prelude and reads
    /// nothing back, so an endpoint file whose port the OS had recycled put the
    /// handshake into a stranger, whose hang-up was then read as this server
    /// exiting. Permanent, and self-sustaining — nothing re-elected, so every
    /// later attempt reached the same stranger and the user was told a
    /// perfectly runnable server would not be retried.
    #[test]
    fn a_closed_handshake_nobody_reported_is_not_a_dead_child() {
        let failure = failure(LspError::Closed);
        assert_eq!(
            failure.as_ref().map(|failure| failure.cause),
            Some(karet_lsp::LaunchCause::Host)
        );
        assert_eq!(
            failure.as_ref().map(|failure| failure.cause.is_permanent()),
            Some(false),
            "a socket closing is not evidence that any process died"
        );
    }

    /// A report is only evidence if the broker got as far as running the
    /// server; anything else is a host problem, and those get retried.
    #[test]
    fn a_report_that_never_ran_the_server_is_not_a_dead_child() {
        let mut reported = report(Vec::new());
        reported.ran = false;
        assert_eq!(
            classify(LspError::Closed, Some(&reported)).map(|failure| failure.cause),
            Some(karet_lsp::LaunchCause::Host)
        );
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

    /// A build with no process supervisor cannot run a language server, and no
    /// number of attempts changes that: the supervisor path is fixed when the
    /// manager is built. Classified as a host problem it read as "a retry might
    /// help", so a headless embedder got `Starting, Retrying, Retrying,
    /// Retrying, Retrying` in three seconds, a "retrying with backoff" toast,
    /// and then the same again every five minutes for the session.
    #[tokio::test]
    async fn a_host_with_no_supervisor_stops_rather_than_retrying_forever()
    -> Result<(), Box<dyn std::error::Error>> {
        let connector = spawn_connector(None, None);
        let Err(LspError::Launch(failure)) = connector(spec(), PathBuf::from("/workspace")).await
        else {
            return Err("a host that cannot launch anything must report a launch failure".into());
        };
        assert_eq!(failure.cause, karet_lsp::LaunchCause::Unsupported);
        assert!(
            failure.cause.is_permanent(),
            "no retry can conjure a supervisor this build does not have"
        );
        assert_eq!(
            failure.diagnosis(),
            "this karet build has no process supervisor, so it cannot run language servers"
        );
        Ok(())
    }
}
