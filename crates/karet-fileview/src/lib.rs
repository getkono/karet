//! `karet-fileview` — read-only file-view primitives for karet.
//!
//! This crate hosts the primitives a consumer needs to display a file it will
//! not edit: the [`hex`] dump for binaries, the terminal [`image`] renderer
//! (Kitty graphics with a truecolor halfblock fallback, behind the
//! `raster`/`images` features), and the [`viewer`] placeholder for oversized /
//! undecodable files. The [`viewer`] module also re-exports the routing items
//! from `karet-filetype` — [`FileKind`](viewer::FileKind),
//! [`classify`](viewer::classify),
//! [`classify_ignoring_size`](viewer::classify_ignoring_size) and
//! [`SIZE_GUARD`](viewer::SIZE_GUARD) — so a consumer can route a path to the
//! right primitive.
//!
//! Every item has exactly one path: the module that owns it
//! ([`hex::HexView`], [`viewer::Placeholder`], `image::ImageWidget`, …), and
//! `viewer` for the `karet-filetype` routing items. The crate root re-exports
//! nothing.
//!
//! An external consumer can render files with this crate without pulling the
//! full editor toolkit (fuzzy matching, the file tree, LSP popups). Composition — which primitive draws
//! which `FileKind`, and how text is highlighted — is the consumer's choice.

pub mod hex;
pub mod image;
pub mod viewer;
