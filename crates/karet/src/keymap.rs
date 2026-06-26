//! The pragmatic keymap: a pure function from key events to [`Action`]s, keyed by
//! the current [`Focus`] (and whether the active tab is a diff). Overlays consume
//! their own keys, so the resolver is only consulted when no overlay is open.
//!
//! This is intentionally a direct match rather than the general `input::Keymap`
//! chord engine; it is a clean later migration target.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

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

/// A high-level editor action produced by the keymap and applied by the app.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
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
}

/// Resolve a key press into an [`Action`], given the focus and whether the active
/// tab is a diff. Returns `None` for keys with no binding.
#[must_use]
pub fn resolve(focus: Focus, active_is_diff: bool, key: KeyEvent) -> Option<Action> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);

    // Global bindings (any focus).
    if let Some(action) = resolve_global(ctrl, shift, key.code) {
        return Some(action);
    }
    match focus {
        Focus::Sidebar => resolve_sidebar(key.code),
        Focus::Editor => resolve_editor(active_is_diff, key.code),
    }
}

/// Global bindings available regardless of focus.
fn resolve_global(ctrl: bool, shift: bool, code: KeyCode) -> Option<Action> {
    match code {
        KeyCode::Char('c' | 'q') if ctrl => Some(Action::Quit),
        KeyCode::Char(c) if ctrl && c.eq_ignore_ascii_case(&'p') => Some(if shift {
            Action::OpenCommandPalette
        } else {
            Action::OpenQuickOpen
        }),
        KeyCode::Char(c) if ctrl && c.eq_ignore_ascii_case(&'f') => Some(if shift {
            Action::OpenGlobalSearch
        } else {
            Action::OpenFind
        }),
        KeyCode::Char('b') if ctrl => Some(Action::ToggleSidebar),
        KeyCode::Char('w') if ctrl => Some(Action::CloseTab),
        KeyCode::Char('1') if ctrl => Some(Action::SelectPanel(SidebarPanel::Explorer)),
        KeyCode::Char('2') if ctrl => Some(Action::SelectPanel(SidebarPanel::Search)),
        KeyCode::Char('3') if ctrl => Some(Action::SelectPanel(SidebarPanel::SourceControl)),
        KeyCode::Tab => Some(Action::ToggleFocus),
        _ => None,
    }
}

/// Bindings active when the sidebar has focus.
fn resolve_sidebar(code: KeyCode) -> Option<Action> {
    match code {
        KeyCode::Char('q') => Some(Action::Quit),
        KeyCode::Esc => Some(Action::Quit),
        KeyCode::Char('j') | KeyCode::Down => Some(Action::SidebarDown),
        KeyCode::Char('k') | KeyCode::Up => Some(Action::SidebarUp),
        KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right => Some(Action::SidebarActivate),
        KeyCode::Char('h') | KeyCode::Left => Some(Action::SidebarCollapse),
        KeyCode::Char(' ') => Some(Action::SidebarToggleExpand),
        _ => None,
    }
}

/// Bindings active when the editor has focus.
fn resolve_editor(active_is_diff: bool, code: KeyCode) -> Option<Action> {
    match code {
        KeyCode::Char('q') => Some(Action::Quit),
        KeyCode::Esc => Some(Action::ToggleFocus),
        KeyCode::Char('j') | KeyCode::Down => Some(Action::ScrollDown),
        KeyCode::Char('k') | KeyCode::Up => Some(Action::ScrollUp),
        KeyCode::Char(' ') | KeyCode::PageDown => Some(Action::PageDown),
        KeyCode::Char('b') | KeyCode::PageUp => Some(Action::PageUp),
        KeyCode::Char('g') | KeyCode::Home => Some(Action::Top),
        KeyCode::Char('G') | KeyCode::End => Some(Action::Bottom),
        KeyCode::Char('\\') if active_is_diff => Some(Action::ToggleDiffLayout),
        KeyCode::Char(']') if active_is_diff => Some(Action::NextChangedFile),
        KeyCode::Char('[') if active_is_diff => Some(Action::PrevChangedFile),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
    }

    #[test]
    fn ctrl_p_variants() {
        assert_eq!(
            resolve(
                Focus::Sidebar,
                false,
                key(KeyCode::Char('p'), KeyModifiers::CONTROL)
            ),
            Some(Action::OpenQuickOpen)
        );
        assert_eq!(
            resolve(
                Focus::Sidebar,
                false,
                key(
                    KeyCode::Char('p'),
                    KeyModifiers::CONTROL | KeyModifiers::SHIFT
                )
            ),
            Some(Action::OpenCommandPalette)
        );
    }

    #[test]
    fn focus_routes_navigation() {
        // 'j' scrolls in the editor but moves the selection in the sidebar.
        assert_eq!(
            resolve(
                Focus::Editor,
                false,
                key(KeyCode::Char('j'), KeyModifiers::NONE)
            ),
            Some(Action::ScrollDown)
        );
        assert_eq!(
            resolve(
                Focus::Sidebar,
                false,
                key(KeyCode::Char('j'), KeyModifiers::NONE)
            ),
            Some(Action::SidebarDown)
        );
    }

    #[test]
    fn diff_only_bindings() {
        // Backslash toggles layout only on a diff tab.
        assert_eq!(
            resolve(
                Focus::Editor,
                true,
                key(KeyCode::Char('\\'), KeyModifiers::NONE)
            ),
            Some(Action::ToggleDiffLayout)
        );
        assert_eq!(
            resolve(
                Focus::Editor,
                false,
                key(KeyCode::Char('\\'), KeyModifiers::NONE)
            ),
            None
        );
    }

    #[test]
    fn panel_selection_and_quit() {
        assert_eq!(
            resolve(
                Focus::Sidebar,
                false,
                key(KeyCode::Char('2'), KeyModifiers::CONTROL)
            ),
            Some(Action::SelectPanel(SidebarPanel::Search))
        );
        assert_eq!(
            resolve(
                Focus::Editor,
                false,
                key(KeyCode::Char('q'), KeyModifiers::CONTROL)
            ),
            Some(Action::Quit)
        );
    }
}
