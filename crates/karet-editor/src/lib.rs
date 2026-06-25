//! `karet-editor` — the composable code-editor widget for karet.
//!
//! Combines the text engines (`karet-text`, `karet-syntax`, `karet-theme`) into
//! a ratatui editor widget, rendering the highlight/fold/bracket data that
//! `karet-syntax` produces. By design it depends on **none** of the feature
//! producers (`karet-lsp`/`karet-vcs`/`karet-dap`/`karet-search`/`karet-terminal`):
//! diagnostics, git markers, breakpoints, inlay hints and code lenses arrive as
//! `karet-core` decorations supplied by the application.
//!
//! # Responsibilities (to implement)
//! - `view` — the core editor widget: layout, painting, input handling.
//! - `gutter` — line numbers + marker column + fold chevrons.
//! - `minimap` — downsampled overview + viewport highlight.
//! - `scroll` — vertical/horizontal scroll, scrollbar, smooth + sticky scroll.
//! - `aids` — indent guides, bracket/rainbow match, whitespace, current-line, word wrap.
//! - `overlay` — inlay hint / code lens / decoration rendering.
//! - `snippet` — snippet insertion + tab-stop navigation.
//!
//! # Internal dependencies
//! - `karet-core` — coordinates, decorations.
//! - `karet-text` — the buffer & cursors being edited.
//! - `karet-syntax` — highlight spans, fold regions, bracket pairs.
//! - `karet-theme` — styling (with the `view` feature for ratatui `Style`s).

// TODO: view    — editor widget render + input.
// TODO: gutter  — line numbers, markers, fold chevrons.
// TODO: minimap — downsampled overview.
// TODO: scroll  — scroll state, scrollbar, smooth/sticky.
// TODO: aids    — indent guides, bracket/rainbow, whitespace, current-line, wrap.
// TODO: overlay — inlay hints, code lens, decorations.
// TODO: snippet — snippet insertion + tab-stops.
