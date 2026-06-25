# karet-vcs

> Editor-oriented git integration for karet, backed by `gix`.

A `gix`-backed engine for working-tree state. It discovers the repository containing a
path, decides whether to show **staged** (HEAD ↔ index) or **unstaged** (index ↔
worktree, plus untracked) changes — VS Code's default — and enumerates each changed
file's full *before*/*after* content so a consumer can diff it (e.g. with `karet-diff`),
with binary detection and rename handling. Headless by default.

Per-line blame, branch listing and interactive staging are reserved (the public joints
are defined).

Part of the [karet](https://github.com/getkono/karet) workspace.

## Features

- `view` — ratatui source-control panels (reserved).

## License

Licensed under either of MIT or Apache-2.0 at your option.
