//! `karet-dap` — an async Debug Adapter Protocol client for karet.
//!
//! Headless by default: drives a debug adapter and exposes debugging state
//! (breakpoints, variables, call stack), emitting breakpoint markers as
//! `karet-core` decorations. Enable `view` for ratatui debug panels.
//!
//! # Responsibilities (to implement)
//! - `protocol` — DAP message types + JSON-RPC framing (hand-rolled over serde;
//!   the `dap` crate is still alpha at the time of writing).
//! - `session` — adapter lifecycle, launch/attach, event handling.
//! - `state` — breakpoints (incl. conditional/hit-count), variables, call stack, watches.
//! - `view` — variable inspector, call stack, console, step controls (feature `view`).
//!
//! # Internal dependencies
//! - `karet-core` — emitted breakpoint decorations.

// TODO: protocol — DAP message types + framing.
// TODO: session  — adapter lifecycle + events.
// TODO: state    — breakpoints, variables, call stack, watches.
// TODO: view     — ratatui debug panels (feature = "view").
