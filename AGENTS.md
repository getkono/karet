# karet

Primitives for TUI dev tools. A Rust library crate (edition 2024).

## Packages

- **tracing** — structured diagnostics for instrumenting primitive operations.
- **thiserror** — derive `std::error::Error` for this crate's public error types.
- **tokio** — async runtime for any async primitives the crate exposes.

## Quality

Validate changes:

```bash
mise run test       # correctness
cargo fmt -- --check # formatting
mise run lint        # lint (clippy, deny warnings)
mise run coverage    # coverage (cargo-llvm-cov)
```

## Conventions

- This is a **library**: every `pub` item needs a doc comment, and `pub` functions
  returning values should be marked `#[must_use]` where ignoring the result is a likely bug.
- No `unwrap`/`expect`/`panic!` in library code paths — surface errors through `thiserror`-derived types.
- Keep `clippy` clean at `-D warnings`; do not add `#[allow(...)]` without a comment explaining why.
- Tests live in the same file under `#[cfg(test)] mod tests`; keep coverage non-zero by testing every new public item.
