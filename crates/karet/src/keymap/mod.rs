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

mod chord;

pub use chord::KeyChord;
use chord::chord;
use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;

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

/// The single pane that currently holds keyboard focus.
///
/// This is the one value that decides which keybinding layer is live. It is a
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

/// The context in which a binding is active.
#[derive(Clone, Copy, PartialEq, Eq)]
enum When {
    /// Active regardless of focus.
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

impl When {
    /// Whether this context is active for the focused pane.
    fn matches(self, target: FocusTarget) -> bool {
        use FocusTarget as T;
        match self {
            When::Global => true,
            When::Sidebar => matches!(target, T::Explorer | T::Search | T::SourceControl),
            When::SourceControl => target == T::SourceControl,
            When::Editor => matches!(target, T::Editor | T::DiffEditor),
            When::DiffEditor => target == T::DiffEditor,
        }
    }
}

/// One key binding: a [`KeyChord`] bound to a [`Command`] in a [`When`] context.
struct Binding {
    when: When,
    chord: KeyChord,
    command: Command,
}

/// A terse constructor for a [`Binding`]. Chords are authored in canonical form
/// (see [`chord`]): a `Ctrl`/`Alt` letter lower-cased, a bare letter with
/// `shift = false`.
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
        chord: chord(ctrl, shift, alt, code),
        command,
    }
}

use KeyCode::Backspace;
use KeyCode::Char;
use KeyCode::Delete;
use KeyCode::Down;
use KeyCode::End;
use KeyCode::Enter;
use KeyCode::Esc;
use KeyCode::Home;
use KeyCode::Left;
use KeyCode::PageDown;
use KeyCode::PageUp;
use KeyCode::Right;
use KeyCode::Tab;
use KeyCode::Up;
use When::DiffEditor;
use When::Editor;
use When::Global;
use When::Sidebar;
use When::SourceControl;

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

    // Sidebar focus. Selection verbs are shared across every list panel (explorer
    // and source control) — they route to the focused panel's selection in dispatch.
    b(Sidebar, false, true,  false, Down,      Command::SelectExtendDown),
    b(Sidebar, false, true,  false, Up,        Command::SelectExtendUp),
    b(Sidebar, false, false, false, Char('x'), Command::SelectToggle),
    b(Sidebar, true,  false, false, Char('a'), Command::SelectAll),
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

    // Editor focus. The editor is non-modal: arrows/Home/End/PageUp-Down navigate,
    // and any unbound printable is text input (the shell inserts it after the keymap
    // declines). Bare-letter motions are intentionally gone so letters can be typed.
    b(Editor, false, false, false, Esc,       Command::ToggleFocus),
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
    b(Editor, false, false, false, Home,      Command::Top),
    b(Editor, false, false, false, End,       Command::Bottom),
    // Editing.
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

    // Editor focus, diff tab only.
    b(DiffEditor, false, false, false, Char('\\'), Command::ToggleDiffLayout),
    b(DiffEditor, false, false, false, Char(']'),  Command::NextChangedFile),
    b(DiffEditor, false, false, false, Char('['),  Command::PrevChangedFile),
];

/// Resolve a key press into a [`Command`], given the focus, the active sidebar
/// panel, and whether the active tab is a diff. Returns `None` for keys with no
/// binding.
#[must_use]
pub fn resolve(focus: Focus, panel: SidebarPanel, is_diff: bool, key: KeyEvent) -> Option<Command> {
    let target = FocusTarget::from(focus, panel, is_diff);
    BINDINGS
        .iter()
        .find(|bind| bind.when.matches(target) && bind.chord.matches(key))
        .map(|bind| bind.command)
}

/// Resolve only the global bindings (used by panels that capture text input, like
/// the Search panel, which still want Ctrl-chords and Tab to work).
#[must_use]
pub fn global(key: KeyEvent) -> Option<Command> {
    BINDINGS
        .iter()
        .find(|bind| bind.when == Global && bind.chord.matches(key))
        .map(|bind| bind.command)
}

/// The display hint (e.g. `"Ctrl+W"`) for `command`'s first binding, if any.
#[must_use]
pub fn hint_for(command: Command) -> Option<String> {
    BINDINGS
        .iter()
        .find(|bind| bind.command == command)
        .map(|bind| format_chord(bind.chord))
}

/// Format a chord as a human-readable label like `"Ctrl+Shift+P"`.
fn format_chord(c: KeyChord) -> String {
    let mut s = String::new();
    if c.mods.ctrl {
        s.push_str("Ctrl+");
    }
    if c.mods.alt {
        s.push_str("Alt+");
    }
    if c.mods.shift {
        s.push_str("Shift+");
    }
    s.push_str(&format_code(c.code));
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
    use crossterm::event::KeyModifiers;

    use super::*;

    fn key(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
    }

    #[test]
    fn bindings_are_authored_in_canonical_form() {
        // Every binding chord must equal its canonical form, so `Binding::chord`
        // compares equal to a `KeyChord::from_event` of the intended key press.
        for bind in BINDINGS {
            assert_eq!(
                bind.chord,
                bind.chord.canonical(),
                "non-canonical chord for {:?}",
                bind.command
            );
        }
    }

    /// Resolve with the Explorer panel active (the default for non-SCM tests).
    fn res(focus: Focus, is_diff: bool, key: KeyEvent) -> Option<Command> {
        resolve(focus, SidebarPanel::Explorer, is_diff, key)
    }

    #[test]
    fn ctrl_p_variants() {
        assert_eq!(
            res(
                Focus::Sidebar,
                false,
                key(KeyCode::Char('p'), KeyModifiers::CONTROL)
            ),
            Some(Command::OpenQuickOpen)
        );
        assert_eq!(
            res(
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
        // A bare 'j' is text in the editor (unbound → typed) but moves the
        // selection in the sidebar.
        assert_eq!(
            res(
                Focus::Editor,
                false,
                key(KeyCode::Char('j'), KeyModifiers::NONE)
            ),
            None
        );
        assert_eq!(
            res(
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
            res(
                Focus::Editor,
                true,
                key(KeyCode::Char('\\'), KeyModifiers::NONE)
            ),
            Some(Command::ToggleDiffLayout)
        );
        assert_eq!(
            res(
                Focus::Editor,
                false,
                key(KeyCode::Char('\\'), KeyModifiers::NONE)
            ),
            None
        );
    }

    #[test]
    fn home_and_end_jump_to_edges() {
        assert_eq!(
            res(Focus::Editor, false, key(KeyCode::Home, KeyModifiers::NONE)),
            Some(Command::Top)
        );
        assert_eq!(
            res(Focus::Editor, false, key(KeyCode::End, KeyModifiers::NONE)),
            Some(Command::Bottom)
        );
    }

    #[test]
    fn panel_selection_and_quit() {
        assert_eq!(
            res(
                Focus::Sidebar,
                false,
                key(KeyCode::Char('2'), KeyModifiers::CONTROL)
            ),
            Some(Command::SelectPanel(SidebarPanel::Search))
        );
        assert_eq!(
            res(
                Focus::Editor,
                false,
                key(KeyCode::Char('q'), KeyModifiers::CONTROL)
            ),
            Some(Command::Quit)
        );
    }

    #[test]
    fn focus_target_derivation() {
        assert_eq!(
            FocusTarget::from(Focus::Sidebar, SidebarPanel::SourceControl, false),
            FocusTarget::SourceControl
        );
        assert_eq!(
            FocusTarget::from(Focus::Sidebar, SidebarPanel::Explorer, false),
            FocusTarget::Explorer
        );
        // Opening a diff moves focus to the editor: the active layer becomes
        // DiffEditor, NOT SourceControl, even while the SCM panel is still the
        // underlying sidebar panel. This is the fact behind the "SCM keys do
        // nothing after previewing a diff" bug.
        assert_eq!(
            FocusTarget::from(Focus::Editor, SidebarPanel::SourceControl, true),
            FocusTarget::DiffEditor
        );
        assert_eq!(
            FocusTarget::from(Focus::Editor, SidebarPanel::Explorer, false),
            FocusTarget::Editor
        );
    }

    #[test]
    fn source_control_bindings_are_panel_scoped() {
        // In the SCM panel, bare 's' stages and Space toggles staging.
        assert_eq!(
            resolve(
                Focus::Sidebar,
                SidebarPanel::SourceControl,
                false,
                key(KeyCode::Char('s'), KeyModifiers::NONE)
            ),
            Some(Command::ScmStage)
        );
        assert_eq!(
            resolve(
                Focus::Sidebar,
                SidebarPanel::SourceControl,
                false,
                key(KeyCode::Char(' '), KeyModifiers::NONE)
            ),
            Some(Command::ScmToggleStage)
        );
        // Shift+Down extends the selection — a shared Sidebar-layer verb that is
        // active in the SCM panel too (it is not shadowed by an SCM binding).
        assert_eq!(
            resolve(
                Focus::Sidebar,
                SidebarPanel::SourceControl,
                false,
                key(KeyCode::Down, KeyModifiers::SHIFT)
            ),
            Some(Command::SelectExtendDown)
        );
        // `x` toggles the cursor row into the selection in both list panels.
        assert_eq!(
            resolve(
                Focus::Sidebar,
                SidebarPanel::SourceControl,
                false,
                key(KeyCode::Char('x'), KeyModifiers::NONE)
            ),
            Some(Command::SelectToggle)
        );
        assert_eq!(
            resolve(
                Focus::Sidebar,
                SidebarPanel::Explorer,
                false,
                key(KeyCode::Down, KeyModifiers::SHIFT)
            ),
            Some(Command::SelectExtendDown)
        );
        // The same 's' in the Explorer panel is not an SCM command.
        assert_eq!(
            resolve(
                Focus::Sidebar,
                SidebarPanel::Explorer,
                false,
                key(KeyCode::Char('s'), KeyModifiers::NONE)
            ),
            None
        );
        // Space in the Explorer toggles expansion, not staging.
        assert_eq!(
            resolve(
                Focus::Sidebar,
                SidebarPanel::Explorer,
                false,
                key(KeyCode::Char(' '), KeyModifiers::NONE)
            ),
            Some(Command::SidebarToggleExpand)
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
    fn editor_editing_chords() {
        assert_eq!(
            res(
                Focus::Editor,
                false,
                key(KeyCode::Char('c'), KeyModifiers::CONTROL)
            ),
            Some(Command::Copy)
        );
        assert_eq!(
            res(
                Focus::Editor,
                false,
                key(KeyCode::Char('s'), KeyModifiers::CONTROL)
            ),
            Some(Command::Save)
        );
        assert_eq!(
            res(
                Focus::Editor,
                false,
                key(KeyCode::Char('z'), KeyModifiers::CONTROL)
            ),
            Some(Command::Undo)
        );
        // A bare letter is no longer a command in the editor (it is typed instead).
        assert_eq!(
            res(
                Focus::Editor,
                false,
                key(KeyCode::Char('y'), KeyModifiers::NONE)
            ),
            None
        );
        // Quit is Ctrl+Q.
        assert_eq!(
            res(
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
