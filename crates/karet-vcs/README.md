# karet-vcs

> Editor-oriented git integration for karet, backed by `gix`.

A `gix`-backed engine for working-tree state. It discovers the repository containing a
path, decides whether to show **staged** (HEAD ↔ index) or **unstaged** (index ↔
worktree, plus untracked) changes — VS Code's default — and enumerates each changed
file's full *before*/*after* content so a consumer can diff it (e.g. with `karet-diff`),
with binary detection and rename handling. Headless by default.

The same headless API provides blame, staging, guarded commit undo, branch creation,
switching, rename and deletion, stash management, upstream-aware sync and publish,
conflict recovery, and reusable pull-request branch checkout. Read-oriented repository
inspection uses `gix`; mutations and network operations use the installed Git CLI so
they honor the user's credential helpers, signing, hooks, and Git configuration.

Part of the [karet](https://github.com/getkono/karet) workspace.

## Features

- `view` — ratatui source-control panels (reserved).

## License

Licensed under either of MIT or Apache-2.0 at your option.
