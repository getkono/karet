# karet-search

> In-file and workspace code search & replace (gitignore-aware, streaming) for karet.

A ripgrep-style search/replace engine with **zero karet dependencies**: incremental in-file
search plus a gitignore-aware parallel workspace walk with streamed results and replace
planning. Positions are reported as plain byte offsets + line/column, so any consumer can map
them onto its own coordinate types.

Part of the [karet](https://github.com/getkono/karet) workspace.

## License

Licensed under either of MIT or Apache-2.0 at your option.
