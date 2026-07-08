# karet-github

A minimal GitHub REST client for [karet](https://github.com/getkono/karet), and the
single home for GitHub-specific networking in the workspace.

Today it exposes one call — a commit's signature-verification status
(`commit_verification`) — used by the editor's commit view to show GitHub's
"Verified" badge. It is a standalone crate so its surface can grow (eventually via
codegen of the GitHub API) without leaking `reqwest` or GitHub URL shapes into the rest
of the workspace.

Blocking transport (`reqwest::blocking`, no async runtime needed); pure-Rust rustls TLS
(no OpenSSL/native-tls system dependency). `publish = false`.
