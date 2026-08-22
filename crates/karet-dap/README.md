# karet-dap

> Async Debug Adapter Protocol client for karet TUI editors (breakpoints, variables, call stack).

A headless, async DAP client: it launches one debug adapter (stdio, or
spawn-then-TCP with `${port}` substitution), runs the capability-gated
initialize/launch/configuration handshake, and exposes typed requests
(breakpoints, run controls, the threads → stack → scopes → variables
waterfall, evaluate) plus a broadcast of adapter events. Framing is shared
with `karet-lsp`'s public codec. Bring your own UI.

Part of the [karet](https://github.com/getkono/karet) workspace; not published
(internal-but-separate — no external-consumer story yet).

## License

Licensed under either of MIT or Apache-2.0 at your option.
