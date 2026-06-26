//! The keymap: a single [`BINDINGS`] table is the source of truth for both the
//! resolver ([`resolve`]) and the palette's shortcut hints ([`hint_for`]), so a
//! binding and its displayed hint can never drift.
//!
//! Each binding is gated by a [`When`] context (the current [`Focus`], and whether
//! the active tab is a diff). Overlays consume their own keys, so the resolver is
//! only consulted when no overlay is open.
//!
//! Matching mirrors how terminals encode keys under the kitty protocol: for a
//! letter with `Ctrl`/`Alt` the case is irrelevant and `Shift` is a separate flag
//! (so `Ctrl+Shift+P` is distinguishable from `Ctrl+P`); for a bare letter the
//! `Shift` is folded into the character case (`g` vs `G`).

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::command::Command;

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

/// The context in which a binding is active.
#[derive(Clone, Copy, PartialEq, Eq)]
enum When {
    /// Active regardless of focus.
    Global,
    /// Active when the sidebar has focus.
    Sidebar,
    /// Active when the editor has focus.
    Editor,
    /// Active when the editor has focus and the active tab is a diff.
    DiffEditor,
}

/// One key binding: a chord (key code + modifier flags) bound to a [`Command`] in
/// a [`When`] context.
struct Binding {
    when: When,
    ctrl: bool,
    shift: bool,
    alt: bool,
    code: KeyCode,
    command: Command,
}

/// A terse constructor for a [`Binding`].
const fn b(
    when: When,
    ctrl: bool,
    shift: bool,
    alt: bool,
    code: KeyCode,
    command: Command,
) -> Binding {
    Binding {
        when,
        ctrl,
        shift,
        alt,
        code,
        command,
    }
}

use KeyCode::{Char, Down, End, Enter, Esc, Home, Left, PageDown, PageUp, Right, Tab, Up};
use When::{DiffEditor, Editor, Global, Sidebar};

/// The single source of truth for key bindings. Order matters: the first matching
/// binding wins, and [`hint_for`] returns the first binding for a command (so list
/// the preferred chord first).
#[rustfmt::skip]
static BINDINGS: &[Binding] = &[
    // Global (any focus).
    b(Global, true,  false, false, Char('q'), Command::Quit),
    b(Global, true,  false, false, Char('c'), Command::Copy),
    b(Global, true,  false, false, Char('p'), Command::OpenQuickOpen),
    b(Global, true,  true,  false, Char('p'), Command::OpenCommandPalette),
    b(Global, true,  false, false, Char('f'), Command::OpenFind),
    b(Global, true,  true,  false, Char('f'), Command::OpenGlobalSearch),
    b(Global, true,  false, false, Char('b'), Command::ToggleSidebar),
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

    // Sidebar focus.
    b(Sidebar, false, false, false, Char('q'), Command::Quit),
    b(Sidebar, false, false, false, Esc,       Command::Quit),
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

    // Editor focus. Arrows move the caret (VS Code); j/k/space/b scroll (vim).
    b(Editor, false, false, false, Char('q'), Command::Quit),
    b(Editor, false, false, false, Esc,       Command::ToggleFocus),
    b(Editor, false, false, false, Down,      Command::CaretDown),
    b(Editor, false, false, false, Up,        Command::CaretUp),
    b(Editor, false, false, false, Left,      Command::CaretLeft),
    b(Editor, false, false, false, Right,     Command::CaretRight),
    b(Editor, false, true,  false, Down,      Command::SelectDown),
    b(Editor, false, true,  false, Up,        Command::SelectUp),
    b(Editor, false, true,  false, Left,      Command::SelectLeft),
    b(Editor, false, true,  false, Right,     Command::SelectRight),
    b(Editor, false, false, false, Char('j'), Command::ScrollDown),
    b(Editor, false, false, false, Char('k'), Command::ScrollUp),
    b(Editor, false, false, false, Char(' '), Command::PageDown),
    b(Editor, false, false, false, PageDown,  Command::PageDown),
    b(Editor, false, false, false, Char('b'), Command::PageUp),
    b(Editor, false, false, false, PageUp,    Command::PageUp),
    b(Editor, false, false, false, Char('y'), Command::Copy),
    b(Editor, false, false, false, Char('g'), Command::Top),
    b(Editor, false, false, false, Home,      Command::Top),
    b(Editor, false, true,  false, Char('G'), Command::Bottom),
    b(Editor, false, false, false, End,       Command::Bottom),

    // Editor focus, diff tab only.
    b(DiffEditor, false, false, false, Char('\\'), Command::ToggleDiffLayout),
    b(DiffEditor, false, false, false, Char(']'),  Command::NextChangedFile),
    b(DiffEditor, false, false, false, Char('['),  Command::PrevChangedFile),
];

/// Whether `when` is active for the given focus and diff state.
fn when_active(when: When, focus: Focus, is_diff: bool) -> bool {
    match when {
        Global => true,
        Sidebar => focus == Focus::Sidebar,
        Editor => focus == Focus::Editor,
        DiffEditor => focus == Focus::Editor && is_diff,
    }
}

/// Whether `key` matches binding `bind`.
fn chord_matches(bind: &Binding, key: KeyEvent) -> bool {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    // Some terminals report Shift+Tab as `BackTab` with the Shift folded into the
    // code; normalize it back to `Tab` + Shift so bindings can stay uniform.
    let (code, shift) = match key.code {
        KeyCode::BackTab => (KeyCode::Tab, true),
        other => (other, key.modifiers.contains(KeyModifiers::SHIFT)),
    };

    match (bind.code, code) {
        (KeyCode::Char(bc), KeyCode::Char(kc)) if bind.ctrl || bind.alt => {
            // With Ctrl/Alt the case is irrelevant; Shift is a distinct flag.
            bind.ctrl == ctrl
                && bind.alt == alt
                && bind.shift == shift
                && bc.eq_ignore_ascii_case(&kc)
        }
        (KeyCode::Char(bc), KeyCode::Char(kc)) => {
            // A bare letter: case carries Shift, so compare exactly and ignore the
            // Shift flag (but still reject Ctrl/Alt chords).
            !ctrl && !alt && bc == kc
        }
        (bcode, kcode) => {
            bcode == kcode && bind.ctrl == ctrl && bind.alt == alt && bind.shift == shift
        }
    }
}

/// Resolve a key press into a [`Command`], given the focus and whether the active
/// tab is a diff. Returns `None` for keys with no binding.
#[must_use]
pub fn resolve(focus: Focus, is_diff: bool, key: KeyEvent) -> Option<Command> {
    BINDINGS
        .iter()
        .find(|bind| when_active(bind.when, focus, is_diff) && chord_matches(bind, key))
        .map(|bind| bind.command)
}

/// Resolve only the global bindings (used by panels that capture text input, like
/// the Search panel, which still want Ctrl-chords and Tab to work).
#[must_use]
pub fn global(key: KeyEvent) -> Option<Command> {
    BINDINGS
        .iter()
        .find(|bind| bind.when == Global && chord_matches(bind, key))
        .map(|bind| bind.command)
}

/// The display hint (e.g. `"Ctrl+W"`) for `command`'s first binding, if any.
#[must_use]
pub fn hint_for(command: Command) -> Option<String> {
    BINDINGS
        .iter()
        .find(|bind| bind.command == command)
        .map(format_chord)
}

/// Format a binding as a human-readable chord like `"Ctrl+Shift+P"`.
fn format_chord(bind: &Binding) -> String {
    let mut s = String::new();
    if bind.ctrl {
        s.push_str("Ctrl+");
    }
    if bind.alt {
        s.push_str("Alt+");
    }
    if bind.shift {
        s.push_str("Shift+");
    }
    s.push_str(&format_code(bind.code));
    s
}

/// Format a single key code for display.
fn format_code(code: KeyCode) -> String {
    match code {
        KeyCode::Char(' ') => "Space".to_string(),
        KeyCode::Char(c) => c.to_ascii_uppercase().to_string(),
        KeyCode::Tab => "Tab".to_string(),
        KeyCode::Enter => "Enter".to_string(),
        KeyCode::Esc => "Esc".to_string(),
        KeyCode::Up => "Up".to_string(),
        KeyCode::Down => "Down".to_string(),
        KeyCode::Left => "Left".to_string(),
        KeyCode::Right => "Right".to_string(),
        KeyCode::Home => "Home".to_string(),
        KeyCode::End => "End".to_string(),
        KeyCode::PageUp => "PgUp".to_string(),
        KeyCode::PageDown => "PgDn".to_string(),
        other => format!("{other:?}"),
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
            Some(Command::OpenQuickOpen)
        );
        assert_eq!(
            resolve(
                Focus::Sidebar,
                false,
                key(
                    KeyCode::Char('P'),
                    KeyModifiers::CONTROL | KeyModifiers::SHIFT
                )
            ),
            Some(Command::OpenCommandPalette)
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
            Some(Command::ScrollDown)
        );
        assert_eq!(
            resolve(
                Focus::Sidebar,
                false,
                key(KeyCode::Char('j'), KeyModifiers::NONE)
            ),
            Some(Command::SidebarDown)
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
            Some(Command::ToggleDiffLayout)
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
    fn top_and_bottom_distinguish_case() {
        assert_eq!(
            resolve(
                Focus::Editor,
                false,
                key(KeyCode::Char('g'), KeyModifiers::NONE)
            ),
            Some(Command::Top)
        );
        assert_eq!(
            resolve(
                Focus::Editor,
                false,
                key(KeyCode::Char('G'), KeyModifiers::SHIFT)
            ),
            Some(Command::Bottom)
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
            Some(Command::SelectPanel(SidebarPanel::Search))
        );
        assert_eq!(
            resolve(
                Focus::Editor,
                false,
                key(KeyCode::Char('q'), KeyModifiers::CONTROL)
            ),
            Some(Command::Quit)
        );
    }

    #[test]
    fn global_only_ignores_context_keys() {
        // A bare 'j' is not global, but Ctrl+B is.
        assert_eq!(global(key(KeyCode::Char('j'), KeyModifiers::NONE)), None);
        assert_eq!(
            global(key(KeyCode::Char('b'), KeyModifiers::CONTROL)),
            Some(Command::ToggleSidebar)
        );
    }

    #[test]
    fn ctrl_c_copies_and_y_copies_in_editor() {
        assert_eq!(
            resolve(
                Focus::Editor,
                false,
                key(KeyCode::Char('c'), KeyModifiers::CONTROL)
            ),
            Some(Command::Copy)
        );
        assert_eq!(
            resolve(
                Focus::Editor,
                false,
                key(KeyCode::Char('y'), KeyModifiers::NONE)
            ),
            Some(Command::Copy)
        );
        // Quit is Ctrl+Q only now.
        assert_eq!(
            resolve(
                Focus::Editor,
                false,
                key(KeyCode::Char('q'), KeyModifiers::CONTROL)
            ),
            Some(Command::Quit)
        );
    }

    #[test]
    fn hint_for_formats_chords() {
        assert_eq!(hint_for(Command::CloseTab).as_deref(), Some("Ctrl+W"));
        assert_eq!(
            hint_for(Command::OpenCommandPalette).as_deref(),
            Some("Ctrl+Shift+P")
        );
        assert_eq!(hint_for(Command::ToggleFocus).as_deref(), Some("Tab"));
    }
}
