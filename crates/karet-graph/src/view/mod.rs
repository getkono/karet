//! Ratatui painters for the graph models (behind the `view` feature).
//!
//! Two independent renderers live here, one per shape the crate lays out:
//!
//! - [`render_rail`] paints a [`RailRow`](crate::RailRow) — the commit-DAG lane gutter
//!   produced by [`assign_lanes`](crate::assign_lanes).
//! - [`graph_tree_lines`] flattens a [`GraphView`](karet_core::GraphView) into an
//!   indented tree.
//!
//! Neither depends on a theme crate: the caller passes plain ratatui styles (a
//! `lane_style` closure, or [`TreeStyles`]) and maps its own palette onto them.

mod rail;
mod tree;

pub use rail::render_rail;
pub use tree::TreeStyles;
pub use tree::graph_tree_lines;
