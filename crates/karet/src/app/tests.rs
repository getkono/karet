//! The app test suite, one real module per surface. Shared fixtures and the
//! RecordingBackend live in [`support`]; every sibling starts with the same
//! two-glob prelude (the app scope plus the support helpers).

mod support;

mod blame;
mod commit_graph;
mod commit_navigation;
mod commit_view;
mod debugging;
mod definition;
mod deps;
mod diff_view;
mod editor_drag;
mod editor_mouse;
mod explorer;
mod github;
mod hover;
mod inline_macros;
mod language_servers;
mod lifecycle;
mod markdown_edit;
mod preview;
mod remote;
mod review;
mod save;
mod scm;
mod scroll;
mod seam;
mod search_completion;
mod spellcheck;
mod spelling_panel;
mod startup;
mod surface_select;
mod tab_focus;
mod tabs_search;
mod todos;
mod view;
