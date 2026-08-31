//! `git.aiCommit.*` — generating a commit message from the staged diff.
//!
//! These settings are resolved *before* a generation runs, not discovered by
//! failing one: the backend probes the configured agent and reports what it
//! found (see [`crate::api::Event::AiCommitAvailability`]), so a client can show
//! what will run — and what will not — while the user is still deciding.
//!
//! They live in their own module because the vocabulary is wider than a couple
//! of scalars: two harnesses, a six-value effort scale, and per-harness limits
//! on which of those values are legal.

use serde::Deserialize;
use serde::Serialize;

/// The local agent CLI that drafts the message.
///
/// Each is driven through `agent-text`'s adapter for it, over pipes to the
/// installed executable — karet never talks to a model provider directly.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub enum AiCommitAgent {
    /// Anthropic's `claude` CLI (Claude Code).
    #[default]
    Claude,
    /// OpenAI's `codex` CLI. Requires `codex` 0.146.0 or newer.
    Codex,
}

impl AiCommitAgent {
    /// Every agent, in the order a picker should offer them.
    pub const ALL: [Self; 2] = [Self::Claude, Self::Codex];

    /// The executable this agent runs when `binary` is unset.
    #[must_use]
    pub const fn default_binary(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }

    /// The stable identifier used in settings and in the UI.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }

    /// Whether this agent accepts `effort`.
    ///
    /// The adapters reject the extremes of the shared scale rather than
    /// clamping them, so an unsupported pair is a configuration error we can
    /// catch up front instead of a failure seconds into a generation.
    #[must_use]
    pub const fn supports_effort(self, effort: AiCommitEffort) -> bool {
        !matches!(
            (self, effort),
            (Self::Claude, AiCommitEffort::Minimal) | (Self::Codex, AiCommitEffort::Max)
        )
    }

    /// The efforts this agent accepts, ascending — what a picker should offer
    /// once the agent is chosen.
    #[must_use]
    pub fn supported_efforts(self) -> Vec<AiCommitEffort> {
        AiCommitEffort::ALL
            .into_iter()
            .filter(|effort| self.supports_effort(*effort))
            .collect()
    }
}

/// How much thinking the commit-message model spends (`git.aiCommit.effort`).
///
/// Mirrors `agent_text::ReasoningEffort`. Not every value is legal on every
/// agent — see [`AiCommitAgent::supports_effort`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub enum AiCommitEffort {
    /// The least the model will do. Not supported by `claude`.
    Minimal,
    /// Fastest, cheapest.
    Low,
    /// A balance of speed and quality.
    #[default]
    Medium,
    /// Slower, more thorough.
    High,
    /// Slower still. Spelled `xhigh`, matching the agents' own vocabulary —
    /// `camelCase` would render this variant `xHigh`, which no agent accepts.
    #[serde(rename = "xhigh")]
    XHigh,
    /// The most the model will do. Not supported by `codex`.
    Max,
}

impl AiCommitEffort {
    /// Every effort, ascending.
    pub const ALL: [Self; 6] = [
        Self::Minimal,
        Self::Low,
        Self::Medium,
        Self::High,
        Self::XHigh,
        Self::Max,
    ];

    /// The stable identifier used in settings and in the UI.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::XHigh => "xhigh",
            Self::Max => "max",
        }
    }
}

/// The model name that defers the choice to the diff-size heuristic.
pub const AUTO_MODEL: &str = "auto";

/// How long a single generation may run before it is abandoned, in
/// milliseconds. The generator's own default is five minutes, which is far
/// longer than anyone waits for a commit message.
pub const DEFAULT_TIMEOUT_MS: u64 = 60_000;

/// `git.aiCommit.*` — generate a commit message from the staged diff.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct AiCommit {
    /// Allow generating commit messages from the staged diff. When off, the
    /// generate action reports that it is disabled rather than running.
    pub enabled: bool,
    /// Which agent CLI drafts the message.
    pub agent: AiCommitAgent,
    /// The model to run: `"auto"` picks a cheap model for small diffs and a
    /// stronger one for large or many-file diffs; any other value pins that
    /// model name (e.g. `"haiku"`, `"sonnet"`, or a full model id).
    pub model: String,
    /// Thinking effort for the model. `null` leaves the model's default;
    /// ignored when `model` is `"auto"` (which chooses its own effort).
    pub effort: Option<AiCommitEffort>,
    /// Extra natural-language instructions appended to the prompt (e.g. "mention
    /// the user-visible effect", "reference the ticket in the branch name").
    pub instructions: Vec<String>,
    /// Path to the selected agent's executable. `null` searches `PATH` for
    /// [`AiCommitAgent::default_binary`]. This overrides whichever agent is
    /// selected, so it is cleared when the agent changes.
    pub binary: Option<String>,
    /// How long one generation may run, in milliseconds.
    pub timeout_ms: u64,
}

impl Default for AiCommit {
    fn default() -> Self {
        Self {
            enabled: true,
            agent: AiCommitAgent::Claude,
            model: AUTO_MODEL.to_string(),
            effort: None,
            instructions: Vec::new(),
            binary: None,
            timeout_ms: DEFAULT_TIMEOUT_MS,
        }
    }
}

impl AiCommit {
    /// Whether the model choice is deferred to the diff-size heuristic.
    #[must_use]
    pub fn is_auto_model(&self) -> bool {
        self.model.eq_ignore_ascii_case(AUTO_MODEL)
    }

    /// The executable that will be launched for the configured agent.
    #[must_use]
    pub fn resolved_binary(&self) -> &str {
        self.binary
            .as_deref()
            .map(str::trim)
            .filter(|path| !path.is_empty())
            .unwrap_or_else(|| self.agent.default_binary())
    }

    /// The configured effort, dropped when the selected agent rejects it.
    ///
    /// A settings file can name an effort the agent does not take, and the agent
    /// can change under an already-pinned effort. Rather than failing the
    /// generation, fall back to the model's own default and let
    /// [`AiCommit::effort_conflict`] describe the discrepancy.
    #[must_use]
    pub fn effective_effort(&self) -> Option<AiCommitEffort> {
        self.effort
            .filter(|effort| self.agent.supports_effort(*effort))
    }

    /// The configured effort when the selected agent rejects it, for reporting.
    #[must_use]
    pub fn effort_conflict(&self) -> Option<AiCommitEffort> {
        self.effort
            .filter(|effort| !self.agent.supports_effort(*effort))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_sane() {
        let cfg = AiCommit::default();
        assert!(cfg.enabled);
        assert_eq!(cfg.agent, AiCommitAgent::Claude);
        assert!(cfg.is_auto_model());
        assert_eq!(cfg.effort, None);
        assert_eq!(cfg.timeout_ms, DEFAULT_TIMEOUT_MS);
        assert_eq!(cfg.resolved_binary(), "claude");
    }

    #[test]
    fn each_agent_rejects_exactly_one_extreme() {
        assert!(!AiCommitAgent::Claude.supports_effort(AiCommitEffort::Minimal));
        assert!(AiCommitAgent::Claude.supports_effort(AiCommitEffort::Max));
        assert!(AiCommitAgent::Codex.supports_effort(AiCommitEffort::Minimal));
        assert!(!AiCommitAgent::Codex.supports_effort(AiCommitEffort::Max));
        for agent in AiCommitAgent::ALL {
            let supported = agent.supported_efforts();
            assert_eq!(supported.len(), AiCommitEffort::ALL.len() - 1);
            // Whatever is offered must be ascending and actually accepted.
            assert!(supported.windows(2).all(|pair| pair[0] < pair[1]));
            assert!(supported.iter().all(|e| agent.supports_effort(*e)));
        }
    }

    #[test]
    fn an_unsupported_effort_degrades_instead_of_failing() {
        let cfg = AiCommit {
            agent: AiCommitAgent::Claude,
            effort: Some(AiCommitEffort::Minimal),
            ..AiCommit::default()
        };
        assert_eq!(cfg.effective_effort(), None, "dropped, not passed through");
        assert_eq!(cfg.effort_conflict(), Some(AiCommitEffort::Minimal));

        let cfg = AiCommit {
            agent: AiCommitAgent::Codex,
            effort: Some(AiCommitEffort::Minimal),
            ..AiCommit::default()
        };
        assert_eq!(cfg.effective_effort(), Some(AiCommitEffort::Minimal));
        assert_eq!(cfg.effort_conflict(), None);
    }

    #[test]
    fn binary_overrides_the_agent_default_but_blank_does_not() {
        let mut cfg = AiCommit {
            agent: AiCommitAgent::Codex,
            ..AiCommit::default()
        };
        assert_eq!(cfg.resolved_binary(), "codex");
        cfg.binary = Some("/opt/bin/codex".to_string());
        assert_eq!(cfg.resolved_binary(), "/opt/bin/codex");
        // A blank override is a mistake, not a request to launch "".
        cfg.binary = Some("   ".to_string());
        assert_eq!(cfg.resolved_binary(), "codex");
    }

    #[test]
    fn settings_from_before_the_agent_field_still_parse() {
        // `deny_unknown_fields` makes this the compatibility test that matters:
        // a file written against the original three-value schema must still load.
        let cfg: AiCommit = serde_json::from_str(
            r#"{ "enabled": true, "model": "sonnet", "effort": "high", "instructions": ["be terse"] }"#,
        )
        .unwrap_or_default();
        assert_eq!(cfg.agent, AiCommitAgent::Claude, "defaults to claude");
        assert_eq!(cfg.model, "sonnet");
        assert_eq!(cfg.effort, Some(AiCommitEffort::High));
        assert_eq!(cfg.timeout_ms, DEFAULT_TIMEOUT_MS);
    }

    #[test]
    fn agent_and_effort_identifiers_round_trip_through_serde() {
        for agent in AiCommitAgent::ALL {
            let json = serde_json::to_string(&agent).unwrap_or_default();
            assert_eq!(json, format!("\"{}\"", agent.as_str()));
        }
        for effort in AiCommitEffort::ALL {
            let json = serde_json::to_string(&effort).unwrap_or_default();
            assert_eq!(json, format!("\"{}\"", effort.as_str()));
        }
    }
}
