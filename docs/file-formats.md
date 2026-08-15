# Supported file formats

This is the catalogue of file formats karet recognizes. It is backed by a single
crate, [`karet-filetype`](../crates/karet-filetype), which every other crate
consumes:

- **Identity & icons** — `karet-filetype` resolves any path to a `FileType`
  with independent display, grammar, LSP, and configuration identities plus a
  `Category` and icon per `IconStyle`. The explorer and activity bar render the
  presentation metadata; parsers and servers consume only their own identity.
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
`all-languages` feature). This table mirrors the registry in
`karet-filetype/src/registry.rs` and the grammar catalog in
`karet-treesitter/src/registry.rs`; changes there must update it in the same
change.

**Programming languages**

| Language | Extensions |
|---|---|
| Rust | `rs` |
| Python | `py`, `pyi`, `pyw` |
| JavaScript / JSX | `js`, `mjs`, `cjs`, `jsx` |
| TypeScript / TSX | `ts`, `mts`, `cts`, `tsx` |
| Go | `go` |
| C | `c`, `h` |
| C++ | `cc`, `cpp`, `cxx`, `hpp`, `hh`, `hxx` |
| C# | `cs` |
| Java | `java` |
| Kotlin | `kt`, `kts` |
| Swift | `swift` |
| Scala | `scala`, `sbt`, `sc` |
| Ruby | `rb` |
| PHP | `php` |
| Lua | `lua` |
| Haskell | `hs`, `lhs` |
| OCaml | `ml`, `mli` (interface grammar) |
| Elixir | `ex`, `exs` |
| Erlang | `erl`, `hrl` |
| Dart | `dart` |
| R | `r` |
| Zig | `zig` |
| Perl | `pl`, `pm` |
| Clojure / EDN | `clj`, `cljs`, `cljc`, `edn` |
| Emacs Lisp | `el` |
| Vim script | `vim` |

**Web and markup**

| Language | Extensions |
|---|---|
| HTML / XHTML | `html`, `htm`, `xhtml` |
| CSS | `css` |
| SCSS / Sass | `scss`, `sass` |
| Less | `less` |
| Vue | `vue` |
| Svelte | `svelte` |
| Astro | `astro` |
| ERB | `erb` |
| XML / SVG | `xml`, `svg` |
| Markdown | `md`, `markdown`, `mdown`, `mkd`, `README` — layered block + inline grammar, including fenced-language injection |
| MDX | `mdx` |
| reStructuredText | `rst` |
| AsciiDoc | `adoc`, `asciidoc` |
| TeX / LaTeX | `tex`, `sty`, `cls` |

**Data, config, and build**

| Language | Extensions / filenames |
|---|---|
| JSON / JSONC / JSON5 | `json`, `jsonc`, `json5` |
| YAML | `yml`, `yaml` |
| TOML | `toml` |
| INI | `ini`, `cfg`, `conf` |
| Properties / dotenv | `properties`, `.env` |
| SQL | `sql` |
| GraphQL | `graphql`, `gql` |
| Protobuf | `proto` |
| CBOR diagnostic | `cbor` (via the CBOR text seam) |
| Dockerfile | `Dockerfile`, `Containerfile` |
| Makefile | `mk`, `Makefile`, `GNUmakefile`, `makefile` |
| CMake | `cmake`, `CMakeLists.txt` |
| Git config | `.gitmodules` (INI grammar) |
| EditorConfig | `.editorconfig` (INI grammar) |
| Lockfiles | `Cargo.lock`, `poetry.lock` (TOML) · `package-lock.json`, `composer.lock`, `Pipfile.lock` (JSON) · `pnpm-lock.yaml` (YAML) · `yarn.lock` (dedicated grammar) |

**Shells**

| Language | Extensions |
|---|---|
| Shell | `sh`, `bash` |
| Zsh | `zsh` |
| Fish | `fish` |
| PowerShell | `ps1`, `psm1` |
| Batch | `bat`, `cmd` |

## Recognized for icons / labels (no bundled grammar)

These get an icon, a display name, and renderer routing, but open as plain
(un-highlighted) text. Highlighting can be added later by wiring a grammar.

- **Pkl** — no published pure-Rust tree-sitter binding yet (see below);
  recognized with an LSP id and config selector, deliberately grammar-less.
- **Data:** CSV/TSV, generic `*.lock` files not matched by a known lockfile name.
- **Config:** git dotfiles (`.gitignore`, `.gitattributes`, `.gitkeep`).
- **Prose / docs:** plain text, `LICENSE` / `LICENCE` / `COPYING` / `AUTHORS`.
- **Shells:** Ksh.

## Non-text renderers

`classify` routes these away from the editor (by extension, confirmed by magic
bytes so a mislabeled file still routes sensibly):

| Kind | Handling | Extensions / detection |
|---|---|---|
| Image | inline image widget — Kitty graphics with a truecolor halfblock fallback (or a placeholder if it can't decode) | `png`, `jpg`, `jpeg`, `gif`, `webp`, `bmp`, `ico`, `tiff`, `tif` + magic bytes |
| PDF | pages rasterized and shown inline via the **Kitty graphics protocol** — via [`karet-pdf`](../crates/karet-pdf) (pure-Rust [`hayro`](https://github.com/LaurenzV/hayro)); on a terminal without Kitty graphics, a message explaining the requirement | `pdf` + `%PDF-` magic |
| DOCX | OOXML converted to a standalone rendered Markdown preview by the pure-Rust `karet-docx` engine | `docx` |
| Jupyter notebook | nbformat-4 converted to a standalone rendered Markdown preview by [`karet-notebook`](../crates/karet-notebook) — code cells fenced in the notebook's language, outputs text-first (images as placeholders), tracebacks ANSI-stripped. With the `notebook-kernels` feature, `Notebook: Run All Cells` executes the notebook on its discovered Jupyter kernel (kernelspec by name, then language) and streams refreshed previews per cell; interrupt is out-of-band, restart marks outputs stale, and the file on disk is never written | `ipynb` |
| CBOR | decoded to editable [diagnostic notation](https://www.rfc-editor.org/rfc/rfc8949#section-8) text and re-encoded on save (hex view if it can't decode) — via [`karet-cbor`](../crates/karet-cbor) | `cbor` + `0xD9D9F7` self-describe tag |
| Binary | hex view | NUL byte / invalid UTF-8 in the sampled head |
| Too large | placeholder, with an "open anyway" override | larger than 10 MiB |

Image decoding uses Gamut exclusively:

| Formats | Decoder | Current scope |
|---|---|---|
| PNG (`png`) | [`gamut`](https://github.com/justin13888/gamut) | still images; every PNG colour type and bit depth, including Adam7 interlacing |
| JPEG (`jpg`, `jpeg`) | [`gamut`](https://github.com/justin13888/gamut) | baseline and progressive 8-bit JPEG, converted to the shared RGBA model |
| WebP (`webp`) | [`gamut`](https://github.com/justin13888/gamut) | VP8/VP8L still images, including alpha |
| TIFF (`tiff`, `tif`) | [`gamut`](https://github.com/justin13888/gamut) | baseline 8-bit grayscale/RGB/RGBA and palette; supported strip/tile compression modes |

The shared Kitty/halfblock raster path no longer depends on a codec library; PDF
pixels use the same built-in RGBA resampler. GIF, BMP, and ICO are still
classified as images by extension or magic bytes, but fall back to the image
placeholder because Gamut does not publish those codecs. AVIF/JPEG XL/HEIC are
not advertised yet because the published Gamut versions do not currently provide
the pure-Rust decode path karet needs (and no C `*-sys` dependency is permitted).

The **Image** and **PDF** renderers are optional, default-on Cargo features
(`images` and `pdf` on the `karet` app; `raster`/`images`/`pdf` on
`karet-fileview`). Building the app with `--no-default-features` drops their heavy
dependency trees (Gamut, `hayro`) and routes those kinds to the
placeholder branch instead — see [binary-size.md](binary-size.md). Classification
(`FileKind`) is unaffected; only rendering degrades.

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
- **Per-segment clicks on compacted folders** — a compacted `a/b/c` row toggles
  as a unit; clicking an individual segment is a future refinement.
