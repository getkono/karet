# The Seam view

karet reads a package by its **seams** rather than its files.

A seam — generalizing Feathers — is any location where behavior can be observed,
substituted, or varied *without editing the code at that location*. Reading a package
that way answers a different set of questions than reading it file by file: what is
exposed, what can be swapped, what varies before compiling, what crosses the package
line, and where that is dangerous.

Open it with **Seam: Open Seam View** in the command palette, or `Ctrl+K S`. That reads
the workspace root. To read something narrower — one crate, a subdirectory, a Python
project beside the Rust — use **Seam: Open Seam View at…**, or press `r` from inside the
view, and pick from the start points on offer: the workspace root, the explorer selection,
the directory of the file you are in, and every package found under the root.

One index sits behind the view, so opening at another start point re-points the view you
have rather than opening a second beside it.

```
 Seam  karet-core   config: default @ x86_64-linux   1◉ api  2◊ sub  3⌥ var  4⇥ bnd  5☡ haz
 package          │ module           │ item                 │ member
 karet-core   ▸ 47│ model         ▸12│ Symbol           ◉ 6 │ name              ◉
                  │ coord         ▸ 8│ SymbolKind       ◉   │ kind              ◉
                  │▸provider      ▸ 3│▸SymbolProvider  ◉◊ 2 │ detail            ◉
                  │ graph         ▸ 9│ impl … for Vec   ◊   │ range             ◉
 ─ karet-core::provider::SymbolProvider ────────── interface ── provider.rs:12 ─────
 ◉ api           pub                effective: karet_core::SymbolProvider
 ◊ substitution  trait · default-method ×1
 ⌥ variation     —
 ⇥ boundary      —
 ☡ hazard        —
   edges         … not resolved — structural relations only
 / lens:substitution !kind:member                              ⌫ widen (1)
```

## What gets indexed

A root is read as a *repository*, not as a single package. Whatever is found under it
becomes one index with a root per package, so the spine's first column is the package list
and a query spans all of them at once:

```
 Seam  karet · 30 packages   config: unconfigured (variation incomplete)   1◉ api  …
 package          │ module           │ item
 blameline     ▸ 9│ model         ▸12│ Symbol           ◉ 6
 karet-core   ▸ 47│ coord         ▸ 8│ SymbolKind       ◉
 karet-diff   ▸ 31│▸provider      ▸ 3│▸SymbolProvider  ◉◊ 2
```

Four Cargo shapes are read: a package, a virtual `[workspace]` root, a root that is both,
and a root with no manifest whose crates sit a level or two down (`rust/api/`,
`services/worker/`). Python projects — marked by `pyproject.toml`, `setup.py`, or
`setup.cfg` — are read alongside them, so a polyglot repository answers about all of
itself. A package's name comes from its manifest; a Python package's comes from the
directory holding its `__init__.py`, because that, not the distribution name, is what an
import path is made of.

Build output, dependency caches, and virtual environments are never walked. `seam.maxIndexedFiles`
caps the whole index rather than each package, and the header says so when it bites.

## The five lenses

The set is closed at five and is language-neutral. A new language maps into it; it does
not extend it. Subtypes *within* a lens are open, so each language names its own.

| Lens | Question | Rust | Python |
|---|---|---|---|
| `api` | What is visible from outside? | `pub`, `pub(crate)`, `pub(super)`, `pub(in …)`, `pub use` | leading underscore, dunders, `__all__` |
| `substitution` | What behavior can be swapped? | traits, impls, blanket impls, default methods, `dyn`, `impl Trait`, bounds, fn pointers, boxed closures | `Protocol`, `ABC`, subclassing, `Callable[…]`, `@overload` |
| `variation` | What changes shape before compiling? | `cfg`, `cfg_attr`, features, macro defs and calls, derives, attribute macros, `include!` | `TYPE_CHECKING`, `sys.platform` branches, decorators |
| `boundary` | What crosses the package line? | `extern` blocks and fns, `no_mangle`, `export_name`, `link`, entry points | `ctypes`/`cffi`, non-relative imports, entry points |
| `hazard` | Where is substitution dangerous? | `unsafe`, `async`, await points, `Send`/`Sync` bounds | `async`, await points, `global`, `nonlocal` |

A node may carry facets from several lenses. A facet is present or absent — there is no
severity and no score, because ranking seams would decide for the reader which ones
matter, and that judgement is what the view exists to support rather than replace.

### What the hazard lens deliberately does not claim

Lock acquisitions and task spawns are **not** reported by the structural tier. `.lock()`
is a method name anyone may use and `spawn` could be any function, so recognizing them by
name would make the lens occasionally wrong. A lens that is occasionally wrong destroys
the one thing this view promises — that absence of evidence and evidence of absence stay
distinguishable — so those wait for the semantic tier, which has the types to be right.

## Navigating

The **spine** is the primary surface: cascading columns over containment, where each
column lists the children of the selection to its left. Below the width that needs
(roughly 37 columns for two), it falls back to an indented tree. That is not a grudging
degraded mode — eighty columns is the common terminal, and four columns of twenty cells
shows nothing but truncated stems.

The **facet pane** shows everything about the current selection, grouped by lens, with its
edges as jump targets. It is what keeps the glyphs honest: a marker in the spine
summarizes, but every glyph corresponds to a spelled-out line below, so nobody is left
decoding pictograms.

| Key | |
|---|---|
| `↑` `↓` / `k` `j` | move within a column |
| `←` `→` / `h` `l` | move between columns |
| `Enter` | narrow to the selection and step into it — on the current root, just step in; on a leaf, open its source |
| `Backspace` / `-` | step back out of the last narrowing |
| `Tab` | move focus between the spine and the facet pane |
| `Enter` (facet pane) | pivot — reroot on the far end of the selected edge |
| `1`–`5` | toggle a lens; `0` clears them all |
| `/` | filter; `Esc` leaves the box, `Esc` again clears it |
| `o` | open the selection's source in an editor tab |
| `c` | cycle the active configuration |
| `y` | copy the selection's identity |
| `q` | close |

The **source preview** shows the lines the selection is made of, with muted lines of
context on each side, so the decision to press `Enter` is made against the code rather than
against a name. It sits beside the facet pane on a terminal 80 columns or wider, below it on
a narrower but taller one, and nowhere at all on one that is neither — the spine is the
primary surface and keeps its rows.

What it never cuts is the **declaration head**: the signature with its parameters, the
`struct` line with its bounds, the `impl` with what it binds. A signature that wraps over
four lines is painted over four lines. Everything else in the block gives way to it —
the context below it first, the context above it last — because a signature cut after its
second parameter has told you less than nothing.

The block's height follows the terminal (nine rows to sixteen) and never the selection: a
pane that changed height as you arrowed down would make you re-find the edge list on every
keystroke. Context a file does not have — at its top or its bottom — is left blank rather
than closed up, for the same reason. A node longer than the budget shows its head and says
how many lines it hid; the gutter numbers make the jump plain, and the lines shown after a
very long node are the ones that really follow it rather than whatever the fetch cap
happened to stop at. A file that could not be read says so, in the same `?` that every
other unresolvable answer uses.

### With the mouse

Every affordance the keys offer has a place on screen, and each of those places answers the
pointer.

| | |
|---|---|
| click a spine row | select it; click it again to step in, or to open its source at a leaf |
| click a breadcrumb crumb | step back out to that crumb — the package name widens all the way |
| click a lens in the legend | toggle it, exactly as its digit does |
| click `config:` | cycle the active configuration |
| click an edge in the facet pane | select it; click it again to pivot |
| click the query box, or `⌫ widen` | focus the filter, or step back out one narrow |
| wheel | move the selection one row; a horizontal wheel moves between columns |

One row per wheel notch rather than a free scroll, because a column's scroll position is
pinned to its selection — the window travels with you rather than away from you.

**Every narrow is reversible, and the way back is visible.** Rerooting and pivoting push
onto one stack that the breadcrumb renders, and the footer shows how many steps remain. A
narrowing you cannot undo is a trap; one you can undo but cannot see is a maze. A narrow
that would not change the root set is refused rather than recorded, so the breadcrumb only
ever shows steps that actually moved the view — and one `Backspace` always undoes exactly
one `Enter`.

## Identity

A node is named by its **semantic path** — `karet-core::provider::{impl SymbolProvider for
Vec<Symbol>}::symbols` — never by where it sits in a file. Inserting a hundred lines above
a function changes every byte offset in it and nothing about its identity, which is what
lets your place in the view survive editing.

Anonymous constructs get a braced segment describing what they bind. Siblings that still
collide take a `#n` ordinal by source order — positional by necessity, and the one edit
shape identity cannot survive. Generic parameters and `where` clauses are excluded: adding
a bound is not a rename and must not cost you your place.

Identity is also the citation unit. It is what `y` copies, what the JSON surface reports,
and what view state is restored against — so a rename invalidates that node's place and
nothing else, with selection falling back to the nearest surviving ancestor.

## Configurations

A package is not one tree. It is a family of trees indexed by build-time variation, and
presenting the default one as "the package" is a correctness bug rather than a
simplification — the reader asking "what is exposed here?" gets an answer silently
conditional on choices nobody showed them.

So exactly one **named** configuration is active at a time and the header always says
which. Nodes the configuration excludes are dimmed, not hidden; hiding is opt-in via
`seam.hideInactive`. Test-only code is a configuration, not a special case.

`cfg` evaluation is three-valued. `target_os = "redox"` on a Linux host is false; an
unrecognized vendor key is *neither*, and collapsing that into false would quietly delete
code from the view while the header still claimed completeness. Unknown propagates through
`all`/`any`/`not` by Kleene logic, with one decisive operand still settling the result.

The distinction is drawn by what is **enumerated**: a manifest lists every feature a
package has, so `feature = "absent"` is decidably off, while nothing enumerates vendor keys.
Until the manifest tier lands the header says `variation incomplete` rather than letting a
partial answer look whole.

## Querying

One language serves the filter box and the programmatic surface. Whatever you can type, a
program can ask — and whatever you reach by pressing keys serializes back to a string
(**Seam: Copy Query**), so an agent's narrowing arrives as a state you can inspect and
adopt rather than take on trust.

Terms are whitespace-separated and implicitly conjoined; `!` negates one. A bare word
fuzzy-matches the name, a `"quoted phrase"` matches a literal substring. There is
deliberately no `or` and no grouping.

| Form | Matches |
|---|---|
| `lens:<name>` | nodes carrying any facet of that lens |
| `<lens>:<subtype>` | that specific facet, e.g. `substitution:dyn` |
| `vis:<level>` | effective visibility at least that reachable |
| `kind:<kind>` | a universal node kind |
| `in:<path>` | subtree containment |
| `cfg:<text>` | gated by a matching variation predicate |
| `config:<name>` | evaluate under a named configuration |
| `pivot:<edge>:<node>` | the result set of following an edge |

An unknown term is a positioned parse error with the closest valid names, never silently
ignored — ignoring one hands back a different node set than you asked for with no
indication anything was wrong, which is exactly how a filter stops being trustworthy.

## From the command line

```bash
karet crates/karet-core --seam-query 'lens:hazard !kind:member'
karet crates/karet-core --seam-query 'in:karet-core::model substitution:dyn'
karet . --seam-query 'lens:api' --seam-config test
```

The last reads every package in the workspace at once; `in:<package>` narrows a query back
to one of them.

Prints JSON and exits, without entering the TUI. Each node carries its identity, location,
facets, and per-lens rollup counts — enough to cite a finding, navigate back to it, and
judge where seam density is concentrated without materializing a subtree.

An unreadable query and a path that is not a package both fail with a non-zero exit rather
than printing an empty result, since either would otherwise read as "no such seams here".
A query that is understood and matches nothing is a success reporting zero.

> This is an unstable automation surface: the output shape may change between major
> versions without notice.

## What is shown when something is unknown

The view keeps three answers apart that a less careful one would flatten into a blank:

| | Meaning |
|---|---|
| `—` | the index looked, and there is nothing |
| `…` | not resolved yet |
| `?` | never resolvable |

The same care applies at the package level. A truncated index says so, unresolved modules
are counted in the header rather than silently omitted, a file that did not parse cleanly
marks its nodes provisional, and a package that could not be indexed at all says that
instead of rendering an empty tree.

## Architecture

| Piece | Role |
|---|---|
| [`karet-seam`](../crates/karet-seam) | the index: containment tree, facets, edges, configurations, query language |
| [`karet-treesitter`](../crates/karet-treesitter) | `SyntaxTree::walk`, the neutral traversal facet extraction is built on |
| [`karet-widgets`](../crates/karet-widgets) | the cascading-column navigator, seam-agnostic |
| [`karet-session`](../crates/karet-session) | owns the index on a worker; `Command`/`Event` seam |
| [`karet`](../crates/karet) | the view, the keymap, and `--seam-query` |

Containment is a tree and everything else is an edge; the two are never merged, in the
model or the UI. Facts arrive in tiers: the **structural** tier is synchronous and always
available and produces a usable tree even from a file that does not parse; the
**semantic** tier resolves edges asynchronously and never gates rendering; the
**manifest** tier supplies the configuration set. Nothing structural waits on anything
semantic.

The whole node list crosses the session seam at once rather than page by page. That looks
profligate until you price the alternative — a round trip per keystroke is the latency a
cascading navigator cannot afford — and a large crate flattens to a few hundred kilobytes.

## Adding a language

Adding one must not change the view, the query language, the lens set, or the model. A
language contributes a mapping from its constructs to the universal node kinds, a mapping
from its constructs to lens facets, and a declaration of what its semantic tier can
resolve — which may be empty, in which case the view degrades rather than failing.

Python is the conformance test, chosen because it shares almost nothing structurally with
Rust: no visibility keyword, no `cfg`, no monomorphized generics, no module-per-file rule.
It needed no new lens and no new node kind, and the query language filters it with the same
terms. See [`crates/karet-seam/src/lang`](../crates/karet-seam/src/lang).

## Settings

See [`configuration.md`](configuration.md#seam) for the `seam.*` keys.

## Status

| Piece | Status |
|---|---|
| Containment tree, spine, rollups | ✅ shipped |
| `api` · `substitution` · `variation` · `boundary` · `hazard` lenses | ✅ shipped |
| Facet pane, pivot, breadcrumb | ✅ shipped |
| Source preview with context | ✅ shipped |
| Query language + `--seam-query` | ✅ shipped |
| Configuration switching (three-valued `cfg`) | ✅ shipped |
| Rust + Python mappings | ✅ shipped |
| Workspace / multi-package / nested-crate discovery | ✅ shipped |
| Choosing a start point | ✅ shipped |
| Manifest-derived feature/target configurations | 🧭 designed — needs `dependable-core` 0.2.0 |
| Semantic-tier edge resolution via rust-analyzer | 🧭 designed — `karet-lsp` requests are in place |
