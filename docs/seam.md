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
 Seam  karet-core   config: default @ x86_64-linux   1◉ api  2◊ sub  …  5☡ haz    ↻  ↻!
 package          │ module           │ item                 │ member
 karet-core   ▸ 47│ model         ▸12│ Symbol           ◉ 6 │ name              ◉
                  │ coord         ▸ 8│ SymbolKind       ◉   │ kind              ◉
                  │▸provider      ▸ 3│▸SymbolProvider  ◉◊ 2 │ symbol_at         ◉
                  │ graph         ▸ 9│ Diagnostic       ◉ 4 │ impl for Vec    ◊ │
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

Seven ecosystems are read, and every one of them always runs — a repository with a Rust
workspace, a Python service and a TypeScript front end is one repository, and answering
with only part of it would be a quieter kind of wrong than failing.

| Marked by | Sources under | One package per |
|---|---|---|
| `Cargo.toml` | followed from `src/lib.rs`, `src/main.rs` | crate |
| `pyproject.toml`, `setup.py`, `setup.cfg` | the importable package directories | importable package |
| `package.json` | `src`, or the package directory | package |
| `Package.swift` | `Sources/<Target>` | target |
| `build.gradle{,.kts}` | `src/<set>/{kotlin,java}` | source set |
| `pom.xml` | `src/<set>/java` | source set |
| `CMakeLists.txt`, `meson.build`, `Makefile`, `GNUmakefile` | the project directory | project |

C and C++ have no manifest, so the build file stands in for one. A CMake project usually
has one in every directory, so the outermost claims the tree beneath it. A bare `Makefile`
is every kind of project's build file, so it marks a C project only when C or C++ sources
sit beside it or in its `src`.

Four Cargo shapes are read: a package, a virtual `[workspace]` root, a root that is both,
and a root with no manifest whose crates sit a level or two down (`rust/api/`,
`services/worker/`).

**Rust declares its module tree and everything else has one on disk.** `mod net;` says
where to look next, so a Cargo crate is walked by following declarations. Everywhere else
a file is a module and a directory holding source is a namespace, so the tree is read off
the filesystem — with the `__init__.py`/`index.ts` convention honoured where a language
has one, meaning that file *is* its directory rather than a module beside it. Swift,
Kotlin, Java, C and C++ have no such convention, so every file there is its own module.

**A package's name comes from the directory, not the manifest**, everywhere except Cargo.
A `[project] name` is a *distribution* name and a `package.json` name may be scoped;
neither need match anything importable, and the directory is both what a reader sees and
what an import path is made of.

Build output (including `cmake-build-*`), dependency caches, `vendor`, and virtual
environments are never walked. A C or C++ project also leaves out `third_party`,
`3rdparty` and meson's `subprojects`, where those languages copy their dependencies in —
only there, since a Gradle build keeps its own modules under `subprojects`. `seam.maxIndexedFiles`
caps the whole index rather than each package, and the header says so when it bites.

## Syncing, and what is kept between runs

Packages are read concurrently, and each one reaches the view the moment it is finished
rather than at the end. On a repository of any size the first rows are readable and
navigable while the rest is still being parsed. Every package is final when it appears —
its configuration is resolved and its counts are computed before it is sent — so nothing
is revised under the reader. Packages are listed in discovery order however the work
happened to finish.

What each file produced is stored, keyed on its modification time and length, and replayed
next time instead of parsed. Deliberately not a content hash: hashing means reading, and
reading is most of what this avoids. The trade is one-sided — a file touched without being
changed is parsed needlessly, which costs a moment; a file that changed is never mistaken
for one that did not, because writing to it moves its modification time.

| | |
|---|---|
| `s`, or `↻` | re-index what changed: new and removed packages, and files whose stamp moved |
| `S`, or `↻!` | discard the stored index entirely and read every file again |

The second exists because a stored answer can be wrong in a way nothing else can detect.
Everything else — a changed file, an added crate, a deleted module — is caught by the
first, so reach for the forced one only when the view itself looks untrue.

A sync leaves the tree on screen until its replacement arrives. Blanking it would trade an
answer that is still true for every package not yet re-read against nothing at all. When it
finishes, the status line says what it did: `up to date (524 files)`, or `re-read 3 of 524
files`.

The stored index lives in the user cache directory, one file per workspace. It is a cache
in the strict sense: deleting it costs a rebuild and nothing else, and it is discarded
whole whenever the engine version or the set of compiled-in grammars changes, since either
can alter what every file extracts to.

## Where things sit

Containment follows what a construct *belongs to*, not where it was written.

A Rust `impl` block is not a thing anyone navigates to — it is where methods happen to be
written, and the language lets it be written anywhere in the crate. So an inherent
`impl Widget` **dissolves**: its methods become members of `Widget`, beside the fields, in
source order. A trait implementation stays as one level, because it *is* a thing — a
binding of a contract to a type, carrying its own seam facets — and it reads as
`impl Display` once it sits under `Widget`, since repeating the type it is already inside
says nothing.

An implementation whose self type this package does not declare has no local type to sit
beneath. `impl MyTrait for String`, and the blanket `impl<T: Bound> MyTrait for T`, sit
under **the trait** instead, which is where a reader looks for a trait's implementors.

Two things are deliberately left alone. A block gated by `cfg` keeps its level even when it
is inherent, because the gate decides whether its members exist at all and the type cannot
carry that fact; dissolving it would list a Unix-only method beside an always-present one
as though both were always there. And a block neither of whose ends this package declares —
`impl Display for Vec<u8>` — stays exactly where it was written, because a name that
resolves to nothing, or to more than one thing, is not a name this index will guess at.

The same machinery serves every language: a language says what a construct belongs to, and
a neutral pass resolves the name. What that means per language:

| Language | Written away from what it belongs to | Where it lands |
|---|---|---|
| Rust | `impl` blocks | inherent dissolves into the type; a trait binding keeps a level under it |
| Swift | `extension Widget`, `extension Widget: Codable` | the same, with `#if` standing in for `cfg` as the gate that keeps a level |
| Kotlin | `fun Widget.render()`, `val Widget.area` | a member of its receiver; a companion object is transparent and needs no move |
| TypeScript | `Widget.prototype.render = …` | a member of `Widget` |
| Python | — | nothing: a method is written inside its class |

A name that resolves to nothing, or to more than one thing, always leaves its node where it
was written. Over this repository that is three `impl From<Local> for Foreign` blocks, each
of which genuinely belongs to neither end.

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

| Lens | TypeScript / JavaScript | Swift | Kotlin |
|---|---|---|---|
| `api` | `export`, `export default`, `declare`, `private`/`protected`, the `#` sigil | `open`, `public`, `package`, `internal`, `fileprivate`, `private` | `public`, `internal`, `protected`, `private` |
| `substitution` | `interface`, `abstract`, `implements`, `extends`, bodiless signatures, optional members, function-typed values | protocols, extensions, conformances, associated types, `some`, `any`, protocol defaults | interfaces, `abstract`, `open`, `sealed`, `override`, `by` delegation, `object`, extensions |
| `variation` | decorators, conditional types, dynamic `import()`, `process.env` branches | `#if`, `@available`, `@propertyWrapper`, attributes | annotations, `expect`/`actual`, `inline`, `reified` |
| `boundary` | package imports, `declare module`, `globalThis` | `@_cdecl`, `@objc`, `@_silgen_name`, `@main`, module imports | `external fun`, `@Jvm*`, `main`, imports |
| `hazard` | `async`, await points, `as any`, `!`, `@ts-ignore`, `eval` | `async`, await points, `throws`, `try!`/`as!`, unsafe pointers, `@unchecked` | `suspend`, `!!`, `lateinit`, `@Volatile` |

C, C++ and Java are mapped for **containment and the `api` lens only**:

| Lens | C / C++ | Java |
|---|---|---|
| `api` | `static` and `extern` linkage; C++ `public:`/`protected:`/`private:` sections | `public`, `protected`, `private`, package-private |
| `substitution` · `variation` · `boundary` · `hazard` | `?` — not read | `?` — not read |

A lens a language does not read shows `?` and "not read for this language" in the facet
pane, its rollup is `null` in `--seam-query`, and the node lists it under
`unread_lenses`, because showing `—` would claim the index looked and found nothing. A
C or C++ file is a module named by its whole file name — `widget.h` and `widget.c` are
two modules, since a header and its source share a stem. A C++ method defined outside its
class (`void Widget::draw() {…}`) is regrouped under the class beside its declaration, as
a Rust `impl` is, and claims no access of its own.

One known limit: a `.h` file is always read with the C grammar, as its extension says. A
C++ class declared in a `.h` therefore parses as damaged C (its nodes are marked
partial), its out-of-line methods have no class to regroup under, and a method called on
it resolves as external. Declaring C++ classes in `.hpp`/`.hh` avoids this.

Each language names its own subtypes and a query accepts any of them, so
`substitution:blanket-impl`, `substitution:protocol` and `substitution:implements` are all
valid terms in one query over a polyglot repository.

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
| `s` / `S` | sync what changed, or discard the stored index and read everything again |
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
| click `↻` / `↻!` | sync what changed, or discard the stored index and read everything |
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

Rust says what is test code with `cfg(test)`. The other languages say it by convention,
and the convention is read as a gate of the same kind: a file under `tests/`, `test/`,
`Tests/` or `__tests__/`, or one named `test_*.py`, `*_test.*`, `*.spec.ts`,
`*.test.ts`, `conftest.py`, or — in Java, Kotlin, Swift and Scala, where a class names
its file — `*Test` or `*Tests`, is inactive unless the
configuration builds tests. The path is read from the package root, so a checkout
that happens to sit under a directory called `test` is not test code.

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
| `reach:<node>` | everything the node transitively calls, through established calls |
| `callers:<node>` | everything that transitively calls the node |

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

A function also carries `loc` — its own code lines, its physical lines, and the
[call-graph measures](#calls) under the active configuration — and `calls`, one entry
per target and resolution with a site count and the first site's line. `loc` is `null`
for anything that is not a function, and its graph measures are `null` for a function the
configuration leaves out, which is a different answer from zero.

An unreadable query and a path that is not a package both fail with a non-zero exit rather
than printing an empty result, since either would otherwise read as "no such seams here".
A query that is understood and matches nothing is a success reporting zero.

> This is an unstable automation surface: the output shape may change between major
> versions without notice.

## Calls

The index also records who calls what, and measures each function by the code it
pulls in. The question it answers: *if I call this, how much code am I depending on?*

### What is measured

| Measure | Meaning |
|---|---|
| own lines | lines of the function whose first code byte is its own: not blank, not comment-only, not inside a nested named function |
| span | physical lines from the first to the last, everything included |
| inclusive | own lines of every *distinct* function it reaches, itself included |
| tree weight | own lines plus each callee's tree weight *per call site* |
| fan-in, fan-out | distinct functions calling it, and that it calls |
| external APIs | distinct callees outside the indexed code |
| depth | the longest chain of calls below it |

Inclusive is the answer to the question above: a helper reached along three paths counts
once. Tree weight counts it three times, which is what makes the ratio between the two a
measure of how much a function leans on shared code. A summed call tree would count a
diamond twice and never finish a cycle, so recursion is condensed first: functions that
call one another are one unit, and every member reports the unit's values.

A closure is not a node: its lines and its calls are its enclosing function's. A nested
named function owns its own lines, so summing every function's own lines never counts a
line twice.

### Two bounds, never a guess

Calls are resolved by name and scope, not by type, so every call edge says how it was
reached:

| Resolution | Meaning | Lower | Upper |
|---|---|---|---|
| `exact` | one candidate survived every scope rule | ✓ | ✓ |
| `dispatch` | a call through a contract, to its declaration and default body | ✓ | ✓ |
| `dispatch-impl` | an implementation that call may reach | | ✓ |
| `ambiguous` | one of several equally plausible candidates | | ✓ |
| `external` | outside the indexed code — a library, a builtin | | |
| `unresolved` | nothing plausible, or a computed callee | | |

Every measure is computed over the **lower** bound, and inclusive also over the **upper**.
The gap between them is how much of a number is guesswork. A site with more than 16
candidates — `x.get()` in a large codebase — counts in neither bound, since linking it to
every `get` would say nothing.

The scope rules, first match wins: a `self`/`this` receiver means the caller's own type;
a qualifier (`net::send`, `Widget.create`) means the container it names, and one the
index does not hold means an external callee — a Cargo package is named as code writes
it, `karet_core` for `karet-core`; a method on a named value is exact when the only
member of that name in the language is in the caller's package, a dispatch when one of
the candidates is a contract's declaration and every other names that contract as what it
implements, and otherwise every candidate is ambiguous; a method on a call's result
(`xs.iter().filter(…)`) is never exact, since its type is unknown; a free call means, in
Rust, Python and TypeScript, the caller's module, then its file, its imports, a type of
that name (a constructor), and its package. Kotlin, Swift, Java and the C family first
check a member of the caller's type, since a bare name can reach one there (C has no
members, so the check finds nothing), and then check the file before the module, since a
module there spans files and a file-private helper beats its namesake elsewhere; the
imports, constructor and package follow as before. A free name found only in
another package is ambiguous, since nothing imported it. Candidates are always in the
caller's language family, the ones the host build has win over gated-out twins, and a
definition wins over its bare declaration.

What this does not see: a function passed as a value, a call through a pointer, and
anything reflection reaches. A Rust macro's arguments are tokens rather than syntax, so a
call written inside one is counted in the upper bound only.

### Tests are left out

Measured under the active configuration, which by default excludes test code. A test
calls everything; counting it would make every helper look shared and every function look
reachable. A configuration that builds tests measures with them included.

`reach:` and `callers:` are the exception: they follow the call edges as recorded, test
callers included, because they answer "what is connected" rather than "how much is
there". Combine them with other terms to narrow the result.

### Adding a language

A language's whole part in this is one tree-sitter query in
[`crates/karet-seam/queries/calls`](../crates/karet-seam/queries/calls), returned from
`SeamLanguage::call_query`. It marks `@call` with `@call.name`, and optionally
`@call.qualifier`, `@call.receiver`, `@call.self`, `@call.super`, `@call.new`,
`@call.external` and `@call.weak`; `@body` on callable bodies; `@comment`; and
`@import.path`, `@import.prefix`, `@import.alias`, `@import.glob` and `@import.head`
(an import binding its first segment, as Python's `import a.b` does). A language whose
bare names can reach a member of the enclosing type — Kotlin, Swift, Java, C++ — also says so with
`SeamLanguage::implicit_receiver`. Attribution, line counting, resolution and measuring
are language-neutral.

## What is shown when something is unknown

The view keeps three answers apart that a less careful one would flatten into a blank:

| | Meaning |
|---|---|
| `—` | the index looked, and there is nothing |
| `…` | not resolved yet |
| `?` | never resolvable — or, on a lens, a lens this language's mapping does not read |

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
resolve — which may be empty, in which case the view degrades rather than failing. A
mapping that reads only some lenses names the rest in `SeamLanguage::unread_lenses`, so the
view can tell "not read" from "none"; its [call query](#adding-a-language) is separate.

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
| Rust, Python, TypeScript/JavaScript, Swift, Kotlin mappings | ✅ shipped |
| C, C++, Java mappings (containment + `api`; other lenses `?`) | ✅ shipped |
| Workspace / multi-package / nested-crate discovery | ✅ shipped |
| Choosing a start point | ✅ shipped |
| Call edges, own/inclusive/tree-weighted lines, `reach:`/`callers:` | ✅ shipped |
| Calls and measures in the facet pane | 🧭 designed |
| Manifest-derived feature/target configurations | 🧭 designed — needs `dependable-core` 0.2.0 |
| Semantic-tier edge resolution via rust-analyzer | 🧭 designed — `karet-lsp` requests are in place |
