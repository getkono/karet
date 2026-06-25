# karet-lsp

> Async LSP client for karet TUI editors (diagnostics, completion, hover, goto, symbols, inlay hints, …).

A headless, async Language Server Protocol client that turns server responses into neutral
`karet-core` models (diagnostics, symbols, completions, hovers, inlay hints, …) and implements
`SymbolProvider`. Usable from a CLI or any UI — the ratatui completion/hover popups live in
`karet-widgets`, which renders these models, so this crate stays free of UI dependencies.

Part of the [karet](https://github.com/getkono/karet) workspace.

## License

Licensed under either of MIT or Apache-2.0 at your option.
