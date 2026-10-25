//! Stable string ids for [`Command`]s — the names a user keymap file
//! (`keymap.jsonc`) and `--command` use to refer to a command.
//!
//! An id is part of the user-facing configuration surface, so it is spelled out
//! here rather than derived from the variant name: renaming a variant must not
//! break anyone's keymap. Ids are lowerCamelCase (matching the setting keys); a
//! parameterized command appends its argument after a dot (`selectPanel.explorer`,
//! `goToTab.3`).
//!
//! The `match` in [`Command::id`] is exhaustive, so adding a variant without
//! deciding its id fails to compile. The few commands with no id are listed in the
//! `unbindable` block: they carry data no key can name (a typed character) or an
//! argument outside the bindable range.

use super::*;

/// Generate [`COMMAND_IDS`] and [`Command::id`] from one table, so the two
/// directions can never disagree.
macro_rules! command_ids {
    (
        unbindable: [$($un:pat),* $(,)?];
        $($v:ident $(($($arg:tt)*))? => $id:literal,)*
    ) => {
        /// Every command that has a stable id, paired with that id, in declaration
        /// order. The reverse index [`Command::from_id`] searches.
        pub(crate) static COMMAND_IDS: &[(Command, &str)] = &[
            $((Command::$v $(($($arg)*))?, $id),)*
        ];

        impl Command {
            /// This command's stable id (e.g. `"openQuickOpen"`), or `None` for a
            /// command no key binding can name ([`Command::InsertChar`]).
            #[must_use]
            pub fn id(self) -> Option<&'static str> {
                match self {
                    $(Command::$v $(($($arg)*))? => Some($id),)*
                    $($un => None,)*
                }
            }
        }
    };
}

impl Command {
    /// The command whose stable id is exactly `id` (case-sensitive), if any.
    #[must_use]
    pub fn from_id(id: &str) -> Option<Self> {
        COMMAND_IDS
            .iter()
            .find(|(_, candidate)| *candidate == id)
            .map(|(command, _)| *command)
    }
}

command_ids! {
    unbindable: [Command::InsertChar(_), Command::GoToTab(_), Command::FoldLevel(_)];
    Quit => "quit",
    ToggleSidebar => "toggleSidebar",
    ToggleFocus => "toggleFocus",
    SelectPanel(SidebarPanel::Explorer) => "selectPanel.explorer",
    SelectPanel(SidebarPanel::Search) => "selectPanel.search",
    SelectPanel(SidebarPanel::SourceControl) => "selectPanel.sourceControl",
    SelectPanel(SidebarPanel::Spelling) => "selectPanel.spelling",
    SelectPanel(SidebarPanel::Todos) => "selectPanel.todos",
    SelectPanel(SidebarPanel::Debug) => "selectPanel.debug",
    SelectPanel(SidebarPanel::Problems) => "selectPanel.problems",
    SelectView(View::Editor) => "selectView.editor",
    SelectView(View::GitHub) => "selectView.github",
    SpellingScan => "spellingScan",
    TodoScan => "todoScan",
    TodoToggleGrouping => "todoToggleGrouping",
    NextProblem => "nextProblem",
    PrevProblem => "prevProblem",
    ProblemsFilter => "problemsFilter",
    ProblemsCycleSeverity => "problemsCycleSeverity",
    ProblemsFilterAccept => "problemsFilterAccept",
    ProblemsFilterCancel => "problemsFilterCancel",
    OpenQuickOpen => "openQuickOpen",
    GoToSymbolInEditor => "goToSymbolInEditor",
    GoToSymbolInWorkspace => "goToSymbolInWorkspace",
    GoToLine => "goToLine",
    OpenRecentWorkspace => "openRecentWorkspace",
    OpenCommandPalette => "openCommandPalette",
    OpenFind => "openFind",
    OpenGlobalSearch => "openGlobalSearch",
    CloseTab => "closeTab",
    NextTab => "nextTab",
    PrevTab => "prevTab",
    MoveTabLeft => "moveTabLeft",
    MoveTabRight => "moveTabRight",
    GoToTab(1) => "goToTab.1",
    GoToTab(2) => "goToTab.2",
    GoToTab(3) => "goToTab.3",
    GoToTab(4) => "goToTab.4",
    GoToTab(5) => "goToTab.5",
    GoToTab(6) => "goToTab.6",
    GoToTab(7) => "goToTab.7",
    GoToTab(8) => "goToTab.8",
    GoToTab(9) => "goToTab.9",
    CloseOtherTabs => "closeOtherTabs",
    CloseTabsToRight => "closeTabsToRight",
    CloseAllTabs => "closeAllTabs",
    ReopenClosedTab => "reopenClosedTab",
    OpenAnyway => "openAnyway",
    ToggleTerminal => "toggleTerminal",
    NewTerminal => "newTerminal",
    TerminalCopy => "terminalCopy",
    TerminalPaste => "terminalPaste",
    TerminalScrollPageUp => "terminalScrollPageUp",
    TerminalScrollPageDown => "terminalScrollPageDown",
    TerminalScrollToTop => "terminalScrollToTop",
    TerminalScrollToBottom => "terminalScrollToBottom",
    DismissNotification => "dismissNotification",
    DismissAllNotifications => "dismissAllNotifications",
    MarkdownPreviewSide => "markdownPreviewSide",
    FormatMarkdownTables => "formatMarkdownTables",
    LatexBuildPreview => "latexBuildPreview",
    SplitRight => "splitRight",
    SplitDown => "splitDown",
    FocusNextPane => "focusNextPane",
    FocusPrevPane => "focusPrevPane",
    ResizePaneLeft => "resizePaneLeft",
    ResizePaneRight => "resizePaneRight",
    ResizePaneUp => "resizePaneUp",
    ResizePaneDown => "resizePaneDown",
    Copy => "copy",
    CopyPath => "copyPath",
    CopyRelativePath => "copyRelativePath",
    RevealActiveInExplorer => "revealActiveInExplorer",
    CopyRemoteFileUrl => "copyRemoteFileUrl",
    GithubClosePage => "githubClosePage",
    CopyGithubPermalink => "copyGithubPermalink",
    CopyGithubHeadLink => "copyGithubHeadLink",
    OpenChangesWithPrevious => "openChangesWithPrevious",
    OpenChangesWithRevision => "openChangesWithRevision",
    OpenChangesWithBranch => "openChangesWithBranch",
    SidebarUp => "sidebarUp",
    SidebarDown => "sidebarDown",
    SidebarActivate => "sidebarActivate",
    SidebarCollapse => "sidebarCollapse",
    SidebarToggleExpand => "sidebarToggleExpand",
    ToggleOutline => "toggleOutline",
    ToggleZenMode => "toggleZenMode",
    ToggleMaximizePane => "toggleMaximizePane",
    OutlineUp => "outlineUp",
    OutlineDown => "outlineDown",
    OutlineActivate => "outlineActivate",
    OutlineCollapse => "outlineCollapse",
    CaretUp => "caretUp",
    CaretDown => "caretDown",
    CaretLeft => "caretLeft",
    CaretRight => "caretRight",
    SelectUp => "selectUp",
    SelectDown => "selectDown",
    SelectLeft => "selectLeft",
    SelectRight => "selectRight",
    CaretWordLeft => "caretWordLeft",
    CaretWordRight => "caretWordRight",
    CaretLineStart => "caretLineStart",
    CaretLineEnd => "caretLineEnd",
    CaretDocStart => "caretDocStart",
    CaretDocEnd => "caretDocEnd",
    CaretPageUp => "caretPageUp",
    CaretPageDown => "caretPageDown",
    SelectWordLeft => "selectWordLeft",
    SelectWordRight => "selectWordRight",
    SelectLineStart => "selectLineStart",
    SelectLineEnd => "selectLineEnd",
    SelectDocStart => "selectDocStart",
    SelectDocEnd => "selectDocEnd",
    SelectPageUp => "selectPageUp",
    SelectPageDown => "selectPageDown",
    EditorSelectAll => "editorSelectAll",
    AddCursorAbove => "addCursorAbove",
    AddCursorBelow => "addCursorBelow",
    AddCursorNextOccurrence => "addCursorNextOccurrence",
    SelectAllOccurrences => "selectAllOccurrences",
    SkipOccurrence => "skipOccurrence",
    UndoCaretOperation => "undoCaretOperation",
    AddCursorsToLineEnds => "addCursorsToLineEnds",
    ColumnSelectUp => "columnSelectUp",
    ColumnSelectDown => "columnSelectDown",
    ColumnSelectLeft => "columnSelectLeft",
    ColumnSelectRight => "columnSelectRight",
    GoToBracket => "goToBracket",
    SelectToBracket => "selectToBracket",
    ExpandSelection => "expandSelection",
    ShrinkSelection => "shrinkSelection",
    CollapseCarets => "collapseCarets",
    ScrollUp => "scrollUp",
    ScrollDown => "scrollDown",
    PageUp => "pageUp",
    PageDown => "pageDown",
    Top => "top",
    Bottom => "bottom",
    ToggleDiffLayout => "toggleDiffLayout",
    ToggleFold => "toggleFold",
    FoldAll => "foldAll",
    UnfoldAll => "unfoldAll",
    FoldLevel(1) => "foldLevel.1",
    FoldLevel(2) => "foldLevel.2",
    FoldLevel(3) => "foldLevel.3",
    FoldLevel(4) => "foldLevel.4",
    FoldLevel(5) => "foldLevel.5",
    FoldLevel(6) => "foldLevel.6",
    FoldLevel(7) => "foldLevel.7",
    FoldRecursively => "foldRecursively",
    UnfoldRecursively => "unfoldRecursively",
    FoldAllComments => "foldAllComments",
    FoldAllImports => "foldAllImports",
    NextChangedFile => "nextChangedFile",
    PrevChangedFile => "prevChangedFile",
    OpenDiffFile => "openDiffFile",
    StageHunk => "stageHunk",
    UnstageHunk => "unstageHunk",
    TriggerCompletion => "triggerCompletion",
    Hover => "hover",
    SignatureHelp => "signatureHelp",
    ToggleInlayHints => "toggleInlayHints",
    ShowDiagnostic => "showDiagnostic",
    DebugStart => "debugStart",
    DebugStop => "debugStop",
    DebugPause => "debugPause",
    DebugToggleBreakpoint => "debugToggleBreakpoint",
    DebugStepOver => "debugStepOver",
    DebugStepIn => "debugStepIn",
    DebugStepOut => "debugStepOut",
    DebugEvaluatePrompt => "debugEvaluatePrompt",
    NotebookRunAll => "notebookRunAll",
    NotebookInterrupt => "notebookInterrupt",
    NotebookRestartKernel => "notebookRestartKernel",
    ToggleLineComment => "toggleLineComment",
    ToggleBlockComment => "toggleBlockComment",
    ToggleBold => "toggleBold",
    ToggleItalic => "toggleItalic",
    ToggleStrikethrough => "toggleStrikethrough",
    ToggleInlineCode => "toggleInlineCode",
    ToggleTaskCheckbox => "toggleTaskCheckbox",
    MarkdownTocCreate => "markdownTocCreate",
    MarkdownTocUpdate => "markdownTocUpdate",
    MarkdownHeadingUp => "markdownHeadingUp",
    MarkdownHeadingDown => "markdownHeadingDown",
    MarkdownLintFixAll => "markdownLintFixAll",
    DepsRefresh => "depsRefresh",
    DepsUpdate => "depsUpdate",
    DepsUpdateAll => "depsUpdateAll",
    CommitGraphMenu => "commitGraphMenu",
    CommitGraphTag => "commitGraphTag",
    CommitGraphCherryPick => "commitGraphCherryPick",
    CommitGraphRevert => "commitGraphRevert",
    CommitGraphResetSoft => "commitGraphResetSoft",
    CommitGraphResetMixed => "commitGraphResetMixed",
    CommitGraphResetHard => "commitGraphResetHard",
    CommitGraphCheckout => "commitGraphCheckout",
    CommitGraphInteractiveRebase => "commitGraphInteractiveRebase",
    ScmFetch => "scmFetch",
    CommitToggleFileReviewed => "commitToggleFileReviewed",
    CommitFoldFileTree => "commitFoldFileTree",
    CommitUnfoldFileTree => "commitUnfoldFileTree",
    CommitGraphCopyIssueUrls => "commitGraphCopyIssueUrls",
    GoToDefinition => "goToDefinition",
    OpenLinkAtCaret => "openLinkAtCaret",
    GoToDeclaration => "goToDeclaration",
    GoToTypeDefinition => "goToTypeDefinition",
    GoToImplementation => "goToImplementation",
    FindReferences => "findReferences",
    ShowIncomingCalls => "showIncomingCalls",
    ShowOutgoingCalls => "showOutgoingCalls",
    ShowSupertypes => "showSupertypes",
    ShowSubtypes => "showSubtypes",
    RenameSymbol => "renameSymbol",
    RenameSubmit => "renameSubmit",
    RenameCancel => "renameCancel",
    FormatDocument => "formatDocument",
    FormatSelection => "formatSelection",
    StartLinkedEditing => "startLinkedEditing",
    ToggleFormatOnType => "toggleFormatOnType",
    CodeActions => "codeActions",
    SourceAction => "sourceAction",
    OrganizeImports => "organizeImports",
    FixAll => "fixAll",
    JumpBack => "jumpBack",
    JumpForward => "jumpForward",
    InsertNewline => "insertNewline",
    Indent => "indent",
    Outdent => "outdent",
    DeleteBackward => "deleteBackward",
    DeleteForward => "deleteForward",
    DeleteWordBackward => "deleteWordBackward",
    DeleteWordForward => "deleteWordForward",
    Undo => "undo",
    Redo => "redo",
    Save => "save",
    Cut => "cut",
    Paste => "paste",
    SelectExtendUp => "selectExtendUp",
    SelectExtendDown => "selectExtendDown",
    SelectToggle => "selectToggle",
    SelectAll => "selectAll",
    ScmStage => "scmStage",
    ScmUnstage => "scmUnstage",
    ScmToggleStage => "scmToggleStage",
    ScmStageAll => "scmStageAll",
    ScmUnstageAll => "scmUnstageAll",
    ScmDiscard => "scmDiscard",
    ScmCommit => "scmCommit",
    ScmRefresh => "scmRefresh",
    ScmSync => "scmSync",
    ScmMenu => "scmMenu",
    ScmSwitchBranch => "scmSwitchBranch",
    ScmCreateBranch => "scmCreateBranch",
    ScmPickPullRequest => "scmPickPullRequest",
    ScmUndoCommit => "scmUndoCommit",
    ScmStash => "scmStash",
    ScmManageStashes => "scmManageStashes",
    ScmPublish => "scmPublish",
    ScmRenameBranch => "scmRenameBranch",
    ScmDeleteBranch => "scmDeleteBranch",
    ScmDeleteRemoteBranch => "scmDeleteRemoteBranch",
    ScmContinue => "scmContinue",
    ScmAbort => "scmAbort",
    ScmSkip => "scmSkip",
    ToggleInlineBlame => "toggleInlineBlame",
    OpenBlameDetail => "openBlameDetail",
    ShowLoadedConfig => "showLoadedConfig",
    SelectColorTheme => "selectColorTheme",
    ManageLanguageServers => "manageLanguageServers",
    CheckLanguageServerUpdates => "checkLanguageServerUpdates",
    ShowLanguageServerLog => "showLanguageServerLog",
    CancelLanguageServerWork => "cancelLanguageServerWork",
    LanguageServerUp => "languageServerUp",
    LanguageServerDown => "languageServerDown",
    LanguageServerRefresh => "languageServerRefresh",
    LanguageServerCheckSelected => "languageServerCheckSelected",
    LanguageServerCheckAll => "languageServerCheckAll",
    LanguageServerPrimaryAction => "languageServerPrimaryAction",
    LanguageServerRestart => "languageServerRestart",
    LanguageServerUninstall => "languageServerUninstall",
    LanguageServerFilter => "languageServerFilter",
    LanguageServerUndecline => "languageServerUndecline",
    ExplorerNewFile => "explorerNewFile",
    ExplorerNewFolder => "explorerNewFolder",
    ExplorerRename => "explorerRename",
    ExplorerRefresh => "explorerRefresh",
    ExplorerCollapseAll => "explorerCollapseAll",
    ExplorerCopy => "explorerCopy",
    ExplorerCut => "explorerCut",
    ExplorerPaste => "explorerPaste",
    ExplorerDuplicate => "explorerDuplicate",
    ExplorerDelete => "explorerDelete",
    ExplorerCopyPath => "explorerCopyPath",
    ExplorerCopyRelativePath => "explorerCopyRelativePath",
    ExplorerOpenContextMenu => "explorerOpenContextMenu",
    OverlayUp => "overlayUp",
    OverlayDown => "overlayDown",
    OverlayAccept => "overlayAccept",
    OverlayCancel => "overlayCancel",
    FindNext => "findNext",
    FindPrev => "findPrev",
    FindCancel => "findCancel",
    FindSubmit => "findSubmit",
    FindReplaceAll => "findReplaceAll",
    FindToggleReplace => "findToggleReplace",
    FindToggleField => "findToggleField",
    FindToggleRegex => "findToggleRegex",
    FindToggleCase => "findToggleCase",
    FindToggleWord => "findToggleWord",
    FindTogglePreserveCase => "findTogglePreserveCase",
    FindToggleInSelection => "findToggleInSelection",
    FindSelectAllMatches => "findSelectAllMatches",
    CommitSubmit => "commitSubmit",
    CommitCancel => "commitCancel",
    CommitGenerate => "commitGenerate",
    CommitGenerateUndo => "commitGenerateUndo",
    CommitConfigureAi => "commitConfigureAi",
    CommitConsoleClose => "commitConsoleClose",
    CommitConsoleCancel => "commitConsoleCancel",
    CommitConsoleShow => "commitConsoleShow",
    ExplorerEditSubmit => "explorerEditSubmit",
    ExplorerEditCancel => "explorerEditCancel",
    ContextMenuUp => "contextMenuUp",
    ContextMenuDown => "contextMenuDown",
    ContextMenuAccept => "contextMenuAccept",
    ContextMenuCancel => "contextMenuCancel",
    ConfirmUp => "confirmUp",
    ConfirmDown => "confirmDown",
    ConfirmAccept => "confirmAccept",
    ConfirmCancel => "confirmCancel",
    CloseConfirmSave => "closeConfirmSave",
    CloseConfirmDiscard => "closeConfirmDiscard",
    RecoverSwaps => "recoverSwaps",
    DiscardSwaps => "discardSwaps",
    CloseConfirmCancel => "closeConfirmCancel",
    DismissSwaps => "dismissSwaps",
    ShowDependencyGraph => "showDependencyGraph",
    ShowSeamView => "showSeamView",
    ShowSeamViewAt => "showSeamViewAt",
    SeamConfiguration => "seamConfiguration",
    SeamCopyIdentity => "seamCopyIdentity",
    SeamCopyQuery => "seamCopyQuery",
    SeamSync => "seamSync",
    SeamForceSync => "seamForceSync",
    SeamNextRow => "seamNextRow",
    SeamPrevRow => "seamPrevRow",
    SeamNextColumn => "seamNextColumn",
    SeamPrevColumn => "seamPrevColumn",
    SeamEnter => "seamEnter",
    SeamWiden => "seamWiden",
    SeamOpenSource => "seamOpenSource",
    SeamToggleFocus => "seamToggleFocus",
    SeamFocusQuery => "seamFocusQuery",
    SeamEscape => "seamEscape",
    SeamLens1 => "seamLens1",
    SeamLens2 => "seamLens2",
    SeamLens3 => "seamLens3",
    SeamLens4 => "seamLens4",
    SeamLens5 => "seamLens5",
    SeamClearLenses => "seamClearLenses",
    ShowCommitGraph => "showCommitGraph",
    CommitGraphNext => "commitGraphNext",
    CommitGraphPrev => "commitGraphPrev",
    CommitGraphOpen => "commitGraphOpen",
    CommitGraphPanLeft => "commitGraphPanLeft",
    CommitGraphPanRight => "commitGraphPanRight",
    OpenCommitByHash => "openCommitByHash",
    RevInputSubmit => "revInputSubmit",
    RevInputCancel => "revInputCancel",
    ShowFileHistory => "showFileHistory",
    DiffUnpushed => "diffUnpushed",
    DiffSinceBase => "diffSinceBase",
    CommitGraphMarkBase => "commitGraphMarkBase",
    CommitGraphCompare => "commitGraphCompare",
    SearchSelectUp => "searchSelectUp",
    SearchSelectDown => "searchSelectDown",
    SearchOpen => "searchOpen",
    SearchBeginInput => "searchBeginInput",
    SearchQuit => "searchQuit",
    SearchExpand => "searchExpand",
    SearchCollapse => "searchCollapse",
    SearchToggleAll => "searchToggleAll",
    SearchRun => "searchRun",
    SearchEndInput => "searchEndInput",
    SearchToggleReplace => "searchToggleReplace",
    SearchToggleFilters => "searchToggleFilters",
    SearchToggleField => "searchToggleField",
    SearchReplaceAll => "searchReplaceAll",
    SearchToggleRegex => "searchToggleRegex",
    SearchToggleCase => "searchToggleCase",
    SearchToggleWord => "searchToggleWord",
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_and_round_trip() {
        let mut seen = std::collections::HashSet::new();
        for &(command, id) in COMMAND_IDS {
            assert!(seen.insert(id), "duplicate command id {id}");
            // Case-folded uniqueness too: `--command` matches ids case-insensitively.
            assert_eq!(command.id(), Some(id));
            assert_eq!(Command::from_id(id), Some(command));
        }
        let folded: std::collections::HashSet<String> = COMMAND_IDS
            .iter()
            .map(|(_, id)| id.to_lowercase())
            .collect();
        assert_eq!(folded.len(), COMMAND_IDS.len());
    }

    #[test]
    fn every_palette_command_has_an_id() {
        for command in palette() {
            assert!(command.id().is_some(), "{command:?} has no id");
        }
    }

    #[test]
    fn command_flag_resolves_every_palette_id() {
        for command in palette() {
            let Some(id) = command.id() else { continue };
            assert_eq!(resolve_named(id), Ok(command), "--command {id}");
        }
        assert_eq!(
            resolve_named("SHOWCOMMITGRAPH"),
            Ok(Command::ShowCommitGraph)
        );
    }

    #[test]
    fn parameterized_commands_name_their_argument() {
        assert_eq!(
            Command::from_id("selectPanel.sourceControl"),
            Some(Command::SelectPanel(SidebarPanel::SourceControl))
        );
        assert_eq!(Command::from_id("goToTab.9"), Some(Command::GoToTab(9)));
        // Outside the bindable range, and a typed character, have no id.
        assert_eq!(Command::GoToTab(10).id(), None);
        assert_eq!(Command::InsertChar('x').id(), None);
        assert_eq!(Command::from_id("OpenQuickOpen"), None);
    }
}
