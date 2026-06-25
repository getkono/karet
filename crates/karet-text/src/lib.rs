//! `karet-text` — a headless text-editing model for the karet toolkit.
//!
//! A rope-backed buffer with editing history and a cursor/selection model,
//! usable by any editor backend (TUI or otherwise) without pulling in rendering
//! dependencies.
//!
//! # Responsibilities (to implement)
//! - `buffer` — rope storage, indexing, line/char/byte conversions, change events.
//! - `edit` — atomic edits plus undo/redo via the command pattern, transactions.
//! - `cursor` — single/multi/block cursors & selections, column memory,
//!   expand/shrink by word/line/bracket (the planned `karet-cursor`, now a module).
//! - `save` — dirty tracking and save-to-disk.
//! - `mmap` — large-file read backend (behind the `mmap` feature).
//!
//! # Internal dependencies
//! - `karet-core` — shared text coordinates (Range, LineCol, …).

// TODO: buffer  — ropey-backed storage + change notifications.
// TODO: edit    — undo/redo command stack, edit coalescing & transactions.
// TODO: cursor  — multi-cursor & selection model and motions.
// TODO: save    — dirty flag, atomic save-to-disk.
// TODO: mmap    — memory-mapped large-file backend (feature = "mmap").
