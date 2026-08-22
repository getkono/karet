//! [`Command::hint_verb`]: the terse verb the status bar advertises for a
//! command. Split out of `command.rs` to keep that file under the workspace
//! code-line ceiling — a pure relocation, no behaviour change.

use super::*;

impl Command {
    /// The terse verb shown after the chord in the status hints bar, or `None` to
    /// omit the command entirely. `None` covers the self-evident keys — cursor and
    /// scroll motion, selection extension, and raw text editing — that need no
    /// advertising, plus positional tab juggling the palette already covers. The
    /// match is exhaustive, so a new command must declare its hints-bar treatment.
    #[must_use]
    pub fn hint_verb(self) -> Option<&'static str> {
        Some(match self {
            // Seam view. Motion within it is self-evident; the operations that
            // change what is shown are the ones worth advertising.
            Self::SeamNextRow
            | Self::SeamPrevRow
            | Self::SeamNextColumn
            | Self::SeamPrevColumn
            | Self::SeamToggleFocus => return None,
            Self::SeamEnter => "reroot",
            Self::SeamWiden => "widen",
            Self::SeamOpenSource => "open source",
            Self::SeamFocusQuery => "filter",
            Self::SeamEscape => "clear",
            Self::SeamLens1
            | Self::SeamLens2
            | Self::SeamLens3
            | Self::SeamLens4
            | Self::SeamLens5 => "lens",
            Self::SeamClearLenses => "all lenses",
            Self::ShowSeamView => "seams",
            Self::SeamConfiguration => "config",
            Self::SeamCopyIdentity => "copy id",
            Self::SeamCopyQuery => "copy query",
            // Global.
            Self::Quit => "quit",
            Self::ToggleSidebar => "sidebar",
            Self::ToggleOutline => "outline",
            Self::ToggleFocus => "focus",
            Self::SelectPanel(SidebarPanel::Explorer) => "explorer",
            Self::SelectPanel(SidebarPanel::Search) => "search",
            Self::SelectPanel(SidebarPanel::SourceControl) => "git",
            Self::SelectPanel(SidebarPanel::Spelling) => "spelling",
            Self::SelectPanel(SidebarPanel::Todos) => "todos",
            Self::SelectPanel(SidebarPanel::Debug) => "debug",
            Self::TodoScan | Self::TodoToggleGrouping => "todos",
            Self::SpellingScan => "scan",
            Self::GoToDefinition => "definition",
            Self::JumpBack => "back",
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
            Self::ToggleInlineBlame => "blame",
            Self::OpenBlameDetail => "blame detail",
            Self::ShowLoadedConfig => "settings",
            Self::ManageLanguageServers => "language servers",
            Self::CheckLanguageServerUpdates => "lsp updates",
            Self::LanguageServerRefresh => "refresh",
            Self::LanguageServerCheckSelected => "check",
            Self::LanguageServerCheckAll => "check all",
            Self::LanguageServerPrimaryAction => "install/update",
            Self::LanguageServerRestart => "restart",
            Self::LanguageServerUninstall => "uninstall",
            Self::LanguageServerFilter => "filter",
            Self::ToggleFold => "fold",
            Self::AddCursorNextOccurrence => "add cursor",
            // Diff.
            Self::ToggleDiffLayout => "layout",
            Self::NextChangedFile => "next change",
            Self::PrevChangedFile => "prev change",
            Self::OpenDiffFile => "open file",
            Self::StageHunk => "stage hunk",
            Self::UnstageHunk => "unstage hunk",
            // Source control.
            Self::ScmStage => "stage",
            Self::ScmUnstage => "unstage",
            Self::ScmToggleStage => "toggle",
            Self::ScmStageAll => "stage all",
            Self::ScmUnstageAll => "unstage all",
            Self::ScmDiscard => "discard",
            Self::ScmCommit => "commit",
            Self::ScmRefresh => "refresh",
            Self::ScmSync => "sync",
            Self::ScmMenu => "more",
            Self::ScmSwitchBranch => "switch branch",
            Self::ScmCreateBranch => "create branch",
            Self::ScmPickPullRequest => "pull requests",
            Self::ScmUndoCommit => "undo commit",
            Self::ScmStash => "stash",
            Self::ScmManageStashes => "stashes",
            Self::ScmPublish => "publish",
            Self::ScmRenameBranch => "rename branch",
            Self::ScmDeleteBranch => "delete branch",
            Self::ScmDeleteRemoteBranch => "delete remote branch",
            Self::ScmContinue => "continue",
            Self::ScmAbort => "abort",
            Self::ScmSkip => "skip",
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
            Self::CommitCancel => "keep draft",
            Self::CommitGenerate => "generate",
            Self::ExplorerEditSubmit => "confirm",
            Self::ExplorerEditCancel => "cancel",
            Self::ConfirmDiscard => "confirm",
            Self::ConfirmExplorerDelete => "confirm",
            Self::ContextMenuAccept => "accept",
            Self::ContextMenuCancel => "cancel",
            Self::CloseConfirmSave => "save & close",
            Self::CloseConfirmDiscard => "discard & close",
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
            Self::FormatMarkdownTables => "format tables",
            Self::LatexBuildPreview => "build preview",
            Self::ResizePaneLeft
            | Self::ResizePaneRight
            | Self::ResizePaneUp
            | Self::ResizePaneDown => "resize pane",
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
            | Self::RevealActiveInExplorer
            | Self::CopyRemoteFileUrl
            | Self::CopyGithubPermalink
            | Self::CopyGithubHeadLink
            | Self::OpenChangesWithPrevious
            | Self::OpenChangesWithRevision
            | Self::OpenChangesWithBranch
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
            | Self::TriggerCompletion
            | Self::Hover
            | Self::ShowDiagnostic
            | Self::DebugStart
            | Self::DebugStop
            | Self::DebugPause
            | Self::DebugToggleBreakpoint
            | Self::DebugStepOver
            | Self::DebugStepIn
            | Self::DebugStepOut
            | Self::DebugEvaluatePrompt
            | Self::ToggleBold
            | Self::ToggleItalic
            | Self::ToggleStrikethrough
            | Self::ToggleInlineCode
            | Self::ToggleTaskCheckbox
            | Self::MarkdownTocCreate
            | Self::MarkdownTocUpdate
            | Self::MarkdownHeadingUp
            | Self::MarkdownHeadingDown
            | Self::MarkdownLintFixAll
            | Self::DepsRefresh
            | Self::DepsUpdate
            | Self::DepsUpdateAll
            | Self::CommitGraphMenu
            | Self::CommitGraphTag
            | Self::CommitGraphCherryPick
            | Self::CommitGraphRevert
            | Self::CommitGraphResetSoft
            | Self::CommitGraphResetMixed
            | Self::CommitGraphResetHard
            | Self::CommitGraphCheckout
            | Self::CommitGraphInteractiveRebase
            | Self::ScmFetch
            | Self::CommitToggleFileReviewed
            | Self::CommitGraphCopyIssueUrls
            | Self::InsertNewline
            | Self::DeleteBackward
            | Self::DeleteForward
            | Self::DeleteWordBackward
            | Self::DeleteWordForward
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
            | Self::DiffSinceBase
            | Self::LanguageServerUp
            | Self::LanguageServerDown => return None,
        })
    }
}
