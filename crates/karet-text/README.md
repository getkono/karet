# karet-text

A headless text-editing model for the [karet](https://github.com/getkono/karet)
toolkit: a rope-backed `TextBuffer` with editing history plus a cursor/selection
model, usable by any editor backend (TUI or otherwise) without pulling in rendering
dependencies.

It is the one place that converts between byte offsets and line/column positions
(including the UTF-16 conversions LSP needs at its edge), and exposes an
`apply`/`undo`/`redo` mutation surface, dirty/save tracking, and optional
memory-mapped loading of large files (the `mmap` feature).

Part of the karet workspace; released in lockstep with the other `karet-*` crates.
