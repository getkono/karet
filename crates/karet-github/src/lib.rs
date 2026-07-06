//! `karet-github` — a minimal GitHub REST client for karet.
//!
//! This crate is the single home for GitHub-specific networking. Today it exposes just
//! the one call the commit view needs — a commit's signature-verification status — but
//! it is deliberately a standalone crate so the surface can grow (eventually via
//! codegen of the GitHub API) without leaking `reqwest` or GitHub URL shapes into the
//! rest of the workspace.
//!
//! The transport is blocking ([`reqwest::blocking`]) so callers need no async runtime;
//! run [`commit_verification`] on a worker thread. TLS is pure-Rust rustls (ring), so
//! there is no OpenSSL/native-tls system dependency.

use std::time::Duration;

/// Errors produced by the GitHub client.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum GithubError {
    /// The remote URL is not a recognisable GitHub repository.
    #[error("not a GitHub remote")]
    NotGitHub,
    /// The HTTP request failed, or returned a non-success status.
    #[error("request failed: {0}")]
    Http(String),
    /// The response body was not the expected JSON shape.
    #[error("unexpected response: {0}")]
    Decode(String),
}

/// GitHub's verification verdict for a commit's signature (the REST
/// `commit.verification` object), mirroring the badge shown on the web commit page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Verification {
    /// Whether GitHub considers the signature verified.
    pub verified: bool,
    /// GitHub's machine-readable reason (`valid`, `unsigned`, `unknown_key`,
    /// `expired_key`, `bad_email`, …). Displayed when not simply `valid`.
    pub reason: String,
    /// The signer GitHub attributes the commit to (a login), when present. Best-effort.
    pub signer: Option<String>,
}

/// Parse an `(owner, repo)` pair from a GitHub remote URL, accepting the common HTTPS,
/// `scp`-style SSH, and `ssh://` forms and stripping a trailing `.git`. Returns `None`
/// for non-GitHub hosts or unparseable URLs.
#[must_use]
pub fn parse_remote(url: &str) -> Option<(String, String)> {
    let url = url.trim();
    // Reduce every accepted GitHub URL form to the "owner/repo" tail after the host.
    const PREFIXES: [&str; 5] = [
        "git@github.com:",
        "ssh://git@github.com/",
        "https://github.com/",
        "http://github.com/",
        "git://github.com/",
    ];
    let tail = PREFIXES.iter().find_map(|p| url.strip_prefix(p))?;
    let tail = tail.strip_suffix(".git").unwrap_or(tail);
    let mut parts = tail.splitn(2, '/');
    let owner = parts.next()?.trim();
    let repo = parts.next()?.trim();
    if owner.is_empty() || repo.is_empty() || repo.contains('/') {
        return None;
    }
    Some((owner.to_string(), repo.to_string()))
}

/// Fetch GitHub's signature-verification verdict for commit `sha` in `owner/repo`.
///
/// Blocking; run on a worker thread. Uses `GITHUB_TOKEN` / `GH_TOKEN` from the
/// environment when set (raising the rate limit and allowing private repos); works
/// unauthenticated for public repositories otherwise.
///
/// # Errors
/// Returns [`GithubError::Http`] on a network failure or non-success status, and
/// [`GithubError::Decode`] if the response is not the expected JSON shape.
pub fn commit_verification(
    owner: &str,
    repo: &str,
    sha: &str,
) -> Result<Verification, GithubError> {
    let client = reqwest::blocking::Client::builder()
        .user_agent("karet")
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| GithubError::Http(e.to_string()))?;
    let url = format!("https://api.github.com/repos/{owner}/{repo}/commits/{sha}");
    let mut req = client
        .get(url)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28");
    if let Some(token) = auth_token() {
        req = req.bearer_auth(token);
    }
    let resp = req.send().map_err(|e| GithubError::Http(e.to_string()))?;
    if !resp.status().is_success() {
        return Err(GithubError::Http(format!("HTTP {}", resp.status())));
    }
    let body: serde_json::Value = resp
        .json()
        .map_err(|e| GithubError::Decode(e.to_string()))?;
    parse_verification(&body)
}

/// The GitHub token from the environment, if any non-empty one is set.
fn auth_token() -> Option<String> {
    for var in ["GITHUB_TOKEN", "GH_TOKEN"] {
        if let Ok(token) = std::env::var(var)
            && !token.is_empty()
        {
            return Some(token);
        }
    }
    None
}

/// Extract the [`Verification`] from a `GET /commits/{sha}` response body.
fn parse_verification(body: &serde_json::Value) -> Result<Verification, GithubError> {
    let v = &body["commit"]["verification"];
    if v.is_null() {
        return Err(GithubError::Decode("no verification object".to_string()));
    }
    let verified = v["verified"].as_bool().unwrap_or(false);
    let reason = v["reason"].as_str().unwrap_or("unknown").to_string();
    // Prefer the authoring login; fall back to the committer's.
    let signer = body["author"]["login"]
        .as_str()
        .or_else(|| body["committer"]["login"].as_str())
        .map(str::to_string);
    Ok(Verification {
        verified,
        reason,
        signer,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_https_ssh_and_scp_remotes() {
        let want = Some(("getkono".to_string(), "karet".to_string()));
        assert_eq!(parse_remote("https://github.com/getkono/karet.git"), want);
        assert_eq!(parse_remote("https://github.com/getkono/karet"), want);
        assert_eq!(parse_remote("git@github.com:getkono/karet.git"), want);
        assert_eq!(parse_remote("ssh://git@github.com/getkono/karet.git"), want);
        assert_eq!(parse_remote("  git@github.com:getkono/karet  "), want);
    }

    #[test]
    fn rejects_non_github_and_malformed() {
        assert_eq!(parse_remote("https://gitlab.com/a/b.git"), None);
        assert_eq!(parse_remote("git@example.com:a/b.git"), None);
        assert_eq!(parse_remote("https://github.com/onlyowner"), None);
        assert_eq!(parse_remote("https://github.com/a/b/c"), None);
        assert_eq!(parse_remote("not a url"), None);
    }

    #[test]
    fn parses_verification_from_response_body() -> Result<(), GithubError> {
        let body = serde_json::json!({
            "author": { "login": "web-flow" },
            "commit": {
                "verification": {
                    "verified": true,
                    "reason": "valid",
                    "signature": "-----BEGIN SSH SIGNATURE-----\n...",
                }
            }
        });
        let v = parse_verification(&body)?;
        assert!(v.verified);
        assert_eq!(v.reason, "valid");
        assert_eq!(v.signer.as_deref(), Some("web-flow"));
        Ok(())
    }

    #[test]
    fn unsigned_commit_parses_as_unverified() -> Result<(), GithubError> {
        let body = serde_json::json!({
            "commit": { "verification": { "verified": false, "reason": "unsigned" } }
        });
        let v = parse_verification(&body)?;
        assert!(!v.verified);
        assert_eq!(v.reason, "unsigned");
        assert_eq!(v.signer, None);
        Ok(())
    }

    #[test]
    fn missing_verification_object_errors() {
        let body = serde_json::json!({ "commit": {} });
        assert!(matches!(
            parse_verification(&body),
            Err(GithubError::Decode(_))
        ));
    }
}
