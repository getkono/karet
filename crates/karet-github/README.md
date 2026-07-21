# karet-github

A headless GitHub REST client for [karet](https://github.com/getkono/karet), and the
single home for GitHub-specific networking in the workspace.

It exposes stable repository-workflow models and asynchronous operations for issues,
pull requests, Actions, and commit verification. GitHub wire types are generated with
spargen from the vendored official OpenAPI description where the schema is provably
representable; strictly typed adapters cover explicitly tracked generator blockers.
Generation runs in `build.rs` and writes only to Cargo's `OUT_DIR`; generated Rust is
never checked into or loaded from the source tree.

The transport uses `reqwest` with pure-Rust rustls TLS.
