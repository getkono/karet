use super::*;

/// The single source of truth for key bindings. Within a [`Layer`] the first
/// matching binding wins (and [`hint_for`] returns the first binding for a command,
/// so list the preferred chord first); precedence *across* layers is decided by
/// [`active_layers`], not by table order.
#[rustfmt::skip]
pub(super) static BINDINGS: &[Binding] = &[
    // Global (any focus).
    b(Global, true,  false, false, Char('q'), Command::Quit),
    b(Global, true,  false, false, Char('c'), Command::Copy),
    b(Global, true,  false, false, Char('p'), Command::OpenQuickOpen),
    b(Global, true,  true,  false, Char('p'), Command::OpenCommandPalette),
    // F1 is a terminal-safe alternate for the command palette: some emulators
    // capture Ctrl+Shift+P before it reaches the app (see the app README).
    b(Global, false, false, false, F(1),      Command::OpenCommandPalette),
    b(Global, true,  false, false, Char('f'), Command::OpenFind),
    b(Global, true,  true,  false, Char('f'), Command::OpenGlobalSearch),
    b(Global, true,  false, false, Char('b'), Command::ToggleSidebar),
    b(Global, true,  true,  false, Char('o'), Command::ToggleOutline),
    b(Global, true,  false, false, Char('w'), Command::CloseTab),
    b(Global, true,  false, false, Char('1'), Command::SelectPanel(SidebarPanel::Explorer)),
    b(Global, true,  false, false, Char('2'), Command::SelectPanel(SidebarPanel::Search)),
    b(Global, true,  false, false, Char('3'), Command::SelectPanel(SidebarPanel::SourceControl)),
    b(Global, false, false, false, Tab,       Command::ToggleFocus),

    // Tab navigation & reordering (global).
    b(Global, true,  false, false, Tab,       Command::NextTab),
    b(Global, true,  true,  false, Tab,       Command::PrevTab),
    b(Global, true,  false, false, PageDown,  Command::NextTab),
    b(Global, true,  false, false, PageUp,    Command::PrevTab),
    b(Global, true,  true,  false, PageDown,  Command::MoveTabRight),
    b(Global, true,  true,  false, PageUp,    Command::MoveTabLeft),
    b(Global, true,  true,  false, Char('t'), Command::ReopenClosedTab),
    b(Global, false, false, true,  Char('1'), Command::GoToTab(1)),
    b(Global, false, false, true,  Char('2'), Command::GoToTab(2)),
    b(Global, false, false, true,  Char('3'), Command::GoToTab(3)),
    b(Global, false, false, true,  Char('4'), Command::GoToTab(4)),
    b(Global, false, false, true,  Char('5'), Command::GoToTab(5)),
    b(Global, false, false, true,  Char('6'), Command::GoToTab(6)),
    b(Global, false, false, true,  Char('7'), Command::GoToTab(7)),
    b(Global, false, false, true,  Char('8'), Command::GoToTab(8)),
    b(Global, false, false, true,  Char('9'), Command::GoToTab(9)),

    // Multi-key tab-management chords: a `Ctrl+K` prefix, then the action key.
    seq(Global, chord(true, false, false, Char('k')), &[chord(true,  false, false, Char('w'))], Command::CloseAllTabs),
    seq(Global, chord(true, false, false, Char('k')), &[chord(false, false, false, Char('w'))], Command::CloseOtherTabs),

    // Pane splitting & focus (VS Code parity): `Ctrl+\` splits right, `Ctrl+K Ctrl+\`
    // splits down, and `Ctrl+K` + arrow cycles pane focus.
    b(Global, true, false, false, Char('\\'), Command::SplitRight),
    seq(Global, chord(true, false, false, Char('k')), &[chord(true, false, false, Char('\\'))], Command::SplitDown),
    seq(Global, chord(true, false, false, Char('k')), &[chord(false, false, false, Right)], Command::FocusNextPane),
    seq(Global, chord(true, false, false, Char('k')), &[chord(false, false, false, Left)],  Command::FocusPrevPane),

    // Markdown preview to the side (VS Code parity: `Ctrl+K V`). Inert on a non-Markdown tab.
    seq(Global, chord(true, false, false, Char('k')), &[chord(false, false, false, Char('v'))], Command::MarkdownPreviewSide),

    // Source-Control panel (sidebar focus, SCM panel active). Listed before the
    // generic sidebar bindings so its keys win when both would match.
    b(SourceControl, false, false, false, Char(' '), Command::ScmToggleStage),
    b(SourceControl, false, false, false, Char('s'), Command::ScmStage),
    b(SourceControl, false, false, false, Char('u'), Command::ScmUnstage),
    b(SourceControl, false, false, false, Char('S'), Command::ScmStageAll),
    b(SourceControl, false, false, false, Char('U'), Command::ScmUnstageAll),
    b(SourceControl, false, false, false, Char('c'), Command::ScmCommit),
    b(SourceControl, false, false, false, Char('d'), Command::ScmDiscard),
    b(SourceControl, false, false, false, Char('r'), Command::ScmRefresh),
    b(SourceControl, false, false, false, Char('g'), Command::ShowCommitGraph),

    // Explorer panel (sidebar focus, Explorer active). Listed before the generic
    // sidebar bindings so its keys win. Path-copy shortcuts follow VS Code; placing
    // them first also keeps them visible in the width-limited status hints bar.
    b(Explorer, false, true,  true,  Char('c'), Command::ExplorerCopyPath),
    seq(Explorer, chord(true, false, false, Char('k')), &[chord(true, true, false, Char('c'))], Command::ExplorerCopyRelativePath),
    b(Explorer, false, false, false, Char('a'), Command::ExplorerNewFile),
    b(Explorer, false, false, false, Char('A'), Command::ExplorerNewFolder),
    b(Explorer, true,  false, false, Char('c'), Command::ExplorerCopy),
    b(Explorer, true,  false, false, Char('x'), Command::ExplorerCut),
    b(Explorer, true,  false, false, Char('v'), Command::ExplorerPaste),
    b(Explorer, true,  false, false, Char('d'), Command::ExplorerDuplicate),
    b(Explorer, false, false, false, Delete,    Command::ExplorerDelete),
    b(Explorer, false, true,  false, F(10),     Command::ExplorerOpenContextMenu),
    b(Explorer, false, false, false, F(2),      Command::ExplorerRename),
    b(Explorer, false, false, false, F(5),      Command::ExplorerRefresh),

    // Explorer inline name editor (new file/folder or rename).
    b(ExplorerEdit, false, false, false, Esc,   Command::ExplorerEditCancel),
    b(ExplorerEdit, false, false, false, Enter, Command::ExplorerEditSubmit),

    // Sidebar focus. Selection verbs are shared across every list panel (explorer
    // and source control) — they route to the focused panel's selection in dispatch.
    b(Sidebar, false, true,  false, Down,      Command::SelectExtendDown),
    b(Sidebar, false, true,  false, Up,        Command::SelectExtendUp),
    b(Sidebar, false, false, false, Char('x'), Command::SelectToggle),
    b(Sidebar, true,  false, false, Char('a'), Command::SelectAll),
    b(Sidebar, false, false, false, Char('q'), Command::Quit),
    b(Sidebar, false, false, false, Char('j'), Command::SidebarDown),
    b(Sidebar, false, false, false, Down,      Command::SidebarDown),
    b(Sidebar, false, false, false, Char('k'), Command::SidebarUp),
    b(Sidebar, false, false, false, Up,        Command::SidebarUp),
    b(Sidebar, false, false, false, Enter,     Command::SidebarActivate),
    b(Sidebar, false, false, false, Char('l'), Command::SidebarActivate),
    b(Sidebar, false, false, false, Right,     Command::SidebarActivate),
    b(Sidebar, false, false, false, Char('h'), Command::SidebarCollapse),
    b(Sidebar, false, false, false, Left,      Command::SidebarCollapse),
    b(Sidebar, false, false, false, Char(' '), Command::SidebarToggleExpand),

    // Right-side outline panel focus. Same list-navigation shape as the sidebar:
    // j/k or arrows move the selection, Enter/l/→ jumps to the entry, h/←/Esc leaves.
    b(Outline, false, false, false, Char('j'), Command::OutlineDown),
    b(Outline, false, false, false, Down,      Command::OutlineDown),
    b(Outline, false, false, false, Char('k'), Command::OutlineUp),
    b(Outline, false, false, false, Up,        Command::OutlineUp),
    b(Outline, false, false, false, Enter,     Command::OutlineActivate),
    b(Outline, false, false, false, Char('l'), Command::OutlineActivate),
    b(Outline, false, false, false, Right,     Command::OutlineActivate),
    b(Outline, false, false, false, Char('h'), Command::OutlineCollapse),
    b(Outline, false, false, false, Left,      Command::OutlineCollapse),
    b(Outline, false, false, false, Char('q'), Command::Quit),

    // Editor focus. The editor is non-modal: arrows/Home/End/PageUp-Down navigate,
    // and any unbound printable is text input (the shell inserts it after the keymap
    // declines). Bare-letter motions are intentionally gone so letters can be typed.
    // Esc collapses multiple carets to the primary; with a single caret it is a no-op
    // so repeated Esc never leaves the active editor view.
    b(Editor, false, false, false, Esc,       Command::CollapseCarets),
    b(Editor, false, false, false, Down,      Command::CaretDown),
    b(Editor, false, false, false, Up,        Command::CaretUp),
    b(Editor, false, false, false, Left,      Command::CaretLeft),
    b(Editor, false, false, false, Right,     Command::CaretRight),
    b(Editor, false, true,  false, Down,      Command::SelectDown),
    b(Editor, false, true,  false, Up,        Command::SelectUp),
    b(Editor, false, true,  false, Left,      Command::SelectLeft),
    b(Editor, false, true,  false, Right,     Command::SelectRight),
    b(Editor, false, false, false, PageDown,  Command::PageDown),
    b(Editor, false, false, false, PageUp,    Command::PageUp),
    // Word- and line-wise caret motion (VS Code parity). Home/End move to the line
    // edges; Ctrl+Home/End jump the caret to the document edges (moving it, not just
    // scrolling); Ctrl+Left/Right step by word.
    b(Editor, false, false, false, Home,      Command::CaretLineStart),
    b(Editor, false, false, false, End,       Command::CaretLineEnd),
    b(Editor, true,  false, false, Home,      Command::CaretDocStart),
    b(Editor, true,  false, false, End,       Command::CaretDocEnd),
    b(Editor, true,  false, false, Left,      Command::CaretWordLeft),
    b(Editor, true,  false, false, Right,     Command::CaretWordRight),
    // Selection: Shift extends each motion; Ctrl adds word/document granularity.
    b(Editor, false, true,  false, Home,      Command::SelectLineStart),
    b(Editor, false, true,  false, End,       Command::SelectLineEnd),
    b(Editor, true,  true,  false, Home,      Command::SelectDocStart),
    b(Editor, true,  true,  false, End,       Command::SelectDocEnd),
    b(Editor, true,  true,  false, Left,      Command::SelectWordLeft),
    b(Editor, true,  true,  false, Right,     Command::SelectWordRight),
    b(Editor, false, true,  false, PageDown,  Command::SelectPageDown),
    b(Editor, false, true,  false, PageUp,    Command::SelectPageUp),
    b(Editor, true,  false, false, Char('a'), Command::EditorSelectAll),
    // Multi-cursor (VS Code parity): Ctrl+Alt+Up/Down stack carets vertically, Ctrl+D
    // grows the selection to the next occurrence of the word / current selection.
    b(Editor, true,  false, true,  Up,        Command::AddCursorAbove),
    b(Editor, true,  false, true,  Down,      Command::AddCursorBelow),
    b(Editor, true,  false, false, Char('d'), Command::AddCursorNextOccurrence),
    // Editing.
    // Ctrl+Space asks the language server for completions (bypasses the
    // syntax-error auto-trigger gate).
    b(Editor, true,  false, false, Char(' '), Command::TriggerCompletion),
    b(Editor, false, false, false, Enter,     Command::InsertNewline),
    b(Editor, false, false, false, Backspace, Command::DeleteBackward),
    b(Editor, false, false, false, Delete,    Command::DeleteForward),
    b(Editor, true,  false, false, Char('s'), Command::Save),
    b(Editor, true,  false, false, Char('z'), Command::Undo),
    b(Editor, true,  false, false, Char('y'), Command::Redo),
    b(Editor, true,  true,  false, Char('z'), Command::Redo),
    b(Editor, true,  false, false, Char('x'), Command::Cut),
    b(Editor, true,  false, false, Char('v'), Command::Paste),

    // Semantic blame (blameline): whole file, or the function under the caret.
    b(Editor, true,  true,  false, Char('b'), Command::ShowBlame),
    b(Editor, false, false, true,  Char('b'), Command::BlameFunction),

    // Code folding (VS Code parity): `Ctrl+K Ctrl+L` toggles the fold at the cursor.
    seq(Editor, chord(true, false, false, Char('k')), &[chord(true, false, false, Char('l'))], Command::ToggleFold),

    // Editor focus, diff tab only. Enter drops from the read-only diff into the
    // underlying file ("editor mode"), landing on its first changed line.
    b(DiffEditor, false, false, false, Char('\\'), Command::ToggleDiffLayout),
    b(DiffEditor, false, false, false, Char(']'),  Command::NextChangedFile),
    b(DiffEditor, false, false, false, Char('['),  Command::PrevChangedFile),
    b(DiffEditor, false, false, false, Enter,      Command::OpenDiffFile),

    // Read-only scrollable views (commit / compare / blame / graph / hex): arrows and
    // PageUp/Down scroll, Home/End jump to the edges, `q` closes the tab. No caret.
    b(Pager, false, false, false, Down,      Command::ScrollDown),
    b(Pager, false, false, false, Up,        Command::ScrollUp),
    b(Pager, false, false, false, PageDown,  Command::PageDown),
    b(Pager, false, false, false, PageUp,    Command::PageUp),
    b(Pager, false, false, false, Home,      Command::Top),
    b(Pager, false, false, false, End,       Command::Bottom),
    b(Pager, false, false, false, Char(']'), Command::NextChangedFile),
    b(Pager, false, false, false, Char('['), Command::PrevChangedFile),
    b(Pager, false, false, false, Char('q'), Command::CloseTab),

    // The full-screen commit graph browser: j/k or arrows move the selection, Enter
    // opens the selected commit as a standalone view, Esc returns focus to the sidebar.
    b(CommitGraph, false, false, false, Char('j'), Command::CommitGraphNext),
    b(CommitGraph, false, false, false, Down,      Command::CommitGraphNext),
    b(CommitGraph, false, false, false, Char('k'), Command::CommitGraphPrev),
    b(CommitGraph, false, false, false, Up,        Command::CommitGraphPrev),
    b(CommitGraph, false, false, false, Enter,     Command::CommitGraphOpen),
    b(CommitGraph, false, false, false, Char('l'), Command::CommitGraphOpen),
    b(CommitGraph, false, false, false, Right,     Command::CommitGraphOpen),
    // Two-commit compare: `m` marks the selected commit as the base, `c` compares the
    // current selection against it.
    b(CommitGraph, false, false, false, Char('m'), Command::CommitGraphMarkBase),
    b(CommitGraph, false, false, false, Char('c'), Command::CommitGraphCompare),

    // Editor focus, a too-large-file placeholder: bypass the size guard on demand.
    // Enter loads it anyway; Esc is intentionally unbound so repeated Esc never
    // leaves the placeholder view.
    b(Oversize, false, false, false, Enter, Command::OpenAnyway),

    // Modal contexts. Each is exclusive (see `active_layers`); any key with no
    // binding here falls through to the modal's text input.
    // Quick-open / command palette.
    b(Overlay, false, false, false, Esc,       Command::OverlayCancel),
    b(Overlay, false, false, false, Enter,     Command::OverlayAccept),
    b(Overlay, false, false, false, Up,        Command::OverlayUp),
    b(Overlay, true,  false, false, Char('p'), Command::OverlayUp),
    b(Overlay, false, false, false, Down,      Command::OverlayDown),
    b(Overlay, true,  false, false, Char('n'), Command::OverlayDown),
    // In-file find bar (find + replace, mirroring the workspace Search panel).
    b(Find, false, false, false, Esc,       Command::FindCancel),
    b(Find, false, false, false, Enter,     Command::FindSubmit),
    b(Find, false, false, false, Down,      Command::FindNext),
    b(Find, false, false, false, Up,        Command::FindPrev),
    b(Find, true,  false, false, Char('g'), Command::FindNext),
    b(Find, true,  true,  false, Char('g'), Command::FindPrev),
    b(Find, false, false, false, Tab,       Command::FindToggleField),
    b(Find, false, false, true,  Enter,     Command::FindReplaceAll),
    b(Find, false, false, true,  Char('h'), Command::FindToggleReplace),
    b(Find, false, false, true,  Char('r'), Command::FindToggleRegex),
    b(Find, false, false, true,  Char('c'), Command::FindToggleCase),
    b(Find, false, false, true,  Char('w'), Command::FindToggleWord),
    // Commit-message input.
    b(CommitInput, false, false, false, Esc,   Command::CommitCancel),
    b(CommitInput, false, false, false, Enter, Command::CommitSubmit),
    b(CommitInput, true,  false, false, Char('g'), Command::CommitGenerate),
    // Go-to-commit (revision) input.
    b(RevInput, false, false, false, Esc,   Command::RevInputCancel),
    b(RevInput, false, false, false, Enter, Command::RevInputSubmit),
    // Discard confirmation: only the confirm keys are bound; anything else cancels.
    b(DiscardConfirm, false, false, false, Enter,     Command::ConfirmDiscard),
    b(DiscardConfirm, false, false, false, Char('y'), Command::ConfirmDiscard),
    b(DiscardConfirm, false, false, false, Char('Y'), Command::ConfirmDiscard),
    // Explorer delete confirmation: only confirm keys are bound; anything else cancels.
    b(ExplorerDeleteConfirm, false, false, false, Enter,     Command::ConfirmExplorerDelete),
    b(ExplorerDeleteConfirm, false, false, false, Char('y'), Command::ConfirmExplorerDelete),
    b(ExplorerDeleteConfirm, false, false, false, Char('Y'), Command::ConfirmExplorerDelete),
    // Context menu.
    b(ContextMenu, false, false, false, Esc,       Command::ContextMenuCancel),
    b(ContextMenu, false, false, false, Enter,     Command::ContextMenuAccept),
    b(ContextMenu, false, false, false, Up,        Command::ContextMenuUp),
    b(ContextMenu, false, false, false, Char('k'), Command::ContextMenuUp),
    b(ContextMenu, false, false, false, Down,      Command::ContextMenuDown),
    b(ContextMenu, false, false, false, Char('j'), Command::ContextMenuDown),
    // Close confirmation (unsaved changes: quit or tab/pane close): save, discard, or
    // (any other key) cancel — the default is always to abort.
    b(CloseConfirm, false, false, false, Char('s'), Command::CloseConfirmSave),
    b(CloseConfirm, false, false, false, Char('S'), Command::CloseConfirmSave),
    b(CloseConfirm, false, false, false, Char('d'), Command::CloseConfirmDiscard),
    b(CloseConfirm, false, false, false, Char('D'), Command::CloseConfirmDiscard),
    // Startup crash-recovery prompt: recover, discard, or (any other key) dismiss.
    b(SwapRecover, false, false, false, Char('r'), Command::RecoverSwaps),
    b(SwapRecover, false, false, false, Char('R'), Command::RecoverSwaps),
    b(SwapRecover, false, false, false, Char('d'), Command::DiscardSwaps),
    b(SwapRecover, false, false, false, Char('D'), Command::DiscardSwaps),
    // Workspace Search: navigating the results list.
    b(SearchList, false, false, false, Esc,       Command::SearchQuit),
    b(SearchList, false, false, false, Enter,     Command::SearchOpen),
    b(SearchList, false, false, false, Down,      Command::SearchSelectDown),
    b(SearchList, false, false, false, Char('j'), Command::SearchSelectDown),
    b(SearchList, false, false, false, Up,        Command::SearchSelectUp),
    b(SearchList, false, false, false, Char('k'), Command::SearchSelectUp),
    b(SearchList, false, false, false, Char('/'), Command::SearchBeginInput),
    b(SearchList, false, false, false, Char('r'), Command::SearchReplaceAll),
    b(SearchList, false, false, true,  Char('h'), Command::SearchToggleReplace),
    b(SearchList, false, false, true,  Char('r'), Command::SearchToggleRegex),
    b(SearchList, false, false, true,  Char('c'), Command::SearchToggleCase),
    b(SearchList, false, false, true,  Char('w'), Command::SearchToggleWord),
    // Workspace Search: editing the query / replacement.
    b(SearchInput, false, false, false, Esc,   Command::SearchEndInput),
    b(SearchInput, false, false, false, Enter, Command::SearchRun),
    b(SearchInput, false, false, false, Tab,   Command::SearchToggleField),
    b(SearchInput, false, false, true,  Char('h'), Command::SearchToggleReplace),
    b(SearchInput, false, false, true,  Char('r'), Command::SearchToggleRegex),
    b(SearchInput, false, false, true,  Char('c'), Command::SearchToggleCase),
    b(SearchInput, false, false, true,  Char('w'), Command::SearchToggleWord),
];
