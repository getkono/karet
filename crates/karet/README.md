# karet

The **karet application**: an Explorer-first TUI IDE skeleton that composes the
`karet-*` toolkit. It is the composition root — every feature is a thin wiring of
a headless engine or a widget; the app owns only state and layout.

```
karet [PATH]
```

`PATH` (default `.`) is the workspace root. A file opens directly in a code
window; a directory opens the shell rooted there. When the path is inside a git
repository, the **Source Control** panel lists the staged and working-tree changes
and each opens as a diff tab.

> **Modern terminal required.** karet uses the **kitty keyboard protocol** and
> **truecolor**, and exits early with a clear message if the keyboard protocol is
> unavailable (kitty, ghostty, WezTerm, foot, …). Images use the **kitty graphics
> protocol** when available, falling back to truecolor halfblocks.

## Layout

```
┌ tabs ───────────────────────────────────────────────┐
│  main.rs   logo.png   [diff: app.rs]                 │
├ breadcrumb (collapses when the tab has no path) ─────┤
│  crates › karet › src › app.rs                       │
├──────────────┬──────────────────────────────────────┤
│ EXPLORER 1·2·3│  1 //! …                             │
│  ▾ crates     │  2                                    │  ← main area
│   ▾ karet     │     (the active tab)                  │     (code / diff /
│     app.rs  M │                                       │      image / hex /
├──────────────┴──────────────────────────────────────┤      placeholder)
│ SIDEBAR   ^P open · ^F find · ^C copy · ^Q quit    rs│  ← status bar
└──────────────────────────────────────────────────────┘
```

A single switchable **sidebar** (Explorer / Search / Source Control) sits beside
the **main area**. Overlays (quick-open, command palette) draw centered on top; the
find bar is a one-line strip above the main area.

### File-type dispatch (the code window)

Opening a file classifies it (`karet-widgets::viewer::classify`) and picks a
renderer — everything unsupported fails gracefully:

| kind | renderer |
|---|---|
| text / code | read-only `karet-editor` with tree-sitter highlights |
| markdown | the editor (highlighted **source**; rich preview is deferred) |
| image (png/jpg/gif/webp/bmp) | kitty graphics → halfblocks → placeholder |
| PDF | placeholder (name / size — rasterization is deferred) |
| binary | `karet-widgets::HexView` |
| too large (>10 MiB) | placeholder |

Image and PDF rendering are optional, **default-on** features (`images`, `pdf`).
Build with `cargo build --no-default-features` to drop their heavy dependency
trees (`image` codecs, `hayro`); those kinds then show the placeholder instead.
See [docs/binary-size.md](../../docs/binary-size.md).

## Keymap

**Global:** `Ctrl+P` quick-open · `Ctrl+Shift+P` (or `F1`) command palette · `Ctrl+F`
find in file · `Ctrl+Shift+F` workspace search · `Ctrl+B` toggle sidebar ·
`Ctrl+1/2/3` Explorer/Search/Source Control · `Ctrl+C` copy · `Tab` switch focus ·
`Ctrl+Q` quit (`q` also quits in a viewer).

> **Modifiers, terminals & SSH.** karet uses `Ctrl` as its single primary modifier.
> This is deliberate and platform-agnostic: a terminal never receives the macOS
> `Cmd` key — the emulator consumes it — so `Ctrl` is the only modifier a TUI can
> rely on locally or over SSH (where the client platform is unknown). Some emulators
> also capture certain `Ctrl+Shift+…` chords for their own shortcuts before the app
> sees them; **`Ctrl+Shift+P` is the most common casualty** — use **`F1`** for the
> command palette instead, or click the ` +N` overflow marker at the right of the
> status bar (it opens the palette, keeping every command reachable). All bindings
> live in one table (`keymap/mod.rs`), the single place to change or rebind them.

**Tabs:** `Ctrl+Tab` / `Ctrl+Shift+Tab` (or `Ctrl+PageDown`/`Ctrl+PageUp`) next /
previous · `Ctrl+Shift+PageUp`/`PageDown` move left / right · `Alt+1`…`9` go to tab
(`9` = last) · `Ctrl+W` close · `Ctrl+Shift+T` reopen closed. Close Others / Close
to the Right / Close All live in the command palette.

**Sidebar:** `j`/`k`+arrows move · `Enter`/`l`/`→` open or expand · `h`/`←`
collapse · `Space` toggle a directory.

**Editor — motion & selection.** Arrows move the caret; `Ctrl+←`/`Ctrl+→` move by
word; `Home`/`End` go to the line start/end and `Ctrl+Home`/`Ctrl+End` to the
document edges; `PageUp`/`PageDown` page. Hold **`Shift`** with any of these to
*extend* the selection, and `Ctrl+A` selects the whole file. `Ctrl+C` copies the
selection (or the cursor line). `Esc` collapses multiple carets to one, then returns
focus to the sidebar. In a **diff** tab, `\` toggles unified/side-by-side. In diff,
commit, and compare tabs, `[` / `]` walk changed files; commit and compare file-index
rows are also clickable.

**Editor — multi-cursor.** `Ctrl+Alt+↑`/`Ctrl+Alt+↓` add a caret above / below the
primary; `Ctrl+D` selects the word under the caret, then adds a caret at the next
occurrence (wrapping); `Alt+Click` adds or removes a caret and `Alt+Drag` extends the
newly-added one; `Esc` collapses back to a single caret. Typing, deletion and
newlines then apply at every caret at once.

Modifier conventions: **`Shift`** always *extends* the selection (paired with any
motion key or click); **`Ctrl`** widens the granularity (word for arrows, document
for `Home`/`End`, whole file for `A`); **`Alt`** drives pointer multi-cursor
(`Alt+Click`/`Alt+Drag`), and with `Ctrl` stacks carets vertically.

**Overlays / find:** type to filter, `↑`/`↓` (or `Ctrl+N`/`Ctrl+P`) move, `Enter`
accept, `Esc` dismiss; find adds `Ctrl+G` / `Ctrl+Shift+G` for next/previous. The
in-file find bar has the same find/replace model as the Search panel — `Alt+H`
toggles the replace field, `Tab` switches find/replace, `Enter` finds the next
match (or replaces the current one from the replace field), `Alt+Enter` replaces
all, and `Alt+R`/`Alt+C`/`Alt+W` toggle regex/case/whole-word. The command palette
shows each command's shortcut on the right.

**Search panel:** the find and replace fields show by default (`Alt+H` collapses the
replace field). `Tab` switches find/replace; `Enter` runs the search (or, in the
replace field, replaces all matches across the workspace). Option toggles —
`Alt+R` regex, `Alt+C` case-sensitive, `Alt+W` whole-word — are also clickable
`.*` / `Aa` / `\b` buttons on the find row; `r` (browsing results) or the ` ⟳ all`
button replaces everywhere.

**Mouse** (every element is interactive): click a tab to switch, its `×` (or
middle-click) to close, drag to reorder; click explorer rows to open files /
toggle folders and the header `1 2 3` to switch panels; click SCM / search rows to
open them; click to place the caret and drag to select text (double / triple-click
select word / line); `Alt+Click`/`Alt+Drag` add and grow extra carets; the wheel
scrolls; click a status-bar segment to run it.

**Where these live (source of truth).** Every key and mouse interaction is defined in
a few files, so there is one place to read or change each behavior:

- **Keybindings** — the single `BINDINGS` table in `keymap/mod.rs`; it drives both
  the resolver and the palette's shortcut hints, so a binding and its displayed hint
  can never drift.
- **Commands** — every named operation a binding fires is a `Command` in
  `command.rs`.
- **Mouse** — click / drag / multi-click handling lives in `app.rs`
  (`handle_editor_click`, `drag_select_to`, and the `handle_mouse` dispatch).
- **Caret & selection model** — the multi-caret `EditorState` in the `karet-editor`
  crate (`lib.rs`), built on `karet_core::CursorState`.

## Architecture notes

- The shell calls the engines directly (`karet-text`, `karet-search`,
  `karet-treesitter`/`karet-syntax`, `karet-vcs`). Routing through the headless
  `karet-session` backend is additive future work — its `Command`/`Event`
  variants already map onto open / save / search.
- Diff rendering is reused from `render.rs` (the original diff viewer).

## Deferred (documented TODOs)

- **Editing** (insert/delete/undo/save) — the code window is read-only; needs
  `karet-text` edits + history + `karet-session`. Text selection + copy already
  work (read-only), via mouse drag / `Shift`+arrows and `Ctrl+C`.
- **Diff drag-select & copy** — code tabs support it; diff tabs are keyboard-first.
- **Kitty image lifecycle** across scroll/resize — minimal active-tab transmit.
- **Rich markdown preview** (`karet-markdown`), **PDF rasterization**.
- **Parallel workspace search** and **replace**; **explorer git-status overlay**.
- **`karet-fuzzy` ranking** for quick-open / palette (currently subsequence).
```
