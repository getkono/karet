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
pub use layer::EditorTab;
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
use KeyCode::F;
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
use Layer::Explorer;
use Layer::ExplorerEdit;
use Layer::Find;
use Layer::Global;
use Layer::Overlay;
use Layer::Oversize;
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
    // F1 is a terminal-safe alternate for the command palette: some emulators
    // capture Ctrl+Shift+P before it reaches the app (see the app README).
    b(Global, false, false, false, F(1),      Command::OpenCommandPalette),
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

    // Pane splitting & focus (VS Code parity): `Ctrl+\` splits right, `Ctrl+K Ctrl+\`
    // splits down, and `Ctrl+K` + arrow cycles pane focus.
    b(Global, true, false, false, Char('\\'), Command::SplitRight),
    seq(Global, chord(true, false, false, Char('k')), &[chord(true, false, false, Char('\\'))], Command::SplitDown),
    seq(Global, chord(true, false, false, Char('k')), &[chord(false, false, false, Right)], Command::FocusNextPane),
    seq(Global, chord(true, false, false, Char('k')), &[chord(false, false, false, Left)],  Command::FocusPrevPane),

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

    // Explorer panel (sidebar focus, Explorer active). Listed before the generic
    // sidebar bindings so its keys win. New file/folder, rename, refresh; collapse-all
    // and new file/folder are also on the panel's toolbar buttons and in the palette.
    b(Explorer, false, false, false, Char('a'), Command::ExplorerNewFile),
    b(Explorer, false, false, false, Char('A'), Command::ExplorerNewFolder),
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

    // Code folding (VS Code parity): `Ctrl+K Ctrl+L` toggles the fold at the cursor.
    seq(Editor, chord(true, false, false, Char('k')), &[chord(true, false, false, Char('l'))], Command::ToggleFold),

    // Editor focus, diff tab only.
    b(DiffEditor, false, false, false, Char('\\'), Command::ToggleDiffLayout),
    b(DiffEditor, false, false, false, Char(']'),  Command::NextChangedFile),
    b(DiffEditor, false, false, false, Char('['),  Command::PrevChangedFile),

    // Editor focus, a too-large-file placeholder: bypass the size guard on demand.
    // Enter loads it anyway; Esc returns focus to the sidebar (as it does in the
    // editor, whose layer is not stacked here).
    b(Oversize, false, false, false, Enter, Command::OpenAnyway),
    b(Oversize, false, false, false, Esc,   Command::ToggleFocus),

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

/// One advertisable binding for the status hints bar: its rendered chord sequence,
/// the [`Command`] it fires (kept so a click can dispatch it), and the terse verb
/// to label it with.
pub struct Hint {
    /// The trigger chord(s) rendered in the requested style (e.g. `"^S"`, `"^K ^W"`).
    pub chord: String,
    /// The command this binding fires.
    pub command: Command,
    /// The terse verb shown after the chord (from [`Command::hint_verb`]).
    pub verb: &'static str,
}

/// Every advertisable binding live in `ctx`, ordered most-specific layer first and
/// deduped by command (the first binding wins, matching [`hint_for`]). Only commands
/// with a terse verb ([`Command::hint_verb`]) are included, so self-evident motion
/// and text-editing keys are omitted. This is the forward counterpart to `hint_for`
/// that drives the context-aware status hints bar.
#[must_use]
pub fn hints_for(ctx: Context, style: ChordStyle) -> Vec<Hint> {
    let mut hints = Vec::new();
    let mut seen: Vec<Command> = Vec::new();
    for &layer in active_layers(ctx) {
        for bind in BINDINGS.iter().filter(|bind| bind.layer == layer) {
            let Some(verb) = bind.command.hint_verb() else {
                continue;
            };
            if seen.contains(&bind.command) {
                continue;
            }
            seen.push(bind.command);
            hints.push(Hint {
                chord: bind.hint(style),
                command: bind.command,
                verb,
            });
        }
    }
    hints
}

/// The completions of an in-progress chord: every binding live in `ctx` whose
/// trigger has `pending` as a strict prefix, with only the *remaining* chords
/// rendered (e.g. after `^K`, `^W` for "close all" and `W` for "close others").
/// Deduped by command, most-specific layer first. Powers the pending-chord hint bar.
#[must_use]
pub fn completions_for(ctx: Context, pending: &[KeyChord], style: ChordStyle) -> Vec<Hint> {
    let mut hints = Vec::new();
    let mut seen: Vec<Command> = Vec::new();
    for &layer in active_layers(ctx) {
        for bind in BINDINGS.iter().filter(|bind| bind.layer == layer) {
            if !matches!(bind.match_seq(pending), SeqMatch::Prefix) {
                continue;
            }
            if seen.contains(&bind.command) {
                continue;
            }
            seen.push(bind.command);
            hints.push(Hint {
                chord: bind.hint_from(style, pending.len()),
                command: bind.command,
                verb: bind.command.hint_verb().unwrap_or(""),
            });
        }
    }
    hints
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
        self.hint_from(style, 0)
    }

    /// This binding's trigger rendered as a hint, skipping the first `skip` chords —
    /// so an already-typed prefix isn't repeated in a completion hint. `skip == 0`
    /// is equivalent to [`Binding::hint`].
    fn hint_from(&self, style: ChordStyle, skip: usize) -> String {
        let mut s = String::new();
        for i in skip..self.seq_len() {
            if !s.is_empty() {
                s.push(' ');
            }
            s.push_str(&self.chord_at(i).display(style));
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

    /// Resolve a single key press in a focus context to its command. `is_diff` maps
    /// to the diff editor tab; every other editor tab is treated as plain here.
    fn res_in(focus: Focus, panel: SidebarPanel, is_diff: bool, key: KeyEvent) -> Option<Command> {
        let tab = if is_diff {
            EditorTab::Diff
        } else {
            EditorTab::Plain
        };
        let ctx = Context::focus(FocusTarget::from(focus, panel, tab));
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
        // F1 is a terminal-safe alternate for the command palette (some emulators
        // capture Ctrl+Shift+P). It works regardless of focus.
        assert_eq!(
            res(Focus::Editor, false, key(KeyCode::F(1), KeyModifiers::NONE)),
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
    fn home_end_move_to_line_edges_ctrl_to_document() {
        assert_eq!(
            res(Focus::Editor, false, key(KeyCode::Home, KeyModifiers::NONE)),
            Some(Command::CaretLineStart)
        );
        assert_eq!(
            res(Focus::Editor, false, key(KeyCode::End, KeyModifiers::NONE)),
            Some(Command::CaretLineEnd)
        );
        assert_eq!(
            res(
                Focus::Editor,
                false,
                key(KeyCode::Home, KeyModifiers::CONTROL)
            ),
            Some(Command::CaretDocStart)
        );
        assert_eq!(
            res(
                Focus::Editor,
                false,
                key(KeyCode::End, KeyModifiers::CONTROL)
            ),
            Some(Command::CaretDocEnd)
        );
    }

    #[test]
    fn word_motion_and_select_all_bind_in_editor() {
        assert_eq!(
            res(
                Focus::Editor,
                false,
                key(KeyCode::Left, KeyModifiers::CONTROL)
            ),
            Some(Command::CaretWordLeft)
        );
        assert_eq!(
            res(
                Focus::Editor,
                false,
                key(KeyCode::Right, KeyModifiers::CONTROL | KeyModifiers::SHIFT)
            ),
            Some(Command::SelectWordRight)
        );
        assert_eq!(
            res(
                Focus::Editor,
                false,
                key(KeyCode::Char('a'), KeyModifiers::CONTROL)
            ),
            Some(Command::EditorSelectAll)
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
            FocusTarget::from(
                Focus::Sidebar,
                SidebarPanel::SourceControl,
                EditorTab::Plain
            ),
            FocusTarget::SourceControl
        );
        assert_eq!(
            FocusTarget::from(Focus::Sidebar, SidebarPanel::Explorer, EditorTab::Plain),
            FocusTarget::Explorer
        );
        // Opening a diff moves focus to the editor: the active layer becomes
        // DiffEditor, NOT SourceControl, even while the SCM panel is still the
        // underlying sidebar panel. This is the fact behind the "SCM keys do
        // nothing after previewing a diff" bug.
        assert_eq!(
            FocusTarget::from(Focus::Editor, SidebarPanel::SourceControl, EditorTab::Diff),
            FocusTarget::DiffEditor
        );
        assert_eq!(
            FocusTarget::from(Focus::Editor, SidebarPanel::Explorer, EditorTab::Plain),
            FocusTarget::Editor
        );
        // A too-large placeholder in the editor resolves to its override target.
        assert_eq!(
            FocusTarget::from(Focus::Editor, SidebarPanel::Explorer, EditorTab::Oversize),
            FocusTarget::Oversize
        );
    }

    #[test]
    fn oversize_placeholder_binds_open_anyway() {
        // Enter over a too-large placeholder loads it anyway; Esc leaves for the
        // sidebar. Editor editing keys must not leak in (the layer is not stacked).
        let ctx = Context::focus(FocusTarget::Oversize);
        assert_eq!(
            resolve(
                ctx,
                &[KeyChord::from_event(key(
                    KeyCode::Enter,
                    KeyModifiers::NONE
                ))]
            ),
            Resolved::Command(Command::OpenAnyway)
        );
        assert_eq!(
            resolve(
                ctx,
                &[KeyChord::from_event(key(KeyCode::Esc, KeyModifiers::NONE))]
            ),
            Resolved::Command(Command::ToggleFocus)
        );
        // A Ctrl-chord still resolves globally (close tab), but Save (Editor layer)
        // does not reach a placeholder.
        assert_eq!(
            resolve(
                ctx,
                &[KeyChord::from_event(key(
                    KeyCode::Char('w'),
                    KeyModifiers::CONTROL
                ))]
            ),
            Resolved::Command(Command::CloseTab)
        );
        assert_eq!(
            resolve(
                ctx,
                &[KeyChord::from_event(key(
                    KeyCode::Char('s'),
                    KeyModifiers::CONTROL
                ))]
            ),
            Resolved::None
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
    fn hints_for_is_context_aware_and_deduped() {
        let editor = hints_for(Context::focus(FocusTarget::Editor), ChordStyle::Caret);
        let cmds: Vec<Command> = editor.iter().map(|h| h.command).collect();
        // Editor-context commands are advertised…
        assert!(cmds.contains(&Command::Save));
        assert!(cmds.contains(&Command::Undo));
        assert!(cmds.contains(&Command::Cut));
        assert!(cmds.contains(&Command::Copy));
        assert!(cmds.contains(&Command::OpenFind));
        // …while self-evident motion and text-editing keys are omitted.
        assert!(!cmds.contains(&Command::CaretDown));
        assert!(!cmds.contains(&Command::PageDown));
        assert!(!cmds.contains(&Command::DeleteBackward));
        // Every command appears at most once (dedup across layers).
        for &cmd in &cmds {
            assert_eq!(cmds.iter().filter(|c| **c == cmd).count(), 1);
        }
        // The most-specific layer wins ordering: an Editor-layer verb (Save) precedes
        // a Global one (Quit).
        let save = cmds.iter().position(|c| *c == Command::Save);
        let quit = cmds.iter().position(|c| *c == Command::Quit);
        assert!(save < quit);
    }

    #[test]
    fn hints_for_reflects_the_active_modal() {
        let find = hints_for(
            Context::modal(Modal::Find, FocusTarget::Editor),
            ChordStyle::Caret,
        );
        let cmds: Vec<Command> = find.iter().map(|h| h.command).collect();
        assert!(cmds.contains(&Command::FindNext));
        assert!(cmds.contains(&Command::FindPrev));
        assert!(cmds.contains(&Command::FindCancel));
        // The Find modal is exclusive, so editor commands don't leak into the bar.
        assert!(!cmds.contains(&Command::Save));
    }

    #[test]
    fn completions_for_renders_only_the_remaining_chord() {
        let ctrl_k = KeyChord::from_event(key(KeyCode::Char('k'), KeyModifiers::CONTROL));
        let comps = completions_for(
            Context::focus(FocusTarget::Editor),
            &[ctrl_k],
            ChordStyle::Caret,
        );
        let by_cmd = |cmd| comps.iter().find(|h| h.command == cmd);
        // Only the chord AFTER the pending `^K` prefix is rendered.
        assert_eq!(
            by_cmd(Command::CloseAllTabs).map(|h| h.chord.as_str()),
            Some("^W")
        );
        assert_eq!(
            by_cmd(Command::CloseOtherTabs).map(|h| h.chord.as_str()),
            Some("W")
        );
        // Each completion carries a verb so the pending-chord bar is self-explanatory.
        assert!(by_cmd(Command::CloseAllTabs).is_some_and(|h| !h.verb.is_empty()));
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
