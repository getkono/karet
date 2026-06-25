//! `karet-input` — a keymap engine for karet (and any TUI app).
//!
//! Standalone (depends on no other karet crate). Maps key sequences to action
//! identifiers with modal modes, chord sequences, context scoping and host
//! rebinding. Bring your own key-event type, enable `crossterm` for a conversion,
//! and `config` to load keymaps from files.
//!
//! # Responsibilities (to implement)
//! - `key` — backend-agnostic key event + modifiers.
//! - `keymap` — action ↔ key-sequence map, chords, context scoping.
//! - `mode` — modal modes (Normal/Insert/Visual/Command) or non-modal.
//! - `rebind` — host override API.
//! - `config` — (de)serialize keymaps (feature `config`).
//! - `crossterm` — crossterm key-event conversion (feature `crossterm`).

// TODO: key       — key event + modifier model.
// TODO: keymap    — keymap, chords, context scoping.
// TODO: mode      — modal mode state machine.
// TODO: rebind    — rebinding API.
// TODO: config    — keymap (de)serialization (feature = "config").
// TODO: crossterm — crossterm conversion (feature = "crossterm").
