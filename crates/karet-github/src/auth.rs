//! Token discovery shared by the asynchronous [`crate::GitHubClient`] and the
//! blocking pull-request listing: one environment lookup, one `gh` call shape,
//! one bound on how long `gh` may take, and one reading of its answer.

use std::io::Read as _;
use std::process::Stdio;
use std::time::Duration;

use secrecy::SecretString;

use crate::client::AuthSource;

/// Arguments of the non-interactive `gh` call that prints the stored token.
pub(crate) const GH_AUTH_TOKEN: [&str; 4] = ["auth", "token", "--hostname", "github.com"];
/// How long either lookup waits on `gh` before going anonymous.
pub(crate) const GH_TOKEN_TIMEOUT: Duration = Duration::from_secs(3);

/// The process environment's variable `name`, as [`env_token`]'s lookup.
pub(crate) fn process_env(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

/// The first non-blank token in `GITHUB_TOKEN`, then `GH_TOKEN`, trimmed, with
/// each variable read through `var` ([`process_env`] outside tests).
pub(crate) fn env_token(
    var: impl Fn(&str) -> Option<String>,
) -> Option<(SecretString, AuthSource)> {
    [
        ("GITHUB_TOKEN", AuthSource::GitHubToken),
        ("GH_TOKEN", AuthSource::GhToken),
    ]
    .into_iter()
    .find_map(|(name, source)| {
        let token = var(name)?;
        let token = token.trim();
        (!token.is_empty()).then(|| (SecretString::from(token.to_string()), source))
    })
}

/// The token a successful `gh auth token` printed, trimmed; `None` when it
/// failed or printed nothing usable.
pub(crate) fn cli_token(success: bool, stdout: Vec<u8>) -> Option<SecretString> {
    let token = String::from_utf8(stdout).ok().filter(|_| success)?;
    let token = token.trim();
    (!token.is_empty()).then(|| SecretString::from(token.to_string()))
}

/// The token for a blocking caller, by the lookup [`crate::GitHubClient::discover`]
/// uses: the environment, then `gh auth token` under the same bound.
pub(crate) fn blocking_token() -> Option<SecretString> {
    if let Some((token, _)) = env_token(process_env) {
        return Some(token);
    }
    let mut command = std::process::Command::new("gh");
    command.args(GH_AUTH_TOKEN).env("GH_PROMPT_DISABLED", "1");
    let (success, stdout) = output_within(command, GH_TOKEN_TIMEOUT)?;
    cli_token(success, stdout)
}

/// Run `command` and collect its stdout, or kill it and give up once `timeout`
/// passes without the output ending.
///
/// The read runs on its own thread so this one can stop waiting on it; a
/// grandchild still holding the pipe keeps only that thread alive.
fn output_within(mut command: std::process::Command, timeout: Duration) -> Option<(bool, Vec<u8>)> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let (sender, receiver) = std::sync::mpsc::channel();
    if let Some(mut stdout) = child.stdout.take() {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = sender.send(stdout.read_to_end(&mut bytes).map(|_| bytes));
        });
    }
    let Ok(Ok(stdout)) = receiver.recv_timeout(timeout) else {
        let _ = child.kill();
        let _ = child.wait();
        return None;
    };
    let status = child.wait().ok()?;
    Some((status.success(), stdout))
}

#[cfg(test)]
mod tests {
    use secrecy::ExposeSecret as _;

    use super::*;

    /// The token and its source `env_token` picks from `vars`.
    fn env_pick(vars: &[(&str, &str)]) -> Option<(String, AuthSource)> {
        env_token(|name| {
            vars.iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| (*value).to_string())
        })
        .map(|(token, source)| (token.expose_secret().to_string(), source))
    }

    /// `GITHUB_TOKEN` wins over `GH_TOKEN` when both hold a token.
    #[test]
    fn env_token_prefers_github_token_over_gh_token() {
        assert_eq!(
            env_pick(&[("GITHUB_TOKEN", "ghp_a"), ("GH_TOKEN", "ghp_b")]),
            Some(("ghp_a".to_string(), AuthSource::GitHubToken))
        );
        assert_eq!(
            env_pick(&[("GH_TOKEN", "ghp_b")]),
            Some(("ghp_b".to_string(), AuthSource::GhToken))
        );
        assert_eq!(env_pick(&[]), None);
    }

    /// Issue 545: the blocking listing sent an untrimmed env token; both paths
    /// now share this trim.
    #[test]
    fn env_token_trims_surrounding_whitespace() {
        assert_eq!(
            env_pick(&[("GITHUB_TOKEN", "  ghp_a\n")]),
            Some(("ghp_a".to_string(), AuthSource::GitHubToken))
        );
    }

    /// A blank `GITHUB_TOKEN` falls through to `GH_TOKEN` instead of being sent.
    #[test]
    fn env_token_skips_blank_values() {
        assert_eq!(
            env_pick(&[("GITHUB_TOKEN", " \n"), ("GH_TOKEN", "ghp_b")]),
            Some(("ghp_b".to_string(), AuthSource::GhToken))
        );
        assert_eq!(env_pick(&[("GITHUB_TOKEN", ""), ("GH_TOKEN", "  ")]), None);
    }

    /// Both lookups read `gh`'s answer through this, so the blocking one trims
    /// and refuses a failed or blank answer exactly as discovery does.
    #[test]
    fn cli_token_takes_only_a_successful_non_blank_answer() {
        let token = |success, stdout: &[u8]| {
            cli_token(success, stdout.to_vec()).map(|token| token.expose_secret().to_string())
        };
        assert_eq!(token(true, b"  gho_abc\n"), Some("gho_abc".to_string()));
        assert_eq!(token(false, b"gho_abc\n"), None);
        assert_eq!(token(true, b" \n"), None);
        assert_eq!(token(true, b"\xff\xfe"), None);
    }

    #[cfg(unix)]
    #[test]
    fn output_within_returns_the_exit_verdict_and_stdout() {
        let mut command = std::process::Command::new("sh");
        command.args(["-c", "printf ' tok\\n'; exit 3"]);
        assert_eq!(
            output_within(command, Duration::from_secs(10)),
            Some((false, b" tok\n".to_vec()))
        );
    }

    /// The blocking lookup runs on Source Control's serialized worker, so a
    /// `gh` that never answers must not hold it past the bound.
    #[cfg(unix)]
    #[test]
    fn output_within_kills_a_command_that_outlives_the_bound() {
        let mut command = std::process::Command::new("sh");
        command.args(["-c", "exec sleep 30"]);
        let started = std::time::Instant::now();
        assert_eq!(output_within(command, Duration::from_millis(100)), None);
        assert!(started.elapsed() < Duration::from_secs(10));
    }
}
