//! `karet-clipboard` — clipboard integration for the karet toolkit.
//!
//! Standalone (depends on no other karet crate). Writes/reads the clipboard via
//! OSC 52 (works over SSH/tmux) with fallbacks that shell out to `wl-copy`,
//! `xclip`, `xsel` or `pbcopy`. Enable `async` for tokio-based spawning.
//!
//! # Responsibilities (to implement)
//! - `osc52` — OSC 52 base64 read/write.
//! - `external` — wl-clipboard/xclip/xsel/pbcopy fallbacks, in priority order.
//! - `path` — copy absolute / relative path helpers.

// TODO: osc52    — OSC 52 base64 clipboard read/write.
// TODO: external — external clipboard tool fallbacks (sync; async via feature).
// TODO: path     — copy path / relative path actions.
