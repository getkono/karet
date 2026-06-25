# karet

Primitives for TUI dev tools.

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
suite, and a coverage report.

## CI/CD

GitHub Actions runs format checks, Clippy, and tests on pushes to `master` and pull
requests, plus a coverage job that uploads an `lcov.info` artifact.

## Code Coverage

This project uses [`cargo-llvm-cov`](https://github.com/taiki-e/cargo-llvm-cov) for
LLVM-based code coverage.

```bash
mise run coverage
```

## License

Licensed under either of [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE) at your option.
