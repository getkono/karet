//! The keymap: a single binding table is the source of truth for both the
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
//! The table in effect is a [`Keymap`]: the compiled-in [`BINDINGS`] with the
//! user's and the project's `keymap.jsonc` files ([`user`]) parsed into the same
//! [`Binding`] shape and layered over them. The merged keymap is [installed](install)
//! at startup and again on every live reload, and every query here reads it.
//!
//! Modal editing (`editor.keymap` `"vim"`/`"helix"`) is a layer like any other:
//! the mode's layer stacks over the editor's ([`Editing`]), so its bindings are
//! rebindable in a keymap file. What no fixed binding can express — counts,
//! operators over motions, registers, macros — is the grammar in [`vi`], which
//! takes the keys the mode's layer leaves unbound ([`resolve_normal`]).

mod bindings;
mod chord;
mod layer;
pub mod user;
pub(crate) mod vi;

#[cfg(test)]
mod tests;

use std::borrow::Cow;
use std::sync::Arc;
use std::sync::LazyLock;
use std::sync::PoisonError;
use std::sync::RwLock;

use bindings::BINDINGS;
pub use chord::ChordStyle;
pub use chord::KeyChord;
use chord::chord;
use crossterm::event::KeyCode;
pub use layer::Context;
pub use layer::Editing;
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
/// It borrows for the compiled-in defaults and owns for a binding parsed from a
/// user keymap file; `origin` says which.
#[derive(Clone, Debug)]
struct Binding {
    layer: Layer,
    chord: KeyChord,
    rest: Cow<'static, [KeyChord]>,
    command: Command,
    origin: Origin,
}

/// Where a [`Binding`] came from: the compiled-in table, or one entry of a user
/// keymap file (so a conflict report can point at it).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Origin {
    /// The compiled-in [`BINDINGS`] table.
    Default,
    /// Entry `entry` (0-based) of the keymap file in `source`.
    File {
        /// Which keymap file layer.
        source: user::Source,
        /// The entry's index in the file's `bindings` array.
        entry: usize,
    },
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
        rest: Cow::Borrowed(&[]),
        command,
        origin: Origin::Default,
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
        rest: Cow::Borrowed(rest),
        command,
        origin: Origin::Default,
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
use Layer::CommitConsole;
use Layer::CommitGraph;
use Layer::CommitInput;
use Layer::Confirm;
use Layer::ContextMenu;
use Layer::DiffEditor;
use Layer::Editor;
use Layer::Explorer;
use Layer::ExplorerEdit;
use Layer::Find;
use Layer::Github;
use Layer::Global;
use Layer::HelixNormal;
use Layer::Hierarchy;
use Layer::Insert;
use Layer::LanguageServers;
use Layer::Outline;
use Layer::Overlay;
use Layer::Oversize;
use Layer::Pager;
use Layer::Problems;
use Layer::ProblemsFilter;
use Layer::Rename;
use Layer::RevInput;
use Layer::Seam;
use Layer::SearchInput;
use Layer::SearchList;
use Layer::Sidebar;
use Layer::SourceControl;
use Layer::Terminal;
use Layer::VimNormal;

/// A complete, merged binding table: the compiled-in defaults with any user and
/// project keymap files applied over them (see [`user`]). Every query the keymap
/// answers — resolution, hints, completions — reads one of these, so a rebinding
/// moves the binding and its displayed hint together.
#[derive(Clone, Debug)]
pub struct Keymap {
    /// Most-preferred first: within a layer the first matching binding wins, and
    /// [`Keymap::hint_for`] shows a command's first binding.
    bindings: Vec<Binding>,
}

impl Keymap {
    /// The compiled-in default keymap, with no user layer applied.
    #[must_use]
    pub fn defaults() -> Self {
        Self {
            bindings: BINDINGS.to_vec(),
        }
    }
}

/// The keymap every free function in this module reads; `None` until a merged
/// keymap is [installed](install), meaning the compiled-in defaults.
static ACTIVE: RwLock<Option<Arc<Keymap>>> = RwLock::new(None);

/// The compiled-in defaults, built once for the common no-user-keymap case.
static DEFAULTS: LazyLock<Arc<Keymap>> = LazyLock::new(|| Arc::new(Keymap::defaults()));

/// Make `keymap` the one the free functions ([`resolve`], [`hint_for`], …) read.
/// Called at startup and on every live reload of a keymap file.
pub fn install(keymap: Keymap) {
    let mut active = ACTIVE.write().unwrap_or_else(PoisonError::into_inner);
    *active = Some(Arc::new(keymap));
}

/// The keymap currently in effect.
fn active() -> Arc<Keymap> {
    let active = ACTIVE.read().unwrap_or_else(PoisonError::into_inner);
    active.clone().unwrap_or_else(|| Arc::clone(&DEFAULTS))
}

/// Resolve a key press into a [`Command`], given the focus, the active sidebar
/// panel, and whether the active tab is a diff. Returns `None` for keys with no
/// binding.
#[must_use]
pub fn resolve(ctx: Context, pending: &[KeyChord]) -> Resolved {
    active().resolve(ctx, pending)
}

/// Resolve a key sequence typed in a modal preset's Normal mode against the
/// mode's own layer alone (the first of `ctx`'s stack), which takes precedence
/// over the modal grammar (see [`Keymap::resolve_normal`]).
#[must_use]
pub fn resolve_normal(ctx: Context, pending: &[KeyChord]) -> Resolved {
    active().resolve_normal(ctx, pending)
}

/// Resolve a key sequence the modal grammar did not take against the rest of
/// `ctx`'s stack (see [`Keymap::resolve_below_normal`]).
#[must_use]
pub fn resolve_below_normal(ctx: Context, pending: &[KeyChord]) -> Resolved {
    active().resolve_below_normal(ctx, pending)
}

/// The display hint (e.g. `"Ctrl+W"`, `"Ctrl+K Ctrl+W"`) for `command`'s first
/// binding, rendered in `style`, if the command is bound.
#[must_use]
pub fn hint_for(command: Command, style: ChordStyle) -> Option<String> {
    active().hint_for(command, style)
}

/// Every advertisable binding live in `ctx` (see [`Keymap::hints_for`]).
#[must_use]
pub fn hints_for(ctx: Context, style: ChordStyle) -> Vec<Hint> {
    active().hints_for(ctx, style)
}

/// The completions of an in-progress chord (see [`Keymap::completions_for`]).
#[must_use]
pub fn completions_for(ctx: Context, pending: &[KeyChord], style: ChordStyle) -> Vec<Hint> {
    active().completions_for(ctx, pending, style)
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

impl Keymap {
    /// The bindings in `layer`, in preference order.
    fn in_layer(&self, layer: Layer) -> impl Iterator<Item = &Binding> {
        self.bindings.iter().filter(move |bind| bind.layer == layer)
    }

    /// Resolve a key press in `ctx` (see the free [`resolve`]).
    #[must_use]
    pub fn resolve(&self, ctx: Context, pending: &[KeyChord]) -> Resolved {
        self.resolve_in(active_layers(ctx), pending)
    }

    /// Walk `layers` most-specific-first: a complete match in any layer wins;
    /// failing that, report [`Resolved::Pending`] if `pending` is a prefix of some
    /// binding.
    fn resolve_in(&self, layers: &[Layer], pending: &[KeyChord]) -> Resolved {
        self.resolve_filtered(layers, pending, false)
    }

    /// Resolve in the modal Normal layer of `ctx`'s stack alone: the first
    /// layer, when `ctx` is a modal preset's Normal mode. Its bindings are the
    /// preset's commands and the user's rebindings, which outrank the grammar.
    #[must_use]
    pub fn resolve_normal(&self, ctx: Context, pending: &[KeyChord]) -> Resolved {
        match active_layers(ctx) {
            [first @ (VimNormal | HelixNormal), ..] => self.resolve_in(&[*first], pending),
            _ => Resolved::None,
        }
    }

    /// Resolve against the layers under the modal Normal layer, for a key
    /// sequence the grammar did not take. Only bindings whose first chord is
    /// modified (`Ctrl`, `Alt`) or a function key count: a plain key in Normal
    /// mode is the grammar's or nothing, so the editor's Enter, Tab or
    /// Backspace can never edit there.
    #[must_use]
    pub fn resolve_below_normal(&self, ctx: Context, pending: &[KeyChord]) -> Resolved {
        match active_layers(ctx) {
            [VimNormal | HelixNormal, rest @ ..] => self.resolve_filtered(rest, pending, true),
            _ => Resolved::None,
        }
    }

    /// [`Self::resolve_in`], optionally skipping bindings that start with a
    /// plain chord.
    fn resolve_filtered(
        &self,
        layers: &[Layer],
        pending: &[KeyChord],
        modified_only: bool,
    ) -> Resolved {
        let mut prefix = false;
        for &layer in layers {
            for bind in self.in_layer(layer) {
                if modified_only && bind.chord.is_plain() {
                    continue;
                }
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

    /// The display hint for `command`'s first binding (see the free [`hint_for`]).
    #[must_use]
    pub fn hint_for(&self, command: Command, style: ChordStyle) -> Option<String> {
        self.bindings
            .iter()
            .find(|bind| bind.command == command)
            .map(|bind| bind.hint(style))
    }

    /// Every advertisable binding live in `ctx`, ordered most-specific layer first
    /// and deduped by command (the first binding wins, matching
    /// [`Keymap::hint_for`]). Only commands with a terse verb
    /// ([`Command::hint_verb`]) are included, so self-evident motion and
    /// text-editing keys are omitted. This is the forward counterpart to
    /// `hint_for` that drives the context-aware status hints bar.
    #[must_use]
    pub fn hints_for(&self, ctx: Context, style: ChordStyle) -> Vec<Hint> {
        let mut hints = Vec::new();
        let mut seen: Vec<Command> = Vec::new();
        for &layer in active_layers(ctx) {
            for bind in self.in_layer(layer) {
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
    /// rendered (e.g. after `^K`, `^W` for "close all" and `W` for "close
    /// others"). Deduped by command, most-specific layer first. Powers the
    /// pending-chord hint bar.
    #[must_use]
    pub fn completions_for(
        &self,
        ctx: Context,
        pending: &[KeyChord],
        style: ChordStyle,
    ) -> Vec<Hint> {
        let mut hints = Vec::new();
        let mut seen: Vec<Command> = Vec::new();
        for &layer in active_layers(ctx) {
            for bind in self.in_layer(layer) {
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
