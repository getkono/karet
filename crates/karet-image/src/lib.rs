//! `karet-image` — terminal image rendering for the karet toolkit.
//!
//! Decodes images and prepares terminal-graphics payloads. Standalone (depends
//! on no other karet crate): usable for image preview in any TUI. Enable `view`
//! for a ratatui widget built on `ratatui-image`.
//!
//! # Responsibilities (to implement)
//! - `decode` — load and scale images via the `image` crate.
//! - `protocol` — halfblocks / Kitty / Sixel / iTerm2 encoders + safe capability detection.
//! - `view` — ratatui image widget (feature `view`).

// TODO: decode   — image loading & scaling.
// TODO: protocol — halfblocks/Kitty/Sixel/iTerm2 encoding + capability detection.
// TODO: view     — ratatui widget via ratatui-image (feature = "view").
