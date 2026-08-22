//! The app test suite, one real module per surface. Shared fixtures and the
//! RecordingBackend live in [`support`]; every sibling starts with the same
//! two-glob prelude (the app scope plus the support helpers).

mod support;

mod blame;
mod commit_navigation;
mod commit_view;
mod definition;
mod deps;
mod diff_view;
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
mod save;
mod scm;
mod scroll;
mod search_completion;
mod spellcheck;
mod spelling_panel;
mod startup;
mod tabs_search;
mod todos;
