# blameline

**Semantic git blame with structured output.** An enhanced `git blame` that shows
*why* lines changed, not just who and when:

- **Groups** consecutive lines introduced by the same commit into a single entry.
- Carries the **full commit message**, not just the short hash.
- **Tree-sitter powered**: narrow blame to the function or block enclosing a line.
- **Structured output**: every group serializes to
  `{lines, commit_hash, message, author, date}` via [`serde`], so the result is one
  `serde_json::to_string` away — ready to pipe into another tool or an AI agent.
- Works for any bundled tree-sitter language with clear function syntax (Rust,
  Python, JavaScript/TypeScript, Go, C, …).

```rust
use std::path::Path;

// Whole-file semantic blame, grouped by commit.
let groups = blameline::blame_file(Path::new("."), Path::new("src/parser.rs"))?;
println!("{}", blameline::to_json(&groups)?);

// Blame narrowed to the function enclosing line 42 (0-based).
let source = std::fs::read_to_string("src/parser.rs")?;
let scoped = blameline::blame_function(Path::new("."), Path::new("src/parser.rs"), &source, 42)?;
# Ok::<(), blameline::BlameError>(())
```

## Git backend

blameline uses [`gix`](https://crates.io/crates/gix) (gitoxide) with its `blame`
feature — pure Rust, no external `git` process required.

## Versioning

> **blameline does _not_ share the karet workspace's lockstep version.**

The rest of the [karet](https://github.com/getkono/karet) workspace releases every
crate in lockstep under one synchronized version. blameline is different: it is a
**standalone, externally-reusable library** with its own identity, so it follows its
**own [SemVer](https://semver.org) line starting at `1.0.0`** and releases on its own
cadence. From `1.0.0` onward it commits to semantic-versioning stability guarantees:
breaking changes bump the major version independently of the rest of the workspace.

Only the *version* is independent — shared metadata (edition, license, repository)
is still inherited from the workspace.

## License

MIT OR Apache-2.0, matching the workspace.
