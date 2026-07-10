//! Keybinding layers and the focus state that selects them.
//!
//! A binding lives in exactly one [`Layer`]. For the currently focused pane
//! ([`FocusTarget`]) an ordered *stack* of layers is active ([`active_layers`]),
//! walked most-specific-first: the first layer holding a matching binding wins.
//! Precedence is therefore explicit data, not the order of the binding table.

/// Which area currently has keyboard focus.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Focus {
    /// The sidebar panel (explorer / search / source-control).
    #[default]
    Sidebar,
    /// The active editor tab.
    Editor,
    /// The right-side outline panel.
    Outline,
}

/// The sidebar's active panel.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SidebarPanel {
    /// The file explorer.
    #[default]
    Explorer,
    /// Workspace search results.
    Search,
    /// Source control (changed files).
    SourceControl,
}

/// The content kind of the active editor tab — the third input to
/// [`FocusTarget::from`], which picks the editor sub-target (and thus its
/// keybinding layer). Kept keymap-side and coarse: the shell maps its richer tab
/// model down to this, so the keymap need not know about documents or file kinds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EditorTab {
    /// A code/text tab, or any tab with no dedicated layer (image, …).
    #[default]
    Plain,
    /// A diff tab.
    Diff,
    /// A read-only scrollable view driven purely by scroll keys — the commit and
    /// compare views, blame, the dependency graph, and the hex dump.
    Pager,
    /// The full-screen commit graph browser.
    CommitGraph,
    /// A too-large-file placeholder, which offers an "open anyway" override.
    Oversize,
}

/// The single pane that currently holds keyboard focus.
///
/// This is the one value that decides which keybinding layers are live. It is a
/// *derived* view of the stored `(Focus, SidebarPanel, EditorTab)` state (see
/// [`FocusTarget::from`]) rather than a second source of truth — the sidebar
/// always has an active panel for rendering independent of who holds focus, so
/// the two stored fields stay orthogonal and this collapses them for dispatch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FocusTarget {
    /// A code editor tab.
    Editor,
    /// A diff editor tab.
    DiffEditor,
    /// A read-only scrollable view (commit / compare / blame / graph / hex).
    Pager,
    /// The full-screen commit graph browser.
    CommitGraph,
    /// A too-large-file placeholder, which offers an "open anyway" override.
    Oversize,
    /// The file explorer panel.
    Explorer,
    /// The workspace search panel.
    Search,
    /// The source-control panel.
    SourceControl,
    /// The right-side outline panel.
    Outline,
}

impl FocusTarget {
    /// Derive the focused pane from the stored focus, the active sidebar panel,
    /// and the content kind of the active editor tab.
    #[must_use]
    pub fn from(focus: Focus, panel: SidebarPanel, tab: EditorTab) -> Self {
        match focus {
            Focus::Outline => FocusTarget::Outline,
            Focus::Editor => match tab {
                EditorTab::Diff => FocusTarget::DiffEditor,
                EditorTab::Pager => FocusTarget::Pager,
                EditorTab::CommitGraph => FocusTarget::CommitGraph,
                EditorTab::Oversize => FocusTarget::Oversize,
                EditorTab::Plain => FocusTarget::Editor,
            },
            Focus::Sidebar => match panel {
                SidebarPanel::Explorer => FocusTarget::Explorer,
                SidebarPanel::Search => FocusTarget::Search,
                SidebarPanel::SourceControl => FocusTarget::SourceControl,
            },
        }
    }
}

/// A named scope a binding lives in. The [active stack](active_layers) for the
/// current [`Context`] decides which layers are live and in what precedence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layer {
    /// Active regardless of focus (lowest precedence, consulted last).
    Global,
    /// Active when any sidebar panel has focus.
    Sidebar,
    /// Active when the Explorer panel has focus (new file/folder, rename, refresh).
    Explorer,
    /// Active when the Source-Control panel has focus.
    SourceControl,
    /// Active when the right-side outline panel has focus.
    Outline,
    /// Active when a code or diff editor tab has focus.
    Editor,
    /// Active when a diff editor tab has focus.
    DiffEditor,
    /// Active when a read-only scrollable view (commit / compare / blame / graph /
    /// hex) has focus: scroll keys only, never the editor's editing/motion keys.
    Pager,
    /// Active when the full-screen commit graph browser has focus.
    CommitGraph,
    /// Active when a too-large-file placeholder has focus (the "open anyway"
    /// override). A placeholder is not editable, so this does not stack the
    /// [`Editor`](Layer::Editor) layer.
    Oversize,
    /// Active while the quick-open / command-palette overlay is open.
    Overlay,
    /// Active while the in-file find bar is open.
    Find,
    /// Active while editing the workspace Search query.
    SearchInput,
    /// Active while navigating the workspace Search results.
    SearchList,
    /// Active while the commit-message input is open.
    CommitInput,
    /// Active while the go-to-commit (revision) input is open.
    RevInput,
    /// Active while the discard-confirmation prompt is up.
    DiscardConfirm,
    /// Active while the explorer delete-confirmation prompt is up.
    ExplorerDeleteConfirm,
    /// Active while a context menu is open.
    ContextMenu,
    /// Active while the unsaved-changes close-confirmation prompt (quit or tab/pane
    /// close) is up.
    CloseConfirm,
    /// Active while the startup crash-recovery prompt is up.
    SwapRecover,
    /// Active while the explorer inline name editor is open.
    ExplorerEdit,
}

/// A text-capturing or transient context that shadows the focus layers. When one
/// is active the focus (pane) layers are suppressed, so its keys can't leak through
/// to the editor or sidebar; unbound keys become the modal's text input instead.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Modal {
    /// The quick-open / command-palette overlay.
    Overlay,
    /// The in-file find bar.
    Find,
    /// The workspace Search panel while editing its query.
    SearchInput,
    /// The workspace Search panel while navigating results.
    SearchList,
    /// The Source-Control commit-message input.
    CommitInput,
    /// The go-to-commit (revision) input.
    RevInput,
    /// The discard-confirmation prompt.
    DiscardConfirm,
    /// The explorer delete-confirmation prompt.
    ExplorerDeleteConfirm,
    /// A context menu.
    ContextMenu,
    /// The unsaved-changes confirmation prompt shown before an irreversible close
    /// (quit or closing a tab/pane).
    CloseConfirm,
    /// The startup prompt to recover crash-recovery backups.
    SwapRecover,
    /// The explorer inline name editor (new file/folder or rename).
    ExplorerEdit,
}

/// The full input context: an optional exclusive [`Modal`] over the focused pane.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Context {
    /// The active modal, if any (suppresses the focus layers).
    pub modal: Option<Modal>,
    /// The focused pane (still tracked so a modal can layer over it if wanted).
    pub target: FocusTarget,
}

impl Context {
    /// A plain focus context (no modal).
    #[must_use]
    pub fn focus(target: FocusTarget) -> Self {
        Self {
            modal: None,
            target,
        }
    }

    /// A modal context over `target`.
    #[must_use]
    pub fn modal(modal: Modal, target: FocusTarget) -> Self {
        Self {
            modal: Some(modal),
            target,
        }
    }
}

/// The ordered layer stack for `ctx`, most-specific first — so a specific binding
/// shadows a generic one and [`Layer::Global`] is consulted last. A modal context
/// is exclusive (only its own layer), except the two Search modals, which still let
/// global chords through. The resolver walks this stack and returns the first match.
#[must_use]
pub fn active_layers(ctx: Context) -> &'static [Layer] {
    use Layer as L;
    match ctx.modal {
        Some(Modal::Overlay) => &[L::Overlay],
        Some(Modal::Find) => &[L::Find],
        Some(Modal::CommitInput) => &[L::CommitInput],
        Some(Modal::RevInput) => &[L::RevInput],
        Some(Modal::DiscardConfirm) => &[L::DiscardConfirm],
        Some(Modal::ExplorerDeleteConfirm) => &[L::ExplorerDeleteConfirm],
        Some(Modal::ContextMenu) => &[L::ContextMenu],
        Some(Modal::CloseConfirm) => &[L::CloseConfirm],
        Some(Modal::SwapRecover) => &[L::SwapRecover],
        Some(Modal::ExplorerEdit) => &[L::ExplorerEdit],
        Some(Modal::SearchInput) => &[L::SearchInput, L::Global],
        Some(Modal::SearchList) => &[L::SearchList, L::Global],
        None => match ctx.target {
            FocusTarget::Outline => &[L::Outline, L::Global],
            FocusTarget::Editor => &[L::Editor, L::Global],
            // A diff is read-only: its own keys (layout toggle, next/prev change) stack
            // over the shared Pager scroll keys, never the editor's editing/motion keys.
            FocusTarget::DiffEditor => &[L::DiffEditor, L::Pager, L::Global],
            // A pager view is self-contained — its scroll layer stacks straight onto
            // Global, so arrows scroll rather than falling back to caret motion.
            FocusTarget::Pager => &[L::Pager, L::Global],
            // The browser is a self-contained list/detail view — its own layer stacks
            // straight onto Global, never the editor's editing/motion keys.
            FocusTarget::CommitGraph => &[L::CommitGraph, L::Global],
            FocusTarget::Oversize => &[L::Oversize, L::Global],
            FocusTarget::Explorer => &[L::Explorer, L::Sidebar, L::Global],
            FocusTarget::Search => &[L::Sidebar, L::Global],
            FocusTarget::SourceControl => &[L::SourceControl, L::Sidebar, L::Global],
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_control_shadows_sidebar_shadows_global() {
        // The SCM panel layers SourceControl over the shared Sidebar verbs over the
        // global chords, in that precedence order.
        assert_eq!(
            active_layers(Context::focus(FocusTarget::SourceControl)),
            &[Layer::SourceControl, Layer::Sidebar, Layer::Global]
        );
    }

    #[test]
    fn diff_editor_falls_through_to_pager() {
        // A diff stacks its own keys over the shared Pager scroll keys, not the
        // editor's editing/motion keys (it is read-only).
        assert_eq!(
            active_layers(Context::focus(FocusTarget::DiffEditor)),
            &[Layer::DiffEditor, Layer::Pager, Layer::Global]
        );
        assert_eq!(
            active_layers(Context::focus(FocusTarget::Editor)),
            &[Layer::Editor, Layer::Global]
        );
    }

    #[test]
    fn pager_stacks_straight_onto_global() {
        // A pager view scrolls with its own layer over Global — no caret motion.
        assert_eq!(
            active_layers(Context::focus(FocusTarget::Pager)),
            &[Layer::Pager, Layer::Global]
        );
        assert_eq!(
            FocusTarget::from(Focus::Editor, SidebarPanel::Explorer, EditorTab::Pager),
            FocusTarget::Pager
        );
    }

    #[test]
    fn oversize_placeholder_is_its_own_layer_over_global() {
        // A too-large placeholder is not editable, so its layer stacks straight onto
        // Global — the Editor layer's editing/motion keys must not leak in.
        assert_eq!(
            active_layers(Context::focus(FocusTarget::Oversize)),
            &[Layer::Oversize, Layer::Global]
        );
        // A too-large placeholder tab in the editor resolves to the Oversize target.
        assert_eq!(
            FocusTarget::from(Focus::Editor, SidebarPanel::Explorer, EditorTab::Oversize),
            FocusTarget::Oversize
        );
    }

    #[test]
    fn global_is_always_last_for_focus_contexts() {
        for target in [
            FocusTarget::Editor,
            FocusTarget::DiffEditor,
            FocusTarget::Pager,
            FocusTarget::Oversize,
            FocusTarget::Explorer,
            FocusTarget::Search,
            FocusTarget::SourceControl,
        ] {
            assert_eq!(
                active_layers(Context::focus(target)).last(),
                Some(&Layer::Global)
            );
        }
    }

    #[test]
    fn modals_suppress_the_focus_layers() {
        // A plain modal is exclusive — the editor/sidebar layers can't leak through.
        assert_eq!(
            active_layers(Context::modal(Modal::Overlay, FocusTarget::Editor)),
            &[Layer::Overlay]
        );
        // The Search modals are the exception: global chords still work.
        assert_eq!(
            active_layers(Context::modal(Modal::SearchList, FocusTarget::Search)),
            &[Layer::SearchList, Layer::Global]
        );
    }
}
