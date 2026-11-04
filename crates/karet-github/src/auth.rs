//! Token discovery for [`crate::GitHubClient`]: one environment lookup, one
//! `gh` call shape, one bound on how long `gh` may take, and one reading of its
//! answer.

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

    /// Issue 545: an untrimmed env token was once sent as-is; the token is
    /// trimmed before it reaches a header.
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

    /// The production lookup reads the live process environment, and a variable
    /// that is not set reads as absent; it is only read here, never changed.
    #[test]
    fn process_env_reads_the_process_environment() {
        // Cargo sets `CARGO_PKG_NAME` for the test process it runs.
        assert_eq!(
            process_env("CARGO_PKG_NAME"),
            Some(env!("CARGO_PKG_NAME").to_string())
        );
        assert_eq!(process_env("KARET_GITHUB_TEST_UNSET_VARIABLE"), None);
    }

    /// Discovery reads `gh`'s answer through this, so it trims and refuses a
    /// failed or blank answer.
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
}
