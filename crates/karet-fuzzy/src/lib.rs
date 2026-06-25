//! `karet-fuzzy` — fuzzy matching and ranking for the karet toolkit.
//!
//! Standalone (depends on no other karet crate). Wraps `nucleo` with frecency/
//! recency scoring and quick-open query parsing, shared by the widgets toolkit
//! and LSP completion ranking so neither has to depend on the other.
//!
//! # Responsibilities (to implement)
//! - `match` — fuzzy matching & scoring over arbitrary items (via nucleo).
//! - `frecency` — frequency + recency boosting store.
//! - `query` — quick-open query parsing (`@symbol`, `:line`, `>command`, path).

// TODO: match    — nucleo-backed matching & ranking.
// TODO: frecency — frequency/recency store.
// TODO: query    — quick-open query parsing.
