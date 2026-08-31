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
fn agent_for(cfg: &AiCommit, run: &Run) -> Box<dyn Agent> {
    let binary = cfg.resolved_binary();
    let timeout = Duration::from_millis(cfg.timeout_ms);
    match cfg.agent {
        AiCommitAgent::Claude => {
            let mut agent = ClaudeCode::new()
                .with_binary(binary)
                .with_default_timeout(timeout)
                .with_default_model(&run.model);
            if let Some(effort) = run.effort {
                agent = agent.with_default_effort(to_effort(effort));
            }
            Box::new(agent)
        },
        AiCommitAgent::Codex => {
            let mut agent = Codex::new()
                .with_binary(binary)
                .with_default_timeout(timeout)
                .with_default_model(&run.model);
            if let Some(effort) = run.effort {
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

/// The model and effort one generation will actually run with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Run {
    /// The model name handed to the agent.
    pub(crate) model: String,
    /// The effort, or `None` for the model's own default.
    pub(crate) effort: Option<AiCommitEffort>,
}

/// Resolve what a diff of `diff_len` bytes across `file_count` files will run.
///
/// `"auto"` defers to the size heuristic, which picks an effort *alongside* its
/// model — a large diff is escalated to a stronger model precisely so it can
/// think harder, and taking the model while dropping the effort would deliver
/// half of that. An explicitly configured effort still wins over the
/// heuristic's, so pinning one remains meaningful with `"auto"`.
pub(crate) fn resolve_run(cfg: &AiCommit, diff_len: usize, file_count: usize) -> Run {
    if cfg.is_auto_model() {
        let choice = auto_select(diff_len, file_count);
        Run {
            model: choice.model,
            effort: cfg
                .effective_effort()
                .or_else(|| from_effort(choice.effort)),
        }
    } else {
        Run {
            model: cfg.model.clone(),
            effort: cfg.effective_effort(),
        }
    }
}

/// Map the generator's effort back onto the settings vocabulary.
///
/// The heuristic only ever emits `None` or `Medium`, but `ReasoningEffort` is
/// `#[non_exhaustive]`, so an unrecognized value degrades to the model default
/// rather than being guessed at.
fn from_effort(effort: Option<ReasoningEffort>) -> Option<AiCommitEffort> {
    match effort? {
        ReasoningEffort::Minimal => Some(AiCommitEffort::Minimal),
        ReasoningEffort::Low => Some(AiCommitEffort::Low),
        ReasoningEffort::Medium => Some(AiCommitEffort::Medium),
        ReasoningEffort::High => Some(AiCommitEffort::High),
        ReasoningEffort::XHigh => Some(AiCommitEffort::XHigh),
        ReasoningEffort::Max => Some(AiCommitEffort::Max),
        _ => None,
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

    // The heuristic wants the *untruncated* diff length — the generator
    // truncates afterwards, for the prompt, and choosing from the truncated size
    // would under-serve exactly the big diffs the escalation exists for.
    let run = resolve_run(cfg, diff.patch.len(), diff.file_count);
    let binary = cfg.resolved_binary().to_string();
    let agent = agent_for(cfg, &run);

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
        let small = resolve_run(&auto, 10, 1);
        assert_eq!(small.model, "haiku", "small diff stays cheap");
        assert_eq!(small.effort, None, "at the model's own default");

        let large = resolve_run(&auto, 100_000, 40);
        assert_eq!(large.model, "sonnet", "a large diff escalates");
        // The escalation is the point: a stronger model *and* more thinking.
        // Taking the model and dropping the effort would deliver half of it.
        assert_eq!(large.effort, Some(AiCommitEffort::Medium));

        let pinned = AiCommit {
            model: "opus".to_string(),
            ..AiCommit::default()
        };
        assert_eq!(resolve_run(&pinned, 10, 1).model, "opus");
        assert_eq!(resolve_run(&pinned, 100_000, 40).model, "opus");
    }

    #[test]
    fn a_configured_effort_outranks_the_heuristics_own() {
        let cfg = AiCommit {
            effort: Some(AiCommitEffort::High),
            ..AiCommit::default()
        };
        // "auto" picks the model, but an explicit effort still means something.
        let run = resolve_run(&cfg, 100_000, 40);
        assert_eq!(run.model, "sonnet");
        assert_eq!(run.effort, Some(AiCommitEffort::High));

        // One the agent rejects is dropped rather than passed through, and the
        // heuristic's choice is not resurrected in its place.
        let refused = AiCommit {
            agent: AiCommitAgent::Claude,
            effort: Some(AiCommitEffort::Minimal),
            ..AiCommit::default()
        };
        assert_eq!(
            resolve_run(&refused, 100_000, 40).effort,
            Some(AiCommitEffort::Medium),
            "falls back to what the heuristic asked for"
        );
        assert_eq!(resolve_run(&refused, 10, 1).effort, None);
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
    #[cfg(unix)]
    async fn probing_finds_a_binary_that_exists() {
        // A stand-in agent written for the test rather than borrowed from the
        // system: `env --version` is a GNU coreutils extension that BSD/macOS
        // `env` rejects, so reaching for a real binary would make this pass or
        // fail on which platform ran it rather than on the code.
        let Ok(directory) = tempfile::tempdir() else {
            return;
        };
        let script = directory.path().join("fake-agent");
        if std::fs::write(&script, "#!/bin/sh\necho 'fake-agent 9.9.9'\n").is_err() {
            return;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).is_err() {
                return;
            }
        }
        let cfg = AiCommit {
            binary: Some(script.to_string_lossy().into_owned()),
            ..AiCommit::default()
        };
        let result = probe(&cfg).await;
        assert!(result.available, "{result:?}");
        // The probe reports the version line the agent printed, which is what a
        // picker shows beside it.
        assert_eq!(result.detail, "fake-agent 9.9.9");
    }
}
