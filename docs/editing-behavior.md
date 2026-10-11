# Editing behaviour

What the caret, the selection and the pointer do in karet's editor, for every
motion, selection, multi-caret, clipboard, click, multi-click, drag, wheel and
modifier combination. This page is **normative**: where the editor disagrees
with it, the editor is wrong.

The reference is **VS Code** with its default settings and keymap on Linux and
Windows. karet follows it unless a row below records a deviation together with
its reason. Most deviations exist because a terminal cannot report something a
GUI receives, such as Cmd/Super on a mouse event, or because the terminal takes
a gesture for itself.

## Conformance

Every row is checked by a case in the editor conformance harness
(`crates/karet/src/app/tests/harness.rs`). The harness drives a rendered app
with key presses, commands, mouse presses, drags, releases and wheel notches,
with modifiers and click timing, across split panes, and asserts the buffer
text, every caret's anchor and head, the primary caret and the scroll
position, written as marked text:

```text
foo [bar|] baz      forward selection over "bar" (anchor at [, head at ])
foo [|bar] baz      backward selection (head at [, anchor at ])
|ab\n#cd            two carets; # marks the primary
```

The **Status** column says where each row stands:

- **harness**: a passing case checks it today.
- **#N**: the editor does not do this yet, and issue #N tracks the fix. That
  issue adds the row's case to the harness when it lands.
- **—**: no defect is known and no case checks it yet. A case is welcome; the
  harness cannot yet observe the horizontal scroll offset or a definition
  request.

The rows that do not pass yet are left out of the harness, so it can be run
today. Each fix then adds its own failing reproductions.

## Terms

- **Caret**: a selection whose anchor equals its head. **Anchor** is the fixed
  end of a selection; **head** is the end that moves, where the caret is drawn.
- **Primary caret**: the most recently added caret. The viewport follows it, and
  single-caret features (hover, completion, go to definition) act at it.
- **Goal column**: the display column that vertical motion tries to return to.
  It is measured in display cells (tabs expanded, wide characters counted
  twice), so it survives passing over shorter lines.
- **Word**: a run of identifier characters (letters, digits, `_`). A run of
  punctuation and a run of whitespace are each one unit of their own for word
  motion and double-click.
- Columns are counted in grapheme clusters wherever a user can see them:
  motion, deletion, hit-testing and display width (#342).

## Motion

Without Shift a motion first collapses any selection; with Shift it moves the
head and keeps the anchor. Every motion applies to every caret. Carets that
end on the same position merge into one.

| Input | Behaviour | Status |
|---|---|---|
| Left / Right | Move one character. With a selection, collapse to its start (Left) or end (Right) without moving further. | harness (Left, backward selection); #328 (forward selection) |
| Up / Down | Move one line, keeping the goal column. On the first line Up goes to column 0; on the last line Down goes to the line end. With a selection, start from its start (Up) or end (Down). | #329 |
| Home | Smart home: to the first non-blank character, or to column 0 if already there. | #329 |
| End | To the line end. | harness |
| Ctrl+Home / Ctrl+End | To the start / end of the document. With several carets, leave one caret there. | #328 |
| Ctrl+Left / Ctrl+Right, Alt+Left / Alt+Right | To the previous word start / next word end. | harness |
| PageUp / PageDown | Move the caret by the focused pane's visible rows, and scroll by the same amount. | #329 |
| Word wrap on | Up, Down, Home, End and PageUp/PageDown act on visual rows; End pressed again goes to the logical line end. | #329 |
| Collapsed folds | Motion skips over a fold; a jump into a fold (find, go to definition, outline) unfolds it. A caret is never hidden. | #329 |

Alt+Left/Right are bound as well as Ctrl+Left/Right. Deviation: macOS terminals
commonly deliver Option+arrow and not Cmd+arrow, so both bindings lead to the
same motion.

## Selection

| Input | Behaviour | Status |
|---|---|---|
| Shift + any motion above | Extend: the head moves and the anchor stays. | harness (Shift+Left, Shift+Right, Shift+End) |
| Ctrl+A | Select the whole document, with the head at its end. Does not scroll. | harness |
| Esc, one caret with a selection | Clear the selection and keep the caret at its head. | #328 |
| Esc, one caret, no selection | Nothing. Focus stays in the editor. | harness |
| Plain arrow after a selection | Clear the selection (see Motion for where the caret lands). | harness |

## Multiple carets

| Input | Behaviour | Status |
|---|---|---|
| Ctrl+Alt+Down / Ctrl+Alt+Up | Add a caret on the line below / above, at the goal column. The new caret is primary. | harness (add, primary); #329 (goal column) |
| Alt+click | Add a caret at the click, which becomes primary. Alt+click on an existing caret removes it, and the primary stays the same caret. | harness (add); #328 (remove) |
| Ctrl+D | With an empty caret, select the word under it. Then add the next occurrence after the most recently added one, wrapping and skipping ranges already selected. Matching is whole-word when the sequence began from an empty caret, and plain substring when it began from a selection. | #341 |
| Esc, several carets | Keep only the first caret that was placed, with its selection; a second Esc clears that selection. | harness (collapses to one caret); #328 (which caret is kept) |
| Merging | Carets merge when they coincide; selections merge when they overlap. Touching selections stay separate. A merge keeps each selection's direction and the primary. | #328 |
| Typing / deleting | Every caret edits. A caret whose edit is a no-op stays where it is; overlapping edits are merged rather than rejected. | #330 |
| Undo / Redo | Restore the full set of carets and selections from before / after the edit, only in the view that ran the command. | #331 |
| External edit (another split, the language server, a reload) | Every view maps its carets through the change; no caret is left outside the text. | #331 |
| Box selection: Shift+Alt+drag, Ctrl+Shift+Alt+arrows | One caret per row over a rectangle of display cells. | #343 |

Deviation: VS Code numbers its original caret as the primary one. karet makes
the newest caret primary, because the viewport and every single-caret feature
follow the primary, and the caret just added is the one the user is looking
at. Esc still keeps the original caret, as in VS Code.

## Clipboard

| Input | Behaviour | Status |
|---|---|---|
| Ctrl+C, a selection | Copy the selected text. With several carets, copy every selection, joined by newlines. | #332 (several carets) |
| Ctrl+C, no selection | Copy the whole line, newline included, marked as a line copy. | #332 |
| Ctrl+X | As Ctrl+C, then delete exactly what was copied: every selection, or every caret's whole line. | #332 |
| Ctrl+V | Insert at every caret. A line copy is inserted above the caret's line. When the clipboard has as many lines as there are carets, each caret gets one line. | #332 |
| Middle click (Linux) | Paste the primary selection at the click, behind `editor.middleClickPaste`. | #343 |

## Click

Clicks land on the character boundary nearest to the pointer: the right half
of a wide character or of a tab's cells resolves to after it (#337).

| Input | Behaviour | Status |
|---|---|---|
| Click on text | Place the caret there and drop every other caret. A click on visible text never scrolls. | harness (horizontal scroll read from the rig in `editor_drag.rs`; folds and sticky headers in `karet-editor`'s `tests/pointer.rs`) |
| Click past the end of a line | Caret at the line end. | harness |
| Click below the last line | Caret at the end of the document. | #337 |
| Click at a wrap boundary | The caret stays on the visual row that was clicked. | #337 |
| Shift+click | Extend the primary selection from its anchor to the click; drop the other carets. | harness (forward and backward) |
| Ctrl+click (Super+click where the terminal reports it) | Go to the definition of the identifier under the pointer. On anything else (whitespace, past the line end, the gutter) nothing happens. | — (needs a language server); #338 (non-identifiers) |
| Click in an unfocused split | Focus that split and place its caret; the other split's carets do not move. | harness |
| Click while the find bar is open | The editor takes keyboard focus; the find bar stays open. | #333 |
| Right click | Move the caret to the click if it is outside the selection, then open the context menu (Cut, Copy, Paste, spelling suggestions). | #339 |
| Line-number gutter | A click selects the whole line, newline included; a drag selects whole lines; Shift+click extends by lines. | #335 |
| Breakpoint lane, fold chevron | Toggle a breakpoint / toggle the fold; these never move the caret. | #335 |

Deviation, go to definition: a terminal mouse event cannot report Cmd/Super
under SGR mouse reporting, and macOS terminals turn Ctrl+click into a right
click. Ctrl+click is therefore the gesture on every platform, and F12 is the
keyboard path that always works.

## Multi-click

| Input | Behaviour | Status |
|---|---|---|
| Double click | Select the unit under the pointer: a word, a punctuation run or a whitespace run. | harness (word); #338 (punctuation, whitespace) |
| Triple click | Select the whole line, newline included. | #338 (newline) |
| Fourth click and beyond | Stay at line granularity. | #338 |
| Click counting | Clicks count as one streak when they land within one cell of the previous click, within the multi-click interval, with the same modifiers, on the same surface. A pause, a move, or a change of modifier starts a new streak. | harness (pause); #338 (rest) |
| Modifier + repeated click | The modifier wins over the count: two quick Alt+clicks add (then remove) a caret, and two quick Shift+clicks extend twice. Neither selects a word. | #338 |
| Double click on a misspelled word | Select the word, as anywhere else. Spelling suggestions live in the right-click menu. | #338 |

## Drag

A press in the text starts a drag that owns the pointer until it is released.

| Input | Behaviour | Status |
|---|---|---|
| Press, drag | Select from the press to the character under the pointer, with the head at the pointer. | harness (forward and backward) |
| Double click, drag | Extend by whole words; the word first clicked stays selected however far back the pointer goes. | harness (forward and backward) |
| Triple click, drag | Extend by whole lines. | harness |
| Pointer above or below the viewport | Scroll toward it on a fixed-rate timer, faster the further out it is, and keep extending. Sticky header rows count as above. | harness |
| Pointer left or right of the viewport | Scroll horizontally the same way; the gutter counts as left. | harness |
| Pointer inside the viewport | Never autoscroll. | harness |
| Wheel during a drag | Scroll, and extend to the pointer's new text position. | harness |
| Release | End the drag and its autoscroll. | harness |
| Release elsewhere, focus loss, an overlay opening, a tab switch, a key press | Also end the drag. A drag never outlives its button. | harness |
| A press with no release before it | End the stale drag, then act as a fresh press. | harness |
| Pointer strays into another split | Keep mapping it against the split the drag started in, clamped to that split's edge. | harness |
| Window resized mid-drag | Map the pointer against the split's new size. | harness |

## Wheel

| Input | Behaviour | Status |
|---|---|---|
| Wheel | Scroll three lines per notch. The caret and the selection do not move. | harness |
| Shift+wheel, horizontal wheel | Scroll three columns per notch. | — |
| Wheel over an unfocused split | Scroll that split without moving focus to it. | #339 |
| Wheel over a popup (hover, completion) | Scroll the popup, not the editor under it. | #339 |

Deviation: many terminals keep Shift+wheel and Shift+click for their own text
selection and never send them to the application. karet honours them when they
arrive and offers a keyboard path for each (#343).

## Modifiers terminals may not deliver

| Gesture | Often lost on | Keyboard path |
|---|---|---|
| Super/Cmd + click | Every terminal under SGR mouse reporting | Ctrl+click, F12 |
| Ctrl+click | macOS terminals (sent as a right click) | F12 |
| Alt/Option + click | macOS terminals, Linux window managers that move windows on Alt+drag | Ctrl+Alt+Up/Down, Ctrl+D |
| Shift+click, Shift+wheel | Terminals that reserve Shift for native selection | Shift+arrows; horizontal wheel |
| Shift+Alt+drag | All of the above | Ctrl+Shift+Alt+arrows (#343) |

Each gesture must have a bound keyboard path, so nothing in this document
depends on a modifier reaching karet (#343).
