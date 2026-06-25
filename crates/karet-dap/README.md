# karet-dap

> Async Debug Adapter Protocol client for karet TUI editors (breakpoints, variables, call stack).

A headless, async DAP client that drives a debug adapter and exposes debugging state
(breakpoints, variables, call stack), emitting breakpoint markers as neutral `karet-core`
decorations. Bring your own UI.

Part of the [karet](https://github.com/getkono/karet) workspace.

## Features

- `view` — ratatui debug panels (variable inspector, call stack, console, step controls).

## License

Licensed under either of MIT or Apache-2.0 at your option.
