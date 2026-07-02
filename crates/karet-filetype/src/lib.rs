//! `karet-filetype` — the single source of truth for file-type classification and
//! presentation metadata in the karet toolkit.
//!
//! It unifies three concerns that were previously scattered across the workspace:
//!
//! - **Identity** — [`file_type_for_path`] resolves a path to a [`FileType`]
//!   (display [`name`](FileType::name) + [`Category`]), matching well-known
//!   filenames first, then extension.
//! - **Presentation** — [`icon_for_path`] / [`FileType::icon`], [`directory_icon`],
//!   and [`chevron`] return glyphs for an [`IconStyle`] (`NerdFont` / `Unicode` /
//!   `Ascii`).
//! - **Routing** — [`classify`] returns a [`FileKind`] deciding which widget opens
//!   a file, using extension plus magic-byte sniffing.
//!
//! The crate is headless and dependency-free (only `std`), so any consumer can
//! depend on it without pulling in ratatui, tree-sitter, or `karet-core`.

mod classify;
mod icon;
mod registry;

pub use classify::FileKind;
pub use classify::SIZE_GUARD;
pub use classify::classify;
pub use classify::classify_with_guard;
pub use classify::classify_ignoring_size;
pub use icon::Category;
pub use icon::IconStyle;
pub use icon::chevron;
pub use icon::directory_icon;
pub use registry::FileType;
pub use registry::category_for_path;
pub use registry::file_type_for_path;
pub use registry::icon_for_path;
