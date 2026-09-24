# Scope — deliberate non-goals

What karet has decided **not** to build, so the decision is written down once
instead of being relitigated per issue. Each row is a standing decision: the
feature is out of scope until this document says otherwise. (Pattern borrowed
from `dependable`'s `docs/SCOPE.md`.)

For what karet *is*, see the [README](../README.md) feature tour and the
[docs index](README.md).

## TUI theming

**Out of scope.** karet ships one built-in dark theme and loads
[VS Code JSON themes](configuration.md#workbench) via `workbench.colorTheme` —
that is the whole theming surface.

Deliberately not built:

- **TextMate `.tmTheme` loading.** VS Code JSON is the one blessed interchange
  format; a second loader would double the palette-mapping surface for a format
  the target audience has largely migrated off. (Earlier docs claimed `.tmTheme`
  support; no such loader ever shipped, and the claim is now retired.)
- **A karet-native theme format.** Author themes for VS Code, load them here.
- **Chrome/UI theming beyond the palette** — configurable borders, chrome
  layouts, per-panel styling. The `TokenId`/`ThemeRole` vocabulary in
  `karet-core` (32 tokens, 30 roles) is the complete customization surface, and
  widgets resolve every color through it.
- **A light built-in theme.** Load a light VS Code theme instead; the contrast
  checker in `karet-theme` keeps it legible.

## Terminal graphics

Kitty graphics protocol plus a truecolor half-block fallback, detected at
runtime — **sixel and iTerm2 protocols are out of scope**
(`karet-fileview/src/image.rs` states this at the module level), as is
`ratatui-image` (its build script needs the system C library `chafa`, which the
[no-system-deps rule](../AGENTS.md#design-principles) forbids).

## Markdown preview

**No HTML layout engine.** The preview renders CommonMark with GitHub tables, task
lists and strikethrough, and maps a curated subset of embedded HTML onto the same
render model — text formatting, links, images, headings, lists, `<details>`, and
horizontal alignment. [file-formats.md](file-formats.md#markdown-preview) catalogues
exactly what renders how. Rendering HTML "positionally correct"
([#295](https://github.com/getkono/karet/issues/295)) was weighed and declined: no
pure-Rust engine lays HTML out onto a terminal grid, and embedding a browser-grade one
would outweigh the editor and break the
[minimal-dependency and one-backend rules](../AGENTS.md#design-principles).

Deliberately not built:

- **Positional layout beyond alignment.** `align="center"`/`"right"` and `<center>`
  are honoured; floats, `style=`/CSS, tables used for layout, and side-by-side
  columns are not. Images in one paragraph stack vertically, and HTML `<table>` and
  `<pre>` read as plain text. Tags outside the subset keep their text and lose their
  markup.
- **Remote images.** The preview never makes a network request: an `http(s)` image —
  a CI badge — renders as a `🖼 alt` chip that links to it. Opening a file performs no
  network I/O, and a README must not be able to phone home through an image.
- **Active content.** `<script>`, `<style>`, `<iframe>` and `<object>` vanish with
  their content; forms and embedded media are not interactive.
- **Kitty graphics in the preview.** Preview images are truecolor half-blocks. A Kitty
  placement cannot be clipped to the rows of a scrolling pane, and re-transmitting it
  on every scroll would stall the pane. Ctrl/Cmd-click an image to open it in the
  image tab, which keeps [Kitty](#terminal-graphics).
- **Images the preview will not load** render as chips: SVG, GIF, BMP, ICO and any
  other format Gamut does not decode; absolute paths and anything resolving outside
  the workspace (`../`, symlinks); files over the 10 MiB guard or images over
  4096×4096 pixels.
- **Watching image files.** A changed image is picked up on the preview's next
  re-render (an edit, a resize), not by watching the file.

Accepted rough edges, not bugs:

- An image takes up to 20 preview lines but a single source line, so the two panes'
  scroll sync jumps across it.
- A TIFF — or a JPEG whose frame header lies past its first 64 KiB — reserves its rows
  only once decoded, so the layout shifts once.
- Chips are clickable in the preview only; hover popups, dialogs and the GitHub
  surfaces render images as chip text. A lean build (`--no-default-features`)
  renders every image as a chip.

This reopens on **proven demand** for a specific construct, which is then added to
the subset — not on the arrival of an HTML engine.

Not affected: the image tab, PDF pages, and the notebook and DOCX previews (whose
converters already reduce embedded images to placeholders).

## Syntax backends

Tree-sitter only. No syntect, no TextMate grammars, no dual-backend
abstraction — see "commit to one best backend" in
[AGENTS.md](../AGENTS.md#design-principles).

## Agent sessions (ACP)

**Out of scope.** karet does not run coding agents: no ACP client crate, no
session daemon, no Agents/Agent views. The
[Agent Client Protocol](https://agentclientprotocol.com/) epic
([#193](https://github.com/getkono/karet/issues/193) and its sub-issues) was
designed in full and closed unbuilt — a daemon, a harness registry, a
bidirectional request seam, and a PTY karet does not have add more surface than
the editor core itself, for demand nobody has demonstrated. Run the agent in a
terminal beside karet. In-karet Git worktree management went out with the epic;
it existed only as the agent substrate, and would have to earn its way back on
its own merits.

This reopens on **proven demand**, not on protocol news. The closed issues keep
the full design if that day comes.

Not affected: the prerequisites that already landed and stand on their own —
`karet-jsonrpc` (including its `LineDelimited` framing), the transcript /
`TextArea` / dialog / spinner widgets, and the `View` layer above tabs.
