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
  twice, inlay hints not counted), from the start of the visual row when word
  wrap is on, so it survives passing over shorter lines. Each caret keeps its
  own; a horizontal motion, an edit or a click resets it.
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
| Left / Right | Move one character. With a selection, collapse to its start (Left) or end (Right) without moving further. | harness |
| Up / Down | Move one line, keeping the goal column. On the first line Up goes to column 0; on the last line Down goes to the line end. With a selection, start from its start (Up) or end (Down). | harness |
| Home | Smart home: to the first non-blank character, or to column 0 if already there. | harness |
| End | To the line end. | harness |
| Ctrl+Home / Ctrl+End | To the start / end of the document. With several carets, leave one caret there. | harness |
| Ctrl+Left / Ctrl+Right, Alt+Left / Alt+Right | To the previous word start / next word end. | harness |
| Ctrl+Shift+\ | Go to Bracket: to the other bracket of the pair the caret touches (the bracket after it first, then the one before it), or, inside a pair, to its closing bracket. The caret lands just before the target, so pressing again goes back. Brackets in strings and comments are ignored. | harness |
| PageUp / PageDown | Move the caret by the focused pane's visible rows, keeping the goal column, and scroll by the same amount so the caret keeps its screen row. With less than a page left, go to the start / end of the document. | harness |
| Word wrap on | Up, Down, Home, End and PageUp/PageDown act on visual rows; End pressed again goes to the logical line end. | harness |
| Collapsed folds | Motion skips over a fold; a jump into a fold (find, go to definition, outline) unfolds it. Collapsing a fold moves every caret inside it onto its header. A caret is never hidden. | harness |

Alt+Left/Right are bound as well as Ctrl+Left/Right. Deviation: macOS terminals
commonly deliver Option+arrow and not Cmd+arrow, so both bindings lead to the
same motion.

## Selection

| Input | Behaviour | Status |
|---|---|---|
| Shift + any motion above | Extend: the head moves and the anchor stays. Shift+Alt+Left / Right are Shrink / Expand Selection instead (below); word selection is Ctrl+Shift+Left / Right. | harness (Shift+Left, Shift+Right, Shift+End) |
| Shift+Alt+Right | Expand Selection: every selection grows to the smallest range that strictly contains it. The ranges are the language server's selection ranges (`textDocument/selectionRange`) at each caret when the document's server offers them; a caret the server has no enclosing range for, or a document with no such server, grows along the tree-sitter syntax tree instead, injected languages included (a Markdown code fence grows through the fence's language first). A caret touching a word takes the word first, so a word in a string grows to the word, the string's contents, the string, the argument list, the call. Each grown selection runs forward from the range's start. Selections that grow into each other merge. A selection with no larger range stays. While the server's answer is on its way nothing moves, and each further press takes one more step once it lands; an answer for selections or text that changed in the meantime is dropped. | harness; server ranges: app unit tests |
| Shift+Alt+Left | Shrink Selection: restore the exact set of carets and selections the last Expand Selection grew from, directions and primary included, step by step back to where expanding began. Any other change of selection, or an edit made elsewhere, ends that history, and Shrink then does nothing. | harness |
| Ctrl+A | Select the whole document, with the head at its end. Does not scroll. | harness |
| Ctrl+Shift+Alt+\ | Select to Bracket: select the pair Go to Bracket would use, brackets included, with the head after the closing bracket. Pressed on a selection that is exactly a pair, widen to the pair around it. | harness |
| Esc, one caret with a selection | Clear the selection and keep the caret at its head. | harness |
| Esc, one caret, no selection | Nothing. Focus stays in the editor. | harness |
| Plain arrow after a selection | Clear the selection (see Motion for where the caret lands). | harness |

Deviation: VS Code binds no key to Select to Bracket. karet binds
Ctrl+Shift+Alt+\, Go to Bracket's chord with Alt, so the command has a key at
all. A legacy terminal sends neither chord apart from Ctrl+\; both commands
are in the command palette.

Deviation: VS Code grows along the ranges its selection-range providers give.
karet prefers the language server's ranges too, but where the server has none
enclosing a selection it grows that selection along the tree-sitter syntax tree,
so a server that offers no selection ranges, or none at a caret, still leaves
Expand Selection working there. macOS terminals deliver
Option+Shift+arrow as Shift+Alt+arrow, so there it is Expand / Shrink Selection
too, not word selection.

## Multiple carets

| Input | Behaviour | Status |
|---|---|---|
| Ctrl+Alt+Down / Ctrl+Alt+Up | Add a caret on the line below / above, at the goal column. The new caret is primary. | harness |
| Alt+click | Add a caret at the click, which becomes primary. Alt+click on an existing caret removes it, and the primary stays the same caret. | harness |
| Ctrl+D | With an empty caret, select the word under it. Then add the next occurrence after the most recently added one, wrapping and skipping ranges already selected. Matching is whole-word when the sequence began from an empty caret, and plain substring when it began from a selection. | #341 |
| Esc, several carets | Keep only the first caret that was placed, with its selection; a second Esc clears that selection. | harness |
| Merging | Carets merge when they coincide; selections merge when they overlap, and a caret on or inside a selection merges into it. Touching selections stay separate. A merge keeps each selection's direction and the primary. | harness |
| Typing / deleting | Every caret edits. A caret whose edit is a no-op stays where it is, and the primary stays the same caret; overlapping edits are merged into one edit over their union rather than rejected. An edit the buffer rejects moves no caret. | harness |
| Accepting a completion | Every caret takes the item. At the primary caret it replaces the server's edit range for the item (its insert range on Enter or Tab, its replace range on Shift+Enter or Shift+Tab), stretched over whatever was typed since the request, or the typed word for an item without one. Wherever the text it replaced before (and after) the primary caret also sits around another caret, it is replaced there too; at any other caret it replaces the selection, or is inserted at the caret. The item's additional edits (an import, say) apply once, in the same undoable edit. | harness |
| Find: Replace / Replace All | Replace changes the current match once, whatever the number of carets. Replace All changes every match in one undoable edit and leaves every caret and selection on the text it was on. | harness |
| Undo / Redo | Restore the full set of carets and selections from before / after the edit, only in the view that ran the command. | harness (which view, `tests/external_edits.rs`); the restored set in `karet-session`'s `tests/undo_save.rs` |
| External edit (another split, the language server, a reload) | Every view maps its carets through the change; no caret is left outside the text. | harness (`tests/external_edits.rs`) |
| Box selection: Shift+Alt+click, Shift+Alt+drag | Select a rectangle of display cells (tabs expanded to their stops, wide characters counted twice) from the box in progress, or else from the primary selection's anchor, to the cell under the pointer: one selection per line, its head toward the pointer. A line that ends left of the box gets no selection; one that ends inside it is selected to its end. A column past a line's end is kept, so the box stays as wide as the pointer when it reaches a longer line. The selection on the pointer's line is primary. | harness |
| Box selection: Ctrl+Shift+Alt+Up / Down / Left / Right | Grow or shrink the same box one line or one display cell at a time, starting from the primary selection when no box is in progress. Right stops at the end of the longest line the box spans. Any other change of selection ends the box. | harness |

Deviation: VS Code numbers its original caret as the primary one. karet makes
the newest caret primary, because the viewport and every single-caret feature
follow the primary, and the caret just added is the one the user is looking
at. Esc still keeps the original caret, as in VS Code.

## Indentation

The indent unit is the document's resolved indentation: EditorConfig, then the
language's `tabSize` / `insertSpaces` settings.

| Input | Behaviour | Status |
|---|---|---|
| Tab, a caret | Insert indentation at every caret: with spaces, just enough to reach the next indent stop (hard tabs before the caret count to their display stop); with hard tabs, one indent unit. | harness |
| Tab, a selection | Indent every line the selection touches by one unit, at the line start. A line with no characters stays empty, and a selection ending at column 0 does not touch that line. The selection stays on the same text, and an end at column 0 stays there, so a whole-line selection stays whole. A caret on a line a selection indents moves with that line. | harness |
| Shift+Tab | Outdent every line any caret or selection touches by one unit: a leading hard tab is removed, and space indentation moves back to the previous indent stop. Selections stay on their text; an end inside the removed indentation lands at the line start. | harness |
| Tab with the completion popup open | Accept the selected item (see Multiple carets). | harness |
| Tab where an inline macro applies at every caret | Expand the macro (e.g. Rust `fn` → a function skeleton) instead of indenting. | harness |
| Tab in an editor with no editable text (no tab, a preview, an image) | Toggle focus to the sidebar, as Tab does outside the editor. | harness |
| Ctrl+Shift+E | Toggle focus between the editor and the sidebar, from anywhere. | harness |

Deviation: VS Code replaces a selection within one line with a tab. karet
indents the line instead, as Zed and JetBrains IDEs do, so Tab never deletes
selected text.

Deviation: VS Code's Tab never leaves the editor. karet's Tab toggles focus
everywhere else, so the editor's focus key is Ctrl+Shift+E (VS Code's Show
Explorer); a legacy terminal cannot tell it from Ctrl+E, and View: Toggle
Focus in the command palette and the status bar's focus segment are always
available.

## Brackets and quotes

The pairs are the language's (`()`, `[]`, `{}` and `"` everywhere; `'` and
`` ` `` where they are always quotes, so never `'` in Rust), and whether each
kind closes is `editor.autoClosingBrackets` / `editor.autoClosingQuotes`
(`"languageDefined"` by default).

| Input | Behaviour | Status |
|---|---|---|
| An opening bracket or quote, a caret | Insert the pair and leave the caret between its halves, at every caret, as one undoable edit, when the caret is before whitespace, the end of the line or closing punctuation (`;:.,=}])>`). A quote also stays single inside a string or comment and straight after a letter, digit or `_`. | harness |
| A closer the editor inserted | Step over it rather than insert another, at every caret, while the carets are where the last keystroke left them and nothing else has edited the document (typing inside the pair keeps it). A closer that was already there is typed normally. | harness |
| Backspace between an empty pair | Delete both halves. | harness |
| An opening bracket or quote, a selection | Surround the selection with the pair, keeping the same text selected. | harness |
| Enter between a bracket pair | Open a line between the halves indented one unit deeper than the opener's line, with the closer on its own line below at the opener's indentation. | harness |
| A closing bracket on a line holding only whitespace | Replace the line's indentation with the indentation of the line holding its unclosed opener. | harness |

## Clipboard

| Input | Behaviour | Status |
|---|---|---|
| Ctrl+C, a selection | Copy the selected text. With several carets, copy every selection in document order, joined by newlines; an empty caret beside a selection adds nothing. | harness (`clipboard.rs`) |
| Ctrl+C, no selection | Copy the whole line, newline included, marked as a line copy. With several carets, copy each caret's line once. | harness |
| Ctrl+X | As Ctrl+C, then delete exactly what was copied: every selection, or every caret's whole line. Nothing is deleted when the copy fails. | harness |
| Ctrl+V | Insert at every caret. A line copy is inserted above the caret's line (a selection is replaced instead). When the clipboard has as many pieces, or lines, as there are carets, each caret gets one. The line-copy marker and the pieces hold only while the clipboard still has the exact text karet copied. | harness |
| Middle click in the text | With `editor.middleClickPaste` on (by default on Linux only), place a single caret at the click and paste the primary selection there. With no primary selection to read, a notification says so and nothing is edited. A middle click on a tab closes it instead. | harness |

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
| Ctrl+click (Super+click where the terminal reports it) | Go to the definition of the identifier under the pointer. On anything else (whitespace, past the line end, the gutter) nothing happens. | — (needs a language server); non-identifiers by a recording backend in `tests/definition.rs` |
| Ctrl+click on a link written in the source | Open the link instead of going to a definition: a bare `http`/`https` URL in any file, and in Markdown an inline `[text](target)` (or `![alt](src)`) link anywhere from its `[` to its `)`. A URL opens with the desktop's opener; a relative file target opens in karet, and one that resolves outside the workspace asks first; any other target is refused with a notice. Holding Ctrl (or Super) over a link underlines it, whether or not a language server is running; the underline shows in the focused split only, but a Ctrl+click on a link in an unfocused split focuses that split and opens the link. Ctrl+Alt and Ctrl+Shift keep their editor meanings. | `tests/source_links.rs`; scanning in `autolink.rs` |
| Click in an unfocused split | Focus that split and place its caret; the other split's carets do not move. | harness |
| Click while the find bar is open | The editor takes keyboard focus; the find bar stays open. Esc from the text then closes it; a click in its field takes the keys back. | harness |
| Right click | Move the caret to the click if it is outside the selection, then open the context menu (Cut, Copy, Paste, spelling suggestions). | harness (caret, menu, unfocused split; `pointer_routing.rs`); spelling suggestions in `tests/spellcheck.rs` |
| Click on a completion row | Accept that candidate, as Enter would. | harness |
| Click outside the hover popup | Close the popup, then do what the click would have done without it. A click inside it is kept. | harness |
| Click while the diagnostic view is open | It is modal: a click outside it closes it and does nothing else; a click inside it is kept. | harness |
| Line-number gutter | A click selects the whole line, newline included, and never scrolls; a drag selects whole lines; Shift+click extends by lines from the selection's anchored line. Ctrl and Alt keep their text meanings. | harness (`tests/gutter.rs`) |
| Breakpoint lane, fold chevron, `⋯` | The leading marker lane toggles a breakpoint, in a source file only (elsewhere it is refused); the chevron in the lane beside the text, and a collapsed fold's `⋯`, toggle the fold. None of them moves the caret, except that collapsing a fold over the caret moves it onto the header. | harness (`tests/gutter.rs`) |
| Change lane | While `git.decorations` is on, the gutter keeps a column between the line numbers and the fold chevron for uncommitted-change markers, so a breakpoint or diagnostic sign on the same line stays visible. A click on a change marker opens the file's changes against `HEAD` (as Open Changes: With Previous Revision does, which opens at the top) scrolled so the clicked marker's hunk — for a deletion marker, the deletion just below its line — is at the top of the viewport in either diff layout, and leaves the caret; a click there on an unmarked line is a click at the line's start. | harness (`tests/gutter.rs`) |
| Gutter below the last line | A click below the last line is a click below the text in every lane: no breakpoint, no line selection. | harness (`tests/gutter.rs`) |

Deviation, go to definition: a terminal mouse event cannot report Cmd/Super
under SGR mouse reporting, and macOS terminals turn Ctrl+click into a right
click. Ctrl+click is therefore the gesture on every platform, and F12 is the
keyboard path that always works.

Deviation, source links: Alt+click adds a caret, so it cannot also open a
link; links share Ctrl+click with Go to Definition, and a link wins where both
apply. Links are found per line, blind to code fences and inline code, and
reference-style links (`[text][ref]`) are not resolved. Opening a link has no
keyboard path yet.

## Multi-click

| Input | Behaviour | Status |
|---|---|---|
| Double click | Select the unit under the pointer: a word, a punctuation run or a whitespace run. | harness |
| Triple click | Select the whole line, newline included. | harness |
| Fourth click and beyond | Stay at line granularity. | harness |
| Click counting | Clicks count as one streak when they land within one cell of the previous click, within the multi-click interval, with the same modifiers, on the same surface. A pause, a move, or a change of modifier starts a new streak. | harness (pause, move, modifier); surface in `multi_click.rs` |
| Modifier + repeated click | The modifier wins over the count: two quick Alt+clicks add (then remove) a caret, and two quick Shift+clicks extend twice. Neither selects a word. | harness |
| Double click on a misspelled word | Select the word, as anywhere else. Spelling suggestions live in the right-click menu. | `tests/spellcheck.rs` |

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
| Wheel during a drag | Scroll, and extend to the pointer's new text position. Shift+wheel scrolls columns, as it does outside a drag. | harness |
| Release | End the drag and its autoscroll. | harness |
| Release elsewhere, focus loss, an overlay opening, a tab switch, a key press, pointer motion with no button held | Also end the drag. A drag never outlives its button. | harness |
| A press with no release before it | End the stale drag, then act as a fresh press. | harness |
| Pointer strays into another split | Keep mapping it against the split the drag started in, clamped to that split's edge. | harness |
| Window resized mid-drag | Map the pointer against the split's new size. | harness |

## Wheel

| Input | Behaviour | Status |
|---|---|---|
| Wheel | Scroll three lines per notch. The caret and the selection do not move. | harness |
| Shift+wheel, horizontal wheel | Scroll three columns per notch. | — |
| Wheel over an unfocused split | Scroll that split without moving focus to it. | harness |
| Wheel over a popup (hover, completion) | Scroll the popup, not the editor under it. | harness |

Deviation: many terminals keep Shift+wheel and Shift+click for their own text
selection and never send them to the application. karet honours them when they
arrive and offers another path for each (see below).

Deviation, middle click: VS Code calls the setting `editor.selectionClipboard`
and also copies every selection into the primary selection. karet only reads
the primary selection, which the terminal or another application fills; it
does not write it.

## Modifiers terminals may not deliver

| Gesture | Often lost on | Keyboard path |
|---|---|---|
| Super/Cmd + click | Every terminal under SGR mouse reporting | Ctrl+click, F12 |
| Ctrl+click | macOS terminals (sent as a right click) | F12 |
| Alt/Option + click | macOS terminals, Linux window managers that move windows on Alt+drag | Ctrl+Alt+Up/Down, Ctrl+D |
| Shift+click | Terminals that reserve Shift for native selection | Shift+arrows |
| Shift+wheel | Terminals that reserve Shift for native selection | The horizontal wheel; Left/Right, which the view follows |
| Shift+Alt+click, Shift+Alt+drag | All of the above | Ctrl+Shift+Alt+arrows |

Every gesture has a bound keyboard path, so nothing in this document depends on
a modifier reaching karet; a keymap test
(`every_swallowable_gesture_has_a_bound_keyboard_path`) checks each chord in
the table. Where the chord's own modifiers are lost too, as Option is in some
macOS terminals, each of these commands is also in the command palette
(Ctrl+Shift+P, or F1), under the names Go to Definition, Add Cursor Above, Add
Cursor Below, Add Cursor to Next Occurrence and Column Select Up / Down / Left
/ Right.
