# karet-fileview

Read-only [ratatui] file-view **primitives**: a terminal image renderer (Kitty
graphics with a truecolor halfblock fallback), a hex dump for binaries, and a
graceful placeholder for oversized / undecodable files, plus the
`classify`/`FileKind` re-exports from `karet-filetype` to route a path to the
right primitive.

It is the crate an external consumer imports to drop a file preview into their
TUI without pulling the full editor toolkit (fuzzy matching, file tree, LSP
popups). Composition — which primitive draws which `FileKind`, and how text is
highlighted — stays with the consumer: pair the `viewer::classify` result with
your own text/editor widget for code, `HexView` for binaries, and
`image::ImageWidget` for pictures.

## Usage

```rust
use karet_fileview::viewer::{classify, FileKind, Placeholder};
use karet_fileview::HexView;

let bytes = std::fs::read(path)?;
match classify(path, bytes.len() as u64, &bytes) {
    FileKind::Binary => { /* render HexView::new(&bytes) */ },
    FileKind::Image => { /* render image::ImageWidget (feature `images`) */ },
    FileKind::TooLarge => { /* render Placeholder */ },
    _ => { /* your text/editor rendering */ },
}
```

## Images

Halfblock rendering is fully self-contained. For pixel-perfect output, probe the
terminal with `image::detect_protocol()` and use the Kitty escape path.

## Features

All off by default, so a consumer that only renders hex or placeholders pulls no
codec dependencies.

- `raster` — the shared terminal-image primitives (Kitty escape + halfblock
  rendering). Enabled automatically by `images`.
- `images` — decode & render raster image **files** (adds the pure-Rust
  PNG/JPEG/WebP/TIFF codecs on top of `raster`).

## Notes

- Images use karet's own Kitty / halfblock backend (not `ratatui-image`).
- Built against `ratatui` 0.30 on edition 2024.

[ratatui]: https://crates.io/crates/ratatui
