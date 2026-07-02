# karet-editor

The composable ratatui code-editor widget for
[karet](https://github.com/getkono/karet). It combines the text engines
(`karet-text`, `karet-syntax`, `karet-theme`) into one `Editor` widget with a gutter,
scrolling, and visual aids.

By design it depends on **none** of the feature producers
(`karet-lsp`/`karet-vcs`/`karet-dap`/`karet-search`): diagnostics, git markers,
breakpoints, inlay hints, and code lenses all arrive as `karet-core` decorations that
the application supplies. A `read_only` mode (plus `EditorState::center_on` and
scroll-only paging) turns it into a pager — see `examples/read_only.rs`.

Part of the karet workspace; released in lockstep with the other `karet-*` crates.
