//! `karet-core` — the shared vocabulary for the karet TUI editor toolkit.
//!
//! This crate is intentionally tiny and dependency-light (no ratatui, no async
//! runtime): it defines the coordinate types and neutral data models that let
//! the other `karet-*` libraries interoperate, and that keep rendering widgets
//! decoupled from the engines that produce data. Producers (`karet-lsp`,
//! `karet-vcs`, `karet-dap`, …) *emit* these models; widgets (`karet-editor`,
//! `karet-widgets`) *render* them; the application connects the two.
//!
//! # Responsibilities (to implement)
//! - `geometry` — `Point`, `Size`, `Rect`, offsets, and the [`clamp`] helper.
//! - `coord` — text coordinates: `BytePos`, `CharPos`, `LineCol`, `Range`, `Span`.
//! - `model` — neutral models: `Diagnostic`/`Severity`, `Decoration`/`DecorationKind`,
//!   `Symbol`/`SymbolKind`, `InlayHint`, `CodeLens`.
//! - `provider` — interop traits such as `SymbolProvider`.
//! - `token` — semantic `TokenId` / `ThemeRole` vocabulary shared by syntax & theme.

// TODO: geometry  — Point/Size/Rect + offset math (clamp lives here for now).
// TODO: coord     — byte/char/line-col coordinates, Range, Span, conversions.
// TODO: model     — Diagnostic, Decoration, Symbol, InlayHint, CodeLens.
// TODO: provider  — SymbolProvider and other interop traits.
// TODO: token     — semantic TokenId / ThemeRole.

/// Clamp a value into the inclusive range `[min, max]`.
///
/// A foundational building block for laying out and constraining terminal UI
/// geometry (cursor positions, viewport sizes, scroll offsets).
#[must_use]
pub fn clamp(value: u16, min: u16, max: u16) -> u16 {
    value.max(min).min(max)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamps_within_range() {
        assert_eq!(clamp(5, 0, 10), 5);
        assert_eq!(clamp(15, 0, 10), 10);
        assert_eq!(clamp(0, 3, 10), 3);
    }
}
