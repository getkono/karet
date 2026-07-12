//! The bridge from the `git.aiCommit.*` settings plus a staged diff to
//! [`aicommit_core`]. Kept behind the `aicommit` feature so a build without it
//! carries no dependency on the generator (or the `claude` CLI it drives).

use aicommit_core::ClaudeCliBackend;
use aicommit_core::CommitRequest;
use aicommit_core::Effort;
use aicommit_core::auto_select;
use aicommit_core::generate_commit_message;
use karet_vcs::StagedDiff;

use crate::config::schema::AiCommit;
use crate::config::schema::AiCommitEffort;

/// Map a settings effort onto the generator's effort vocabulary.
fn to_effort(effort: AiCommitEffort) -> Effort {
    match effort {
        AiCommitEffort::Low => Effort::Low,
        AiCommitEffort::Medium => Effort::Medium,
        AiCommitEffort::High => Effort::High,
    }
}

/// Generate a commit message for `diff` under the `git.aiCommit.*` settings.
///
/// Blocking (it shells out to the `claude` CLI): call it off the actor thread.
/// Returns a human-readable error string on failure, suitable for a notification.
pub(crate) fn generate(diff: &StagedDiff, cfg: &AiCommit) -> Result<String, String> {
    let mut request = CommitRequest::new(diff.patch.clone());
    request.stat = diff.stat.clone();
    request.file_count = diff.file_count;
    request.instructions = cfg.instructions.clone();

    // `"auto"` defers both the model and its effort to the diff-size heuristic; any
    // other value pins that model, at the configured effort (or the model's default).
    let backend = if cfg.model.eq_ignore_ascii_case("auto") {
        ClaudeCliBackend::from_choice(auto_select(diff.patch.len(), diff.file_count))
    } else {
        ClaudeCliBackend::new(cfg.model.clone()).with_effort(cfg.effort.map(to_effort))
    };
    let backend = match cfg.binary.as_ref() {
        Some(path) => backend.with_binary(path.clone()),
        None => backend,
    };

    generate_commit_message(&request, &backend)
        .map(|generated| generated.message)
        .map_err(|err| err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effort_maps_across_the_vocabulary() {
        assert_eq!(to_effort(AiCommitEffort::Low), Effort::Low);
        assert_eq!(to_effort(AiCommitEffort::Medium), Effort::Medium);
        assert_eq!(to_effort(AiCommitEffort::High), Effort::High);
    }
}
