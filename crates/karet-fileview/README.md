# karet-fileview

One read-only [ratatui] widget that renders **any** file — syntax-highlighted code
(tree-sitter), raster images (Kitty graphics with a truecolor halfblock fallback),
binaries (hex dump), and a graceful placeholder for PDFs / oversized / undecodable
files — behind a single dispatch.

It is the one crate an external consumer imports to drop a file preview or reader
into their TUI, without pulling the full editor toolkit (fuzzy matching, file tree,
LSP popups).

## Usage

Split the work into an expensive **prepare** (run once) and a cheap **render** (per
frame):

```rust
use karet_fileview::{FileDoc, FileView, FileViewState, Limits};

// Once, when the file is opened:
let bytes = std::fs::read(path)?;
let len = bytes.len() as u64;
let doc = FileDoc::prepare(path, &bytes, len, &Limits::default());
let mut state = FileViewState::new();

// Each frame:
frame.render_stateful_widget(FileView::new(&doc).theme(&theme), area, &mut state);
```

`FileViewState` provides `scroll_down`/`scroll_up`/`page_down`/`page_up`/
`scroll_to_top`/`center_on` for paging; the active branch scrolls without the caller
needing to know the file's kind. Search matches (or any overlay) are supplied as
`karet_core::Decoration`s via `FileView::decorations`.

`Limits { max_bytes, highlight_line_budget }` bounds size and highlighting per
context — e.g. a small inline preview vs. a full-file reader.

## Images

Halfblock rendering (the default) is fully self-contained. For pixel-perfect Kitty
graphics, select the protocol and flush after drawing:

```rust
use karet_fileview::{flush_kitty_image, image::{detect_protocol, GraphicsProtocol}};

let protocol = detect_protocol();
frame.render_stateful_widget(FileView::new(&doc).graphics(protocol), area, &mut state);
// after terminal.draw(...):
if protocol == GraphicsProtocol::Kitty {
    flush_kitty_image(&doc, &state, &mut std::io::stdout())?;
}
```

## Features

- `all-languages` — compile in every tree-sitter grammar so the text branch
  highlights. Off by default; without it (or a per-language feature) text still
  renders, just unhighlighted.

## Notes

- **Markdown** renders as highlighted **source** (a rendered-markdown model is not
  wired up yet).
- Images use karet's own Kitty / halfblock backend (not `ratatui-image`).
- Built against `ratatui` 0.30 on edition 2024; MSRV 1.90.

[ratatui]: https://crates.io/crates/ratatui
