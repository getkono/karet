# karet-search

> In-file and workspace code search & replace (gitignore-aware, streaming) for karet.

A ripgrep-style search/replace engine with **zero karet dependencies**: incremental in-file
search plus a gitignore-aware workspace walk with streamed results, and literal or
regex-capture replace (`plan_replacements` / `apply_replacements` in a buffer, or
`WorkspaceSearch::replace` across files). Positions are reported as plain byte offsets +
line/column, so any consumer can map them onto its own coordinate types.

Part of the [karet](https://github.com/getkono/karet) workspace.

## License

Licensed under either of MIT or Apache-2.0 at your option.
