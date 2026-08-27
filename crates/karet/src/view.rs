//! The top-level view: which surface owns the terminal's whole content area.
//!
//! A [`View`] sits *above* panes and tabs. The editor view is the pane/tab shell
//! the app has always had; the others own the content area outright, so a surface
//! that is not a document — GitHub, agent sessions — need not be wedged into a tab
//! it does not behave like.
//!
//! This is the same shape as the sidebar's activity bar one level up: persistent
//! chrome above a body that swaps between N surfaces. The chrome row lives in
//! [`crate::ui::view_chrome`], the switch itself in
//! [`Command::SelectView`](crate::command::Command::SelectView).

use karet_widgets::UiIcon;

/// The surface that owns the content area.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum View {
    /// The pane/tab editor shell: explorer, documents, diffs, everything tabbed.
    #[default]
    Editor,
    /// The GitHub surface — issues, pull requests, and workflow runs.
    GitHub,
    /// Agent sessions across worktrees.
    Agents,
}

impl View {
    /// Every view, in switcher order.
    pub const ALL: [Self; 3] = [Self::Editor, Self::GitHub, Self::Agents];

    /// Whether the sidebar is drawn beside this view.
    ///
    /// The Agents view owns the full terminal width: it lists sessions across
    /// *every* worktree, so the sidebar's workspace-scoped panels — this
    /// checkout's files, this checkout's changes — have nothing to say about
    /// what it shows.
    #[must_use]
    pub const fn shows_sidebar(self) -> bool {
        !matches!(self, Self::Agents)
    }

    /// The view's name, as shown on the chrome row.
    #[must_use]
    pub const fn title(self) -> &'static str {
        match self {
            Self::Editor => "Editor",
            Self::GitHub => "GitHub",
            Self::Agents => "Agents",
        }
    }

    /// The view's chrome-row icon.
    #[must_use]
    pub const fn icon(self) -> UiIcon {
        match self {
            Self::Editor => UiIcon::ViewEditor,
            Self::GitHub => UiIcon::ViewGithub,
            Self::Agents => UiIcon::ViewAgents,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_editor_view_is_the_default() {
        // Startup must land on the shell the app has always opened with; a new
        // view becoming the default would be a silent behaviour change.
        assert_eq!(View::default(), View::Editor);
    }

    #[test]
    fn only_the_agents_view_takes_the_full_width() {
        assert!(View::Editor.shows_sidebar());
        assert!(View::GitHub.shows_sidebar());
        assert!(!View::Agents.shows_sidebar());
    }

    #[test]
    fn every_view_has_a_distinct_title_and_icon() {
        // Both are read as a group on one chrome row.
        let mut titles: Vec<&str> = View::ALL.iter().map(|view| view.title()).collect();
        titles.sort_unstable();
        titles.dedup();
        assert_eq!(titles.len(), View::ALL.len());
        let mut icons: Vec<UiIcon> = View::ALL.iter().map(|view| view.icon()).collect();
        icons.sort_unstable_by_key(|icon| format!("{icon:?}"));
        icons.dedup();
        assert_eq!(icons.len(), View::ALL.len());
    }
}
