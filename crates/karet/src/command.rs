//! The command registry: the single vocabulary of named operations the shell can
//! run.
//!
//! Both the keymap ([`crate::keymap`]) and the command palette
//! ([`crate::overlay`]) are derived from this enum, so a key binding and the hint
//! the palette shows for it can never drift. Positional, non-nameable interactions
//! (close tab *N*, reorder tabs, place the caret at a pixel) are *not* commands —
//! they call [`crate::app::App`] methods directly from the mouse handler.

use crate::keymap::SidebarPanel;

/// A named operation runnable from a key binding or the command palette.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    /// Quit the application.
    Quit,
    /// Show or hide the sidebar.
    ToggleSidebar,
    /// Move focus between the sidebar and the editor.
    ToggleFocus,
    /// Select a sidebar panel.
    SelectPanel(SidebarPanel),
    /// Open the quick-open (go-to-file) overlay.
    OpenQuickOpen,
    /// Open the command palette overlay.
    OpenCommandPalette,
    /// Open the find-in-file bar.
    OpenFind,
    /// Focus the Search panel and start a query.
    OpenGlobalSearch,
    /// Close the active tab.
    CloseTab,
    /// Switch to the next tab.
    NextTab,
    /// Switch to the previous tab.
    PrevTab,
    /// Move the active tab one position towards the start.
    MoveTabLeft,
    /// Move the active tab one position towards the end.
    MoveTabRight,
    /// Switch to the tab at the given 1-based position (9 means "last").
    GoToTab(u8),
    /// Close every tab except the active one.
    CloseOtherTabs,
    /// Close every tab to the right of the active one.
    CloseTabsToRight,
    /// Close all tabs.
    CloseAllTabs,
    /// Reopen the most recently closed file tab.
    ReopenClosedTab,
    /// Copy the selection (or the cursor line) to the clipboard.
    Copy,
    /// Copy the active file's absolute path to the clipboard.
    CopyPath,
    /// Copy the active file's workspace-relative path to the clipboard.
    CopyRelativePath,
    /// Move the sidebar selection up.
    SidebarUp,
    /// Move the sidebar selection down.
    SidebarDown,
    /// Activate the selected sidebar row (open / expand).
    SidebarActivate,
    /// Collapse the selected directory / go to parent.
    SidebarCollapse,
    /// Toggle expansion of the selected directory.
    SidebarToggleExpand,
    /// Move the caret up one line.
    CaretUp,
    /// Move the caret down one line.
    CaretDown,
    /// Move the caret left one column.
    CaretLeft,
    /// Move the caret right one column.
    CaretRight,
    /// Extend the selection up one line.
    SelectUp,
    /// Extend the selection down one line.
    SelectDown,
    /// Extend the selection left one column.
    SelectLeft,
    /// Extend the selection right one column.
    SelectRight,
    /// Scroll the active tab up one line.
    ScrollUp,
    /// Scroll the active tab down one line.
    ScrollDown,
    /// Scroll the active tab up one page.
    PageUp,
    /// Scroll the active tab down one page.
    PageDown,
    /// Jump to the top of the active tab.
    Top,
    /// Jump to the bottom of the active tab.
    Bottom,
    /// Toggle a diff tab between unified and side-by-side.
    ToggleDiffLayout,
    /// Move to the next changed file (diff tab).
    NextChangedFile,
    /// Move to the previous changed file (diff tab).
    PrevChangedFile,
    /// Open a semantic-blame view (blameline) for the active file.
    ShowBlame,
    /// Open a semantic-blame view narrowed to the function under the caret.
    BlameFunction,
}

impl Command {
    /// The human-readable label shown in the command palette and used as the
    /// reverse-lookup key for hints.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Quit => "Quit",
            Self::ToggleSidebar => "View: Toggle Sidebar",
            Self::ToggleFocus => "View: Toggle Focus (Sidebar / Editor)",
            Self::SelectPanel(SidebarPanel::Explorer) => "View: Show Explorer",
            Self::SelectPanel(SidebarPanel::Search) => "View: Show Search",
            Self::SelectPanel(SidebarPanel::SourceControl) => "View: Show Source Control",
            Self::OpenQuickOpen => "Go to File…",
            Self::OpenCommandPalette => "Show All Commands",
            Self::OpenFind => "Find in File…",
            Self::OpenGlobalSearch => "Search: Find in Files…",
            Self::CloseTab => "View: Close Editor",
            Self::NextTab => "View: Open Next Editor",
            Self::PrevTab => "View: Open Previous Editor",
            Self::MoveTabLeft => "View: Move Editor Left",
            Self::MoveTabRight => "View: Move Editor Right",
            Self::GoToTab(_) => "View: Go to Tab",
            Self::CloseOtherTabs => "View: Close Other Editors",
            Self::CloseTabsToRight => "View: Close Editors to the Right",
            Self::CloseAllTabs => "View: Close All Editors",
            Self::ReopenClosedTab => "View: Reopen Closed Editor",
            Self::Copy => "Copy",
            Self::CopyPath => "Copy Path of Active File",
            Self::CopyRelativePath => "Copy Relative Path of Active File",
            Self::SidebarUp => "Sidebar: Select Previous",
            Self::SidebarDown => "Sidebar: Select Next",
            Self::SidebarActivate => "Sidebar: Open Selected",
            Self::SidebarCollapse => "Sidebar: Collapse",
            Self::SidebarToggleExpand => "Sidebar: Toggle Expand",
            Self::CaretUp => "Cursor Up",
            Self::CaretDown => "Cursor Down",
            Self::CaretLeft => "Cursor Left",
            Self::CaretRight => "Cursor Right",
            Self::SelectUp => "Select Up",
            Self::SelectDown => "Select Down",
            Self::SelectLeft => "Select Left",
            Self::SelectRight => "Select Right",
            Self::ScrollUp => "Scroll Up",
            Self::ScrollDown => "Scroll Down",
            Self::PageUp => "Scroll Page Up",
            Self::PageDown => "Scroll Page Down",
            Self::Top => "Go to Top",
            Self::Bottom => "Go to Bottom",
            Self::ToggleDiffLayout => "Diff: Toggle Inline / Side-by-Side",
            Self::NextChangedFile => "Diff: Next Changed File",
            Self::PrevChangedFile => "Diff: Previous Changed File",
            Self::ShowBlame => "Source Control: Show Blame",
            Self::BlameFunction => "Source Control: Blame This Function",
        }
    }

    /// Whether this command appears in the command palette.
    #[must_use]
    pub fn in_palette(self) -> bool {
        matches!(
            self,
            Self::Quit
                | Self::ToggleSidebar
                | Self::ToggleFocus
                | Self::SelectPanel(_)
                | Self::OpenQuickOpen
                | Self::OpenFind
                | Self::OpenGlobalSearch
                | Self::CloseTab
                | Self::NextTab
                | Self::PrevTab
                | Self::MoveTabLeft
                | Self::MoveTabRight
                | Self::CloseOtherTabs
                | Self::CloseTabsToRight
                | Self::CloseAllTabs
                | Self::ReopenClosedTab
                | Self::Copy
                | Self::CopyPath
                | Self::CopyRelativePath
                | Self::ToggleDiffLayout
                | Self::ShowBlame
                | Self::BlameFunction
        )
    }
}

/// The palette commands, in display order.
#[must_use]
pub fn palette() -> Vec<Command> {
    [
        Command::OpenQuickOpen,
        Command::SelectPanel(SidebarPanel::Explorer),
        Command::SelectPanel(SidebarPanel::Search),
        Command::SelectPanel(SidebarPanel::SourceControl),
        Command::ToggleSidebar,
        Command::ToggleFocus,
        Command::OpenFind,
        Command::OpenGlobalSearch,
        Command::ShowBlame,
        Command::BlameFunction,
        Command::Copy,
        Command::CopyPath,
        Command::CopyRelativePath,
        Command::NextTab,
        Command::PrevTab,
        Command::MoveTabLeft,
        Command::MoveTabRight,
        Command::CloseTab,
        Command::CloseOtherTabs,
        Command::CloseTabsToRight,
        Command::CloseAllTabs,
        Command::ReopenClosedTab,
        Command::ToggleDiffLayout,
        Command::Quit,
    ]
    .into_iter()
    .filter(|c| c.in_palette())
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_palette_command_has_a_label() {
        for cmd in palette() {
            assert!(!cmd.label().is_empty(), "{cmd:?} has no label");
            assert!(cmd.in_palette(), "{cmd:?} listed but not in_palette");
        }
    }

    #[test]
    fn command_palette_itself_is_not_listed() {
        assert!(!palette().contains(&Command::OpenCommandPalette));
    }
}
