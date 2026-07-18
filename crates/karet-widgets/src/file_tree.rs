//! A lazy, gitignore-aware file-tree widget with per-file-type icons, VS Code–style
//! folder compaction, and a git-status overlay.
//!
//! [`FileTreeState`] owns the expansion set, selection, and a flattened cache of
//! the currently-visible rows. The [`FileTree`] builder supplies presentation: an
//! [`IconStyle`] (file icons resolved from the [`karet_filetype`] registry), an
//! optional theme, and a path-keyed status overlay (the application maps
//! `karet-vcs` statuses to `karet-core` [`Decoration`]s).
//!
//! **Gitignore (VS Code behavior):** gitignored files are *not* hidden — they are
//! listed and rendered dimmed (their [`ignored`](FileTreeRow::ignored) flag), so a
//! `target/` or `node_modules/` is visible but visually recedes. Dotfiles are shown
//! too; only the `.git` directory itself is always excluded.
//!
//! **Folder compaction:** a directory whose only entry is another directory is
//! merged into a single `a/b/c` row (like VS Code's "compact folders"). The row's
//! [`path`](FileTreeRow::path) is the *deepest* directory — expansion, selection,
//! and opening all act on it; toggling expands/collapses the whole chain.

mod model;
mod state;
mod view;

#[cfg(test)]
mod tests;

use std::collections::BTreeSet;
use std::path::Path;
use std::path::PathBuf;

use karet_core::Decoration;
use karet_core::DecorationKind;
use karet_core::ThemeRole;
use karet_filetype::Category;
use karet_filetype::IconStyle;
use karet_filetype::category_for_path;
use karet_filetype::chevron;
use karet_filetype::directory_icon;
use karet_filetype::icon_for_path;
use karet_theme::Theme;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::style::Modifier;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::widgets::StatefulWidget;
pub use state::FileTreeRow;
pub use state::FileTreeState;
pub use state::PendingEdit;
pub use view::FileTree;

use crate::ListSelection;
