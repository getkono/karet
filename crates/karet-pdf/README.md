# karet-pdf

> Headless, pure-Rust PDF page rasterization for karet.

Wraps the [`hayro`](https://crates.io/crates/hayro) PDF interpreter/renderer (pure
Rust, no C `*-sys` dependencies) to turn PDF bytes into pages of straight
(un-premultiplied) 8-bit RGBA pixels. A renderer such as `karet-fileview` can then
hand those pixels to the Kitty graphics protocol (or a halfblock fallback). The crate
is headless — no ratatui, no terminal — so a PDF can be turned into pixels anywhere.

Parsing happens once in `Document::load`; pages are rasterized on demand via
`Document::render_page`, so a large document is not fully rendered up front. A
document outline (bookmarks) is available via `Document::outline`.

```rust
# fn demo(bytes: Vec<u8>) -> Result<(), karet_pdf::PdfError> {
let doc = karet_pdf::Document::load(bytes)?;
for i in 0..doc.page_count() {
    let page = doc.render_page(i, 2.0)?; // 2× the native 72-DPI size
    assert_eq!(
        page.rgba().len(),
        page.width() as usize * page.height() as usize * 4,
    );
}
# Ok(())
# }
```

Part of the [karet](https://github.com/getkono/karet) workspace.

## License

Licensed under either of MIT or Apache-2.0 at your option.
