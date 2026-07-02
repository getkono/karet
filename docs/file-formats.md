# Supported file formats

This is the catalogue of file formats karet recognizes. It is backed by a single
crate, [`karet-filetype`](../crates/karet-filetype), which every other crate
consumes:

- **Identity & icons** — `karet-filetype` resolves any path to a `FileType`
  (display name, `Category`, and an icon per `IconStyle`). The explorer and
  activity bar render those glyphs.
- **Renderer routing** — `karet-filetype::classify` returns a `FileKind` that
  decides which widget opens a file (editor / image / hex / placeholder).
- **Syntax highlighting** — tree-sitter grammars live in
  [`karet-treesitter`](../crates/karet-treesitter), gated behind `lang-*`
  features; `karet-filetype` supplies display names for languages without a
  bundled grammar.

To add or change a format, edit the `REGISTRY` table in
`crates/karet-filetype/src/registry.rs` (one line per type) and, if it should be
highlighted, add a grammar in `crates/karet-treesitter`.

## Icon styles

Chosen with `--icons <nerd|unicode|ascii>` (or the `KARET_ICONS` env var);
the default is **Nerd Font**.

| Style | Files | Directories | Notes |
|---|---|---|---|
| `nerd` (default) | per-file-type glyph | chevron + folder glyph | needs a [Nerd Font](https://www.nerdfonts.com/) |
| `unicode` | per-category geometric glyph | chevron only | 1-cell BMP symbols, widely supported |
| `ascii` | blank | `>` / `v` chevron | maximally portable |

## Syntax highlighting (bundled tree-sitter grammars)

These extensions highlight via a compiled-in grammar (the `karet` app enables the
`all-languages` feature):

| Language | Extensions |
|---|---|
| Rust | `rs` |
| Python | `py`, `pyi` |
| JavaScript | `js`, `mjs`, `cjs`, `jsx` |
| TypeScript / TSX | `ts`, `mts`, `cts`, `tsx` |
| JSON | `json`, `jsonc` |
| Go | `go` |
| C | `c`, `h` |
| C++ | `cc`, `cpp`, `cxx`, `hpp`, `hh`, `hxx` |
| C# | `cs` |
| Java | `java` |
| Ruby | `rb` |
| PHP | `php` |
| Bash | `sh`, `bash` |
| TOML | `toml` |
| HTML | `html`, `htm` |
| CSS | `css` |
| YAML | `yml`, `yaml` |
| Markdown | `md`, `markdown`, `mdown`, `mkd` — *block grammar only* (headings, code fences, lists); inline emphasis/links are not yet highlighted |

## Recognized for icons / labels (no bundled grammar)

These get an icon, a display name, and renderer routing, but open as plain
(un-highlighted) text. Highlighting can be added later by wiring a grammar.

- **Languages:** Kotlin, Swift, Scala, Lua, Haskell, OCaml, Elixir, Erlang, Dart,
  R, Zig, Perl, Clojure, Emacs Lisp, Vim script, SQL, GraphQL, Protobuf,
  PowerShell, Batch.
- **Config / data:** **Pkl** (no published Rust tree-sitter binding yet — see
  below), INI/cfg/conf, `.properties`, XML, SVG, CSV/TSV, lockfiles,
  `Dockerfile`/`Containerfile`, `Makefile`/`GNUmakefile`/`CMakeLists.txt`, git
  config dotfiles, `.editorconfig`, `.env`.
- **Web:** Less, Vue, Svelte.
- **Prose / docs:** reStructuredText, AsciiDoc, TeX, plain text, `LICENSE` /
  `README` / `AUTHORS`.

## Non-text renderers

`classify` routes these away from the editor (by extension, confirmed by magic
bytes so a mislabeled file still routes sensibly):

| Kind | Handling | Extensions / detection |
|---|---|---|
| Image | inline image widget — Kitty graphics with a truecolor halfblock fallback (or a placeholder if it can't decode) | `png`, `jpg`, `jpeg`, `gif`, `webp`, `bmp`, `ico`, `tiff`, `tif` + magic bytes |
| PDF | pages rasterized and shown inline via the **Kitty graphics protocol** — via [`karet-pdf`](../crates/karet-pdf) (pure-Rust [`hayro`](https://github.com/LaurenzV/hayro)); on a terminal without Kitty graphics, a message explaining the requirement | `pdf` + `%PDF-` magic |
| DOCX | placeholder — visual rendering is pending a pure-Rust rasterizer (see below) | `docx` |
| CBOR | decoded to editable [diagnostic notation](https://www.rfc-editor.org/rfc/rfc8949#section-8) text and re-encoded on save (hex view if it can't decode) — via [`karet-cbor`](../crates/karet-cbor) | `cbor` + `0xD9D9F7` self-describe tag |
| Binary | hex view | NUL byte / invalid UTF-8 in the sampled head |
| Too large | placeholder, with an "open anyway" override | larger than 10 MiB |

The 10 MiB guard is a *routing* default, not a hard limit: the too-large
placeholder offers an "open anyway" action (Enter in the TUI) that re-classifies
the file ignoring its size and opens it with the renderer its content warrants —
so a large `.cbor`, for instance, still decodes to editable diagnostic notation.
`classify_ignoring_size` is the size-independent entry point behind it.

Other office documents (`doc`/`xlsx`/…), archives (`zip`/`tar`/…), fonts, audio,
and video are given icons and labels but currently open as a binary hex view or
placeholder.

## Planned / not yet supported

- **Pkl highlighting** — pkl is recognized (icon + label) but there is no
  published `tree-sitter-pkl` Rust crate; once one exists, add a `lang-pkl`
  feature + registry entry in `karet-treesitter`.
- **DOCX rendering** — `.docx` is recognized and routed to `FileKind::Docx`, but
  visual page rendering is deferred: the only pure-Rust DOCX renderer with a
  layout engine (`rdocx`) currently pulls in the C library `zstd-sys` transitively
  (via its `zip` dependency), which conflicts with karet's pure-Rust policy. When a
  pure-Rust DOCX rasterizer is available, a `karet-docx` engine can rasterize pages
  into the same Kitty-graphics path as PDF.
- **Inline Markdown highlighting** — only `tree-sitter-md`'s block grammar is
  wired; the inline grammar (emphasis, links) needs the multi-grammar injection
  path.
- **Rich rendered Markdown** — the `karet-markdown` render model is still a
  skeleton; Markdown opens as highlighted source for now.
- **Per-segment clicks on compacted folders** — a compacted `a/b/c` row toggles
  as a unit; clicking an individual segment is a future refinement.
