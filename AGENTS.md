# karet

A **Cargo workspace** (edition 2024) that is a toolkit of reusable primitives for
building TUI code editors, plus the `karet` application that composes them.

## Goal: independent, reusable libraries

Every crate except the `karet` application is a **standalone library with a stable
public API** and a **minimal dependency footprint**, so a consumer can depend on a
small subset for a narrow use case (e.g. "highlight a code snippet", "diff two
files", "render markdown") without pulling in unrelated heavy dependencies.

- **Engines are headless** (no `ratatui`). Any TUI widget an engine offers lives
  behind an off-by-default **`view`** feature, so headless consumers get zero
  `ratatui` in their tree. (`karet-widgets` and `karet-editor` are the exception:
  they *are* widgets, so `ratatui` is a hard dependency.)
- **Cross-feature decoupling by inversion**: producers (`karet-lsp`, `karet-vcs`,
  `karet-dap`) emit neutral `karet-core` models (`Decoration`, `Diagnostic`,
  `Symbol`); renderers (`karet-editor`, `karet-widgets`) consume them. Only the
  application connects a producer to a widget — no widget crate depends on a
  producer crate.
- A piece that has no standalone reuse is a **module inside a larger crate**, not a
  crate of its own (avoid boilerplate crates).
- **Tree-sitter is the sole syntax backend** (no multiple-backend abstraction).
- Only the **application** is exempt from the stable-API + minimal-deps rules.

## Versioning

All crates share one version via `version.workspace = true` and are released in
**lockstep** at the workspace version. Internal dependencies use
`{ path = ..., version = ... }` so each library is independently publishable while
staying version-synced. Common metadata (license, authors, repository, edition,
keywords, …) and all dependency versions are centralized in `[workspace.package]`
and `[workspace.dependencies]`.

## Crates

Engines are headless; `view feat` = ratatui widget behind the `view` feature.

| crate | role | one-line scope |
|---|---|---|
| `karet-core` | foundation | shared vocabulary: geometry, text coords, neutral models (Diagnostic/Decoration/Symbol), `SymbolProvider`, `TokenId` |
| `karet-text` | engine | rope buffer, undo/redo, dirty/save, large-file mmap, **cursors & selections** (module) |
| `karet-treesitter` | engine | shared tree-sitter parse host (parser pool, incremental trees, queries) |
| `karet-syntax` | engine | tree-sitter highlighting, **fold regions**, bracket pairs, structural selection |
| `karet-theme` | engine | token palette, .tmTheme + VS Code JSON loaders, contrast (`view` feat) |
| `karet-diff` | engine | pure syntax-aware diffing (tree-sitter + line/word fallback) — no presentation |
| `karet-markdown` | engine | markdown render model (`view` + `highlight` feats) |
| `karet-image` | engine | terminal image rendering: halfblocks/Kitty/Sixel/iTerm2 (`view` feat) |
| `karet-terminal` | engine | VT/PTY emulator, scrollback, OSC 133 (`view` feat) |
| `karet-lsp` | engine | async LSP client → core models (`view` feat = popups) |
| `karet-dap` | engine | async DAP client → breakpoint decorations (`view` feat = panels) |
| `karet-vcs` | engine | git status/blame/branches/staging → decorations (`view` feat = SCM panels) |
| `karet-search` | engine | in-file + workspace search/replace (ripgrep-style) |
| `karet-fuzzy` | engine | fuzzy match + frecency + quick-open query parsing |
| `karet-input` | engine | keymap engine: modal modes, chords, scoping, rebinding |
| `karet-clipboard` | engine | OSC 52 + external clipboard fallbacks |
| `karet-widgets` | widget | ratatui UI toolkit: file tree, picker/palette, outline+breadcrumbs, status bar, toasts, dialogs, dock, which-key, problems, **pane layout**, **hex view** |
| `karet-editor` | widget | the editor widget: **gutter, minimap, scroll, visual aids, snippets** (modules) |
| `karet` | app | composition root / demo editor (folds in **format-on-save, spell-check, settings/session**); `publish = false` |

## Quality

Validate changes (tasks run workspace-wide):

```bash
mise run test       # cargo test --workspace --all-features
cargo fmt -- --check # formatting
mise run lint        # cargo clippy --workspace --all-targets --all-features -D warnings
mise run coverage    # cargo llvm-cov --workspace
```

## Conventions

- Each library is a **library**: every `pub` item needs a doc comment (enforced via
  the workspace `missing_docs` lint), and `pub` functions returning values should be
  `#[must_use]` where ignoring the result is a likely bug.
- No `unwrap`/`expect`/`panic!` in library code paths — surface errors through
  `thiserror`-derived types (enforced via workspace clippy lints). The `karet`
  application opts out of these strict lints.
- Keep `clippy` clean at `-D warnings`; do not add `#[allow(...)]` without a comment
  explaining why.
- Tests live in the same file under `#[cfg(test)] mod tests`; keep coverage non-zero
  by testing every new public item.
- ratatui rendering goes behind a crate's `view` feature; never make a headless
  engine depend on `ratatui` unconditionally.
