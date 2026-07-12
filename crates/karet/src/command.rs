//! The command registry: the single vocabulary of named operations the shell can
//! run.
//!
//! Both the keymap ([`crate::keymap`]) and the command palette
//! ([`crate::overlay`]) are derived from this enum, so a key binding and the hint
//! the palette shows for it can never drift. Positional, non-nameable interactions
//! (close tab *N*, reorder tabs, place the caret at a pixel) are *not* commands —
//! they call [`crate::app::App`] methods directly from the mouse handler.
//!
//! The trailing group of *modal-scoped* commands (overlay / find / search / commit
//! / discard navigation) is resolved only while the matching
//! [`crate::keymap::Modal`] context is active and is excluded from the palette
//! (see [`Command::in_palette`]).

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
    /// Open the active too-large-file placeholder anyway, bypassing the size guard.
    OpenAnyway,
    /// Dismiss the most recent notification.
    DismissNotification,
    /// Dismiss all notifications.
    DismissAllNotifications,
    /// Open a rendered preview of the active Markdown file in a pane to the right.
    MarkdownPreviewSide,
    /// Split the focused pane into a new pane on the right.
    SplitRight,
    /// Split the focused pane into a new pane below.
    SplitDown,
    /// Move focus to the next pane.
    FocusNextPane,
    /// Move focus to the previous pane.
    FocusPrevPane,
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
    /// Show or hide the right-side outline panel (and focus it when shown).
    ToggleOutline,
    /// Move the outline selection up.
    OutlineUp,
    /// Move the outline selection down.
    OutlineDown,
    /// Navigate to the selected outline entry (jump to its page / position).
    OutlineActivate,
    /// Leave the outline panel, returning focus to the editor.
    OutlineCollapse,
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
    /// Move the caret to the previous word boundary.
    CaretWordLeft,
    /// Move the caret to the next word boundary.
    CaretWordRight,
    /// Move the caret to the start of the line.
    CaretLineStart,
    /// Move the caret to the end of the line.
    CaretLineEnd,
    /// Move the caret to the start of the document.
    CaretDocStart,
    /// Move the caret to the end of the document.
    CaretDocEnd,
    /// Extend the selection to the previous word boundary.
    SelectWordLeft,
    /// Extend the selection to the next word boundary.
    SelectWordRight,
    /// Extend the selection to the start of the line.
    SelectLineStart,
    /// Extend the selection to the end of the line.
    SelectLineEnd,
    /// Extend the selection to the start of the document.
    SelectDocStart,
    /// Extend the selection to the end of the document.
    SelectDocEnd,
    /// Extend the selection up one page.
    SelectPageUp,
    /// Extend the selection down one page.
    SelectPageDown,
    /// Select the entire document in the editor.
    EditorSelectAll,
    /// Add a caret on the line above the primary caret.
    AddCursorAbove,
    /// Add a caret on the line below the primary caret.
    AddCursorBelow,
    /// Select the word under the caret, then add a caret at the next occurrence.
    AddCursorNextOccurrence,
    /// Collapse multiple carets to the primary; with one caret, leave the editor.
    CollapseCarets,
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
    /// Fold or unfold the code region at the cursor.
    ToggleFold,
    /// Move to the next changed file (diff tab).
    NextChangedFile,
    /// Move to the previous changed file (diff tab).
    PrevChangedFile,
    /// Insert a printable character at the caret (replacing any selection).
    InsertChar(char),
    /// Insert a newline with leading-whitespace auto-indent.
    InsertNewline,
    /// Delete the selection, or the character before the caret.
    DeleteBackward,
    /// Delete the selection, or the character after the caret.
    DeleteForward,
    /// Indent the caret line (or every selected line) by one level.
    Indent,
    /// Dedent the caret line by one level.
    Dedent,
    /// Undo the last edit group.
    Undo,
    /// Redo the last undone edit group.
    Redo,
    /// Save the active document to disk.
    Save,
    /// Cut the selection to the clipboard.
    Cut,
    /// Paste the clipboard at the caret.
    Paste,
    /// Extend the focused list pane's range selection up one row.
    SelectExtendUp,
    /// Extend the focused list pane's range selection down one row.
    SelectExtendDown,
    /// Toggle the cursor row in the focused list pane's selection.
    SelectToggle,
    /// Select every row in the focused list pane.
    SelectAll,
    /// Stage the selected Source-Control file(s).
    ScmStage,
    /// Unstage the selected Source-Control file(s).
    ScmUnstage,
    /// Stage or unstage the selected file(s), depending on their current section.
    ScmToggleStage,
    /// Stage every change in the worktree.
    ScmStageAll,
    /// Unstage every staged change.
    ScmUnstageAll,
    /// Discard the working-tree changes to the selected file(s).
    ScmDiscard,
    /// Open the commit-message input.
    ScmCommit,
    /// Recompute the Source-Control status.
    ScmRefresh,
    /// Open a semantic-blame view (blameline) for the active file.
    ShowBlame,
    /// Open a semantic-blame view narrowed to the function under the caret.
    BlameFunction,
    /// Open a read-only view of the loaded settings and their provenance.
    ShowLoadedConfig,
    /// Begin creating a new file in the explorer (inline name editor).
    ExplorerNewFile,
    /// Begin creating a new folder in the explorer (inline name editor).
    ExplorerNewFolder,
    /// Begin renaming the selected explorer entry (inline name editor).
    ExplorerRename,
    /// Hard-reload the explorer tree (and re-request VCS status).
    ExplorerRefresh,
    /// Collapse every expanded folder in the explorer.
    ExplorerCollapseAll,
    /// Copy the selected explorer item(s) into the explorer file clipboard.
    ExplorerCopy,
    /// Cut the selected explorer item(s) into the explorer file clipboard.
    ExplorerCut,
    /// Paste the explorer file clipboard into the selected destination.
    ExplorerPaste,
    /// Duplicate the selected explorer item(s) beside themselves.
    ExplorerDuplicate,
    /// Arm deletion of the selected explorer item(s).
    ExplorerDelete,
    /// Copy the selected explorer item path(s) to the clipboard.
    ExplorerCopyPath,
    /// Copy the selected explorer item path(s), relative to the workspace root.
    ExplorerCopyRelativePath,
    /// Open the explorer context menu at the current selection.
    ExplorerOpenContextMenu,

    // Modal-scoped commands. These are resolved only while a modal context is
    // active (see [`crate::keymap::Modal`]) and never appear in the command palette.
    /// Move the overlay selection up.
    OverlayUp,
    /// Move the overlay selection down.
    OverlayDown,
    /// Accept the highlighted overlay row.
    OverlayAccept,
    /// Dismiss the overlay.
    OverlayCancel,
    /// Jump to the next in-file find match.
    FindNext,
    /// Jump to the previous in-file find match.
    FindPrev,
    /// Close the find bar.
    FindCancel,
    /// Confirm the find bar: next match, or replace the current match in the replace
    /// field.
    FindSubmit,
    /// Replace every in-file match at once.
    FindReplaceAll,
    /// Show or hide the find bar's replace field.
    FindToggleReplace,
    /// Switch the edited find-bar field between find and replace.
    FindToggleField,
    /// Toggle the find bar's regex option.
    FindToggleRegex,
    /// Toggle the find bar's case-sensitivity option.
    FindToggleCase,
    /// Toggle the find bar's whole-word option.
    FindToggleWord,
    /// Submit the commit message.
    CommitSubmit,
    /// Cancel the commit input.
    CommitCancel,
    /// Generate a commit message from the staged diff (AI).
    CommitGenerate,
    /// Commit the explorer inline name editor (create / rename).
    ExplorerEditSubmit,
    /// Cancel the explorer inline name editor.
    ExplorerEditCancel,
    /// Confirm the pending discard.
    ConfirmDiscard,
    /// Confirm the pending explorer delete.
    ConfirmExplorerDelete,
    /// Move the context menu selection up.
    ContextMenuUp,
    /// Move the context menu selection down.
    ContextMenuDown,
    /// Accept the selected context menu item.
    ContextMenuAccept,
    /// Dismiss the context menu.
    ContextMenuCancel,
    /// At the quit prompt: save every unsaved document, then exit.
    QuitSaveAll,
    /// At the quit prompt: discard unsaved changes and exit.
    QuitDiscard,
    /// At the startup recovery prompt: restore the unsaved changes from a previous
    /// session's crash-recovery backups.
    RecoverSwaps,
    /// At the startup recovery prompt: discard the crash-recovery backups.
    DiscardSwaps,
    /// Open the workspace package-dependency graph visualization.
    ShowDependencyGraph,
    /// Open the full-screen commit graph browser.
    ShowCommitGraph,
    /// Move the commit graph browser's selection to the next (older) commit.
    CommitGraphNext,
    /// Move the commit graph browser's selection to the previous (newer) commit.
    CommitGraphPrev,
    /// Open the browser's selected commit as a standalone commit view.
    CommitGraphOpen,
    /// Open the go-to-commit input to view any commit by hash or ref.
    OpenCommitByHash,
    /// Submit the go-to-commit revision.
    RevInputSubmit,
    /// Cancel the go-to-commit input.
    RevInputCancel,
    /// Show the history of the active file (its commits) in the graph browser.
    ShowFileHistory,
    /// Compare the current branch's unpushed work against its upstream (`@{u}...HEAD`).
    DiffUnpushed,
    /// Compare the current branch against its base branch (`base...HEAD`).
    DiffSinceBase,
    /// Mark the commit graph browser's selected commit as the comparison base.
    CommitGraphMarkBase,
    /// Compare the browser's marked base commit against the current selection.
    CommitGraphCompare,
    /// Move the Search results selection up.
    SearchSelectUp,
    /// Move the Search results selection down.
    SearchSelectDown,
    /// Open the selected Search result.
    SearchOpen,
    /// Begin editing the Search query.
    SearchBeginInput,
    /// Leave the Search panel (from the results list).
    SearchQuit,
    /// Run the Search query and show its results.
    SearchRun,
    /// Stop editing the Search query without leaving the panel.
    SearchEndInput,
    /// Show or hide the Search replace field.
    SearchToggleReplace,
    /// Switch the edited Search field between find and replace.
    SearchToggleField,
    /// Apply the replacement across every workspace match.
    SearchReplaceAll,
    /// Toggle the Search regex option.
    SearchToggleRegex,
    /// Toggle the Search case-sensitivity option.
    SearchToggleCase,
    /// Toggle the Search whole-word option.
    SearchToggleWord,
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
            Self::OpenAnyway => "File: Open Anyway (Ignore Size Limit)",
            Self::DismissNotification => "Notifications: Dismiss",
            Self::DismissAllNotifications => "Notifications: Dismiss All",
            Self::MarkdownPreviewSide => "Markdown: Open Preview to the Side",
            Self::SplitRight => "View: Split Editor Right",
            Self::SplitDown => "View: Split Editor Down",
            Self::FocusNextPane => "View: Focus Next Pane",
            Self::FocusPrevPane => "View: Focus Previous Pane",
            Self::Copy => "Copy",
            Self::CopyPath => "Copy Path of Active File",
            Self::CopyRelativePath => "Copy Relative Path of Active File",
            Self::SidebarUp => "Sidebar: Select Previous",
            Self::SidebarDown => "Sidebar: Select Next",
            Self::SidebarActivate => "Sidebar: Open Selected",
            Self::SidebarCollapse => "Sidebar: Collapse",
            Self::SidebarToggleExpand => "Sidebar: Toggle Expand",
            Self::ToggleOutline => "View: Toggle Outline",
            Self::OutlineUp => "Outline: Select Previous",
            Self::OutlineDown => "Outline: Select Next",
            Self::OutlineActivate => "Outline: Go to Selected",
            Self::OutlineCollapse => "Outline: Close",
            Self::CaretUp => "Cursor Up",
            Self::CaretDown => "Cursor Down",
            Self::CaretLeft => "Cursor Left",
            Self::CaretRight => "Cursor Right",
            Self::SelectUp => "Select Up",
            Self::SelectDown => "Select Down",
            Self::SelectLeft => "Select Left",
            Self::SelectRight => "Select Right",
            Self::CaretWordLeft => "Cursor Word Left",
            Self::CaretWordRight => "Cursor Word Right",
            Self::CaretLineStart => "Cursor Line Start",
            Self::CaretLineEnd => "Cursor Line End",
            Self::CaretDocStart => "Cursor Document Start",
            Self::CaretDocEnd => "Cursor Document End",
            Self::SelectWordLeft => "Select Word Left",
            Self::SelectWordRight => "Select Word Right",
            Self::SelectLineStart => "Select to Line Start",
            Self::SelectLineEnd => "Select to Line End",
            Self::SelectDocStart => "Select to Document Start",
            Self::SelectDocEnd => "Select to Document End",
            Self::SelectPageUp => "Select Page Up",
            Self::SelectPageDown => "Select Page Down",
            Self::EditorSelectAll => "Selection: Select All",
            Self::AddCursorAbove => "Add Cursor Above",
            Self::AddCursorBelow => "Add Cursor Below",
            Self::AddCursorNextOccurrence => "Add Cursor to Next Occurrence",
            Self::CollapseCarets => "Collapse Cursors",
            Self::ScrollUp => "Scroll Up",
            Self::ScrollDown => "Scroll Down",
            Self::PageUp => "Scroll Page Up",
            Self::PageDown => "Scroll Page Down",
            Self::Top => "Go to Top",
            Self::Bottom => "Go to Bottom",
            Self::ToggleDiffLayout => "Diff: Toggle Inline / Side-by-Side",
            Self::ToggleFold => "Fold: Toggle at Cursor",
            Self::NextChangedFile => "Diff: Next Changed File",
            Self::PrevChangedFile => "Diff: Previous Changed File",
            Self::InsertChar(_) => "Insert Character",
            Self::InsertNewline => "Insert Newline",
            Self::DeleteBackward => "Delete Backward",
            Self::DeleteForward => "Delete Forward",
            Self::Indent => "Indent Line",
            Self::Dedent => "Dedent Line",
            Self::Undo => "Undo",
            Self::Redo => "Redo",
            Self::Save => "Save",
            Self::Cut => "Cut",
            Self::Paste => "Paste",
            Self::SelectExtendUp => "Selection: Extend Up",
            Self::SelectExtendDown => "Selection: Extend Down",
            Self::SelectToggle => "Selection: Toggle Row",
            Self::SelectAll => "Selection: Select All",
            Self::ScmStage => "Source Control: Stage Selected",
            Self::ScmUnstage => "Source Control: Unstage Selected",
            Self::ScmToggleStage => "Source Control: Stage / Unstage Selected",
            Self::ScmStageAll => "Source Control: Stage All Changes",
            Self::ScmUnstageAll => "Source Control: Unstage All Changes",
            Self::ScmDiscard => "Source Control: Discard Selected Changes",
            Self::ScmCommit => "Source Control: Commit…",
            Self::ScmRefresh => "Source Control: Refresh",
            Self::ShowBlame => "Source Control: Show Blame",
            Self::BlameFunction => "Source Control: Blame This Function",
            Self::ShowLoadedConfig => "Settings: Show Loaded Configuration",
            Self::ExplorerNewFile => "Explorer: New File…",
            Self::ExplorerNewFolder => "Explorer: New Folder…",
            Self::ExplorerRename => "Explorer: Rename…",
            Self::ExplorerRefresh => "Explorer: Refresh",
            Self::ExplorerCollapseAll => "Explorer: Collapse Folders",
            Self::ExplorerCopy => "Explorer: Copy",
            Self::ExplorerCut => "Explorer: Cut",
            Self::ExplorerPaste => "Explorer: Paste",
            Self::ExplorerDuplicate => "Explorer: Duplicate",
            Self::ExplorerDelete => "Explorer: Delete…",
            Self::ExplorerCopyPath => "Explorer: Copy Path",
            Self::ExplorerCopyRelativePath => "Explorer: Copy Relative Path",
            Self::ExplorerOpenContextMenu => "Explorer: Open Context Menu",
            Self::OverlayUp => "Overlay: Select Previous",
            Self::OverlayDown => "Overlay: Select Next",
            Self::OverlayAccept => "Overlay: Accept",
            Self::OverlayCancel => "Overlay: Cancel",
            Self::FindNext => "Find: Next Match",
            Self::FindPrev => "Find: Previous Match",
            Self::FindCancel => "Find: Close",
            Self::FindSubmit => "Find: Next / Replace Match",
            Self::FindReplaceAll => "Find: Replace All",
            Self::FindToggleReplace => "Find: Toggle Replace",
            Self::FindToggleField => "Find: Switch Find / Replace",
            Self::FindToggleRegex => "Find: Toggle Regular Expression",
            Self::FindToggleCase => "Find: Toggle Case Sensitivity",
            Self::FindToggleWord => "Find: Toggle Whole Word",
            Self::CommitSubmit => "Commit: Submit",
            Self::CommitCancel => "Commit: Cancel",
            Self::CommitGenerate => "Commit: Generate Message (AI)",
            Self::ExplorerEditSubmit => "Explorer: Confirm Name",
            Self::ExplorerEditCancel => "Explorer: Cancel Edit",
            Self::ConfirmDiscard => "Source Control: Confirm Discard",
            Self::ConfirmExplorerDelete => "Explorer: Confirm Delete",
            Self::ContextMenuUp => "Context Menu: Select Previous",
            Self::ContextMenuDown => "Context Menu: Select Next",
            Self::ContextMenuAccept => "Context Menu: Accept",
            Self::ContextMenuCancel => "Context Menu: Cancel",
            Self::QuitSaveAll => "Quit: Save All and Exit",
            Self::QuitDiscard => "Quit: Discard and Exit",
            Self::RecoverSwaps => "Recover Unsaved Changes",
            Self::DiscardSwaps => "Discard Unsaved Backups",
            Self::ShowDependencyGraph => "Visualize: Dependency Graph",
            Self::ShowCommitGraph => "Source Control: Commit Graph",
            Self::CommitGraphNext => "Commit Graph: Next Commit",
            Self::CommitGraphPrev => "Commit Graph: Previous Commit",
            Self::CommitGraphOpen => "Commit Graph: Open Commit",
            Self::OpenCommitByHash => "Source Control: Go to Commit…",
            Self::RevInputSubmit => "Go to Commit: Submit",
            Self::RevInputCancel => "Go to Commit: Cancel",
            Self::ShowFileHistory => "Source Control: File History",
            Self::DiffUnpushed => "Source Control: Diff Unpushed Changes",
            Self::DiffSinceBase => "Source Control: Diff vs Base Branch",
            Self::CommitGraphMarkBase => "Commit Graph: Mark Compare Base",
            Self::CommitGraphCompare => "Commit Graph: Compare with Marked",
            Self::SearchSelectUp => "Search: Select Previous",
            Self::SearchSelectDown => "Search: Select Next",
            Self::SearchOpen => "Search: Open Selected Result",
            Self::SearchBeginInput => "Search: Edit Query",
            Self::SearchQuit => "Search: Leave Panel",
            Self::SearchRun => "Search: Run Query",
            Self::SearchEndInput => "Search: Stop Editing Query",
            Self::SearchToggleReplace => "Search: Toggle Replace",
            Self::SearchToggleField => "Search: Switch Find / Replace",
            Self::SearchReplaceAll => "Search: Replace All",
            Self::SearchToggleRegex => "Search: Toggle Regular Expression",
            Self::SearchToggleCase => "Search: Toggle Case Sensitivity",
            Self::SearchToggleWord => "Search: Toggle Whole Word",
        }
    }

    /// The terse verb shown after the chord in the status hints bar, or `None` to
    /// omit the command entirely. `None` covers the self-evident keys — cursor and
    /// scroll motion, selection extension, and raw text editing — that need no
    /// advertising, plus positional tab juggling the palette already covers. The
    /// match is exhaustive, so a new command must declare its hints-bar treatment.
    #[must_use]
    pub fn hint_verb(self) -> Option<&'static str> {
        Some(match self {
            // Global.
            Self::Quit => "quit",
            Self::ToggleSidebar => "sidebar",
            Self::ToggleOutline => "outline",
            Self::ToggleFocus => "focus",
            Self::SelectPanel(SidebarPanel::Explorer) => "explorer",
            Self::SelectPanel(SidebarPanel::Search) => "search",
            Self::SelectPanel(SidebarPanel::SourceControl) => "git",
            Self::OpenQuickOpen => "open",
            Self::OpenCommandPalette => "commands",
            Self::OpenFind => "find",
            Self::OpenGlobalSearch => "find in files",
            Self::CloseTab => "close",
            Self::NextTab => "next tab",
            Self::PrevTab => "prev tab",
            Self::CloseOtherTabs => "close others",
            Self::CloseAllTabs => "close all",
            Self::ReopenClosedTab => "reopen",
            Self::OpenAnyway => "open anyway",
            Self::DismissNotification => "dismiss",
            Self::Copy => "copy",
            // Sidebar.
            Self::SidebarActivate => "open",
            Self::SidebarCollapse => "collapse",
            Self::SidebarToggleExpand => "expand",
            Self::SelectToggle => "select",
            Self::SelectAll => "select all",
            // Outline.
            Self::OutlineActivate => "go to",
            // Editor.
            Self::Undo => "undo",
            Self::Redo => "redo",
            Self::Save => "save",
            Self::Cut => "cut",
            Self::Paste => "paste",
            Self::ShowBlame => "blame",
            Self::BlameFunction => "blame fn",
            Self::ShowLoadedConfig => "settings",
            Self::ToggleFold => "fold",
            Self::AddCursorNextOccurrence => "add cursor",
            // Diff.
            Self::ToggleDiffLayout => "layout",
            Self::NextChangedFile => "next change",
            Self::PrevChangedFile => "prev change",
            // Source control.
            Self::ScmStage => "stage",
            Self::ScmUnstage => "unstage",
            Self::ScmToggleStage => "toggle",
            Self::ScmStageAll => "stage all",
            Self::ScmUnstageAll => "unstage all",
            Self::ScmDiscard => "discard",
            Self::ScmCommit => "commit",
            Self::ScmRefresh => "refresh",
            // Explorer.
            Self::ExplorerNewFile => "new file",
            Self::ExplorerNewFolder => "new folder",
            Self::ExplorerRename => "rename",
            Self::ExplorerRefresh => "refresh",
            Self::ExplorerCollapseAll => "collapse all",
            Self::ExplorerCopy => "copy",
            Self::ExplorerCut => "cut",
            Self::ExplorerPaste => "paste",
            Self::ExplorerDuplicate => "duplicate",
            Self::ExplorerDelete => "delete",
            Self::ExplorerCopyPath => "copy path",
            Self::ExplorerCopyRelativePath => "copy rel path",
            Self::ExplorerOpenContextMenu => "menu",
            // Modal-scoped.
            Self::OverlayAccept => "accept",
            Self::OverlayCancel => "cancel",
            Self::FindNext => "next",
            Self::FindPrev => "prev",
            Self::FindCancel => "close",
            Self::FindSubmit => "next",
            Self::FindReplaceAll => "replace all",
            Self::FindToggleReplace => "replace",
            Self::FindToggleField => "field",
            Self::FindToggleRegex => "regex",
            Self::FindToggleCase => "case",
            Self::FindToggleWord => "word",
            Self::CommitSubmit => "submit",
            Self::CommitCancel => "cancel",
            Self::CommitGenerate => "generate",
            Self::ExplorerEditSubmit => "confirm",
            Self::ExplorerEditCancel => "cancel",
            Self::ConfirmDiscard => "confirm",
            Self::ConfirmExplorerDelete => "confirm",
            Self::ContextMenuAccept => "accept",
            Self::ContextMenuCancel => "cancel",
            Self::QuitSaveAll => "save all & quit",
            Self::QuitDiscard => "discard & quit",
            Self::RecoverSwaps => "recover",
            Self::DiscardSwaps => "discard",
            Self::ShowDependencyGraph => "deps",
            Self::ShowCommitGraph => "graph",
            Self::CommitGraphNext => "next",
            Self::CommitGraphPrev => "prev",
            Self::CommitGraphOpen => "open",
            Self::CommitGraphMarkBase => "mark base",
            Self::CommitGraphCompare => "compare",
            Self::RevInputSubmit => "go",
            Self::RevInputCancel => "cancel",
            Self::SearchOpen => "open",
            Self::SearchBeginInput => "edit",
            Self::SearchQuit => "close",
            Self::SearchRun => "run",
            Self::SearchEndInput => "done",
            Self::SearchToggleReplace => "replace",
            Self::SearchToggleField => "field",
            Self::SearchReplaceAll => "replace all",
            Self::SearchToggleRegex => "regex",
            Self::SearchToggleCase => "case",
            Self::SearchToggleWord => "word",
            Self::MarkdownPreviewSide => "preview",
            // Self-evident motion, selection, and editing — no hint.
            Self::MoveTabLeft
            | Self::MoveTabRight
            | Self::GoToTab(_)
            | Self::CloseTabsToRight
            | Self::DismissAllNotifications
            | Self::SplitRight
            | Self::SplitDown
            | Self::FocusNextPane
            | Self::FocusPrevPane
            | Self::CopyPath
            | Self::CopyRelativePath
            | Self::SidebarUp
            | Self::SidebarDown
            | Self::OutlineUp
            | Self::OutlineDown
            | Self::OutlineCollapse
            | Self::CaretUp
            | Self::CaretDown
            | Self::CaretLeft
            | Self::CaretRight
            | Self::SelectUp
            | Self::SelectDown
            | Self::SelectLeft
            | Self::SelectRight
            | Self::CaretWordLeft
            | Self::CaretWordRight
            | Self::CaretLineStart
            | Self::CaretLineEnd
            | Self::CaretDocStart
            | Self::CaretDocEnd
            | Self::SelectWordLeft
            | Self::SelectWordRight
            | Self::SelectLineStart
            | Self::SelectLineEnd
            | Self::SelectDocStart
            | Self::SelectDocEnd
            | Self::SelectPageUp
            | Self::SelectPageDown
            | Self::EditorSelectAll
            | Self::AddCursorAbove
            | Self::AddCursorBelow
            | Self::CollapseCarets
            | Self::ScrollUp
            | Self::ScrollDown
            | Self::PageUp
            | Self::PageDown
            | Self::Top
            | Self::Bottom
            | Self::InsertChar(_)
            | Self::InsertNewline
            | Self::DeleteBackward
            | Self::DeleteForward
            | Self::Indent
            | Self::Dedent
            | Self::SelectExtendUp
            | Self::SelectExtendDown
            | Self::OverlayUp
            | Self::OverlayDown
            | Self::SearchSelectUp
            | Self::SearchSelectDown
            | Self::ContextMenuUp
            | Self::ContextMenuDown
            | Self::OpenCommitByHash
            | Self::ShowFileHistory
            | Self::DiffUnpushed
            | Self::DiffSinceBase => return None,
        })
    }

    /// Whether this command appears in the command palette.
    #[must_use]
    pub fn in_palette(self) -> bool {
        matches!(
            self,
            Self::Quit
                | Self::ToggleSidebar
                | Self::ToggleOutline
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
                | Self::ToggleFold
                | Self::AddCursorAbove
                | Self::AddCursorBelow
                | Self::AddCursorNextOccurrence
                | Self::Undo
                | Self::Redo
                | Self::Save
                | Self::Cut
                | Self::Paste
                | Self::ScmStageAll
                | Self::ScmUnstageAll
                | Self::ScmCommit
                | Self::ScmRefresh
                | Self::ShowBlame
                | Self::BlameFunction
                | Self::ShowLoadedConfig
                | Self::ExplorerNewFile
                | Self::ExplorerNewFolder
                | Self::ExplorerRename
                | Self::ExplorerRefresh
                | Self::ExplorerCollapseAll
                | Self::ExplorerCopy
                | Self::ExplorerCut
                | Self::ExplorerPaste
                | Self::ExplorerDuplicate
                | Self::ExplorerDelete
                | Self::ExplorerCopyPath
                | Self::ExplorerCopyRelativePath
                | Self::DismissNotification
                | Self::DismissAllNotifications
                | Self::MarkdownPreviewSide
                | Self::SplitRight
                | Self::SplitDown
                | Self::FocusNextPane
                | Self::FocusPrevPane
                | Self::ShowDependencyGraph
                | Self::ShowCommitGraph
                | Self::OpenCommitByHash
                | Self::ShowFileHistory
                | Self::DiffUnpushed
                | Self::DiffSinceBase
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
        Command::ToggleOutline,
        Command::ToggleFocus,
        Command::OpenFind,
        Command::OpenGlobalSearch,
        Command::ShowBlame,
        Command::ShowCommitGraph,
        Command::OpenCommitByHash,
        Command::ShowFileHistory,
        Command::DiffUnpushed,
        Command::DiffSinceBase,
        Command::BlameFunction,
        Command::ShowDependencyGraph,
        Command::ShowLoadedConfig,
        Command::ExplorerNewFile,
        Command::ExplorerNewFolder,
        Command::ExplorerRename,
        Command::ExplorerRefresh,
        Command::ExplorerCollapseAll,
        Command::ExplorerCopy,
        Command::ExplorerCut,
        Command::ExplorerPaste,
        Command::ExplorerDuplicate,
        Command::ExplorerDelete,
        Command::ExplorerCopyPath,
        Command::ExplorerCopyRelativePath,
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
        Command::Save,
        Command::Undo,
        Command::Redo,
        Command::Cut,
        Command::Paste,
        Command::ToggleDiffLayout,
        Command::ToggleFold,
        Command::AddCursorAbove,
        Command::AddCursorBelow,
        Command::AddCursorNextOccurrence,
        Command::ScmStageAll,
        Command::ScmUnstageAll,
        Command::ScmCommit,
        Command::ScmRefresh,
        Command::MarkdownPreviewSide,
        Command::SplitRight,
        Command::SplitDown,
        Command::FocusNextPane,
        Command::FocusPrevPane,
        Command::DismissNotification,
        Command::DismissAllNotifications,
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

    #[test]
    fn loaded_config_is_in_the_palette() {
        assert!(palette().contains(&Command::ShowLoadedConfig));
        assert_eq!(
            Command::ShowLoadedConfig.label(),
            "Settings: Show Loaded Configuration"
        );
    }

    #[test]
    fn hint_verbs_are_terse_and_gate_motion_keys() {
        // Advertised commands carry a non-empty terse verb…
        for cmd in [
            Command::Save,
            Command::Copy,
            Command::ScmStage,
            Command::FindNext,
            Command::CloseAllTabs,
        ] {
            assert!(
                cmd.hint_verb().is_some_and(|v| !v.is_empty()),
                "{cmd:?} should advertise a terse verb"
            );
        }
        // …while self-evident motion and text-editing keys are gated out of the bar.
        for cmd in [
            Command::CaretDown,
            Command::PageUp,
            Command::DeleteBackward,
            Command::InsertNewline,
            Command::SelectExtendDown,
        ] {
            assert!(
                cmd.hint_verb().is_none(),
                "{cmd:?} should not be advertised"
            );
        }
    }
}
