//! GitHub remote parsing and repository identity.

/// A public GitHub repository identity.
#[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct RepositoryIdentity {
    /// Repository owner or organization.
    pub owner: String,
    /// Repository name without a trailing `.git`.
    pub repo: String,
}

impl RepositoryIdentity {
    /// The `owner/repository` display form.
    #[must_use]
    pub fn full_name(&self) -> String {
        format!("{}/{}", self.owner, self.repo)
    }
}

/// A validated remote hosted on the public `github.com` service.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct GitHubRemote {
    identity: RepositoryIdentity,
}

impl GitHubRemote {
    /// Parse a conventional public GitHub clone URL.
    ///
    /// GitHub Enterprise hosts, aliases, subdomains, query strings, fragments, and
    /// paths other than exactly `owner/repository` are rejected.
    #[must_use]
    pub fn parse(input: &str) -> Option<Self> {
        let input = input.trim();
        if let Some(identity) = parse_scp(input) {
            return Some(Self { identity });
        }

        let url = reqwest::Url::parse(input).ok()?;
        if !matches!(url.scheme(), "http" | "https" | "git" | "ssh")
            || !url
                .host_str()
                .is_some_and(|host| host.eq_ignore_ascii_case("github.com"))
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return None;
        }
        if url.scheme() == "ssh" && !url.username().is_empty() && url.username() != "git" {
            return None;
        }
        let parts: Vec<&str> = url
            .path_segments()?
            .filter(|part| !part.is_empty())
            .collect();
        parse_parts(&parts).map(|identity| Self { identity })
    }

    /// Borrow the repository identity.
    #[must_use]
    pub fn identity(&self) -> &RepositoryIdentity {
        &self.identity
    }

    /// Consume the remote and return its repository identity.
    #[must_use]
    pub fn into_identity(self) -> RepositoryIdentity {
        self.identity
    }
}

fn parse_scp(input: &str) -> Option<RepositoryIdentity> {
    let (authority, path) = input.split_once(':')?;
    let (user, host) = authority.split_once('@')?;
    if user != "git" || !host.eq_ignore_ascii_case("github.com") {
        return None;
    }
    let parts: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();
    parse_parts(&parts)
}

fn parse_parts(parts: &[&str]) -> Option<RepositoryIdentity> {
    let [owner, repo] = parts else {
        return None;
    };
    let repo = repo.strip_suffix(".git").unwrap_or(repo);
    if !valid_component(owner) || !valid_component(repo) {
        return None;
    }
    Some(RepositoryIdentity {
        owner: (*owner).to_string(),
        repo: repo.to_string(),
    })
}

fn valid_component(value: &str) -> bool {
    !value.is_empty()
        && !value.contains('%')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

/// Parse an `(owner, repository)` pair from a public GitHub remote URL.
#[must_use]
pub fn parse_remote(url: &str) -> Option<(String, String)> {
    let identity = GitHubRemote::parse(url)?.into_identity();
    Some((identity.owner, identity.repo))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_public_github_clone_forms() {
        let expected = RepositoryIdentity {
            owner: "getkono".to_string(),
            repo: "karet".to_string(),
        };
        for input in [
            "https://github.com/getkono/karet.git",
            "http://github.com/getkono/karet",
            "git://github.com/getkono/karet.git",
            "ssh://git@github.com/getkono/karet.git",
            "git@github.com:getkono/karet.git",
            "  git@GitHub.com:getkono/karet  ",
        ] {
            assert_eq!(
                GitHubRemote::parse(input).map(GitHubRemote::into_identity),
                Some(expected.clone())
            );
        }
    }

    #[test]
    fn rejects_enterprise_aliases_lookalikes_and_nested_paths() {
        for input in [
            "https://github.example.com/getkono/karet.git",
            "https://www.github.com/getkono/karet.git",
            "https://github.com.evil.test/getkono/karet.git",
            "git@work:getkono/karet.git",
            "https://github.com/getkono/karet/extra",
            "https://github.com/getkono/karet?tab=readme",
            "https://github.com/getkono%2fkaret/repo",
            "https://gitlab.com/getkono/karet.git",
        ] {
            assert!(GitHubRemote::parse(input).is_none(), "accepted {input}");
        }
    }

    #[test]
    fn full_name_joins_owner_and_repository() {
        let identity = RepositoryIdentity {
            owner: "getkono".to_string(),
            repo: "karet".to_string(),
        };
        assert_eq!(identity.full_name(), "getkono/karet");
    }
}
