# karet

A **Cargo workspace** (edition 2024): a toolkit of reusable primitives for
building TUI code editors, plus the `karet` application that composes them.

## Goal: independent, reusable libraries

Every crate except the `karet` app is a **standalone library with a stable public
API** and a **minimal dependency footprint**, so a consumer can depend on a small
subset (e.g. "highlight a snippet", "diff two files", "render markdown") without
pulling in unrelated heavy deps. Only the **app** is exempt from these rules.

- **Engines are headless** (no `ratatui`); any TUI widget an engine offers lives
  behind an off-by-default **`view`** feature. Exception: `karet-widgets` and
  `karet-editor` *are* widgets, so `ratatui` is a hard dependency.
- **Decouple by inversion**: producers (`karet-lsp`, `karet-vcs`, `karet-dap`)
  emit neutral `karet-core` models (`Decoration`, `Diagnostic`, `Symbol`);
  renderers (`karet-editor`, `karet-widgets`) consume them. Only the app connects
  a producer to a widget — no widget crate depends on a producer crate.
- **Logic vs presentation**: the headless `karet-session` backend owns the
  documents/workspace and runs the producers, exposing a `Command`/`Event`
  vocabulary and a `Backend` trait. Presentation (the app, `karet-editor`,
  `karet-widgets`) talks to it only through that seam, keeping the deferred
  client-server split additive rather than a rewrite.
- A piece with no standalone reuse is a **module inside a larger crate**, not its
  own crate — e.g. terminal-image rendering, the keymap engine, the clipboard.
- **Published to crates.io**: `karet-core`, `karet-filetype`, `karet-treesitter`,
  `karet-diff`, `karet-lsp`, `karet-dap`, `karet-vcs`, `karet-search`. Everything
  else is `publish = false`.

## Versioning

All crates share one version (`version.workspace = true`) and release in
**lockstep**. Internal deps use `{ path, version }` so each library is
independently publishable yet version-synced. Common metadata (license, repo,
edition, …) and all dependency versions are centralized in `[workspace.package]`
and `[workspace.dependencies]`.

## Crates

Engines are headless; `view` = ratatui widget behind the `view` feature; **pub** =
published to crates.io (everything else is `publish = false`).

| crate | role | pub | one-line scope |
|---|---|---|---|
| `karet-core` | foundation | ✓ | shared vocabulary: geometry, text coords, neutral models (Diagnostic/Decoration/Symbol/Completion/Hover/…), neutral edits, `SymbolProvider`, `TokenId` |
| `karet-filetype` | engine | ✓ | single registry: path → file type (name, category, per-`IconStyle` icon) + renderer routing (`FileKind`/`classify`); dependency-free |
| `karet-text` | engine | — | rope buffer, undo/redo, dirty/save, large-file mmap, cursors & selections (module) |
| `karet-treesitter` | engine | ✓ | shared tree-sitter parse host (parser pool, incremental trees, queries) |
| `karet-syntax` | engine | — | tree-sitter highlighting, fold regions, bracket pairs, structural selection |
| `karet-theme` | engine | — | token palette, .tmTheme + VS Code JSON loaders, contrast (`view` feat) |
| `karet-diff` | engine | ✓ | pure syntax-aware diffing (tree-sitter + line/word fallback) — no presentation |
| `karet-markdown` | engine | — | markdown render model (`view` + `highlight` feats) |
| `karet-terminal` | engine | — | VT/PTY emulator, scrollback, OSC 133 (`view` feat) |
| `karet-lsp` | engine | ✓ | async LSP client → core models (headless; ratatui popups live in `karet-widgets`) |
| `karet-dap` | engine | ✓ | async DAP client → breakpoint decorations (`view` feat = panels) |
| `karet-vcs` | engine | ✓ | git status/blame/branches/staging → decorations (`view` feat = SCM panels) |
| `karet-search` | engine | ✓ | in-file + workspace search/replace (ripgrep-style; no karet deps) |
| `karet-fuzzy` | engine | — | fuzzy match + frecency + quick-open query parsing |
| `karet-session` | backend | — | headless editor backend: owns documents/workspace, orchestrates producers, applies `Command`s, emits `Event`s; holds format-on-save, spell-check, settings/session |
| `karet-widgets` | widget | — | ratatui UI toolkit: file tree, picker/palette, outline+breadcrumbs, status bar, dialogs, dock, problems, pane layout, hex view, terminal image, LSP completion/hover popups |
| `karet-editor` | widget | — | the editor widget: gutter, minimap, scroll, visual aids, snippets (modules) |
| `karet` | app | — | composition root / TUI client (local mode); merges the clipboard + input (keymap) modules; `publish = false` |

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
  `thiserror`-derived types (clippy-enforced). The `karet` app opts out.
- Keep `clippy` clean at `-D warnings`; `#[allow(...)]` needs a justifying comment.
- Tests live in-file under `#[cfg(test)] mod tests`; test every new public item.
- ratatui rendering goes behind the `view` feature; never make a headless engine
  depend on `ratatui` unconditionally.
