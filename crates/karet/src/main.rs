//! karet — a VS Code–parity TUI code editor built from the `karet-*` toolkit.
//!
//! This binary is the composition root: it is the only crate that depends on
//! every library and wires the headless *producers* (`karet-lsp`, `karet-vcs`,
//! `karet-dap`, `karet-search`, …) to the *widgets* (`karet-editor`,
//! `karet-widgets`) by routing the producers' `karet-core` decorations,
//! diagnostics and symbols into the views. Engines never depend on each other
//! across features — the cross-feature wiring lives here, in the application.
//!
//! Intended wiring (to implement):
//! 1. Init `tracing` + `color-eyre`; parse CLI args (`clap`).
//! 2. Load a theme (`karet-theme`) and keymaps (`karet-input`); resolve per-file
//!    settings (editorconfig/.vscode via the `config` module).
//! 3. Build the layout/pane tree and mount `karet-editor` + `karet-widgets`.
//! 4. Open files into `karet-text`; highlight/fold via `karet-syntax`.
//! 5. Spawn async producers (`karet-lsp`, `karet-dap`, `karet-vcs`, `karet-terminal`)
//!    on the tokio runtime and forward their output as decorations/diagnostics.
//! 6. Run the event loop (`crossterm` events → `karet-input` → actions → render).
//!
//! App-only modules with no standalone reuse live alongside this entry point:
//! - `format` — format-on-save via external formatters (minimal edits via `karet-diff`).
//! - `spell` — spell-check (spellbook) emitting `karet-core` diagnostics.
//! - `config` — settings precedence + session restore (editorconfig/.vscode, recents, multi-root).

// TODO: format — external formatter invocation → minimal edits.
// TODO: spell  — spell-check comments/strings → diagnostics.
// TODO: config — settings precedence + session restore.

fn main() {
    // TODO: real entry point — see the wiring walkthrough in the module docs above.
    println!("karet — TUI code editor (skeleton; not yet implemented)");
}
