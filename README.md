# karet

A Cargo workspace of reusable primitives ("engines") for building TUI code editors,
plus the `karet` application that composes them.

## `karet` — terminal git-diff viewer

The `karet` binary is a fast terminal viewer for your git diff:

```bash
karet [PATH]        # diff the repo containing PATH (default: current directory)
karet --staged      # force the staged diff (HEAD vs index)
karet src/main.rs   # scope the diff to a single path
```

With no flags it shows the staged changes if any are staged, otherwise the unstaged
(working-tree) changes — like VS Code's default. It prints a message and exits if
`PATH` isn't in a git repository or there's nothing to show. In the viewer: `j`/`k`
scroll, `h`/`l` switch file, `Tab` toggles unified / side-by-side, `q` quits. Syntax
highlighting is tree-sitter-based (Rust, Python, JS/TS, Go, Java, C/C++, C#, Ruby, PHP,
HTML, CSS, YAML, JSON, TOML, Bash); the detected language is shown in the status bar,
and unknown/unsupported languages render as plaintext. `--no-syntax` (or `NO_COLOR`)
disables highlighting.

## Prerequisites

- [Rust (rustup)](https://rustup.rs) — toolchain (pinned via `rust-toolchain.toml`)
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
  (`version.workspace = true`). Eight of them are published to crates.io — `karet-core`,
  `karet-filetype`, `karet-treesitter`, `karet-diff`, `karet-lsp`, `karet-dap`, `karet-vcs`,
  `karet-search` — and the rest are `publish = false`. See the crate table in
  [`AGENTS.md`](AGENTS.md) for the full breakdown.
- **[`blameline`](crates/blameline) is a standalone library on its own SemVer line** (from
  `1.0.0`), published on an independent cadence; see [its README](crates/blameline/README.md).

## License

Licensed under either of [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE) at your option.
