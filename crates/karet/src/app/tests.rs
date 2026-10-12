//! The app test suite, one real module per surface. Shared fixtures and the
//! RecordingBackend live in [`support`]; every sibling starts with the same
//! two-glob prelude (the app scope plus the support helpers).

mod aicommit;
pub(crate) mod support;

mod blame;
mod code_actions;
mod comment;
mod commit_file_tree;
mod commit_graph;
mod commit_navigation;
mod commit_view;
mod confirm;
mod debugging;
mod definition;
mod deps;
mod diff_view;
mod editor_drag;
mod editor_mouse;
mod explorer;
mod github;
pub(crate) mod harness;
mod hover;
mod inlay;
mod inline_macros;
mod language_server_badge;
mod language_server_progress;
mod language_server_staleness;
mod language_servers;
mod lifecycle;
mod line_changes;
mod markdown_edit;
mod overlay_focus;
mod preview;
#[cfg(feature = "images")]
mod preview_images;
mod problems;
mod quick_open;
mod remote;
mod rename;
mod review;
mod save;
mod save_cursor;
mod scm;
mod scroll;
mod seam;
mod search_completion;
mod search_panel;
mod spellcheck;
mod spelling_panel;
mod startup;
mod surface_select;
mod tab_focus;
mod tabs_search;
mod todos;
mod unsupported;
mod view;
