//! The keymap: a single [`BINDINGS`] table is the source of truth for both the
//! resolver ([`resolve`]) and the palette's shortcut hints ([`hint_for`]), so a
//! binding and its displayed hint can never drift.
//!
//! Each binding lives in a [`Layer`]; the [`active_layers`] stack for the focused
//! pane decides which layers are live and in what precedence. A binding's trigger
//! is a *sequence* of one or more [`KeyChord`]s, so a multi-key chord like
//! `Ctrl+K Ctrl+W` resolves the same way a single chord does — the resolver reports
//! [`Resolved::Pending`] while such a sequence is still being typed. Chord matching
//! (kitty-protocol case/shift rules) lives in the [`chord`] submodule.
//!
//! Extending the keymap is additive (these are the seams the retired generic
//! `input` scaffolding sketched, now grounded in the live table): multi-key
//! sequences already resolve; user rebinding would parse a config file into the same
//! [`Binding`] shape and layer it over the defaults; and a modal-editing mode (vi
//! Normal/Insert) would just be another [`Layer`] in the [`active_layers`] stack —
//! no separate engine required.

mod chord;
mod layer;

pub use chord::ChordStyle;
pub use chord::KeyChord;
use chord::chord;
use crossterm::event::KeyCode;
pub use layer::Context;
pub use layer::Focus;
pub use layer::FocusTarget;
use layer::Layer;
pub use layer::Modal;
pub use layer::SidebarPanel;
use layer::active_layers;

use crate::command::Command;

/// One key binding: a [`KeyChord`] *sequence* — `chord` then `rest` — bound to a
/// [`Command`] in a [`Layer`]. `rest` is empty for the common single-chord binding.
struct Binding {
    layer: Layer,
    chord: KeyChord,
    rest: &'static [KeyChord],
    command: Command,
}

/// A terse constructor for a single-chord [`Binding`]. Chords are authored in
/// canonical form (see [`chord`]): a `Ctrl`/`Alt` letter lower-cased, a bare letter
/// with `shift = false`.
const fn b(
    layer: Layer,
    ctrl: bool,
    shift: bool,
    alt: bool,
    code: KeyCode,
    command: Command,
) -> Binding {
    Binding {
        layer,
        chord: chord(ctrl, shift, alt, code),
        rest: &[],
        command,
    }
}

/// A constructor for a multi-chord [`Binding`]: `first` followed by `rest` (e.g.
/// `Ctrl+K` then `Ctrl+W`). No terminal binding may also be a prefix of a longer
/// one (enforced by a test), so resolution stays deterministic without timers.
const fn seq(
    layer: Layer,
    first: KeyChord,
    rest: &'static [KeyChord],
    command: Command,
) -> Binding {
    Binding {
        layer,
        chord: first,
        rest,
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
use Layer::CommitInput;
use Layer::DiffEditor;
use Layer::DiscardConfirm;
use Layer::Editor;
use Layer::Find;
use Layer::Global;
use Layer::Overlay;
use Layer::SearchInput;
use Layer::SearchList;
use Layer::Sidebar;
use Layer::SourceControl;

/// The single source of truth for key bindings. Within a [`Layer`] the first
/// matching binding wins (and [`hint_for`] returns the first binding for a command,
/// so list the preferred chord first); precedence *across* layers is decided by
/// [`active_layers`], not by table order.
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

    // Multi-key tab-management chords: a `Ctrl+K` prefix, then the action key.
    seq(Global, chord(true, false, false, Char('k')), &[chord(true,  false, false, Char('w'))], Command::CloseAllTabs),
    seq(Global, chord(true, false, false, Char('k')), &[chord(false, false, false, Char('w'))], Command::CloseOtherTabs),

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

    // Modal contexts. Each is exclusive (see `active_layers`); any key with no
    // binding here falls through to the modal's text input.
    // Quick-open / command palette.
    b(Overlay, false, false, false, Esc,       Command::OverlayCancel),
    b(Overlay, false, false, false, Enter,     Command::OverlayAccept),
    b(Overlay, false, false, false, Up,        Command::OverlayUp),
    b(Overlay, true,  false, false, Char('p'), Command::OverlayUp),
    b(Overlay, false, false, false, Down,      Command::OverlayDown),
    b(Overlay, true,  false, false, Char('n'), Command::OverlayDown),
    // In-file find bar.
    b(Find, false, false, false, Esc,       Command::FindCancel),
    b(Find, false, false, false, Enter,     Command::FindNext),
    b(Find, false, false, false, Down,      Command::FindNext),
    b(Find, false, false, false, Up,        Command::FindPrev),
    b(Find, true,  false, false, Char('g'), Command::FindNext),
    b(Find, true,  true,  false, Char('g'), Command::FindPrev),
    // Commit-message input.
    b(CommitInput, false, false, false, Esc,   Command::CommitCancel),
    b(CommitInput, false, false, false, Enter, Command::CommitSubmit),
    // Discard confirmation: only the confirm keys are bound; anything else cancels.
    b(DiscardConfirm, false, false, false, Enter,     Command::ConfirmDiscard),
    b(DiscardConfirm, false, false, false, Char('y'), Command::ConfirmDiscard),
    b(DiscardConfirm, false, false, false, Char('Y'), Command::ConfirmDiscard),
    // Workspace Search: navigating the results list.
    b(SearchList, false, false, false, Esc,       Command::SearchQuit),
    b(SearchList, false, false, false, Enter,     Command::SearchOpen),
    b(SearchList, false, false, false, Down,      Command::SearchSelectDown),
    b(SearchList, false, false, false, Char('j'), Command::SearchSelectDown),
    b(SearchList, false, false, false, Up,        Command::SearchSelectUp),
    b(SearchList, false, false, false, Char('k'), Command::SearchSelectUp),
    b(SearchList, false, false, false, Char('/'), Command::SearchBeginInput),
    // Workspace Search: editing the query.
    b(SearchInput, false, false, false, Esc,   Command::SearchEndInput),
    b(SearchInput, false, false, false, Enter, Command::SearchRun),
];

/// Resolve a key press into a [`Command`], given the focus, the active sidebar
/// panel, and whether the active tab is a diff. Returns `None` for keys with no
/// binding.
#[must_use]
pub fn resolve(ctx: Context, pending: &[KeyChord]) -> Resolved {
    resolve_in(active_layers(ctx), pending)
}

/// The outcome of resolving a (possibly partial) chord sequence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resolved {
    /// The sequence is a complete binding for this command.
    Command(Command),
    /// The sequence is a prefix of at least one longer binding; await more keys.
    Pending,
    /// The sequence matches no binding.
    None,
}

/// Walk `layers` most-specific-first: a complete match in any layer wins; failing
/// that, report [`Resolved::Pending`] if `pending` is a prefix of some binding.
fn resolve_in(layers: &[Layer], pending: &[KeyChord]) -> Resolved {
    let mut prefix = false;
    for &layer in layers {
        for bind in BINDINGS.iter().filter(|bind| bind.layer == layer) {
            match bind.match_seq(pending) {
                SeqMatch::Full => return Resolved::Command(bind.command),
                SeqMatch::Prefix => prefix = true,
                SeqMatch::No => {},
            }
        }
    }
    if prefix {
        Resolved::Pending
    } else {
        Resolved::None
    }
}

/// The display hint (e.g. `"Ctrl+W"`, `"Ctrl+K Ctrl+W"`) for `command`'s first
/// binding, rendered in `style`, if the command is bound.
#[must_use]
pub fn hint_for(command: Command, style: ChordStyle) -> Option<String> {
    BINDINGS
        .iter()
        .find(|bind| bind.command == command)
        .map(|bind| bind.hint(style))
}

/// How a pending chord sequence relates to a binding's trigger sequence.
enum SeqMatch {
    /// `pending` equals the full trigger sequence.
    Full,
    /// `pending` is a strict prefix of the trigger sequence.
    Prefix,
    /// `pending` diverges from the trigger sequence.
    No,
}

impl Binding {
    /// The `i`-th chord of this binding's trigger sequence.
    fn chord_at(&self, i: usize) -> KeyChord {
        if i == 0 { self.chord } else { self.rest[i - 1] }
    }

    /// The number of chords in this binding's trigger sequence.
    fn seq_len(&self) -> usize {
        1 + self.rest.len()
    }

    /// How `pending` relates to this binding's trigger sequence.
    fn match_seq(&self, pending: &[KeyChord]) -> SeqMatch {
        if pending.is_empty() || pending.len() > self.seq_len() {
            return SeqMatch::No;
        }
        for (i, &pc) in pending.iter().enumerate() {
            if self.chord_at(i).canonical() != pc {
                return SeqMatch::No;
            }
        }
        if pending.len() == self.seq_len() {
            SeqMatch::Full
        } else {
            SeqMatch::Prefix
        }
    }

    /// This binding's trigger rendered as a hint (chords space-separated).
    fn hint(&self, style: ChordStyle) -> String {
        let mut s = self.chord.display(style);
        for c in self.rest {
            s.push(' ');
            s.push_str(&c.display(style));
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::KeyEvent;
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

    /// Resolve a single key press in a focus context to its command.
    fn res_in(focus: Focus, panel: SidebarPanel, is_diff: bool, key: KeyEvent) -> Option<Command> {
        let ctx = Context::focus(FocusTarget::from(focus, panel, is_diff));
        match resolve(ctx, &[KeyChord::from_event(key)]) {
            Resolved::Command(c) => Some(c),
            _ => None,
        }
    }

    /// Resolve with the Explorer panel active (the default for non-SCM tests).
    fn res(focus: Focus, is_diff: bool, key: KeyEvent) -> Option<Command> {
        res_in(focus, SidebarPanel::Explorer, is_diff, key)
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
            res_in(
                Focus::Sidebar,
                SidebarPanel::SourceControl,
                false,
                key(KeyCode::Char('s'), KeyModifiers::NONE)
            ),
            Some(Command::ScmStage)
        );
        assert_eq!(
            res_in(
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
            res_in(
                Focus::Sidebar,
                SidebarPanel::SourceControl,
                false,
                key(KeyCode::Down, KeyModifiers::SHIFT)
            ),
            Some(Command::SelectExtendDown)
        );
        // `x` toggles the cursor row into the selection in both list panels.
        assert_eq!(
            res_in(
                Focus::Sidebar,
                SidebarPanel::SourceControl,
                false,
                key(KeyCode::Char('x'), KeyModifiers::NONE)
            ),
            Some(Command::SelectToggle)
        );
        assert_eq!(
            res_in(
                Focus::Sidebar,
                SidebarPanel::Explorer,
                false,
                key(KeyCode::Down, KeyModifiers::SHIFT)
            ),
            Some(Command::SelectExtendDown)
        );
        // The same 's' in the Explorer panel is not an SCM command.
        assert_eq!(
            res_in(
                Focus::Sidebar,
                SidebarPanel::Explorer,
                false,
                key(KeyCode::Char('s'), KeyModifiers::NONE)
            ),
            None
        );
        // Space in the Explorer toggles expansion, not staging.
        assert_eq!(
            res_in(
                Focus::Sidebar,
                SidebarPanel::Explorer,
                false,
                key(KeyCode::Char(' '), KeyModifiers::NONE)
            ),
            Some(Command::SidebarToggleExpand)
        );
    }

    #[test]
    fn search_modal_still_resolves_global_chords() {
        // The Search modals layer their own keys over Global, so Ctrl+B still toggles
        // the sidebar while a bare 'j' navigates the results rather than typing.
        let list = Context::modal(Modal::SearchList, FocusTarget::Search);
        assert_eq!(
            resolve(
                list,
                &[KeyChord::from_event(key(
                    KeyCode::Char('b'),
                    KeyModifiers::CONTROL
                ))]
            ),
            Resolved::Command(Command::ToggleSidebar)
        );
        // A plain overlay is exclusive: Ctrl+B does not leak through to Global.
        let overlay = Context::modal(Modal::Overlay, FocusTarget::Editor);
        assert_eq!(
            resolve(
                overlay,
                &[KeyChord::from_event(key(
                    KeyCode::Char('b'),
                    KeyModifiers::CONTROL
                ))]
            ),
            Resolved::None
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
        assert_eq!(
            hint_for(Command::CloseTab, ChordStyle::Verbose).as_deref(),
            Some("Ctrl+W")
        );
        assert_eq!(
            hint_for(Command::OpenCommandPalette, ChordStyle::Verbose).as_deref(),
            Some("Ctrl+Shift+P")
        );
        assert_eq!(
            hint_for(Command::ToggleFocus, ChordStyle::Verbose).as_deref(),
            Some("Tab")
        );
        // The same binding renders compactly for the status bar.
        assert_eq!(
            hint_for(Command::CloseTab, ChordStyle::Caret).as_deref(),
            Some("^W")
        );
    }

    #[test]
    fn hint_renders_multi_key_sequence() {
        assert_eq!(
            hint_for(Command::CloseAllTabs, ChordStyle::Verbose).as_deref(),
            Some("Ctrl+K Ctrl+W")
        );
    }

    #[test]
    fn chord_sequences_resolve_through_a_pending_prefix() {
        let ctrl_k = KeyChord::from_event(key(KeyCode::Char('k'), KeyModifiers::CONTROL));
        let ctrl_w = KeyChord::from_event(key(KeyCode::Char('w'), KeyModifiers::CONTROL));
        let w = KeyChord::from_event(key(KeyCode::Char('w'), KeyModifiers::NONE));
        let esc = KeyChord::from_event(key(KeyCode::Esc, KeyModifiers::NONE));
        let ed = Context::focus(FocusTarget::Editor);
        // Ctrl+K alone is a prefix of a longer binding — the resolver waits.
        assert_eq!(resolve(ed, &[ctrl_k]), Resolved::Pending);
        // Completing the sequence fires the command; the second chord disambiguates.
        assert_eq!(
            resolve(ed, &[ctrl_k, ctrl_w]),
            Resolved::Command(Command::CloseAllTabs)
        );
        assert_eq!(
            resolve(ed, &[ctrl_k, w]),
            Resolved::Command(Command::CloseOtherTabs)
        );
        // A key that continues nothing breaks the sequence.
        assert_eq!(resolve(ed, &[ctrl_k, esc]), Resolved::None);
    }

    #[test]
    fn no_terminal_binding_is_a_prefix_of_another() {
        // The resolver commits on the first full match, so a binding must never also
        // be a strict prefix of a longer one (which it would shadow). This invariant
        // is what lets resolution stay deterministic without a timeout.
        for a in BINDINGS {
            for b in BINDINGS {
                if a.seq_len() >= b.seq_len() {
                    continue;
                }
                let a_prefixes_b = (0..a.seq_len()).all(|i| a.chord_at(i) == b.chord_at(i));
                assert!(
                    !a_prefixes_b,
                    "{:?} is a prefix of {:?}",
                    a.command, b.command
                );
            }
        }
    }
}
