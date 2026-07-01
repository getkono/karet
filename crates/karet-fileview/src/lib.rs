//! `karet-fileview` — read-only "render any file" widgets for karet.
//!
//! This crate hosts the file-view primitives a consumer needs to display a file
//! it will not edit: the [`hex`] dump for binaries, the terminal [`image`]
//! renderer (Kitty graphics with a truecolor halfblock fallback), and the
//! [`viewer`] placeholder for PDFs / oversized / undecodable files. It also
//! re-exports [`classify`](viewer::classify)/[`FileKind`](viewer::FileKind) from
//! `karet-filetype`.
//!
//! These primitives were relocated here from `karet-widgets` so an external
//! consumer can render files without pulling the full editor toolkit (fuzzy
//! matching, the file tree, LSP popups). The composed `FileDoc`/`FileView`
//! dispatch that ties the primitives (and the `karet-editor` text branch)
//! together is layered on top of them.

pub mod hex;
pub mod image;
pub mod viewer;

pub use hex::HexView;
