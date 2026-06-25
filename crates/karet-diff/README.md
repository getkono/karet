# karet-diff

> Pure, headless text diffing engine for karet.

A presentation-free diff engine. It diffs two texts into a neutral model (files,
hunks, lines) at line + intra-word granularity (via `imara-diff`), parses existing
unified diffs (`git diff` output) back into the same model, aligns hunks into
side-by-side rows, computes intra-line change segments, and reconstructs or stages
unified-diff patches. How a diff is displayed — colors, layout, syntax highlighting —
is entirely up to the consumer.

Tree-sitter structural ("difftastic-style") diffing is reserved behind
`DiffStrategy::Structural`; today both strategies use the line/word path.

Part of the [karet](https://github.com/getkono/karet) workspace.

## License

Licensed under either of MIT or Apache-2.0 at your option.
