# karet-github

A minimal GitHub REST client for [karet](https://github.com/getkono/karet), and the
single home for GitHub-specific networking in the workspace.

It exposes commit signature verification for the editor's "Verified" badge and a
paginated open-pull-request query used by Source Control. Remote parsing accepts the
common SSH and HTTPS GitHub forms, and authentication uses the user's available GitHub
token without leaking `reqwest` or GitHub URL shapes into the rest of the workspace.

Blocking transport (`reqwest::blocking`, no async runtime needed); pure-Rust rustls TLS
(no OpenSSL/native-tls system dependency). `publish = false`.
