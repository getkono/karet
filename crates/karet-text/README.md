# karet-text

A headless text-editing model for the [karet](https://github.com/getkono/karet)
toolkit: a rope-backed `TextBuffer` with editing history, usable by any editor
backend (TUI or otherwise) without pulling in rendering dependencies. Cursors and
selections are not modelled here; they live in `karet_core::CursorState`.

It is the one place that converts between byte offsets and line/column positions
(including the UTF-16 conversions LSP needs at its edge), and exposes an
`apply`/`undo`/`redo` mutation surface and dirty/save tracking.

Part of the karet workspace; released in lockstep with the other `karet-*` crates.
