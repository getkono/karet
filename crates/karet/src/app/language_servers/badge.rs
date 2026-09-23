//! The compact, glanceable language-server condition shown beside a pane's file.
//!
//! The badge exists to answer one question without opening anything: is the
//! language server for *this* file working, and if not, whose move is it? That
//! second half is why the states separate by *cause* rather than by lifecycle
//! alone. "Nothing is installed" and "the binary is there and keeps dying" are
//! the same lifecycle dead end but opposite user actions, and collapsing them --
//! as a single `Unavailable` state once did -- left the badge unable to say
//! which one it meant.

use std::path::Path;

use karet_session::LanguageServerRuntimeState;
use karet_session::LanguageServerSource;
use karet_session::LanguageServerStatus;

use crate::app::util::path_contains_or_equals;

/// One file's language-server condition, separated by cause.
///
/// The discriminant order *is* the severity order: `Ord` is derived so the
/// worst condition across several providers is `max()`, and adding a state
/// means placing it at its severity rather than editing a lookup table.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum LanguageServerBadge {
    /// A provider covers this language but is switched off in settings.
    Off,
    /// Resolvable and healthy; nothing has needed it yet.
    Idle,
    /// Connected and synchronized.
    Ready,
    /// Connecting.
    Starting,
    /// Dropped, and karet is reconnecting on its own.
    Retrying,
    /// Missing, and only the user can supply it: it needs their SDK or
    /// toolchain, or they declined the offer to install it.
    NeedsSetup,
    /// Missing, and karet can install it once the user approves.
    NotInstalled,
    /// The executable is present and does not work.
    Failed,
}

impl LanguageServerBadge {
    /// Whether this condition needs nothing from anyone.
    pub(crate) fn is_healthy(self) -> bool {
        matches!(self, Self::Idle | Self::Ready)
    }
}

/// A pane's badge: the worst condition among the providers covering its file,
/// plus how many of them are healthy.
///
/// The count is what keeps "worst wins" honest. A Python file is served by
/// Pyright *and* Ruff, so a missing Ruff would otherwise paint the whole file
/// as broken while Pyright is answering every request normally.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct LanguageServerBadgeSummary {
    /// The worst condition among the covering providers.
    pub(crate) state: LanguageServerBadge,
    /// How many covering providers are healthy.
    pub(crate) healthy: usize,
    /// How many providers cover this file.
    pub(crate) total: usize,
}

impl LanguageServerBadgeSummary {
    /// Whether the count is worth showing: it only informs when providers
    /// disagree, so the common single-provider case stays a bare glyph.
    pub(crate) fn shows_count(self) -> bool {
        self.total > 1 && self.healthy < self.total
    }
}

/// One provider's condition for a file, or `None` when it does not cover it.
///
/// Resolution is checked before the runtime state on purpose. A provider that
/// resolved to no executable has never run, so its recorded runtime state is
/// either stale or a meaningless `Idle`; what the user needs to know is that
/// nothing is installed and which kind of install it would take.
fn provider_badge(status: &LanguageServerStatus, path: &Path) -> Option<LanguageServerBadge> {
    if !status.enabled {
        return Some(LanguageServerBadge::Off);
    }
    let mut worst = None;
    let covering = status
        .instances
        .iter()
        .filter(|instance| path_contains_or_equals(&instance.root, path));
    for instance in covering {
        let badge =
            if instance.command.is_none() || instance.source == LanguageServerSource::Unavailable {
                unresolved_badge(status)
            } else {
                match instance.runtime {
                    LanguageServerRuntimeState::Idle => LanguageServerBadge::Idle,
                    LanguageServerRuntimeState::Starting => LanguageServerBadge::Starting,
                    LanguageServerRuntimeState::Running => LanguageServerBadge::Ready,
                    LanguageServerRuntimeState::Retrying => LanguageServerBadge::Retrying,
                    // The circuit breaker protecting the editor from a crash loop is
                    // an implementation detail; to the user the provider is broken.
                    LanguageServerRuntimeState::CircuitOpen
                    | LanguageServerRuntimeState::Unavailable => LanguageServerBadge::Failed,
                    // `LanguageServerRuntimeState` is `#[non_exhaustive]`: a state
                    // added upstream is a condition this build cannot name, which is
                    // a failure to report rather than a silent success.
                    _ => LanguageServerBadge::Failed,
                }
            };
        worst = Some(worst.map_or(badge, |current: LanguageServerBadge| current.max(badge)));
    }
    worst
}

/// Which kind of missing a provider with no executable is.
fn unresolved_badge(status: &LanguageServerStatus) -> LanguageServerBadge {
    if status.manual_install_reason.is_some() || status.declined || !status.managed {
        LanguageServerBadge::NeedsSetup
    } else {
        LanguageServerBadge::NotInstalled
    }
}

/// The badge for `path` in `language`, or `None` when no provider covers it.
pub(crate) fn badge_for(
    servers: &[LanguageServerStatus],
    path: &Path,
    language: &str,
) -> Option<LanguageServerBadgeSummary> {
    let claimants = servers.iter().filter(|status| {
        status
            .languages
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(language))
    });
    let mut state: Option<LanguageServerBadge> = None;
    let mut healthy = 0usize;
    let mut total = 0usize;
    for status in claimants {
        let Some(badge) = provider_badge(status, path) else {
            continue;
        };
        // A provider the user switched off is not a provider that is missing, so it
        // is not counted: a running Pyright beside a deliberately disabled Ruff is
        // working completely, and reporting it as `1/2` would invite the user to go
        // and fix a decision they made. It still sets the state when it is the only
        // thing covering the file, which is how `off` is ever seen.
        if badge != LanguageServerBadge::Off {
            total = total.saturating_add(1);
            if badge.is_healthy() {
                healthy = healthy.saturating_add(1);
            }
        }
        state = Some(state.map_or(badge, |current| current.max(badge)));
    }
    state.map(|state| LanguageServerBadgeSummary {
        state,
        healthy,
        total,
    })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use karet_session::LanguageServerId;
    use karet_session::LanguageServerInstanceStatus;

    use super::*;

    fn instance(
        root: &str,
        source: LanguageServerSource,
        command: Option<&str>,
        runtime: LanguageServerRuntimeState,
    ) -> LanguageServerInstanceStatus {
        LanguageServerInstanceStatus {
            root: PathBuf::from(root),
            source,
            command: command.map(str::to_owned),
            args: Vec::new(),
            runtime,
            open_documents: 0,
            error: None,
        }
    }

    fn status(
        key: &'static str,
        language: &str,
        instances: Vec<LanguageServerInstanceStatus>,
    ) -> LanguageServerStatus {
        LanguageServerStatus {
            server: LanguageServerId::new(key),
            languages: vec![language.to_owned()],
            enabled: true,
            managed: true,
            manual_install_reason: None,
            installed: None,
            ever_installed: false,
            declined: false,
            cleanup_pending: false,
            instances,
        }
    }

    fn running(key: &'static str, language: &str) -> LanguageServerStatus {
        status(
            key,
            language,
            vec![instance(
                "/repo",
                LanguageServerSource::Managed,
                Some("server"),
                LanguageServerRuntimeState::Running,
            )],
        )
    }

    fn missing(key: &'static str, language: &str) -> LanguageServerStatus {
        status(
            key,
            language,
            vec![instance(
                "/repo",
                LanguageServerSource::Unavailable,
                None,
                LanguageServerRuntimeState::Idle,
            )],
        )
    }

    #[test]
    fn severity_order_is_the_discriminant_order() {
        let mut states = vec![
            LanguageServerBadge::Failed,
            LanguageServerBadge::Off,
            LanguageServerBadge::NotInstalled,
            LanguageServerBadge::Ready,
            LanguageServerBadge::Retrying,
            LanguageServerBadge::Idle,
            LanguageServerBadge::NeedsSetup,
            LanguageServerBadge::Starting,
        ];
        states.sort_unstable();
        assert_eq!(
            states,
            vec![
                LanguageServerBadge::Off,
                LanguageServerBadge::Idle,
                LanguageServerBadge::Ready,
                LanguageServerBadge::Starting,
                LanguageServerBadge::Retrying,
                LanguageServerBadge::NeedsSetup,
                LanguageServerBadge::NotInstalled,
                LanguageServerBadge::Failed,
            ]
        );
    }

    #[test]
    fn no_provider_for_the_language_has_no_badge() {
        let servers = vec![running("rust-analyzer", "rust")];
        assert_eq!(badge_for(&servers, Path::new("/repo/a.py"), "python"), None);
    }

    #[test]
    fn a_provider_with_no_instance_for_this_path_has_no_badge() {
        let servers = vec![running("rust-analyzer", "rust")];
        assert_eq!(
            badge_for(&servers, Path::new("/elsewhere/a.rs"), "rust"),
            None
        );
    }

    #[test]
    fn a_running_provider_is_ready_without_a_count() {
        let servers = vec![running("rust-analyzer", "rust")];
        let badge = badge_for(&servers, Path::new("/repo/a.rs"), "rust");
        assert_eq!(
            badge,
            Some(LanguageServerBadgeSummary {
                state: LanguageServerBadge::Ready,
                healthy: 1,
                total: 1,
            })
        );
        assert!(!badge.is_some_and(LanguageServerBadgeSummary::shows_count));
    }

    #[test]
    fn language_match_ignores_case() {
        let servers = vec![running("rust-analyzer", "Rust")];
        assert!(badge_for(&servers, Path::new("/repo/a.rs"), "rust").is_some());
    }

    #[test]
    fn a_disabled_provider_reads_off_rather_than_missing() {
        let mut server = running("rust-analyzer", "rust");
        server.enabled = false;
        let badge = badge_for(&[server], Path::new("/repo/a.rs"), "rust");
        assert_eq!(
            badge.map(|badge| badge.state),
            Some(LanguageServerBadge::Off)
        );
    }

    #[test]
    fn a_disabled_provider_is_off_even_with_no_instances() {
        let mut server = status("rust-analyzer", "rust", Vec::new());
        server.enabled = false;
        let badge = badge_for(&[server], Path::new("/repo/a.rs"), "rust");
        assert_eq!(
            badge.map(|badge| badge.state),
            Some(LanguageServerBadge::Off)
        );
    }

    #[test]
    fn a_managed_provider_with_no_executable_is_not_installed() {
        let servers = vec![missing("rust-analyzer", "rust")];
        assert_eq!(
            badge_for(&servers, Path::new("/repo/a.rs"), "rust").map(|badge| badge.state),
            Some(LanguageServerBadge::NotInstalled)
        );
    }

    #[test]
    fn a_manual_provider_with_no_executable_needs_setup() {
        let mut server = missing("gopls", "go");
        server.manual_install_reason = Some("needs the Go toolchain".to_owned());
        assert_eq!(
            badge_for(&[server], Path::new("/repo/a.go"), "go").map(|badge| badge.state),
            Some(LanguageServerBadge::NeedsSetup)
        );
    }

    #[test]
    fn a_declined_provider_needs_setup_rather_than_nagging() {
        let mut server = missing("rust-analyzer", "rust");
        server.declined = true;
        assert_eq!(
            badge_for(&[server], Path::new("/repo/a.rs"), "rust").map(|badge| badge.state),
            Some(LanguageServerBadge::NeedsSetup)
        );
    }

    #[test]
    fn an_unmanaged_provider_karet_cannot_install_needs_setup() {
        let mut server = missing("company-rust", "rust");
        server.managed = false;
        assert_eq!(
            badge_for(&[server], Path::new("/repo/a.rs"), "rust").map(|badge| badge.state),
            Some(LanguageServerBadge::NeedsSetup)
        );
    }

    #[test]
    fn a_present_executable_that_gave_up_is_failed_not_missing() {
        let servers = vec![status(
            "rust-analyzer",
            "rust",
            vec![instance(
                "/repo",
                LanguageServerSource::Managed,
                Some("rust-analyzer"),
                LanguageServerRuntimeState::Unavailable,
            )],
        )];
        assert_eq!(
            badge_for(&servers, Path::new("/repo/a.rs"), "rust").map(|badge| badge.state),
            Some(LanguageServerBadge::Failed)
        );
    }

    #[test]
    fn an_open_circuit_is_failed() {
        let servers = vec![status(
            "rust-analyzer",
            "rust",
            vec![instance(
                "/repo",
                LanguageServerSource::Managed,
                Some("rust-analyzer"),
                LanguageServerRuntimeState::CircuitOpen,
            )],
        )];
        assert_eq!(
            badge_for(&servers, Path::new("/repo/a.rs"), "rust").map(|badge| badge.state),
            Some(LanguageServerBadge::Failed)
        );
    }

    #[test]
    fn each_runtime_state_maps_to_its_badge() {
        for (runtime, expected) in [
            (LanguageServerRuntimeState::Idle, LanguageServerBadge::Idle),
            (
                LanguageServerRuntimeState::Starting,
                LanguageServerBadge::Starting,
            ),
            (
                LanguageServerRuntimeState::Running,
                LanguageServerBadge::Ready,
            ),
            (
                LanguageServerRuntimeState::Retrying,
                LanguageServerBadge::Retrying,
            ),
        ] {
            let servers = vec![status(
                "rust-analyzer",
                "rust",
                vec![instance(
                    "/repo",
                    LanguageServerSource::Managed,
                    Some("rust-analyzer"),
                    runtime,
                )],
            )];
            assert_eq!(
                badge_for(&servers, Path::new("/repo/a.rs"), "rust").map(|badge| badge.state),
                Some(expected),
                "runtime {runtime:?}"
            );
        }
    }

    #[test]
    fn a_healthy_primary_beside_a_missing_companion_reports_the_count() {
        let servers = vec![running("pyright", "python"), missing("ruff", "python")];
        let badge = badge_for(&servers, Path::new("/repo/a.py"), "python");
        assert_eq!(
            badge,
            Some(LanguageServerBadgeSummary {
                state: LanguageServerBadge::NotInstalled,
                healthy: 1,
                total: 2,
            })
        );
        assert!(badge.is_some_and(LanguageServerBadgeSummary::shows_count));
    }

    #[test]
    fn two_healthy_providers_show_no_count() {
        let servers = vec![running("pyright", "python"), running("ruff", "python")];
        let badge = badge_for(&servers, Path::new("/repo/a.py"), "python");
        assert_eq!(
            badge,
            Some(LanguageServerBadgeSummary {
                state: LanguageServerBadge::Ready,
                healthy: 2,
                total: 2,
            })
        );
        assert!(!badge.is_some_and(LanguageServerBadgeSummary::shows_count));
    }

    #[test]
    fn the_worst_instance_of_one_provider_wins() {
        let servers = vec![status(
            "rust-analyzer",
            "rust",
            vec![
                instance(
                    "/repo",
                    LanguageServerSource::Managed,
                    Some("rust-analyzer"),
                    LanguageServerRuntimeState::Running,
                ),
                instance(
                    "/repo/nested",
                    LanguageServerSource::Managed,
                    Some("rust-analyzer"),
                    LanguageServerRuntimeState::CircuitOpen,
                ),
            ],
        )];
        let badge = badge_for(&servers, Path::new("/repo/nested/a.rs"), "rust");
        assert_eq!(
            badge,
            Some(LanguageServerBadgeSummary {
                state: LanguageServerBadge::Failed,
                healthy: 0,
                total: 1,
            })
        );
    }
}
