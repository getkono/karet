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
//! (the ordered list in `resolve::palette` is the single palette authority).

mod hints;
mod resolve;

#[cfg(test)]
mod tests;

// The app is a binary crate, so this compatibility re-export is not referenced
// internally even though it preserves the command module's existing API path.
#[allow(unused_imports)]
pub use resolve::ResolveNamedError;
pub use resolve::palette;
pub use resolve::resolve_named;

use crate::keymap::SidebarPanel;
use crate::view::View;

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
    /// Select the top-level view that owns the content area.
    SelectView(View),
    /// Re-run the workspace spelling scan behind the Spelling panel.
    SpellingScan,
    /// Re-run the workspace codetag scan behind the Todos panel.
    TodoScan,
    /// Switch the Todos panel between by-file and by-tag grouping.
    TodoToggleGrouping,
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
    /// Align every GFM table in the active Markdown document.
    FormatMarkdownTables,
    /// Compile the active TeX document and open its generated PDF preview.
    LatexBuildPreview,
    /// Split the focused pane into a new pane on the right.
    SplitRight,
    /// Split the focused pane into a new pane below.
    SplitDown,
    /// Move focus to the next pane.
    FocusNextPane,
    /// Move focus to the previous pane.
    FocusPrevPane,
    /// Grow the focused pane toward its left boundary.
    ResizePaneLeft,
    /// Grow the focused pane toward its right boundary.
    ResizePaneRight,
    /// Grow the focused pane toward its upper boundary.
    ResizePaneUp,
    /// Grow the focused pane toward its lower boundary.
    ResizePaneDown,
    /// Copy the selection (or the cursor line) to the clipboard.
    Copy,
    /// Copy the active file's absolute path to the clipboard.
    CopyPath,
    /// Copy the active file's workspace-relative path to the clipboard.
    CopyRelativePath,
    /// Reveal the active file in the explorer.
    RevealActiveInExplorer,
    /// Copy a web URL for the active file at the current `HEAD` commit on its
    /// origin remote (GitHub, GitLab, Gitea, or Forgejo).
    CopyRemoteFileUrl,
    /// Close the GitHub page in front, falling back to the one beneath it.
    GithubClosePage,
    /// Copy a GitHub permalink for the active file: the blob at the `HEAD` commit,
    /// anchored to the caret line in a code tab.
    CopyGithubPermalink,
    /// Copy a GitHub link to the active file on the current branch.
    CopyGithubHeadLink,
    /// Diff the active file's working text against its content at `HEAD`.
    OpenChangesWithPrevious,
    /// Pick a commit from the active file's history and diff the working text
    /// against the file's content at it.
    OpenChangesWithRevision,
    /// Pick a branch and diff the active file's working text against its content
    /// at the branch tip.
    OpenChangesWithBranch,
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
    /// Open the diffed file in a normal editor tab, at its first changed line
    /// (diff tab).
    OpenDiffFile,
    /// Stage the hunk at the top of the diff viewport (diff tab, working tree).
    StageHunk,
    /// Un-stage the hunk at the top of the diff viewport (diff tab, staged).
    UnstageHunk,
    /// Ask the language server for completions at the caret (Ctrl+Space).
    TriggerCompletion,
    /// Show hover documentation and diagnostics for the caret (Ctrl+K Ctrl+I).
    Hover,
    /// Open the diagnostics under the caret in a scrollable detail view
    /// (Ctrl+K Ctrl+M) — the surface for long, formatted errors.
    ShowDiagnostic,
    /// Start a debug session, or continue a stopped one (F5).
    DebugStart,
    /// End the debug session (Shift+F5).
    DebugStop,
    /// Pause the running debuggee (F6).
    DebugPause,
    /// Toggle a breakpoint on the caret line (F9).
    DebugToggleBreakpoint,
    /// Step over the current line (F10).
    DebugStepOver,
    /// Step into the call at the stop location (F11).
    DebugStepIn,
    /// Step out of the current frame (Shift+F11).
    DebugStepOut,
    /// Prompt for an expression and evaluate it in the debuggee (the REPL).
    DebugEvaluatePrompt,
    /// Run every code cell of the focused notebook preview on its kernel.
    NotebookRunAll,
    /// Interrupt the notebook kernel's running cell.
    NotebookInterrupt,
    /// Restart the notebook kernel (outputs are marked stale).
    NotebookRestartKernel,
    /// Toggle bold (`**`) around the selection or word (Markdown; Ctrl+B).
    ToggleBold,
    /// Toggle italic (`*`) around the selection or word (Markdown; Ctrl+I).
    ToggleItalic,
    /// Toggle strikethrough (`~~`) around the selection or word (Markdown; Alt+S).
    ToggleStrikethrough,
    /// Toggle an inline code span around the selection or word (Markdown).
    ToggleInlineCode,
    /// Toggle the task checkbox on the caret's line (Markdown; Alt+C).
    ToggleTaskCheckbox,
    /// Insert (or refresh) a `<!-- toc -->` table of contents at the caret.
    MarkdownTocCreate,
    /// Refresh the existing `<!-- toc -->` table of contents.
    MarkdownTocUpdate,
    /// Raise the caret line's heading level (Markdown; Ctrl+Shift+]).
    MarkdownHeadingUp,
    /// Lower the caret line's heading level (Markdown; Ctrl+Shift+[).
    MarkdownHeadingDown,
    /// Apply every markdownlint autofix in the active Markdown document.
    MarkdownLintFixAll,
    /// Re-run the dependency-freshness check for the active manifest.
    DepsRefresh,
    /// Bump the dependency under the caret to its newest version.
    DepsUpdate,
    /// Bump every outdated dependency in the active manifest.
    DepsUpdateAll,
    /// Open the action menu for the selected graph commit.
    CommitGraphMenu,
    /// Tag the selected graph commit (prompts for a name).
    CommitGraphTag,
    /// Cherry-pick the selected graph commit onto `HEAD`.
    CommitGraphCherryPick,
    /// Revert the selected graph commit on top of `HEAD`.
    CommitGraphRevert,
    /// Soft-reset the current branch to the selected graph commit.
    CommitGraphResetSoft,
    /// Mixed-reset the current branch to the selected graph commit.
    CommitGraphResetMixed,
    /// Hard-reset to the selected graph commit (typed confirmation).
    CommitGraphResetHard,
    /// Check the selected graph commit out, detaching `HEAD`.
    CommitGraphCheckout,
    /// Edit an interactive-rebase plan over the commits above the selection.
    CommitGraphInteractiveRebase,
    /// Fetch (and prune) every remote.
    ScmFetch,
    /// Toggle the current file's reviewed mark in a commit view.
    CommitToggleFileReviewed,
    /// Copy the issue URLs referenced by the selected graph commit.
    CommitGraphCopyIssueUrls,
    /// Jump to the definition of the symbol at the caret (F12).
    GoToDefinition,
    /// Return to the position a definition jump started from (Ctrl+Alt+Left).
    JumpBack,
    /// Insert a printable character at the caret (replacing any selection).
    InsertChar(char),
    /// Insert a newline with leading-whitespace auto-indent.
    InsertNewline,
    /// Delete the selection, or the character before the caret.
    DeleteBackward,
    /// Delete the selection, or the character after the caret.
    DeleteForward,
    /// Delete the selection, or the previous word/punctuation run.
    DeleteWordBackward,
    /// Delete the selection, or the next word/punctuation run.
    DeleteWordForward,
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
    /// Pull then push using repository configuration.
    ScmSync,
    /// Open the Source-Control actions menu.
    ScmMenu,
    /// Pick a local or remote branch to switch to.
    ScmSwitchBranch,
    /// Open the full create-branch form.
    ScmCreateBranch,
    /// Pick and check out an open GitHub pull request.
    ScmPickPullRequest,
    /// Guardedly undo the latest commit.
    ScmUndoCommit,
    /// Open the stash creation form.
    ScmStash,
    /// Open the stash manager.
    ScmManageStashes,
    /// Publish the current branch.
    ScmPublish,
    /// Rename the current local branch.
    ScmRenameBranch,
    /// Pick a local branch for safe deletion.
    ScmDeleteBranch,
    /// Pick a remote branch for typed-confirmation deletion.
    ScmDeleteRemoteBranch,
    /// Continue an in-progress Git operation.
    ScmContinue,
    /// Abort an in-progress Git operation.
    ScmAbort,
    /// Skip the current rebase or cherry-pick step.
    ScmSkip,
    /// Toggle inline current-line blame.
    ToggleInlineBlame,
    /// Open the current line's attributed commit.
    OpenBlameDetail,
    /// Open a read-only view of the loaded settings and their provenance.
    ShowLoadedConfig,
    /// Open the persistent language-server inventory and lifecycle manager.
    ManageLanguageServers,
    /// Explicitly check installed managed language servers for updates.
    CheckLanguageServerUpdates,
    /// Move to the previous row in the language-server manager.
    LanguageServerUp,
    /// Move to the next row in the language-server manager.
    LanguageServerDown,
    /// Reload language-server inventory without network access.
    LanguageServerRefresh,
    /// Force an update check for the selected managed provider.
    LanguageServerCheckSelected,
    /// Force an update check for every installed managed provider.
    LanguageServerCheckAll,
    /// Install or update the selected managed provider.
    LanguageServerPrimaryAction,
    /// Restart the selected provider in this session.
    LanguageServerRestart,
    /// Uninstall the selected Karet-managed provider.
    LanguageServerUninstall,
    /// Filter the language-server inventory.
    LanguageServerFilter,
    /// Forget a recorded refusal, so the selected provider is offered again.
    LanguageServerUndecline,
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
    /// Blur the commit input while preserving its draft.
    CommitCancel,
    /// Generate a commit message from the staged diff (AI).
    CommitGenerate,
    /// Commit the explorer inline name editor (create / rename).
    ExplorerEditSubmit,
    /// Cancel the explorer inline name editor.
    ExplorerEditCancel,
    /// Move the context menu selection up.
    ContextMenuUp,
    /// Move the context menu selection down.
    ContextMenuDown,
    /// Accept the selected context menu item.
    ContextMenuAccept,
    /// Dismiss the context menu.
    ContextMenuCancel,
    /// Move the confirmation dialog's selection up.
    ConfirmUp,
    /// Move the confirmation dialog's selection down.
    ConfirmDown,
    /// Run the confirmation dialog's selected choice.
    ConfirmAccept,
    /// Dismiss the confirmation dialog without acting.
    ConfirmCancel,
    /// At the close prompt (quit or tab/pane close): save the at-risk documents, then
    /// run the close.
    CloseConfirmSave,
    /// At the close prompt (quit or tab/pane close): discard unsaved changes and run
    /// the close.
    CloseConfirmDiscard,
    /// At the startup recovery prompt: restore the unsaved changes from a previous
    /// session's crash-recovery backups.
    RecoverSwaps,
    /// At the startup recovery prompt: discard the crash-recovery backups.
    DiscardSwaps,
    /// At the close prompt: abort the close and leave every tab untouched.
    CloseConfirmCancel,
    /// At the startup recovery prompt: leave the backups on disk for a later launch.
    DismissSwaps,
    /// Open the workspace package-dependency graph visualization.
    ShowDependencyGraph,
    /// Open the full-screen Seam view for the workspace package.
    ShowSeamView,
    /// Open the Seam view on a chosen start point rather than the workspace root.
    ShowSeamViewAt,
    /// Choose which configuration the Seam view reads the package under.
    SeamConfiguration,
    /// Copy the selected seam node's identity, which is its citation form.
    SeamCopyIdentity,
    /// Copy the query this Seam view is equivalent to.
    SeamCopyQuery,
    /// Re-index what changed since the Seam view was last built.
    SeamSync,
    /// Discard the stored index and read every file again.
    SeamForceSync,
    /// Move the Seam selection down within its column.
    SeamNextRow,
    /// Move the Seam selection up within its column.
    SeamPrevRow,
    /// Move Seam focus one column deeper.
    SeamNextColumn,
    /// Move Seam focus one column back.
    SeamPrevColumn,
    /// Reroot the Seam view at the selection, or follow the selected edge.
    SeamEnter,
    /// Step back out of the most recent Seam narrowing.
    SeamWiden,
    /// Open the selected seam node's source in an editor tab.
    SeamOpenSource,
    /// Move focus between the Seam spine and its facet pane.
    SeamToggleFocus,
    /// Focus the Seam query box.
    SeamFocusQuery,
    /// Leave the Seam query box, or clear the query when already outside it.
    SeamEscape,
    /// Toggle the first Seam lens (api).
    SeamLens1,
    /// Toggle the second Seam lens (substitution).
    SeamLens2,
    /// Toggle the third Seam lens (variation).
    SeamLens3,
    /// Toggle the fourth Seam lens (boundary).
    SeamLens4,
    /// Toggle the fifth Seam lens (hazard).
    SeamLens5,
    /// Clear every Seam lens filter.
    SeamClearLenses,
    /// Open the full-screen commit graph browser.
    ShowCommitGraph,
    /// Move the commit graph browser's selection to the next (older) commit.
    CommitGraphNext,
    /// Move the commit graph browser's selection to the previous (newer) commit.
    CommitGraphPrev,
    /// Open the graph view's selected commit as a standalone commit view.
    CommitGraphOpen,
    /// Pan the commit graph left (histories can be wider than the pane).
    CommitGraphPanLeft,
    /// Pan the commit graph right.
    CommitGraphPanRight,
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
    /// Expand the selected Search file group, or step into an open one.
    SearchExpand,
    /// Collapse the selected Search file group, or step up to its heading.
    SearchCollapse,
    /// Collapse every Search file group, or expand them all when none are.
    SearchToggleAll,
    /// Run the Search query and show its results.
    SearchRun,
    /// Stop editing the Search query without leaving the panel.
    SearchEndInput,
    /// Show or hide the Search replace field.
    SearchToggleReplace,
    /// Show or hide the Search include/exclude glob fields.
    SearchToggleFilters,
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
            Self::SelectView(View::Editor) => "View: Editor",
            Self::SelectView(View::GitHub) => "View: GitHub",
            // #211: Self::SelectView(View::Agents) => "View: Agents",
            Self::SelectPanel(SidebarPanel::Explorer) => "View: Show Explorer",
            Self::SelectPanel(SidebarPanel::Search) => "View: Show Search",
            Self::SelectPanel(SidebarPanel::SourceControl) => "View: Show Source Control",
            Self::SelectPanel(SidebarPanel::Spelling) => "View: Show Spelling",
            Self::SelectPanel(SidebarPanel::Todos) => "View: Show Todos",
            Self::SelectPanel(SidebarPanel::Debug) => "View: Show Debug",
            Self::SpellingScan => "Spelling: Scan Workspace",
            Self::TodoScan => "Todos: Scan Workspace",
            Self::TodoToggleGrouping => "Todos: Toggle Grouping (File / Tag)",
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
            Self::MarkdownPreviewSide => "Markdown: Toggle Preview to the Side",
            Self::FormatMarkdownTables => "Markdown: Format Tables",
            Self::LatexBuildPreview => "LaTeX: Build and Open PDF Preview",
            Self::SplitRight => "View: Split Editor Right",
            Self::SplitDown => "View: Split Editor Down",
            Self::FocusNextPane => "View: Focus Next Pane",
            Self::FocusPrevPane => "View: Focus Previous Pane",
            Self::ResizePaneLeft => "View: Resize Pane Left",
            Self::ResizePaneRight => "View: Resize Pane Right",
            Self::ResizePaneUp => "View: Resize Pane Up",
            Self::ResizePaneDown => "View: Resize Pane Down",
            Self::Copy => "Copy",
            Self::CopyPath => "Copy Path of Active File",
            Self::CopyRelativePath => "Copy Relative Path of Active File",
            Self::RevealActiveInExplorer => "File: Reveal Active File in Explorer",
            Self::CopyRemoteFileUrl => "Copy Remote File URL of Active File",
            Self::GithubClosePage => "GitHub: Close Page",
            Self::CopyGithubPermalink => "Copy GitHub Permalink of Active File",
            Self::CopyGithubHeadLink => "Copy GitHub Head Link of Active File",
            Self::OpenChangesWithPrevious => "Open Changes: With Previous Revision",
            Self::OpenChangesWithRevision => "Open Changes: With Revision…",
            Self::OpenChangesWithBranch => "Open Changes: With Branch…",
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
            Self::OpenDiffFile => "Diff: Open File",
            Self::StageHunk => "Diff: Stage Hunk",
            Self::UnstageHunk => "Diff: Unstage Hunk",
            Self::InsertChar(_) => "Insert Character",
            Self::TriggerCompletion => "Trigger Suggest",
            Self::Hover => "Show Hover",
            Self::ShowDiagnostic => "Show Diagnostic Detail",
            Self::DebugStart => "Debug: Start / Continue",
            Self::DebugStop => "Debug: Stop",
            Self::DebugPause => "Debug: Pause",
            Self::DebugToggleBreakpoint => "Debug: Toggle Breakpoint",
            Self::DebugStepOver => "Debug: Step Over",
            Self::DebugStepIn => "Debug: Step Into",
            Self::DebugStepOut => "Debug: Step Out",
            Self::DebugEvaluatePrompt => "Debug: Evaluate Expression",
            Self::NotebookRunAll => "Notebook: Run All Cells",
            Self::NotebookInterrupt => "Notebook: Interrupt Kernel",
            Self::NotebookRestartKernel => "Notebook: Restart Kernel",
            Self::ToggleBold => "Markdown: Toggle Bold",
            Self::ToggleItalic => "Markdown: Toggle Italic",
            Self::ToggleStrikethrough => "Markdown: Toggle Strikethrough",
            Self::ToggleInlineCode => "Markdown: Toggle Code Span",
            Self::ToggleTaskCheckbox => "Markdown: Toggle Task Checkbox",
            Self::MarkdownTocCreate => "Markdown: Create Table of Contents",
            Self::MarkdownTocUpdate => "Markdown: Update Table of Contents",
            Self::MarkdownHeadingUp => "Markdown: Increase Heading Level",
            Self::MarkdownHeadingDown => "Markdown: Decrease Heading Level",
            Self::MarkdownLintFixAll => "Markdown: Fix All Lint Issues",
            Self::DepsRefresh => "Dependencies: Re-check Versions",
            Self::DepsUpdate => "Dependencies: Update Dependency at Caret",
            Self::DepsUpdateAll => "Dependencies: Update All",
            Self::CommitGraphMenu => "Commit Graph: Actions",
            Self::CommitGraphTag => "Commit Graph: Create Tag",
            Self::CommitGraphCherryPick => "Commit Graph: Cherry-pick",
            Self::CommitGraphRevert => "Commit Graph: Revert",
            Self::CommitGraphResetSoft => "Commit Graph: Reset (Soft)",
            Self::CommitGraphResetMixed => "Commit Graph: Reset (Mixed)",
            Self::CommitGraphResetHard => "Commit Graph: Reset (Hard)…",
            Self::CommitGraphCheckout => "Commit Graph: Checkout (Detached)",
            Self::CommitGraphInteractiveRebase => "Commit Graph: Interactive Rebase from Here",
            Self::ScmFetch => "Git: Fetch",
            Self::CommitToggleFileReviewed => "Commit: Toggle File Reviewed",
            Self::CommitGraphCopyIssueUrls => "Commit Graph: Copy Issue URLs",
            Self::GoToDefinition => "Go to Definition",
            Self::JumpBack => "Go Back",
            Self::InsertNewline => "Insert Newline",
            Self::DeleteBackward => "Delete Backward",
            Self::DeleteForward => "Delete Forward",
            Self::DeleteWordBackward => "Delete Word Backward",
            Self::DeleteWordForward => "Delete Word Forward",
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
            Self::ScmSync => "Source Control: Sync",
            Self::ScmMenu => "Source Control: More Actions…",
            Self::ScmSwitchBranch => "Source Control: Switch Branch…",
            Self::ScmCreateBranch => "Source Control: Create Branch…",
            Self::ScmPickPullRequest => "Source Control: Pick Open Pull Request…",
            Self::ScmUndoCommit => "Source Control: Undo Last Commit",
            Self::ScmStash => "Source Control: Stash Changes…",
            Self::ScmManageStashes => "Source Control: Manage Stashes…",
            Self::ScmPublish => "Source Control: Publish Branch…",
            Self::ScmRenameBranch => "Source Control: Rename Current Branch…",
            Self::ScmDeleteBranch => "Source Control: Delete Local Branch…",
            Self::ScmDeleteRemoteBranch => "Source Control: Delete Remote Branch…",
            Self::ScmContinue => "Source Control: Continue Operation",
            Self::ScmAbort => "Source Control: Abort Operation",
            Self::ScmSkip => "Source Control: Skip Operation Step",
            Self::ToggleInlineBlame => "Source Control: Toggle Inline Blame",
            Self::OpenBlameDetail => "Source Control: Open Blame Details",
            Self::ShowLoadedConfig => "Settings: Show Loaded Configuration",
            Self::ManageLanguageServers => "Language Servers: Manage",
            Self::CheckLanguageServerUpdates => "Language Servers: Check for Updates…",
            Self::LanguageServerUp => "Language Servers: Select Previous",
            Self::LanguageServerDown => "Language Servers: Select Next",
            Self::LanguageServerRefresh => "Language Servers: Refresh",
            Self::LanguageServerCheckSelected => "Language Servers: Check Selected",
            Self::LanguageServerCheckAll => "Language Servers: Check All",
            Self::LanguageServerPrimaryAction => "Language Servers: Install / Update",
            Self::LanguageServerRestart => "Language Servers: Restart Selected",
            Self::LanguageServerUninstall => "Language Servers: Uninstall Selected",
            Self::LanguageServerFilter => "Language Servers: Filter…",
            Self::LanguageServerUndecline => "Language Servers: Offer Selected Again",
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
            Self::CommitCancel => "Commit: Keep Draft and Close",
            Self::CommitGenerate => "Commit: Generate Message (AI)",
            Self::ExplorerEditSubmit => "Explorer: Confirm Name",
            Self::ExplorerEditCancel => "Explorer: Cancel Edit",
            Self::ContextMenuUp => "Context Menu: Select Previous",
            Self::ContextMenuDown => "Context Menu: Select Next",
            Self::ContextMenuAccept => "Context Menu: Accept",
            Self::ContextMenuCancel => "Context Menu: Cancel",
            Self::ConfirmUp => "Confirm: Select Previous",
            Self::ConfirmDown => "Confirm: Select Next",
            Self::ConfirmAccept => "Confirm: Accept",
            Self::ConfirmCancel => "Confirm: Cancel",
            Self::CloseConfirmSave => "Confirm Close: Save and Close",
            Self::CloseConfirmDiscard => "Confirm Close: Discard and Close",
            Self::RecoverSwaps => "Recover Unsaved Changes",
            Self::DiscardSwaps => "Discard Unsaved Backups",
            Self::CloseConfirmCancel => "Confirm Close: Cancel",
            Self::DismissSwaps => "Recovery: Decide Later",
            Self::ShowDependencyGraph => "Visualize: Dependency Graph",
            // Key-driven navigation inside the view; deliberately absent from the
            // palette, but still named so nothing renders as a blank entry.
            Self::SeamNextRow => "Seam: Next Row",
            Self::SeamPrevRow => "Seam: Previous Row",
            Self::SeamNextColumn => "Seam: Next Column",
            Self::SeamPrevColumn => "Seam: Previous Column",
            Self::SeamEnter => "Seam: Reroot or Follow",
            Self::SeamWiden => "Seam: Widen",
            Self::SeamOpenSource => "Seam: Open Source",
            Self::SeamToggleFocus => "Seam: Toggle Facet Pane",
            Self::SeamFocusQuery => "Seam: Filter",
            Self::SeamEscape => "Seam: Clear Filter",
            Self::SeamLens1 => "Seam: Toggle API Lens",
            Self::SeamLens2 => "Seam: Toggle Substitution Lens",
            Self::SeamLens3 => "Seam: Toggle Variation Lens",
            Self::SeamLens4 => "Seam: Toggle Boundary Lens",
            Self::SeamLens5 => "Seam: Toggle Hazard Lens",
            Self::SeamClearLenses => "Seam: Clear Lenses",
            Self::ShowSeamView => "Seam: Open Seam View",
            Self::ShowSeamViewAt => "Seam: Open Seam View at…",
            Self::SeamConfiguration => "Seam: Set Configuration",
            Self::SeamCopyIdentity => "Seam: Copy Node Identity",
            Self::SeamCopyQuery => "Seam: Copy Query",
            Self::SeamSync => "Seam: Sync",
            Self::SeamForceSync => "Seam: Force Re-sync",
            Self::ShowCommitGraph => "Source Control: Commit Graph",
            Self::CommitGraphNext => "Commit Graph: Next Commit",
            Self::CommitGraphPrev => "Commit Graph: Previous Commit",
            Self::CommitGraphOpen => "Commit Graph: Open Commit",
            Self::CommitGraphPanLeft => "Commit Graph: Pan Left",
            Self::CommitGraphPanRight => "Commit Graph: Pan Right",
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
            Self::SearchExpand => "Search: Expand Group",
            Self::SearchCollapse => "Search: Collapse Group",
            Self::SearchToggleAll => "Search: Collapse or Expand All",
            Self::SearchToggleFilters => "Search: Toggle File Filters",
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
}
