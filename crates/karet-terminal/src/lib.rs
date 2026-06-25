//! `karet-terminal` — an embeddable VT/ANSI terminal emulator for karet.
//!
//! Parses terminal output into a screen + scrollback model, spawns a PTY and
//! pumps its IO, and tracks selection and OSC 133 shell-integration marks. The
//! engine is renderer-agnostic; enable `view` for a ratatui widget.
//!
//! # Responsibilities (to implement)
//! - `parser` — VT/ANSI parsing into a cell grid (vt100).
//! - `pty` — PTY spawn, async read/write loop, resize/SIGWINCH forwarding.
//! - `scrollback` — capped scrollback buffer + scrolling.
//! - `select` — rectangular selection & copy.
//! - `shell` — OSC 133 prompt marks, link/path detection.
//! - `view` — ratatui terminal widget (feature `view`).
//!
//! # Internal dependencies
//! - `karet-core` — geometry for the `view` widget (optional, `view` only).

// TODO: parser     — vt100 parsing into a screen grid.
// TODO: pty        — portable-pty spawn + async IO loop + resize.
// TODO: scrollback — scrollback buffer + scroll.
// TODO: select     — rectangular selection & copy.
// TODO: shell      — OSC 133 marks + link/path detection.
// TODO: view       — ratatui widget (feature = "view").
