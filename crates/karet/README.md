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

**Editor:** arrows move the caret (`Shift`+arrows extend the selection) · `j`/`k`
scroll · `Space`/`PageDown`, `b`/`PageUp` page · `g`/`G` top/bottom · `Ctrl+C`/`y`
copy the selection (or the cursor line) · `Esc` back to the sidebar. In a **diff**
tab: `\` toggles unified/side-by-side, `[` / `]` walk changed files.

**Overlays / find:** type to filter, `↑`/`↓` (or `Ctrl+N`/`Ctrl+P`) move, `Enter`
accept, `Esc` dismiss; find adds `Ctrl+G` / `Ctrl+Shift+G` for next/previous. The
command palette shows each command's shortcut on the right.

**Mouse** (every element is interactive): click a tab to switch, its `×` (or
middle-click) to close, drag to reorder; click explorer rows to open files /
toggle folders and the header `1 2 3` to switch panels; click SCM / search rows to
open them; click to place the caret and drag to select text (double / triple-click
select word / line); the wheel scrolls; click a status-bar segment to run it.

The keymap is a single unit-tested binding table (`keymap.rs`) that drives both the
resolver and the palette's shortcut hints; all named operations are `Command`s
(`command.rs`).

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
