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
//! matching, the file tree, LSP popups).
//!
//! # Composed dispatch
//!
//! [`FileDoc::prepare`] runs the expensive step once — classify, then decode /
//! parse / highlight — and [`FileView`] renders the result cheaply each frame,
//! dispatching text to a read-only [`karet_editor::Editor`], images to the Kitty /
//! halfblock renderer, binaries to [`HexView`], and everything else to a
//! [`Placeholder`](viewer::Placeholder). [`Limits`] bounds the size and highlight
//! budgets per context.
//!
//! ```no_run
//! use karet_fileview::{FileDoc, FileView, FileViewState, Limits};
//! # use std::path::Path;
//! # fn demo(path: &Path, frame: &mut ratatui::Frame, area: ratatui::layout::Rect) {
//! let bytes = std::fs::read(path).unwrap_or_default();
//! let len = bytes.len() as u64;
//! let doc = FileDoc::prepare(path, &bytes, len, &Limits::default()); // once
//! let mut state = FileViewState::new();
//! frame.render_stateful_widget(FileView::new(&doc), area, &mut state); // per frame
//! # }
//! ```

pub mod doc;
pub mod hex;
pub mod image;
pub mod view;
pub mod viewer;

pub use doc::FileDoc;
pub use doc::Limits;
pub use hex::HexView;
pub use view::FileView;
pub use view::FileViewState;
pub use view::flush_kitty_image;
