# karet-core

> Shared vocabulary (geometry, text coordinates, neutral models) for the karet TUI editor toolkit.

The dependency-light foundation every other `karet-*` crate speaks: terminal-cell geometry,
byte/line-column text coordinates, and the neutral models (diagnostics, decorations, symbols,
completions, hovers, edits, …) plus the provider traits that let producers and renderers
interoperate without depending on each other.

Part of the [karet](https://github.com/getkono/karet) workspace.

## Features

- `serde` — derive `Serialize`/`Deserialize` on every model (wire-ready).

## License

Licensed under either of MIT or Apache-2.0 at your option.
