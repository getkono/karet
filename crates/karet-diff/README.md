# karet-diff

> Pure, syntax-aware text diffing engine (tree-sitter structural diff with line/word fallback) for karet.

A presentation-free diff engine: parse both sides with tree-sitter and diff the structure
(difftastic-style), falling back to line/word diffing for formats without a grammar. Produces
hunks and a per-hunk staging model; how a diff is displayed is entirely up to the consumer.

Part of the [karet](https://github.com/getkono/karet) workspace.

## License

Licensed under either of MIT or Apache-2.0 at your option.
