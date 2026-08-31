//! The bridge from the `git.aiCommit.*` settings plus a staged diff to
//! [`aicommit_core`], and the pre-flight that reports whether the configured
//! agent can actually run.
//!
//! Kept behind the `aicommit` feature so a build without it carries no
//! dependency on the generator or the agent CLIs it drives.
//!
//! Two things happen here, and the split matters. [`probe`] is cheap, runs on
//! its own, and answers "would this work?" — it is what lets the client show a
//! resolved, verified configuration *before* anyone asks for a message.
//! [`generate`] is the expensive round-trip. Both are `async` and hold no state.

use std::time::Duration;

use agent_text::Agent;
use agent_text::ClaudeCode;
use agent_text::Codex;
use agent_text::ReasoningEffort;
use aicommit_core::CommitRequest;
use aicommit_core::auto_select;
use aicommit_core::generate_commit_message;
use karet_vcs::StagedDiff;

use crate::config::schema::AiCommit;
use crate::config::schema::AiCommitAgent;
use crate::config::schema::AiCommitEffort;

/// How long a [`probe`] may take. A version check is a local process launch; if
/// it has not answered in this long, treating the agent as unavailable is more
/// useful than making the caller wait.
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// Map a settings effort onto the generator's effort vocabulary.
///
/// Total by construction: [`AiCommitEffort`] mirrors [`ReasoningEffort`], and
/// the two values that only *some* agents accept are filtered out upstream by
/// [`AiCommit::effective_effort`], not silently remapped here.
const fn to_effort(effort: AiCommitEffort) -> ReasoningEffort {
    match effort {
        AiCommitEffort::Minimal => ReasoningEffort::Minimal,
        AiCommitEffort::Low => ReasoningEffort::Low,
        AiCommitEffort::Medium => ReasoningEffort::Medium,
        AiCommitEffort::High => ReasoningEffort::High,
        AiCommitEffort::XHigh => ReasoningEffort::XHigh,
        AiCommitEffort::Max => ReasoningEffort::Max,
    }
}

/// What a [`probe`] found out about one agent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ProbeResult {
    /// Whether a generation could run with this agent right now.
    pub(crate) available: bool,
    /// What was found, or why it is unusable — shown next to the agent in a
    /// picker, so it is phrased for a reader rather than a log.
    pub(crate) detail: String,
}

/// Build the configured agent, boxed so the choice stays a runtime one —
/// `generate_commit_message` accepts `&(impl Agent + ?Sized)`.
fn agent_for(cfg: &AiCommit, model: Option<&str>) -> Box<dyn Agent> {
    let binary = cfg.resolved_binary();
    let timeout = Duration::from_millis(cfg.timeout_ms);
    match cfg.agent {
        AiCommitAgent::Claude => {
            let mut agent = ClaudeCode::new()
                .with_binary(binary)
                .with_default_timeout(timeout);
            if let Some(model) = model {
                agent = agent.with_default_model(model);
            }
            if let Some(effort) = cfg.effective_effort() {
                agent = agent.with_default_effort(to_effort(effort));
            }
            Box::new(agent)
        },
        AiCommitAgent::Codex => {
            let mut agent = Codex::new()
                .with_binary(binary)
                .with_default_timeout(timeout);
            if let Some(model) = model {
                agent = agent.with_default_model(model);
            }
            if let Some(effort) = cfg.effective_effort() {
                agent = agent.with_default_effort(to_effort(effort));
            }
            Box::new(agent)
        },
    }
}

/// Turn a failed launch into the sentence a user needs.
///
/// A missing executable is by far the most common failure and the only one with
/// an obvious fix, so it is worth separating from every other spawn error.
fn describe_spawn_failure(binary: &str, error: &agent_text::Error) -> String {
    match error {
        agent_text::Error::Spawn { source, .. }
            if source.kind() == std::io::ErrorKind::NotFound =>
        {
            format!("`{binary}` was not found on PATH")
        },
        other => other.to_string(),
    }
}

/// Ask whether `cfg`'s agent is installed and usable, without generating
/// anything.
///
/// `codex` publishes a version and a supported floor, so it gets a real
/// compatibility check. `claude` exposes no such probe, so the best available
/// signal is that the executable launches at all — which is exactly the failure
/// (a missing or misconfigured binary) worth catching before a generation.
pub(crate) async fn probe(cfg: &AiCommit) -> ProbeResult {
    let binary = cfg.resolved_binary().to_string();
    match cfg.agent {
        AiCommitAgent::Codex => {
            let agent = Codex::new()
                .with_binary(&binary)
                .with_default_timeout(PROBE_TIMEOUT);
            match agent.verify_compatibility().await {
                Ok(version) => ProbeResult {
                    available: true,
                    detail: format!("codex {version}"),
                },
                Err(error) => ProbeResult {
                    available: false,
                    detail: describe_spawn_failure(&binary, &error),
                },
            }
        },
        AiCommitAgent::Claude => match probe_launchable(&binary).await {
            Ok(version) => ProbeResult {
                available: true,
                detail: version,
            },
            Err(detail) => ProbeResult {
                available: false,
                detail,
            },
        },
    }
}

/// Run `binary --version` and report its first line.
///
/// This is the fallback probe for an agent whose adapter offers none. It is
/// deliberately not routed through `agent-text`: asking for a version must not
/// start a model turn.
async fn probe_launchable(binary: &str) -> Result<String, String> {
    let launch = tokio::process::Command::new(binary)
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .output();
    let output = match tokio::time::timeout(PROBE_TIMEOUT, launch).await {
        Err(_) => return Err(format!("`{binary} --version` timed out")),
        Ok(Err(error)) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(format!("`{binary}` was not found on PATH"));
        },
        Ok(Err(error)) => return Err(format!("`{binary}` could not be launched: {error}")),
        Ok(Ok(output)) => output,
    };
    if !output.status.success() {
        return Err(format!("`{binary} --version` failed"));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let line = text.lines().next().unwrap_or_default().trim();
    Ok(if line.is_empty() {
        binary.to_string()
    } else {
        line.to_string()
    })
}

/// The model this configuration will actually run for a diff of `diff_len`
/// bytes across `file_count` files.
///
/// Resolving it here rather than inside the generator is what lets the client
/// name the model before the user commits to a round-trip.
pub(crate) fn resolved_model(cfg: &AiCommit, diff_len: usize, file_count: usize) -> String {
    if cfg.is_auto_model() {
        auto_select(diff_len, file_count).model
    } else {
        cfg.model.clone()
    }
}

/// Generate a commit message for `diff` under the `git.aiCommit.*` settings.
///
/// Returns a human-readable error string on failure, suitable for display.
/// Cancellation is by drop: the adapters set `kill_on_drop`, so abandoning this
/// future kills the agent process with it.
pub(crate) async fn generate(diff: &StagedDiff, cfg: &AiCommit) -> Result<String, String> {
    let request = CommitRequest {
        diff: diff.patch.clone(),
        stat: diff.stat.clone(),
        file_count: diff.file_count,
        instructions: cfg.instructions.clone(),
        ..CommitRequest::default()
    };

    // `"auto"` defers the model to the diff-size heuristic, which wants the
    // *untruncated* diff length — the generator truncates afterwards, for the
    // prompt, and choosing from the truncated size would under-serve big diffs.
    let model = resolved_model(cfg, diff.patch.len(), diff.file_count);
    let binary = cfg.resolved_binary().to_string();
    let agent = agent_for(cfg, Some(&model));

    generate_commit_message(&request, agent.as_ref())
        .await
        .map(|generated| generated.message)
        .map_err(|error| match error {
            aicommit_core::CoreError::Agent(inner) => describe_spawn_failure(&binary, &inner),
            other => other.to_string(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effort_maps_across_the_whole_vocabulary() {
        // Every settings value has a generator counterpart; a new variant on
        // either side breaks this match rather than silently degrading.
        assert_eq!(to_effort(AiCommitEffort::Minimal), ReasoningEffort::Minimal);
        assert_eq!(to_effort(AiCommitEffort::Low), ReasoningEffort::Low);
        assert_eq!(to_effort(AiCommitEffort::Medium), ReasoningEffort::Medium);
        assert_eq!(to_effort(AiCommitEffort::High), ReasoningEffort::High);
        assert_eq!(to_effort(AiCommitEffort::XHigh), ReasoningEffort::XHigh);
        assert_eq!(to_effort(AiCommitEffort::Max), ReasoningEffort::Max);
    }

    #[test]
    fn auto_resolves_a_model_by_diff_size_and_a_pin_wins() {
        let auto = AiCommit::default();
        assert_eq!(
            resolved_model(&auto, 10, 1),
            "haiku",
            "small diff stays cheap"
        );
        assert_eq!(
            resolved_model(&auto, 100_000, 40),
            "sonnet",
            "a large diff escalates"
        );

        let pinned = AiCommit {
            model: "opus".to_string(),
            ..AiCommit::default()
        };
        assert_eq!(resolved_model(&pinned, 10, 1), "opus");
        assert_eq!(resolved_model(&pinned, 100_000, 40), "opus");
    }

    #[test]
    fn a_missing_binary_reads_as_a_path_problem() {
        let error = agent_text::Error::Spawn {
            binary: "claude".into(),
            source: std::io::Error::from(std::io::ErrorKind::NotFound),
        };
        let described = describe_spawn_failure("claude", &error);
        assert!(described.contains("not found on PATH"), "{described}");
        assert!(described.contains("claude"), "{described}");

        // Any other failure keeps the underlying wording rather than claiming
        // the binary is missing.
        let other = agent_text::Error::EmptyOutput;
        assert_eq!(describe_spawn_failure("claude", &other), other.to_string());
    }

    #[tokio::test]
    async fn probing_a_missing_binary_reports_it_as_unavailable() {
        let cfg = AiCommit {
            binary: Some("karet-no-such-agent-binary".to_string()),
            ..AiCommit::default()
        };
        let result = probe(&cfg).await;
        assert!(!result.available);
        assert!(result.detail.contains("not found on PATH"), "{result:?}");
    }

    #[tokio::test]
    async fn probing_finds_a_binary_that_exists() {
        // `env` is on PATH anywhere this test runs and answers `--version`, so
        // it stands in for an installed agent without needing one.
        let cfg = AiCommit {
            binary: Some("env".to_string()),
            ..AiCommit::default()
        };
        let result = probe(&cfg).await;
        assert!(result.available, "{result:?}");
        assert!(!result.detail.is_empty());
    }
}
