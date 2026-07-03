# Code visualizations

karet renders relationships in your code and history as compact, TUI-native graphs.
Every visualization emits the same neutral model — `karet_core::GraphView` (nodes,
directed edges, roots) — and is drawn by one backend, `karet-graph`, so the lenses share
layout, colour, and navigation. That shared backbone is the cohesion: a producer just
builds a `GraphView`; the renderer does the rest.

## The suite — lenses of granularity

The visualizations are designed top-down, from coarse package structure to fine
function usage, plus an orthogonal history lens:

| Lens | Granularity | Backend | Answers | Status |
|---|---|---|---|---|
| **Workspace dependency map** | package / crate | `dependable-core` (offline, from `Cargo.lock`) | "how do the packages depend on each other?" | ✅ shipped |
| **Reverse-dependency / impact** | package / crate | `dependable` inverted, rooted at a package | "what breaks if I change this package?" | 🧭 designed |
| **Symbol usage / call graph** | function / symbol | tree-sitter `tags`, resolved across the workspace | "who calls this / what does this call, downstream?" | 🧭 designed |
| **Commit-history DAG** | commit / branch | `gix` (parents → lanes) | "how did the branches and merges actually flow?" | ✅ shipped |

`GraphView` already carries the vocabulary for all four — `GraphNodeKind::{Package,
Module, Symbol, External, Commit}` and `GraphEdgeKind::{Dependency, Call, Contains,
Parent}` — and the session seam exposes a `GraphKind::{Dependency, Usage}`, so the
designed lenses are additive: a new producer that emits a `GraphView`, not a new
renderer.

## Commit-history DAG (shipped)

The Source-Control panel renders the commit log as a **lane-based DAG** instead of a
flat list. `karet-vcs` captures each commit's parents; `karet-graph::assign_lanes` walks
the commits (newest first) and produces one rail gutter per row, drawn to the left of
the existing hash / summary / age columns.

Glyphs: `●` a commit, `◉` `HEAD`, `◆` a merge (2+ parents); `│` a rail, `─` a
connector, `╭ ╮ ╰ ╯` rounded corners where a lane opens (a branch/merge) or folds back.
Each lane gets a stable colour so parallel branches read apart. A branch-and-merge
history renders like:

```
◉─╮  e4f1a2  feat(pdf): page scroll bar          Justin  2h
● │  529b7c  Merge #18 editor-refinements         Justin  5h
│ ●  9c8d7e  feat(app): unified outline sidebar    Justin  6h
●─╮  9569303 feat(editor): caret restore on undo   Justin  7h
  ●  37da4ba fix(editor): undo/redo caret          Justin  8h
```

## Workspace dependency map (shipped)

Run **Visualize: Dependency Graph** from the command palette. The session's `viz`
module parses the workspace `Cargo.lock` into a `GraphView` — one node per resolved
package (local crates vs. external registry/git deps, versioned via the node badge),
one edge per resolved dependency — entirely offline (no network). It opens in a
read-only pane as a scrollable, cycle-safe indented tree rooted at the workspace's local
crates; nodes already expanded elsewhere are marked `⟲` rather than re-expanded.

This is language-agnostic by design: `dependable` supports ~10 ecosystems, and the
`GraphView` container is ecosystem-neutral. Only the offline *builder* is Rust-specific
today (see the upstream requests below).

## Symbol usage / call graph (designed)

Given a function, this lens shows every function/site that uses it downstream — a
semantically-aware, language-agnostic view backed by tree-sitter `tags` queries (each
grammar's `TAGS_QUERY`, which most karet grammars ship). Definitions and references are
extracted per file and resolved across the workspace into a `GraphView` of
`GraphNodeKind::Symbol` nodes and `GraphEdgeKind::Call` edges, rendered through the same
rail/tree renderer rooted at the focused symbol.

The neutral model and the `GraphKind::Usage` seam are in place; landing this lens needs
a match-grouped tree-sitter query pass in `karet-treesitter` (to pair each `@name`
capture with its `@definition.*` / `@reference.*`) plus the workspace tags walk — a
self-contained follow-up on top of this backbone.

## Upstream `dependable` requests

To deepen the dependency lens (tracked upstream on `getkono/dependable`); karet works
against the current API meanwhile:

1. **serde on the graph model** (`DependencyGraph` / `Node` / `Tree`) so consumers need
   not re-walk or shell out.
2. **Library-level JSON/DOT export** — move the CLI-private graph DTOs into the public
   API.
3. **Richer graph queries**: `dependents_of`, `dependencies_of`, `reachable_from`,
   cycle/SCC detection, topological order (powers the impact lens and cycle highlighting).
4. **Multi-ecosystem graph builders** (npm / pyproject / go → `DependencyGraph`) so the
   dependency map is language-agnostic, not Rust-only.
5. **An offline graph builder in `dependable-core`** (IO-free) so structure-only graphs
   never pull the `dependable-fetch` network stack — which is exactly why karet builds
   the graph from the parsed lockfile itself today.
