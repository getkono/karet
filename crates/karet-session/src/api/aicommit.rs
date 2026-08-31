//! What the client is told about AI commit-message generation *before* it asks
//! for one.
//!
//! Generation is a multi-second round-trip to an external process, so the two
//! ways it can be unusable — the backend was built without it, or the agent it
//! would run is not installed — must be answerable without running it. The
//! backend resolves and probes the configuration and pushes an
//! [`AiCommitAvailability`] whenever the answer could have changed; a client
//! renders that state rather than discovering it from a failure.

use crate::config::schema::AiCommit;
use crate::config::schema::AiCommitAgent;

/// What the backend found out about one agent CLI.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AiCommitAgentStatus {
    /// The agent this describes.
    pub agent: AiCommitAgent,
    /// Whether a generation could run with it right now.
    pub available: bool,
    /// The version that was found, or why the agent is unusable. Phrased for
    /// display next to the agent in a picker.
    pub detail: String,
}

/// Whether AI commit messages can be generated, and with what.
///
/// Emitted as [`crate::api::Event::AiCommitAvailability`] at startup, whenever
/// settings reload, and on [`crate::api::Command::ProbeAiCommit`].
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AiCommitAvailability {
    /// Whether the backend was built with generation support at all. `false`
    /// makes every other field advisory: nothing can run in this build.
    pub supported: bool,
    /// The `git.aiCommit.enabled` setting.
    pub enabled: bool,
    /// The settings a generation would run under, as resolved.
    pub options: AiCommit,
    /// Per-agent probe results, in [`AiCommitAgent::ALL`] order. Empty when the
    /// build has no generation support.
    pub agents: Vec<AiCommitAgentStatus>,
    /// Set when the configured effort is one the configured agent rejects. The
    /// generation still runs, at the model's default effort — this names the
    /// discrepancy so a client can show it rather than silently disagreeing
    /// with the settings file.
    #[serde(default)]
    pub effort_conflict: Option<String>,
}

impl AiCommitAvailability {
    /// The status of the currently configured agent, if it was probed.
    #[must_use]
    pub fn selected(&self) -> Option<&AiCommitAgentStatus> {
        self.agents
            .iter()
            .find(|status| status.agent == self.options.agent)
    }

    /// Whether pressing "generate" right now would actually start a generation.
    #[must_use]
    pub fn ready(&self) -> bool {
        self.supported && self.enabled && self.selected().is_some_and(|status| status.available)
    }

    /// Why generation is unavailable, or `None` when it is [`ready`](Self::ready).
    ///
    /// Ordered by what the user would have to fix first: a build without the
    /// feature cannot be configured around, a disabled setting outranks a
    /// missing binary, and only then does the agent's own report matter.
    #[must_use]
    pub fn blocker(&self) -> Option<String> {
        if !self.supported {
            return Some("this build has no AI commit support".to_string());
        }
        if !self.enabled {
            return Some("disabled by git.aiCommit.enabled".to_string());
        }
        match self.selected() {
            Some(status) if status.available => None,
            Some(status) => Some(status.detail.clone()),
            None => Some(format!("{} was not probed", self.options.agent.as_str())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn available(agent: AiCommitAgent) -> AiCommitAgentStatus {
        AiCommitAgentStatus {
            agent,
            available: true,
            detail: "1.2.3".to_string(),
        }
    }

    fn availability() -> AiCommitAvailability {
        AiCommitAvailability {
            supported: true,
            enabled: true,
            options: AiCommit::default(),
            agents: AiCommitAgent::ALL.into_iter().map(available).collect(),
            effort_conflict: None,
        }
    }

    #[test]
    fn a_fully_configured_setup_is_ready_with_no_blocker() {
        let status = availability();
        assert!(status.ready());
        assert_eq!(status.blocker(), None);
        assert_eq!(
            status.selected().map(|s| s.agent),
            Some(AiCommitAgent::Claude)
        );
    }

    #[test]
    fn blockers_are_reported_most_fundamental_first() {
        // An unsupported build outranks everything: nothing can fix it at runtime.
        let mut status = availability();
        status.supported = false;
        status.enabled = false;
        assert_eq!(
            status.blocker().as_deref(),
            Some("this build has no AI commit support")
        );

        // Then the setting, which outranks a missing binary.
        let mut status = availability();
        status.enabled = false;
        status.agents = vec![AiCommitAgentStatus {
            agent: AiCommitAgent::Claude,
            available: false,
            detail: "`claude` was not found on PATH".to_string(),
        }];
        assert_eq!(
            status.blocker().as_deref(),
            Some("disabled by git.aiCommit.enabled")
        );

        // Only then the agent's own report, verbatim.
        let mut status = availability();
        status.agents = vec![AiCommitAgentStatus {
            agent: AiCommitAgent::Claude,
            available: false,
            detail: "`claude` was not found on PATH".to_string(),
        }];
        assert!(!status.ready());
        assert_eq!(
            status.blocker().as_deref(),
            Some("`claude` was not found on PATH")
        );
    }

    #[test]
    fn an_unprobed_agent_is_a_blocker_rather_than_ready() {
        let mut status = availability();
        status.agents.clear();
        assert!(!status.ready(), "no probe is not the same as available");
        assert!(status.blocker().is_some_and(|b| b.contains("claude")));
    }

    #[test]
    fn selection_follows_the_configured_agent() {
        let mut status = availability();
        status.options.agent = AiCommitAgent::Codex;
        assert_eq!(
            status.selected().map(|s| s.agent),
            Some(AiCommitAgent::Codex)
        );
        assert!(status.ready());
    }
}
