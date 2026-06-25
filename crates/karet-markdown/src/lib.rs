//! `karet-markdown` — a markdown rendering model for karet (and LSP hover docs).
//!
//! Parses markdown into a block/inline render model decoupled from any renderer.
//! Enable `view` for a ratatui renderer, and `highlight` to syntax-highlight
//! code fences via `karet-syntax`.
//!
//! # Responsibilities (to implement)
//! - `parse` — pulldown-cmark → block/inline render model.
//! - `wrap` — width-aware soft wrapping.
//! - `view` — ratatui renderer (feature `view`).
//! - `code` — code-fence highlighting via karet-syntax (feature `highlight`).
//!
//! # Internal dependencies
//! - `karet-core` — spans / coordinates.
//! - `karet-syntax` — code-fence highlighting (optional, feature `highlight`).

// TODO: parse — pulldown-cmark → render model.
// TODO: wrap  — soft wrapping for a target width.
// TODO: view  — ratatui renderer (feature = "view").
// TODO: code  — highlighted code fences (feature = "highlight").
