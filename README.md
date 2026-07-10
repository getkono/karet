# karet

`karet` is a TUI for high-velocity, terminal-centric coding, focused on review
and visualization tools. It is an application that should feel more like a GUI in
the terminal: spatial, keyboard-first, and composed from reusable Rust libraries.

This repository is also a Cargo workspace of reusable primitives ("engines") for
building TUI code editors and coding tools, plus the `karet` application that
composes them.

## Who is it for?

- **`karet`** — a high-velocity TUI application for terminal-centric coding
  workflows, especially review and visualization.
- **The `karet-*` libraries** — reusable, presentation-free building blocks for
  coding tools, so downstream consumers can pick the pieces they need without
  inheriting the full application.

## Design principles

One idea runs through the workspace: **accommodate where it counts, and be
opinionated where a sane default serves everyone.** We stay flexible on what a
downstream consumer actually feels — engines are **headless** (no `ratatui` unless
you opt into a `view` feature), keep a **minimal dependency footprint** so you can
pick a small subset, emit **neutral models** that any renderer can consume, and
depend only on **pure-Rust** crates. And we stay opinionated where choice is just
surface area — a crate must **earn its existence** through real standalone reuse,
**publishing** is a stricter bar than merely being a separate crate, we **commit to
one best backend** (tree-sitter for syntax), and the quality floor (nightly rustfmt,
no `unwrap`/`expect`/`panic` in libraries) is **non-negotiable**. See
[`AGENTS.md`](AGENTS.md) for the full treatment.

## `karet` — terminal coding TUI

The `karet` binary currently opens review-oriented terminal workflows around a
workspace path:

```bash
karet [PATH]        # open the repo or workspace containing PATH
karet --staged      # start from the staged diff (HEAD vs index)
karet src/main.rs   # scope review to a single path
```

For git review, it shows staged changes if any are staged, otherwise the unstaged
(working-tree) changes — like VS Code's default. It prints a message and exits if
`PATH` is not in a git repository or there is nothing to show. In the viewer: `j`/`k`
scroll, `h`/`l` switch file, `Tab` toggles unified / side-by-side, `q` quits.
Syntax highlighting is tree-sitter-based (Rust, Python, JS/TS, Go, Java, C/C++,
C#, Ruby, PHP, HTML, CSS, YAML, JSON, TOML, Bash); the detected language is shown
in the status bar, and unknown/unsupported languages render as plaintext.
`--no-syntax` (or `NO_COLOR`) disables highlighting.

On a Markdown file, `Ctrl+K V` (or "Markdown: Open Preview to the Side" in the
command palette) opens a rendered preview in a pane to the right. It re-renders as
you type, and the two panes scroll together — whichever one has focus leads.

## Prerequisites

- [Rust (rustup)](https://rustup.rs) — toolchain (stable pinned in `rust-toolchain.toml`; the rustfmt-only nightly in `rust-nightly.txt`)
- [mise](https://mise.jdx.dev) — task runner and tool manager
- [hk](https://hk.jdx.dev) — git hooks manager (installed by `mise install`)
- [pkl](https://pkl-lang.org) — config language for `hk.pkl` (installed by `mise install`)
- [cargo-llvm-cov](https://github.com/taiki-e/cargo-llvm-cov) — code coverage (installed by `mise install`)

## Quick Start

```bash
mise install        # provision hk, pkl, and cargo-llvm-cov
hk install          # activate git hooks
cargo build
```

Run the editor with `cargo run -- <path>`. Icons default to Nerd Font glyphs;
pass `--icons unicode` or `--icons ascii` (or set `KARET_ICONS`) if your terminal
font lacks them. See [docs/file-formats.md](docs/file-formats.md) for the catalogue
of recognized file types, icons, and syntax-highlighting support.

## Development

| Command             | Description          |
| ------------------- | -------------------- |
| `cargo build`       | Build the crate      |
| `mise run test`     | Run tests            |
| `mise run format`   | Format code          |
| `mise run lint`     | Lint (deny warnings) |
| `mise run lint-fix` | Lint and auto-fix    |
| `mise run coverage` | Report coverage      |

Tests live in-file (`#[cfg(test)] mod tests`); test every new public item. Headless
engines carry the bulk of the coverage, widget crates render-test into a ratatui
`Buffer`, and coverage is a signal rather than a merge gate. See the per-package
[testing policy](AGENTS.md#testing-policy) in `AGENTS.md`.

## Tech Stack

- **Language:** Rust (edition 2024)
- **Task runner / tools:** mise
- **Formatter / Linter:** rustfmt + Clippy
- **Git hooks:** hk
- **Key Dependencies:** tracing, thiserror, tokio

## Git Hooks

This project uses [hk](https://hk.jdx.dev). The pre-commit hook auto-fixes formatting
and lint on staged Rust files; the pre-push hook runs format checks, Clippy, the test
suite, and a coverage report. The commit-msg hook validates the message against
[Conventional Commits](https://www.conventionalcommits.org) with
[convco](https://convco.github.io) — merge/revert-in-progress commits are exempt.

## CI/CD

GitHub Actions runs format checks, Clippy, and tests on pushes to `master` and pull
requests, plus a coverage job that uploads an `lcov.info` artifact.

## Code Coverage

This project uses [`cargo-llvm-cov`](https://github.com/taiki-e/cargo-llvm-cov) for
LLVM-based code coverage.

```bash
mise run coverage
```

### MSRV

Rust 1.90

## Versioning

Commits follow [Conventional Commits](https://www.conventionalcommits.org) (enforced by
[convco](https://convco.github.io) in the commit-msg hook and in CI on pull requests).
Version bumps, CHANGELOGs, git tags, and crates.io publishing are automated by
[release-plz](https://release-plz.dev); publishing uses crates.io
[Trusted Publishing](https://crates.io/docs/trusted-publishing) (OIDC) — no long-lived tokens.

Two release lines coexist:

- **The `karet-*` crates release in lockstep** under one synchronized workspace version
  (`version.workspace = true`). Fourteen of them are published to crates.io — `karet-core`,
  `karet-text`, `karet-treesitter`, `karet-syntax`, `karet-theme`, `karet-diff`,
  `karet-filetype`, `karet-pdf`, `karet-lsp`, `karet-dap`, `karet-vcs`, `karet-search`,
  `karet-editor`, `karet-fileview` — and the rest are `publish = false`. See the crate
  table in [`AGENTS.md`](AGENTS.md) for the full breakdown.
- **[`blameline`](crates/blameline) is a standalone library on its own SemVer line** (from
  `1.0.0`), published on an independent cadence; see [its README](crates/blameline/README.md).

## Contribution Policy

Issues will receive a response within one week. Karet tools and libraries will
remain open-source and publicly available.

## License

Licensed under either of [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE) at your option.
