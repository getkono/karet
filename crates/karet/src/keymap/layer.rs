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

/// The single pane that currently holds keyboard focus.
///
/// This is the one value that decides which keybinding layers are live. It is a
/// *derived* view of the stored `(Focus, SidebarPanel, is_diff)` state (see
/// [`FocusTarget::from`]) rather than a second source of truth — the sidebar
/// always has an active panel for rendering independent of who holds focus, so
/// the two stored fields stay orthogonal and this collapses them for dispatch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FocusTarget {
    /// A code editor tab.
    Editor,
    /// A diff editor tab.
    DiffEditor,
    /// The file explorer panel.
    Explorer,
    /// The workspace search panel.
    Search,
    /// The source-control panel.
    SourceControl,
}

impl FocusTarget {
    /// Derive the focused pane from the stored focus, the active sidebar panel,
    /// and whether the active editor tab is a diff.
    #[must_use]
    pub fn from(focus: Focus, panel: SidebarPanel, is_diff: bool) -> Self {
        match focus {
            Focus::Editor if is_diff => FocusTarget::DiffEditor,
            Focus::Editor => FocusTarget::Editor,
            Focus::Sidebar => match panel {
                SidebarPanel::Explorer => FocusTarget::Explorer,
                SidebarPanel::Search => FocusTarget::Search,
                SidebarPanel::SourceControl => FocusTarget::SourceControl,
            },
        }
    }
}

/// A named scope a binding lives in. The [active stack](active_layers) for the
/// focused pane decides which layers are live and in what precedence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layer {
    /// Active regardless of focus (lowest precedence, consulted last).
    Global,
    /// Active when any sidebar panel has focus.
    Sidebar,
    /// Active when the Source-Control panel has focus.
    SourceControl,
    /// Active when a code or diff editor tab has focus.
    Editor,
    /// Active when a diff editor tab has focus.
    DiffEditor,
}

/// The ordered layer stack for `target`, most-specific first — so a pane-specific
/// binding shadows a generic one and [`Layer::Global`] is consulted last. The
/// resolver walks this stack and returns the first matching binding.
#[must_use]
pub fn active_layers(target: FocusTarget) -> &'static [Layer] {
    use Layer as L;
    match target {
        FocusTarget::Editor => &[L::Editor, L::Global],
        FocusTarget::DiffEditor => &[L::DiffEditor, L::Editor, L::Global],
        FocusTarget::Explorer | FocusTarget::Search => &[L::Sidebar, L::Global],
        FocusTarget::SourceControl => &[L::SourceControl, L::Sidebar, L::Global],
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
            active_layers(FocusTarget::SourceControl),
            &[Layer::SourceControl, Layer::Sidebar, Layer::Global]
        );
    }

    #[test]
    fn diff_editor_falls_through_to_editor() {
        assert_eq!(
            active_layers(FocusTarget::DiffEditor),
            &[Layer::DiffEditor, Layer::Editor, Layer::Global]
        );
        assert_eq!(
            active_layers(FocusTarget::Editor),
            &[Layer::Editor, Layer::Global]
        );
    }

    #[test]
    fn global_is_always_last() {
        for target in [
            FocusTarget::Editor,
            FocusTarget::DiffEditor,
            FocusTarget::Explorer,
            FocusTarget::Search,
            FocusTarget::SourceControl,
        ] {
            assert_eq!(active_layers(target).last(), Some(&Layer::Global));
        }
    }
}
