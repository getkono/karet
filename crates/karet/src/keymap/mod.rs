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

mod bindings;
mod chord;
mod layer;

#[cfg(test)]
mod tests;

use bindings::BINDINGS;
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
use Layer::CloseConfirm;
use Layer::CommitGraph;
use Layer::CommitInput;
use Layer::ContextMenu;
use Layer::DiffEditor;
use Layer::DiscardConfirm;
use Layer::Editor;
use Layer::Explorer;
use Layer::ExplorerDeleteConfirm;
use Layer::ExplorerEdit;
use Layer::Find;
use Layer::Global;
use Layer::Outline;
use Layer::Overlay;
use Layer::Oversize;
use Layer::Pager;
use Layer::RevInput;
use Layer::SearchInput;
use Layer::SearchList;
use Layer::Sidebar;
use Layer::SourceControl;
use Layer::SwapRecover;

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
