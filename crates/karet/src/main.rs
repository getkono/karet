//! karet — a VS Code–parity TUI code editor built from the `karet-*` toolkit.
//!
//! This binary is the **presentation/client** half of karet and the composition
//! root. It constructs the headless backend (`karet-session`), drives it through
//! the `Backend` seam, and renders its `Event`s with `karet-editor` and
//! `karet-widgets`. The business logic (document/producer orchestration,
//! format-on-save, spell-check, settings/session-restore) lives server-side in
//! `karet-session`, so this crate stays a thin client — and a future remote split
//! is additive.
//!
//! Intended wiring (to implement):
//! 1. Init `tracing` + `color-eyre`; parse CLI args (`clap`) into a `SessionConfig`.
//! 2. Build the backend: `Session::new` + `karet_session::local` (in-process today;
//!    a remote client later, behind the same `Backend` trait).
//! 3. Set up the crossterm terminal and the pane/layout tree.
//! 4. Run the event loop: crossterm key events → the `input` keymap → `Command`s
//!    submitted via the `Backend`; drain the `EventRx` → render the editor (from
//!    `session.document(..)`) and the `karet-widgets` panels/popups.
//!
//! Client-side concerns merged in as modules (no standalone reuse beyond the app):
//! - `clipboard` — OSC 52 + external clipboard fallbacks (was `karet-clipboard`).
//! - `input` — the keymap engine (was `karet-input`).

// Skeleton: the merged `clipboard`/`input` modules and the composition wiring
// define the client API that the (still-`todo!()`) event loop will use. Allow
// dead_code until that loop is implemented and exercises them.
#![allow(dead_code)]

mod clipboard;
mod input;

use karet_session::{Session, SessionConfig, local};

fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;
    // TODO: init tracing-subscriber; parse CLI args (clap) into the SessionConfig.
    let config = SessionConfig::default();
    let (session, events) = Session::new(config);
    let backend = local(session);
    run(backend, events)
}

/// Run the TUI client: translate input into `karet_session::Command`s via the
/// backend, and render `karet_session::Event`s drained from `events`.
fn run(
    backend: karet_session::LocalBackend,
    events: karet_session::EventRx,
) -> color_eyre::Result<()> {
    // TODO: crossterm terminal setup; loop {
    //   read key -> input::Keymap<Command>::resolve -> backend.send(Command);
    //   while let Some((id, event)) = events.recv().await -> render editor + widgets;
    // }
    let _ = (backend, events);
    todo!()
}
