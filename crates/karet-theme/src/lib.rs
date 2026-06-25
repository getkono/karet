//! `karet-theme` — color tokens, theme loading and contrast checking for karet.
//!
//! Maps the semantic `TokenId`/`ThemeRole` vocabulary (from `karet-core`) to
//! concrete colors, independent of any renderer (emits plain RGBA). Enable the
//! `view` feature to convert resolved colors into ratatui `Style`s.
//!
//! # Responsibilities (to implement)
//! - `palette` — semantic token → color resolution.
//! - `load` — `.tmTheme` (feature `tmtheme`) and VS Code JSON (feature `vscode`) loaders.
//! - `detect` — dark/light detection (COLORFGBG, OSC 10/11 query results).
//! - `contrast` — WCAG AA contrast checking.
//! - `view` — ratatui `Style` conversion (feature `view`).
//!
//! # Internal dependencies
//! - `karet-core` — TokenId / ThemeRole.

// TODO: palette  — TokenId/ThemeRole → RGBA resolution.
// TODO: load     — tmTheme + VS Code JSON theme loaders.
// TODO: detect   — terminal dark/light detection.
// TODO: contrast — WCAG AA contrast checker.
// TODO: view     — ratatui Style conversion (feature = "view").
