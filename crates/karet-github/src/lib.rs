//! `karet-github` — a minimal GitHub REST client for karet.
//!
//! This crate is the single home for GitHub-specific networking: commit verification
//! and open pull-request discovery without leaking `reqwest` or GitHub wire shapes
//! into the rest of the workspace.
//!
//! The transport is blocking ([`reqwest::blocking`]) so callers need no async runtime;
//! run [`commit_verification`] on a worker thread. TLS is pure-Rust rustls (ring), so
//! there is no OpenSSL/native-tls system dependency.

use std::time::Duration;

const API: &str = "https://api.github.com";
const API_VERSION: &str = "2022-11-28";

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

/// A compact open pull request suitable for a branch picker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PullRequest {
    /// Repository-local pull request number.
    pub number: u64,
    /// Pull request title.
    pub title: String,
    /// Author login, when GitHub supplied one.
    pub author: Option<String>,
    /// Whether the pull request is still a draft.
    pub draft: bool,
    /// Source branch name.
    pub head_ref: String,
    /// Source repository's `owner/name`, including a fork when applicable.
    pub head_repo: String,
    /// Current source commit.
    pub head_sha: String,
    /// Target branch name.
    pub base_ref: String,
    /// Target repository's `owner/name`.
    pub base_repo: String,
    /// Browser URL for the pull request.
    pub url: String,
}

/// One page of open pull requests.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PullRequestPage {
    /// Pull requests in GitHub's updated-descending order.
    pub items: Vec<PullRequest>,
    /// Next page number when GitHub advertised one.
    pub next_page: Option<u32>,
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
    let url = format!("{API}/repos/{owner}/{repo}/commits/{sha}");
    let mut req = client()?
        .get(url)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", API_VERSION);
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

/// Fetch one page of open pull requests for `owner/repo`.
///
/// Authentication uses `GITHUB_TOKEN`, then `GH_TOKEN`, then a non-interactive
/// `gh auth token` lookup. Public repositories also work without authentication.
/// `per_page` must be between 1 and 100 and `page` is 1-based.
///
/// # Errors
/// Returns [`GithubError::Http`] for invalid paging or an HTTP failure, and
/// [`GithubError::Decode`] when GitHub returns an unexpected response shape.
pub fn open_pull_requests(
    owner: &str,
    repo: &str,
    page: u32,
    per_page: u8,
) -> Result<PullRequestPage, GithubError> {
    if owner.is_empty() || repo.is_empty() || page == 0 || !(1..=100).contains(&per_page) {
        return Err(GithubError::Http("invalid pull-request query".to_string()));
    }
    let url = format!("{API}/repos/{owner}/{repo}/pulls");
    let mut request = client()?
        .get(url)
        .query(&[
            ("state", "open".to_string()),
            ("sort", "updated".to_string()),
            ("direction", "desc".to_string()),
            ("page", page.to_string()),
            ("per_page", per_page.to_string()),
        ])
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", API_VERSION);
    if let Some(token) = auth_token() {
        request = request.bearer_auth(token);
    }
    let response = request
        .send()
        .map_err(|error| GithubError::Http(error.to_string()))?;
    if !response.status().is_success() {
        return Err(GithubError::Http(format!("HTTP {}", response.status())));
    }
    let next_page = response
        .headers()
        .get(reqwest::header::LINK)
        .and_then(|value| value.to_str().ok())
        .and_then(parse_next_page);
    let body: serde_json::Value = response
        .json()
        .map_err(|error| GithubError::Decode(error.to_string()))?;
    Ok(PullRequestPage {
        items: parse_pull_requests(&body)?,
        next_page,
    })
}

fn client() -> Result<reqwest::blocking::Client, GithubError> {
    reqwest::blocking::Client::builder()
        .user_agent("karet")
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|error| GithubError::Http(error.to_string()))
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
    std::process::Command::new("gh")
        .args(["auth", "token", "--hostname", "github.com"])
        .env("GH_PROMPT_DISABLED", "1")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|token| !token.is_empty())
}

fn parse_pull_requests(body: &serde_json::Value) -> Result<Vec<PullRequest>, GithubError> {
    let rows = body
        .as_array()
        .ok_or_else(|| GithubError::Decode("expected a pull-request array".to_string()))?;
    rows.iter().map(parse_pull_request).collect()
}

fn parse_pull_request(row: &serde_json::Value) -> Result<PullRequest, GithubError> {
    Ok(PullRequest {
        number: required_u64(row, "number")?,
        title: required(row, "title")?.to_string(),
        author: row["user"]["login"].as_str().map(str::to_string),
        draft: row["draft"].as_bool().unwrap_or(false),
        head_ref: required(&row["head"], "ref")?.to_string(),
        head_repo: required(&row["head"]["repo"], "full_name")?.to_string(),
        head_sha: required(&row["head"], "sha")?.to_string(),
        base_ref: required(&row["base"], "ref")?.to_string(),
        base_repo: required(&row["base"]["repo"], "full_name")?.to_string(),
        url: required(row, "html_url")?.to_string(),
    })
}

fn required<'a>(value: &'a serde_json::Value, key: &str) -> Result<&'a str, GithubError> {
    value[key]
        .as_str()
        .ok_or_else(|| GithubError::Decode(format!("missing `{key}`")))
}

fn required_u64(value: &serde_json::Value, key: &str) -> Result<u64, GithubError> {
    value[key]
        .as_u64()
        .ok_or_else(|| GithubError::Decode(format!("missing `{key}`")))
}

fn parse_next_page(header: &str) -> Option<u32> {
    header.split(',').find_map(|part| {
        let (url, relation) = part.trim().split_once(';')?;
        if !relation.contains("rel=\"next\"") {
            return None;
        }
        url.trim()
            .trim_start_matches('<')
            .trim_end_matches('>')
            .split('?')
            .nth(1)?
            .split('&')
            .find_map(|field| field.strip_prefix("page=")?.parse().ok())
    })
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

    #[test]
    fn pull_request_page_parses_forks_and_drafts() -> Result<(), GithubError> {
        let body = serde_json::json!([{
            "number": 42,
            "title": "Improve checkout",
            "draft": true,
            "html_url": "https://github.com/getkono/karet/pull/42",
            "user": { "login": "octocat" },
            "head": {
                "ref": "topic",
                "sha": "abc123",
                "repo": { "full_name": "octocat/karet" }
            },
            "base": {
                "ref": "master",
                "repo": { "full_name": "getkono/karet" }
            }
        }]);
        let pulls = parse_pull_requests(&body)?;
        assert_eq!(pulls.len(), 1);
        assert_eq!(pulls[0].number, 42);
        assert_eq!(pulls[0].head_repo, "octocat/karet");
        assert!(pulls[0].draft);
        Ok(())
    }

    #[test]
    fn pull_request_parser_rejects_missing_required_fields() {
        assert!(parse_pull_requests(&serde_json::json!([{"number": 1}])).is_err());
    }

    #[test]
    fn next_link_extracts_page() {
        let header = "<https://api.github.com/repos/o/r/pulls?page=2>; rel=\"next\", \
                      <https://api.github.com/repos/o/r/pulls?page=4>; rel=\"last\"";
        assert_eq!(parse_next_page(header), Some(2));
        assert_eq!(parse_next_page(""), None);
    }

    #[test]
    fn open_pull_requests_validates_paging_before_network() {
        assert!(matches!(
            open_pull_requests("owner", "repo", 0, 100),
            Err(GithubError::Http(_))
        ));
        assert!(matches!(
            open_pull_requests("owner", "repo", 1, 0),
            Err(GithubError::Http(_))
        ));
    }
}
