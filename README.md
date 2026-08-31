# karet

<p align="center">
  <img src="assets/karet.svg" alt="A karet window: the view switcher along the top, the file explorer on the left, a Rust source file with syntax highlighting in the editor, and the status bar along the bottom" width="100%">
</p>

> **Status: beta (v0.5.0).** The editor is usable for day-to-day terminal coding.
> Pre-1.0: the `karet-*` library APIs may still change on a minor bump.

`karet` is a terminal IDE: a keyboard-first code editor that behaves less like a
pager and more like a GUI in the terminal — an explorer, panes and tabs, source
control, language servers, a debugger, and a command palette, all in the cells your
terminal already has.

It is built from **`karet-*`**, a Cargo workspace of headless Rust libraries that are
useful on their own. Highlight a snippet, diff two files, read a repository's git
facts, rasterize a PDF page — each is a small crate with a stable API and a minimal
dependency footprint, and none of them drags an editor in with it.

## Install

**mise** ([mise-en-place](https://mise.jdx.dev)) — the released binary: no clone, no Rust
toolchain, no C compiler. karet is not in mise's registry, so name the `ubi` backend:

```bash
mise use -g ubi:getkono/karet            # latest release, on PATH
mise use -g ubi:getkono/karet@0.6.0      # or pin a version
```

`mise use -g ubi:getkono/karet@latest` moves a pinned install up again. To pin karet for
one project rather than globally, put it in that repository's `mise.toml`:

```toml
[tools]
"ubi:getkono/karet" = "0.6.0"
```

**Homebrew**

```bash
brew install getkono/tap/karet
```

**Prebuilt binaries** — every [release](https://github.com/getkono/karet/releases)
attaches a tarball per target, each holding a single `karet` executable:

| Platform | Asset |
| --- | --- |
| macOS (Apple silicon) | `karet-aarch64-apple-darwin.tar.gz` |
| macOS (Intel) | `karet-x86_64-apple-darwin.tar.gz` |
| Linux (aarch64, musl) | `karet-aarch64-unknown-linux-musl.tar.gz` |
| Linux (x86_64, musl) | `karet-x86_64-unknown-linux-musl.tar.gz` |

The Linux builds are static musl binaries and run on glibc distributions too. No Windows
binary is prebuilt — build from source there.

**From source** — no clone needed for this either; needs Rust 1.92+ and a C compiler
(tree-sitter and its grammars compile vendored C):

```bash
cargo install --git https://github.com/getkono/karet --locked karet
```

The app is not published to crates.io — only the `karet-*` libraries are — so there is no
`cargo install karet`; the `--git` form above is its equivalent. Name the `karet` package
explicitly, because the workspace holds three binary targets, and keep `--locked` so the
build uses the committed `Cargo.lock` — the exact dependency set `mise run verify` gates.

karet expects a modern terminal (the kitty keyboard protocol, and kitty graphics for
inline images). Check yours, and see what degrades:

```bash
karet --doctor
```

`karet --install-desktop` adds a per-user launcher — an XDG `.desktop` entry on
Linux, a `~/Applications/karet.app` bundle on macOS, a Start-Menu entry on Windows
10/11 — and `--uninstall-desktop` removes it again.

## Getting started

```bash
karet                  # open the current directory
karet path/to/repo     # open a workspace
karet src/main.rs      # open a single file
```

karet opens an Explorer-first shell rooted at the given path. A file opens directly;
a git repository's changes fill the Source Control panel as the backend reports them.
A path outside a repository is fine — you just get no Source Control.

### Startup flags

| Flag | What it does |
| --- | --- |
| `--view <editor\|github\|agents>` | Top-level view to start in |
| `--startup-panel <explorer\|search\|source-control\|none>` | Sidebar panel at startup, overriding `workbench.startupPanel` |
| `--focus <sidebar\|editor>` | Where focus lands once startup is done |
| `--open <PATH>` | Open an extra file as a tab (repeatable) |
| `--preview <PATH>` | Open one preview tab without moving focus |
| `--icons <nerd\|unicode\|ascii>` | Icon style; also read from `KARET_ICONS`. Defaults to Nerd Font glyphs — pass `unicode` or `ascii` if your font lacks them |
| `--no-syntax` | Disable syntax highlighting (`NO_COLOR` does the same) |
| `--doctor` | Print terminal-capability diagnostics and exit |
| `--log` | Print the paths of existing log files and exit |

### Keys worth knowing

Every binding lives in one table (`crates/karet/src/keymap/bindings.rs`) and follows
VS Code where VS Code has an answer.

| Key | Action |
| --- | --- |
| `Ctrl+Shift+P` / `F1` | Command palette (`F1` for terminals that swallow the chord) |
| `Ctrl+P` | Quick open |
| `Ctrl+B` / `Ctrl+Shift+O` | Toggle the sidebar / the outline |
| `Ctrl+1`…`Ctrl+6` | Explorer, Search, Source Control, Spelling, Todos, Debug |
| `Ctrl+F` / `Ctrl+Shift+F` | Find in file / search the workspace |
| `Ctrl+\` / `Ctrl+K Ctrl+\` | Split right / split down |
| `Ctrl+Tab` / `Ctrl+W` | Next tab / close tab |
| `Ctrl+K 1/2/3` | Switch top-level view |
| `Ctrl+K S` / `Ctrl+K V` | Seam view / Markdown preview to the side |
| `F5` / `F9` | Start debugging / toggle breakpoint |
| `Ctrl+Q` | Quit |

In a diff, `\` toggles inline versus side-by-side, `]` / `[` move between changed
files, and `s` / `u` stage and unstage the hunk under the caret.

### Scripting

A few flags exist for automation and view capture rather than for hands:
`--goto PATH[:LINE[:COL]]`, `--diff OLD NEW`, `--split PATH`, `--command NAME` (runs a
palette command by title or slug), `--seam-query QUERY` (prints JSON and exits without
entering the TUI), and `--capture` (renders one frame to stdout as truecolor ANSI).
Each is an **unstable automation surface**: the behaviour and output shape may change
between major versions without notice.

## What you get

**Editing.** Multi-caret editing, code folding, sticky scroll, word wrap, and
merge-conflict decorations, across panes, splits, and tabs.

**Navigation.** An explorer with the full file-management set (new, rename, delete,
cut/copy/paste, duplicate, context menu), a symbol outline, quick open, workspace
find-and-replace with regex, case, and whole-word matching, and a Todos panel that
collects codetags across the tree.

**Source control.** Status and per-hunk staging, commit and compare views, stash,
the branch lifecycle (create, switch, publish, sync), rebase, cherry-pick, revert,
reset, and blame — both inline and as a detail view. The commit log renders as a
lane-based DAG rather than a flat list; see [`docs/visualizations.md`](docs/visualizations.md).

**Language intelligence.** Tree-sitter highlighting with 60+ bundled grammars,
including injected languages, folds, and outlines — the per-extension table is in
[`docs/file-formats.md`](docs/file-formats.md). Language servers are managed from
inside the editor (**Language Servers: Manage**) against an explicit, machine-local
registry, and shared across editor windows by a cross-process broker; the support
matrix is in [`docs/language-servers.md`](docs/language-servers.md). Servers coupled
to a project SDK are marked manual — karet bundles no compiler or toolchain. A DAP
debugger with breakpoints, stepping, and variable inspection is wired end to end
([`docs/debugging.md`](docs/debugging.md)).

**The Seam view** (`Ctrl+K S`). Read a repository by its *seams* rather than its
files: one navigable tree of what is exposed, what can be swapped, what varies before
compile, what crosses the package line, and where that is dangerous. A Cargo
workspace, its nested crates, and Python packages beside them become a single tree
with a root per package. The same query language drives both the filter box and
`--seam-query`, so a narrowing you reach by pressing keys and one a script asks for
are the same string. See [`docs/seam.md`](docs/seam.md).

**Documents and media.** Markdown preview to the side, re-rendering as you type with
the two panes scroll-synced and mermaid diagrams drawn inline; Jupyter notebooks with
kernel execution; PDF pages; DOCX as read-only markdown; images inline; and a hex
view for everything else. LaTeX has tree-sitter highlighting plus a **LaTeX: Build and
Open PDF Preview** workflow around a local `latexmk`.

**Spellcheck.** Off by default. Install an `en_US` or `en_GB` Hunspell dictionary and
enable it in `setting.jsonc`; the Spelling panel (`Ctrl+4`) then lists every
misspelling across the workspace, and selecting one opens its file at the word.

## Configuration

Settings live in `setting.jsonc` across three layers, with per-language overrides.
[`docs/configuration.md`](docs/configuration.md) is the per-key reference.

| Document | What it answers |
| --- | --- |
| [`configuration.md`](docs/configuration.md) | Every setting, the three layers, per-language overrides |
| [`file-formats.md`](docs/file-formats.md) | What opens how: grammars per extension, media and document formats |
| [`language-servers.md`](docs/language-servers.md) | The canonical language-support matrix and managed installs |
| [`debugging.md`](docs/debugging.md) | The DAP debugger: adapters, configurations, keys, breakpoints |
| [`seam.md`](docs/seam.md) | The Seam view and its query language |
| [`visualizations.md`](docs/visualizations.md) | Graph lenses and their status |
| [`scope.md`](docs/scope.md) | Deliberate non-goals |
| [`binary-size.md`](docs/binary-size.md) | What the optional features cost, measured |

Diagnostics are written to daily `karet.log.*` files in the platform state directory;
the seven most recent are kept, `RUST_LOG` tunes verbosity, and `karet --log` prints
the paths.

## The `karet-*` toolkit

One idea runs through the workspace: **accommodate where it counts, and be
opinionated where a sane default serves everyone.**

Flexible on what a consumer actually feels — engines are **headless** (no `ratatui`
unless you opt into a `view` feature), keep a **minimal dependency footprint** so you
can take a small subset, emit **neutral models** any renderer can consume, and depend
only on **pure-Rust** crates with no system libraries. Opinionated where choice is
just surface area — a crate must **earn its existence** through real standalone reuse,
**publishing** is a stricter bar than merely being a separate crate, we **commit to
one best backend** (tree-sitter for syntax), and the quality floor is
**non-negotiable**.

Fourteen `karet-*` crates carry a published API — thirteen are on crates.io today,
and `karet-jsonrpc`, the newest, lands with the next release:

| Crate | Scope |
| --- | --- |
| [`karet-core`](crates/karet-core) | Shared vocabulary: text coordinates, neutral models, neutral edits |
| [`karet-text`](crates/karet-text) | Rope buffer, undo/redo, EOL/encoding detection, atomic save |
| [`karet-treesitter`](crates/karet-treesitter) | Shared parse host: parser pool, incremental trees, language injection |
| [`karet-syntax`](crates/karet-syntax) | Highlighting, folds, semantic blocks, symbol outlines |
| [`karet-theme`](crates/karet-theme) | Token palette, VS Code JSON theme loader, contrast checking |
| [`karet-diff`](crates/karet-diff) | Histogram line diff, side-by-side alignment, per-hunk staging |
| [`karet-filetype`](crates/karet-filetype) | Path → file type and renderer routing; dependency-free |
| [`karet-pdf`](crates/karet-pdf) | Pure-Rust PDF page rasterization |
| [`karet-jsonrpc`](crates/karet-jsonrpc) | Protocol-agnostic JSON-RPC 2.0 client core |
| [`karet-lsp`](crates/karet-lsp) | Async LSP client emitting neutral models |
| [`karet-vcs`](crates/karet-vcs) | Git facts: `gix` reads, hardened `git`-CLI writes |
| [`karet-search`](crates/karet-search) | In-file and workspace search/replace, gitignore-aware walk |
| [`karet-editor`](crates/karet-editor) | The editor widget: gutter, folds, sticky scroll, multi-caret |
| [`karet-fileview`](crates/karet-fileview) | Read-only file views: hex, terminal image, placeholder |

The rest of the workspace — the session backend, widget toolkit, seam index, process
supervisor, FS watcher, fuzzy matcher, and the markdown, notebook, DOCX, CBOR, DAP,
GitHub, and graph engines — is `publish = false` for now.
[`AGENTS.md`](AGENTS.md) carries the full crate table and the reasoning behind each
line above.

[`blameline`](crates/blameline) is the deliberate exception: a standalone semantic
git-blame library on its own SemVer line, not `karet`-branded and with no `karet`
coupling in its public API. See [its README](crates/blameline/README.md).

## Development

**Prerequisites**

- [Rust (rustup)](https://rustup.rs) — the stable toolchain is pinned in
  `rust-toolchain.toml`, the rustfmt-only nightly in `rust-nightly.txt`
- A C compiler — tree-sitter grammars compile vendored C
- [mise](https://mise.jdx.dev) — task runner and tool manager
- [hk](https://hk.jdx.dev), [pkl](https://pkl-lang.org), and
  [cargo-llvm-cov](https://github.com/taiki-e/cargo-llvm-cov) — installed by
  `mise install`

```bash
mise install        # provision hk, pkl, and cargo-llvm-cov
hk install          # activate the git hooks
cargo build
cargo run -- .      # run karet against this repo
```

Three commands share the word *install* and mean different things: `mise install`
provisions the tools above, `mise run install` builds *this checkout* into your path
(`cargo install --path crates/karet --locked`), and `mise use -g ubi:getkono/karet` — the
one in [Install](#install) — fetches a released binary and needs no checkout at all.

**Tasks**

| Command | Description |
| --- | --- |
| `mise run test` | `cargo test --workspace --all-features` |
| `mise run format` / `format-check` | rustfmt on the pinned nightly |
| `mise run lint` / `lint-fix` | Clippy, denying warnings |
| `mise run file-lines` | Enforce the 800-code-line ceiling per `.rs` file |
| `mise run build-lean` | Build the app and `karet-fileview` across feature subsets |
| `mise run coverage` | Generate `lcov.info` and print the summary |
| `mise run verify` | **The merge gate** — the whole chain below, in order |
| `mise run install` | `cargo install --path crates/karet --locked` — this checkout onto your path |
| `mise run svg` | Recapture the README hero image |

`mise run verify` is one composite task, run identically by CI and the pre-push hook
so the two cannot drift: `file-lines` → `publish-closure` → `publish-ready` →
`format-check` → `lint` → `test` → `build-lean` → `coverage`.

**Quality floor.** Every `pub` item is documented (`missing_docs`); no
`unwrap`/`expect`/`panic!` in library code — errors surface through `thiserror` types.
The app opts out of the docs lint. Tests live in-file under `#[cfg(test)] mod tests`;
headless engines carry the bulk of the coverage and widget crates render-test into a
ratatui `Buffer`. Coverage is a signal, not a merge gate. The per-crate
[testing policy](AGENTS.md#testing-policy) has the details.

**The hero image** is a real frame, not a drawing. `mise run svg` builds `karet`, runs
it with `--capture` against a throwaway demo repository, and converts the ANSI grid
into `assets/karet.svg`. Pinned content, branch, and commit dates plus an empty
environment make it byte-identical on every run — see `scripts/gen-svg.sh`.

**Git hooks** ([hk](https://hk.jdx.dev)): pre-commit checks the file-line ceiling and
auto-fixes formatting and Clippy on staged Rust; pre-push runs `mise run verify`;
commit-msg validates [Conventional Commits](https://www.conventionalcommits.org) with
[convco](https://convco.github.io), exempting merges and reverts in progress.

**CI** runs the Conventional Commits check on pull requests, then `mise run verify`
plus a grammar feature-subset `cargo check`, and uploads `lcov.info`. Pull requests
stacked on another feature branch are covered too.

**MSRV** is Rust 1.92 (the workspace `rust-version`).

## Versioning and releases

All `karet-*` crates share one workspace version and release in **lockstep**;
`blameline` runs its own SemVer line on an independent cadence.

Version bumps, CHANGELOGs, git tags, GitHub Releases, and crates.io publishing are
automated by [release-plz](https://release-plz.dev). Publishing uses crates.io
[Trusted Publishing](https://crates.io/docs/trusted-publishing) (OIDC) — no long-lived
tokens. A release also cross-compiles the `karet` binary for four targets, attaches
them to a `v{version}` release, and refreshes the Homebrew tap.

## Contribution policy

Issues will receive a response within one week. Karet tools and libraries will remain
open-source and publicly available.

## License

Licensed under either of [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE) at your
option.
