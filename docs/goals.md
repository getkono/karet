# Design goals

What karet is trying to be as a code editor: a Zed- and VS Code-class editing
experience in a terminal, with the limits of a cell grid worked around rather
than accepted. This page is the positive counterpart to [scope.md](scope.md),
which records what karet has decided *not* to build.

Each goal ends in a **rule** that a change can be checked against. An issue or
pull request that would break a rule either fixes the gap it opens or says
which goal it trades away and why; a rule is changed here, not waived in
passing.

## 1. Intelligence in place

Inlay hints, diagnostics, signature help and ghost text are part of the text,
not a layer floating over it. karet follows Zed's model, adapted to a cell
grid: language intelligence renders inline as first-class **virtual text**
(cells between or after characters) and **virtual lines** (rows between
buffer lines), and a popup is used only for what cannot fit inline — a hover's
documentation, a completion list, a code-action menu.

**Rule:** a feature that annotates code renders inside the text through the
editor's virtual-text and virtual-line model. A new popup must name what about
its content cannot be shown inline. Virtual text never moves a caret, never
takes part in hit-testing as buffer text, and never counts towards the goal
column (see [editing-behavior.md](editing-behavior.md#terms)).

## 2. No dead layers

A capability an engine builds but the user cannot reach is a defect, not a
partial feature. karet's layers are the client request in an engine crate, the
`karet-session` `Command`/`Event` seam, a UI surface, a command-palette entry,
a key binding, and tests.

**Rule:** a language or editor feature is done only when every layer exists:
client request → session `Command`/`Event` → UI surface → palette command →
key binding → tests. A feature that deliberately omits a layer (no default key
binding, say) records that in its pull request. Every key in the settings
schema is read by code; a schema key nothing reads is removed or wired up.

## 3. Instant

Typing never waits on work that grows with the document, the workspace, or a
language server.

**Rule:** no work proportional to document size runs on the input path per
keystroke; parsing, highlighting, diffing and language-server traffic run
incrementally or off the UI thread. The frame is repainted immediately after
input is applied, before any pending background work is awaited. Slow work
follows the loading-state policy in [AGENTS.md](../AGENTS.md#ui-loading-states):
update the visible surface first, and reveal a placeholder only after
`LOADING_REVEAL_DELAY` (200 ms).

## 4. TUI-native, degrade gracefully

karet uses what the terminal can do and still works where it cannot.

**Rule:** every optional terminal capability — the kitty keyboard protocol,
undercurl, truecolor, graphics, and OSC 52 (clipboard), OSC 22 (pointer shape)
and OSC 8 (hyperlinks) — is detected or probed, and its absence selects a
fallback instead of an error or a garbled screen. Every action is reachable
from the keyboard (*keyboard-first*), and every mouse gesture a GUI editor
offers on the text has a working equivalent where the terminal reports it
(*mouse-complete*).

## 5. Familiar by default, customisable where it counts

Someone arriving from VS Code is productive without reading anything; someone
with strong habits can keep them.

**Rule:** the default keymap and editing behaviour follow VS Code. Customisation
goes through the existing settings layers ([configuration.md](configuration.md)),
a user keymap, and an opt-in modal (vim/helix-style) layer — never through a
change of the defaults. A new setting joins the existing `setting.jsonc`
layers and schema rather than a file of its own.

## 6. Caret and selection beyond reproach

The caret is the one thing a user watches all the time, and a terminal editor
that gets it wrong feels broken however good everything else is.

**Rule:** every keyboard and mouse combination behaves as VS Code does, as
specified row by row in [editing-behavior.md](editing-behavior.md), and each
row is checked by the editor conformance harness; a recorded deviation states
its reason. A caret never disappears, jumps without a motion asking it to,
merges with another by accident, or moves text under the pointer. Where a
terminal cannot report a modifier on a mouse event, the same action has a
keyboard fallback.

## 7. Text correctness

What karet saves is what the user wrote, and what it shows lines up with it.

**Rule:** columns are counted in grapheme clusters and measured in display
cells for motion, deletion, hit-testing and rendering. A file's encoding and
line endings survive an open–edit–save round trip unchanged unless the user
changes them. Undo restores the exact text and carets of every step it
reverses, and never loses a step.

## 8. Reusable headless engines

karet is a toolkit as much as an application, and the editor must not erode
that.

**Rule:** the [design principles in AGENTS.md](../AGENTS.md#design-principles)
hold for every change: engines stay headless, new presentation goes behind a
crate's `view` feature or into a widget crate, and only the `karet` app wires a
producer (`karet-lsp`, `karet-vcs`, …) to a widget.
