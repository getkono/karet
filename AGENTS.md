# karet

A **Cargo workspace** (edition 2024): a toolkit of reusable primitives for
building TUI code editors, plus the `karet` application that composes them.

## Goal: independent, reusable libraries

Every crate except the `karet` app is a **standalone library with a stable public
API** and a **minimal dependency footprint**, so a consumer can depend on a small
subset (e.g. "highlight a snippet", "diff two files", "render markdown") without
pulling in unrelated heavy deps. Only the **app** is exempt from these rules. The
principles below say how we hold that line.

## Design principles

One framing runs through every decision: **accommodate where it counts, and be
opinionated where a sane default serves everyone**. We stay flexible on the axes a
downstream consumer actually feels — what they depend on, what they render with —
and we refuse to relitigate the axes where choice only adds surface area.

**Accommodate where it counts** — the reuse promise; keep the consumer free:

- **Minimal dependency footprint** — a consumer depends on a *small subset*.
  Internal deps stay lean so a narrow use case ("diff two strings", "highlight a
  snippet") never drags in heavy unrelated crates. Mind the publish closure:
  `cargo publish` forces every dep — including optional `view`-feature deps — to be
  published too, so the published set must be dependency-closed.
- **Headless by default** — engines carry no `ratatui`; presentation is the
  consumer's choice. Any TUI widget an engine offers lives behind an off-by-default
  **`view`** feature, so headless consumers get zero ratatui.
- **Neutral models, decoupled by inversion** — producers (`karet-lsp`, `karet-vcs`,
  `karet-dap`) emit neutral `karet-core` models (`Decoration`, `Diagnostic`,
  `Symbol`); renderers (`karet-editor`, `karet-widgets`) consume them. Only the app
  wires a producer to a widget — no widget crate depends on a producer crate.
- **Serde-ready `Backend` seam** — the headless `karet-session` backend owns the
  documents/workspace and runs the producers behind a `Command`/`Event` vocabulary
  and a `Backend` trait. Presentation talks to it only through that seam. The
  remote client-server split is deferred, but the seams (serde-ready core models,
  in-proc `Backend`) are pre-placed so it lands as an *additive* change, not a rewrite.
- **Pure-Rust dependencies** — no C `*-sys` deps (verify with `cargo tree`), so
  consumers get clean, portable, cross-compilable builds.

**Opinionated where sane defaults** — one blessed way; shrink the decision surface:

- **A crate must earn its existence** through genuine standalone reuse *and* a
  distinct separation of concerns. A piece with no independent reuse story is a
  **module inside a larger crate**, not its own crate — e.g. terminal-image
  rendering, the keymap engine, the clipboard. Over-fragmentation is just boilerplate.
- **Publishing is a stricter bar than crate-existence** — internal-but-separate is
  fine; crates.io requires a real *external-consumer* story. Gate `publish`
  conservatively: unset on the publishable few, `publish = false` on the rest.
- **Commit to one best backend** rather than a multiple-backend abstraction — e.g.
  tree-sitter only for syntax, not a syntect+tree-sitter dual path.
- **Lockstep versioning** for the `karet-*` crates (one workspace version); a truly
  standalone library (`blameline`) may run its own SemVer line — the deliberate
  exception, not the rule.
- **Quality is non-negotiable** — nightly rustfmt, `missing_docs`, and clippy
  denying `unwrap`/`expect`/`panic` in library code (the app opts out). This floor
  is workspace policy, not a per-crate choice. See [Quality](#quality).

## Versioning

All `karet-*` crates share one version (`version.workspace = true`) and release in
**lockstep**. Internal deps use `{ path, version }` so each library is
independently publishable yet version-synced. Common metadata (license, repo,
edition, …) and all dependency versions are centralized in `[workspace.package]`
and `[workspace.dependencies]`.

The lone exception is **`blameline`**, a standalone library on its own SemVer line
(from `1.0.0`) published on an independent cadence — it is not `karet`-branded and
carries no `karet` coupling in its public API, so lockstep would only get in its way.

## Crates

Engines are headless; `view` = ratatui widget behind the `view` feature; **pub** =
published to crates.io (everything else is `publish = false`).

| crate | role | pub | one-line scope |
|---|---|---|---|
| `karet-core` | foundation | ✓ | shared vocabulary: geometry, text coords, neutral models (Diagnostic/Decoration/Symbol/Completion/Hover/…), neutral edits, `SymbolProvider`, `TokenId` |
| `karet-filetype` | engine | ✓ | single registry: path → file type (name, category, per-`IconStyle` icon) + renderer routing (`FileKind`/`classify`); dependency-free |
| `karet-text` | engine | ✓ | rope buffer, undo/redo, dirty/save, large-file mmap |
| `karet-treesitter` | engine | ✓ | shared tree-sitter parse host (parser pool, incremental trees, queries) |
| `karet-syntax` | engine | ✓ | tree-sitter highlighting, fold regions, bracket pairs, structural selection |
| `karet-theme` | engine | ✓ | token palette, .tmTheme + VS Code JSON loaders, contrast (`view` feat) |
| `karet-diff` | engine | ✓ | pure syntax-aware diffing (tree-sitter + line/word fallback) — no presentation |
| `karet-graph` | engine | — | DAG lane-assignment layout + rail renderer (`view` feat) for the commit graph & code visualizations; consumes `karet_core::GraphView` |
| `karet-markdown` | engine | — | markdown render model (`view` + `highlight` feats) |
| `karet-cbor` | engine | — | CBOR decode/encode ↔ editable diagnostic-notation text (via `ciborium`); no presentation |
| `karet-pdf` | engine | ✓ | pure-Rust PDF page → RGBA rasterization (via `hayro`); no presentation |
| `karet-terminal` | engine | — | VT/PTY emulator, scrollback, OSC 133 (`view` feat) |
| `karet-lsp` | engine | ✓ | async LSP client → core models (headless; ratatui popups live in `karet-widgets`) |
| `karet-dap` | engine | ✓ | async DAP client → breakpoint decorations (`view` feat = panels) |
| `karet-vcs` | engine | ✓ | git status/blame/branches/staging → decorations (`view` feat = SCM panels) |
| `karet-search` | engine | ✓ | in-file + workspace search/replace (ripgrep-style; no karet deps) |
| `karet-watch` | engine | — | debounced cross-platform FS-watch → neutral `FsEvent` Tokio stream; enumerates off-thread (headless) |
| `karet-fuzzy` | engine | — | fuzzy match + frecency + quick-open query parsing |
| `karet-session` | backend | — | headless editor backend: owns documents/workspace, orchestrates producers, applies `Command`s, emits `Event`s; holds format-on-save, spell-check, settings/session |
| `karet-widgets` | widget | — | ratatui UI toolkit: file tree, picker/palette, outline+breadcrumbs, status bar, dialogs, dock, problems, pane layout, LSP completion/hover popups |
| `karet-editor` | widget | ✓ | the editor widget: gutter, minimap, scroll, visual aids, snippets (modules); `read_only` pager mode |
| `karet-fileview` | widget | ✓ | read-only "render any file" widget: dispatches `FileKind` → editor/hex/image/placeholder; hosts the hex view + terminal image; Markdown renders as source |
| `karet` | app | — | composition root / TUI client (local mode); merges the clipboard + input (keymap) modules; `publish = false` |
| `blameline` | standalone | ✓ | semantic git-blame (via `gix`): group lines by commit, tree-sitter function narrowing, serde/JSON output; headless, on its **own** SemVer line (see [Versioning](#versioning)) |

## Quality

Validate changes (tasks run workspace-wide):

```bash
mise run test        # cargo test --workspace --all-features
cargo fmt -- --check # formatting
mise run lint        # cargo clippy --workspace --all-targets --all-features -D warnings
mise run coverage    # cargo llvm-cov --workspace
```

## Conventions

- Every `pub` item needs a doc comment (`missing_docs` lint); add `#[must_use]` on
  value-returning fns where ignoring the result is a likely bug.
- No `unwrap`/`expect`/`panic!` in library code — surface errors via
  `thiserror`-derived types (clippy-enforced, tests included). The `karet` app opts
  out. In tests, use `?`, `unwrap_or_default()`, or `assert!` instead.
- Keep `clippy` clean at `-D warnings`; `#[allow(...)]` needs a justifying comment.
- ratatui rendering goes behind the `view` feature; never make a headless engine
  depend on `ratatui` unconditionally.

## Testing policy

Pragmatic and keyed to what a crate *is* — we spend test effort where it buys
correctness and stay light where it would only tax velocity.

- **Baseline (every crate):** tests live in-file under `#[cfg(test)] mod tests`;
  **test every new public item**. Reach for a `tests/` integration dir only to
  exercise a public API across the crate boundary (as `blameline` and `karet-cbor`
  do). `tempfile` is the sanctioned scratch-tree dev-dep — no snapshot/property
  framework is mandated; adding `insta`/`proptest` is a case-by-case call.
- **Headless engines** (`karet-core`, `karet-text`, `karet-diff`, `karet-syntax`,
  `karet-vcs`, …): the primary test investment. Logic is pure and cheap to cover —
  aim for complete unit coverage of the model and its edge cases strictly using high-value test cases and for regression using justified adversarial tests.
- **`view`-feature engines:** test the **headless model exhaustively**; the optional
  renderer gets light `Line`/`Buffer`-level tests **at most**, run under
  `--features view` (see `karet-graph`). A thin, lightly-tested renderer is an
  **accepted tradeoff** — don't block on it.
- **Widget crates** (`karet-editor`, `karet-widgets`, `karet-fileview`): rendering
  *is* the product — render-test by rendering into a ratatui `Buffer` and asserting
  on cells (fg/bg/`Modifier`); test any extractable headless logic directly.
- **Backend (`karet-session`):** cover the `Command`/`Event` contract — commands in,
  events/state out — since that seam is the API everything downstream depends on.
- **App (`karet`):** module-level unit tests (already dense) are the norm; a
  black-box binary smoke test is a **known gap** — welcome, not required.
- **Coverage** (`mise run coverage`) is a **signal, not a gate** — no numeric
  threshold blocks a merge. Judgment over a percentage.
