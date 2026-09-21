# Language support and language servers

This is the canonical support matrix and precedence policy for karet. Changes to
the built-in catalog in `karet-session/src/lsp/catalog.rs` (which is also where
every provider's launch command and arguments live), the release recipes in
`karet-session/src/lsp_registry/catalog.rs`, or the grammar registry in
`karet-treesitter/src/registry.rs` must update this document in the same change.

## The built-in experience

TOML formatting never requires a server: karet bundles taplo's formatter as a
fallback (`toml.format`), honoring the workspace `.taplo.toml`. Installing the
`taplo` language server additionally brings schema-driven validation,
completion, and hover (its `#:schema` directives work as documented upstream).


karet resolves a provider separately for every open document:

1. `lsp.languages.<language>` and `lsp.servers.<id>` in merged settings;
2. an executable in the document's repository (`node_modules/.bin`, `.venv/bin`,
   or `venv/bin`);
3. the user's `PATH`;
4. a checksum-verified managed installation.

The workspace passed on the command line is not assumed to be one repository.
karet walks upward from each file to the nearest `.git` file or directory. Thus a
directory containing several recursively nested repositories gets independent
roots, configuration, provider selection, and server processes. Files in the
same repository and using the same launch share a process.

The managed fallback covers providers with a publisher-authenticated, runnable
release for the current platform. Providers coupled to a user SDK/runtime remain
explicitly manual:

| Language | Default providers | Managed by karet | Tree-sitter |
|---|---|---:|---:|
| Rust | **rust-analyzer** | yes | yes |
| JavaScript, TypeScript, JSX, TSX | **typescript-language-server** (with a managed TypeScript 5); **Biome** diagnostics/formatting when a Biome config exists | yes (both) | yes |
| Python | **Pyright** intelligence/type checking + **Ruff** diagnostics/formatting | yes (both) | yes |
| TeX / LaTeX | **texlab** | yes | yes |
| C / C++ | **clangd** | yes on x86_64; project/PATH elsewhere | yes |
| C# | **Microsoft.CodeAnalysis.LanguageServer** | project/PATH | yes |
| Go | **gopls** | project/PATH | yes |
| Java | **jdtls** | project/PATH | yes |
| Zig | **zls** | yes | yes |
| Astro | **Astro language server** (with a managed TypeScript 5) | yes | yes, with injections |
| Svelte | **svelte-language-server** | yes | yes, with injections |
| Vue | **vue-language-server** | yes | yes, with injections |
| YAML | **yaml-language-server** | yes | yes |
| XML / SVG | **lemminx** (`xml`) | project/PATH | yes |
| HTML | vscode-html-language-server | yes | yes |
| CSS / Sass / Less | vscode-css-language-server | yes | yes |
| JSON | vscode-json-language-server | yes | yes |
| Shell / Bash | bash-language-server | yes | yes |
| Ruby | ruby-lsp | project/PATH | when compiled in |
| PHP | phpactor | project/PATH | when compiled in |
| Swift | sourcekit-lsp | project/PATH | when compiled in |
| Scala | metals | project/PATH | when compiled in |
| Lua | lua-language-server | yes | when compiled in |
| Haskell | haskell-language-server | project/PATH | when compiled in |
| OCaml | ocamllsp | project/PATH | when compiled in |
| Erlang | elp | project/PATH | when compiled in |
| Dart | `dart language-server` | project/PATH | when compiled in |
| R | languageserver | project/PATH | when compiled in |
| Clojure | clojure-lsp | yes | when compiled in |
| TOML | taplo | project/PATH | yes |
| Pkl | pkl-lsp | project/PATH | when compiled in |
| Protobuf | `buf beta lsp` | yes | when compiled in |
| GraphQL | graphql-lsp | yes | when compiled in |
| PowerShell | PowerShell Editor Services | project/PATH | when compiled in |
| Markdown | marksman | yes | yes, with injections |
| reStructuredText | esbonio | project/PATH | when compiled in |
| Dockerfile | docker-langserver | yes | when compiled in |
| CMake | neocmakelsp | yes | when compiled in |

“Project/PATH” is still built-in support: selection, lifecycle, synchronization,
diagnostics, and editor features work without configuration when the conventional
executable is present. It does not mean karet downloads that third-party tool.

The manual entries are explicit, not an unexplained remainder:

| Providers requiring user installation | Reason |
|---|---|
| C# Language Server | distributed with Microsoft's C# tooling and requires the user's .NET SDK/MSBuild |
| gopls | the official installation and analysis flow uses the project's Go toolchain |
| jdtls, LemMinX | require a compatible user-selected Java runtime; current jdtls requires Java 21 plus project JDK configuration |
| ruby-lsp, phpactor | must run inside the project's Ruby/Bundler or PHP environment |
| sourcekit-lsp, Dart Language Server | ship with the matching Swift/Xcode or Dart/Flutter SDK |
| Metals, Haskell Language Server, ocamllsp, ELP | must match the project's Scala/JVM, GHC, opam-switch, or Erlang/OTP toolchain |
| R languageserver, PowerShell Editor Services, Esbonio, pkl-lsp | require the user's R, PowerShell, Python/Sphinx, or Java/Pkl runtime environment |
| Taplo | current native release assets do not provide a publisher-authenticated SHA-256 digest; the older npm channel is not treated as a current update source |

On an architecture for which a normally managed provider has no verified upstream
artifact, the manager reports that platform-specific reason and treats the provider
as manual. Currently this applies to clangd on ARM.

### How providers are launched

Every provider is launched over stdio with a command and arguments recorded once,
in `karet-session/src/lsp/catalog.rs`. The same row is used whether the executable
came from the project, from `PATH`, or from a karet-managed install, so those can
never disagree.

Of the 41 providers, 17 speak LSP on stdio when run bare and are launched with no
arguments at all. The remaining 24 are launched as shown here — this table is the
complete set, so a provider absent from it takes no arguments.

Eleven need only the conventional flag that selects stdio over a socket:

| Provider | Invocation |
|---|---|
| astro-language-server | `astro-ls --stdio` |
| csharp | `Microsoft.CodeAnalysis.LanguageServer --stdio` |
| docker-langserver | `docker-langserver --stdio` |
| pyright | `pyright-langserver --stdio` |
| svelte-language-server | `svelteserver --stdio` |
| typescript-language-server | `typescript-language-server --stdio` |
| vscode-css-language-server | `vscode-css-language-server --stdio` |
| vscode-html-language-server | `vscode-html-language-server --stdio` |
| vscode-json-language-server | `vscode-json-language-server --stdio` |
| vue-language-server | `vue-language-server --stdio` |
| yaml-language-server | `yaml-language-server --stdio` |

Thirteen enter their language-server mode some other way — a subcommand, a
different executable, or an expression. These are the rows worth reading before
changing anything, because a bare invocation of any of them does something else
entirely (`taplo` prints usage to *stdout*, which is fatal to the framing):

| Provider | Invocation |
|---|---|
| taplo | `taplo lsp stdio` |
| neocmakelsp | `neocmakelsp stdio` |
| phpactor | `phpactor language-server` |
| elp | `elp server` |
| haskell-language-server | `haskell-language-server-wrapper --lsp` |
| marksman | `marksman server` |
| ruff | `ruff server` |
| biome | `biome lsp-proxy` |
| buf | `buf beta lsp` |
| graphql-lsp | `graphql-lsp server -m stream` |
| bash-language-server | `bash-language-server start` |
| dart | `dart language-server` |
| R | `R --no-echo -e languageserver::run()` |

Two rows are known-incomplete rather than verified:

- **PowerShell Editor Services** has no standalone executable — it is a module
  bundle entered through `Start-EditorServices.ps1`. Point `lsp.servers` at your
  own bundle; the built-in row only keeps the language routed.
- **esbonio** is launched as the published `esbonio` console script, which suits
  karet's `.venv/bin` resolution better than hardcoding an interpreter. Its
  bare-invocation behavior has not been verified against a live install.

### TypeScript's managed companion

typescript-language-server and the Astro language server both drive `tsserver`,
which they do not ship. karet installs TypeScript alongside them, **pinned to
5**: `latest` is now TypeScript 7, a ground-up rewrite whose `lib` directory
contains no `tsserver.js` at all, so taking the newest release leaves both
servers unable to start.

Astro additionally takes the SDK location as an `initializationOptions` value
rather than looking for one itself, so karet records the installed path with the
activation and sends it with `initialize`. Nothing on the command line can
substitute for it — the option is part of the protocol.

### GraphQL specifics

GraphQL highlighting is not limited to `.graphql`/`.gql`/`.graphqls` files
(`.graphqls` is the conventional schema-definition extension). In JavaScript,
TypeScript, and TSX, template literals are highlighted as GraphQL when tagged
(`` gql`…` `` or `` graphql`…` ``, including member tags like
`` api.gql`…` ``), when preceded by a `/* GraphQL */` comment, or when the
template body starts with a `#graphql` comment — the marker conventions the
`graphql-lsp` ecosystem documents. The built-in `graphql-lsp` provider
(`graphql-lsp server -m stream`, a Node tool) expects a project config file at
the repository root (`.graphqlrc*` or `graphql.config.*`) to serve schema-aware
features.

### Java (jdtls) specifics

karet launches jdtls with a stable per-project workspace: unless the configured
args already pass `-data`, it appends `-data <cache dir>/karet/jdtls/<hash of
the repository root>`, so re-opening a project reuses the previous import
instead of re-indexing from scratch. Because a JDK 21 or newer is required to
*run* jdtls, karet probes `java -version` before the first launch and reports a
specific diagnosis (missing `java`, or an older version) rather than an opaque
spawn failure; karet never downloads a JDK. During the initial import — which
can take a minute or two on a large build — jdtls's `language/status`
notifications are forwarded to the status line so the server never looks hung.

### Checking the catalogue against upstream

The support matrix above is a claim about other people's release processes, so
it can go stale without anything in this repository changing: an asset gets
renamed, a publisher stops attaching a SHA-256 digest, an npm flag is removed.
No offline test can see that.

`mise run test-servers-live` checks it for real. For every managed provider it
installs into a throwaway registry, opens a file of that provider's language,
waits for the connection to reach `running`, and prints a table:

```text
| provider | version | install | launch | secs | note |
```

It needs network and takes minutes, so it is `#[ignore]`d and never runs as part
of `mise run verify`. `KARET_LIVE_SERVERS=taplo,bash-language-server` narrows it
to a subset. A weekly non-blocking workflow (`.github/workflows/servers.yml`)
runs it so drift surfaces before a user hits it.

Run it whenever this matrix, the launch table, or the release recipes change.

## Capability ownership and overlap

Only one provider owns a capability that produces edits or navigation results.
Diagnostics are the exception: independent diagnostic layers are merged by
provider, path, and document version, then sorted and deduplicated.

| Capability | Owner and behavior |
|---|---|
| Parsing, syntax colours, folds, brackets, structural selection, injections | Tree-sitter, always the baseline |
| Completion, hover, symbols, rename, signature help, code actions, inlay hints | first capable LSP in the language's ordered `servers` list |
| Definition (`F12` / `Ctrl+Click`, with `Ctrl`-hover underline and Go Back) | first capable LSP in the language's ordered `servers` list; a `LocationLink` reply lands the caret on the definition's *name*, a plain `Location` on whatever the server calls its start |
| Semantic tokens | Tree-sitter owns highlighting today; `semanticTokens` reserves one future LSP overlay owner and is never allowed to replace parsing |
| Diagnostics | every provider in `diagnostics`, version-gated and merged |
| Formatting | exactly one `formatter`; a user selection wins, then a repository-native provider, then the language default |

Tree-sitter and LSP are complementary. Tree-sitter is local, incremental, stable
while a server restarts, and understands injected regions in Astro, Svelte, Vue,
Markdown, and similar documents. LSP semantic tokens are repository-aware and can
distinguish meanings that syntax alone cannot. karet therefore paints Tree-sitter
as the complete current layer. When semantic-token transport is enabled, it will
be a sparse, single-provider overlay; servers are not advertised that capability
until the overlay is present. LSP data never controls parsing, indentation
structure, or injection boundaries.

### Python

Pyright owns Python intelligence and type diagnostics. Ruff owns lint diagnostics
and formatting by default. A repository declaring Ruff in `ruff.toml`,
`.ruff.toml`, or `[tool.ruff]` gets Ruff explicitly; it is also the zero-config
default.

Flake8 and autopep8 are not language servers:

| Tool | Intelligence | Diagnostics | Formatting | karet treatment |
|---|---:|---:|---:|---|
| Pyright | yes | types | no | primary Python LSP |
| Ruff server | limited | lint/imports | yes | default companion |
| Flake8 | no | lint | no | a `.flake8`, `[flake8]` in `setup.cfg`, or `tox.ini` selects `pylsp` when Ruff is not configured; the pylsp environment must contain its Flake8 plugin |
| autopep8 | no | no | yes | tracked as formatter-only; never launched as an LSP. The built-in formatter remains Ruff unless a real formatter integration is configured |
| Black | no | no | yes | tracked as formatter-only and mutually exclusive with Ruff/autopep8 formatting; never launched as an LSP |

Repository-local markers are evaluated at the nearest Git root, so two nested
Python repositories can independently choose Ruff and Flake8. Explicit
`lsp.languages.python` settings override marker-based defaults.

### JavaScript and web languages

TypeScript Language Server remains the intelligence owner. A repository containing
`biome.json`, `biome.jsonc`, or legacy `rome.json` adds Biome as a diagnostics
layer and preferred repository formatter without duplicating completion or
navigation. Astro, Svelte, and Vue use their framework server for the outer
document and Tree-sitter language injections for embedded script and style syntax.

## Shared process lifecycle

Installations and live processes are machine-local shared resources. A hidden,
authenticated loopback broker owns one server for each exact tuple of:

- executable and arguments;
- nearest repository root;
- broker protocol version;
- karet version.

Every karet window speaks normal LSP to the broker. It rewrites JSON-RPC request
IDs, broadcasts notifications, and reference-counts `didOpen`/`didClose`, so
concurrent windows do not duplicate the server or prematurely close a document.
Different karet protocol versions deliberately get different brokers rather than
corrupting each other's streams. Endpoint state is private to the current user.

The broker launches the real process through karet's process-group supervisor
(Windows Job Object on Windows). It retires 30 seconds after its last client.
Crashes close the broker connection; each session retains the latest full text of
every open document, reconnects with exponential delays from 250 ms to 30 seconds,
and replays `didOpen`. Five failures in one minute open a five-minute circuit.

A lost connection is noticed the moment it happens, not the next time karet has
something to ask: each server task waits on the connection's liveness alongside its
own command queue, so a server that exits while you are reading rather than typing
starts reconnecting immediately. A server that keeps its pipe open but stops
answering is also treated as dead, after three consecutive request timeouts -- but
only once it has answered a request the editor issued *after* the handshake.
"Stopped answering" presupposes having answered: a cold rust-analyzer or a jdtls
mid-import can go minutes without answering its first request, and killing one
there restarts the import, which guarantees the next requests time out too. The
handshake itself does not count, since every connected server answers `initialize`.
Two silent deaths in a row put the provider behind the circuit; they are counted
directly rather than through the sliding failure window, because establishing each
one costs at least three 30-second timeouts and the window always expires between
them.

A connection that dies without having lasted ten seconds is charged against the
restart budget, so five such cycles in a minute open the circuit. Connecting is
not the same as working: a server that exits as soon as it has read `didOpen`
connects perfectly every time, and would otherwise respawn without bound.

A dead provider's diagnostics are dropped one second after it goes down, and only
its own: they are keyed by the exact provider-and-root that published them, so
other servers' markers, the same provider's markers at another repository root,
spell-check and lint results are all untouched. The delay is deliberate -- the
usual death is followed by a reconnect at 250 ms, and clearing immediately would
flicker every marker off and back on for an outage nobody would otherwise have
noticed. Retiring a provider outright -- turning LSP off, editing its settings,
closing the last document that needed it -- clears its markers with no delay,
since no reconnect is coming to make them true again.
Requests made during an outage receive an empty response rather than hanging.
Both protocol and per-server command queues are bounded at 256 messages.

Normal shutdown sends `shutdown`/`exit`. Forced editor or broker death closes the
supervisor lease, which kills and reaps the entire server process group. Stale
broker leases are time-bounded and replaced on a subsequent connection attempt.

## Language Servers manager

Run **Language Servers: Manage** from the command palette to open the singleton
Language Servers tab. The tab inventories every effective built-in and configured
provider, including providers that are disabled or currently unavailable. Each row
shows its languages, executable source, managed version, and runtime state. The
selected-server detail lists every repository root, resolved command and arguments,
open-document count, retry/circuit state, and most recent error.

### The badge

Every editor pane carries its own language-server badge, right-aligned in its
breadcrumb row, so a split showing a healthy Rust file beside a broken Python one
says so twice rather than reporting only whichever pane has focus. The status bar
spells the same condition out for the focused pane. A buffer with no path has no
badge, because provider resolution is path-based and there is nothing to resolve.

The eight states separate by *cause*, because what the user should do differs:

| Badge | Condition | Whose move |
|---|---|---|
| `off` | a provider covers the language but your settings suppress it -- `lsp.enabled`, or an `enabled: false` / empty-command entry the language actually selects | yours, if you want it |
| `idle` | resolvable and healthy; nothing has needed it yet | nobody's |
| `ready` | connected and synchronized | nobody's |
| `starting` | connecting | nobody's |
| `retrying` | dropped; karet is reconnecting on its own | nobody's |
| `needs setup` | missing, and karet cannot fetch it -- it needs your SDK or toolchain, or you declined it | yours |
| `not installed` | missing, and karet can install it once you approve | yours, one click |
| `failed` | the executable is present and does not work | yours, via the manager |

Resolution is read before the runtime state, so "nothing is installed" is never
reported as a crash: `failed` is reserved for a binary that exists and does not
work. An open circuit is `failed` here and an error in the manager table too --
the breaker is karet's mechanism, but what you have is a provider that crashed
five times in a minute and will not be retried for five more.

Where several providers cover one file -- Python is Pyright *and* Ruff -- the worst
state wins so a problem is never hidden, and a `healthy/total` count rides along so
a partial failure reads as partial. A provider you switched off is not counted: a
running Pyright beside a deliberately disabled Ruff is working completely, and
reporting `1/2` would invite you to go and fix your own decision. A file whose Pyright is answering normally and
whose Ruff is missing shows `not installed 1/2`, not a blanket failure.

**Clicking a badge opens the Language Servers tab, and does nothing else.** It
starts no install and restarts no process: an install spends your bandwidth and a
restart kills a running server, and neither is something a single click on a status
glyph should decide. The manager is where those live.

Retry and crash failures also create persistent LSP notifications containing the
underlying protocol or launch error, so the failure remains visible without opening
the manager. A provider karet *cannot* install raises no notification at all -- the
badge says `needs setup` and the manager's row names the SDK or toolchain required
and the executable karet looked for. karet does not tell you to put things on
`PATH`.

The bordered table responds to terminal width by dropping secondary columns before
primary state and wrapping row actions when necessary. Runtime, availability,
ownership, update, and error text uses the corresponding semantic theme color so
state changes remain visible at a glance. Global refresh, check-all, and filter
controls remain in the action strip; provider-specific controls live on their row and
appear only when applicable. A missing managed provider shows **Install**, an
installed provider shows **Check updates** (or **Update** after discovery) and
**Uninstall**, and an active session instance shows **Restart**. Pending operations
replace only that provider's action with a progress label, so other providers remain
independently actionable. This view keeps the detailed per-provider progress; a
confirmed install also reports itself as a notification, so a user who approved it
from an editor buffer is not left watching a tab they never opened. The operation
state is
owned independently of the tab, so closing and reopening the manager does not lose
it. The loading placeholder follows the shared 200 ms reveal delay. The view can be
operated with either mouse or these focused-tab keys:

A missing manual provider instead shows a non-interactive **Install manually**
label. Its selected-row detail names the required SDK, runtime, toolchain, or
publisher-verification constraint.

| Key | Action |
|---|---|
| `j` / `Down`, `k` / `Up` | select the next or previous provider |
| `r` | refresh local inventory without network access |
| `u` | force an update check for the selected installed managed provider |
| `U` | force an update check for every installed managed provider |
| `Enter` / `i` | run the selected row's contextual install, update, or check action |
| `R` | restart the selected provider connections in this editor session |
| `x` | uninstall a Karet-managed provider (asks first) |
| `/` | filter by provider or language; submit an empty filter to clear it |
| `o` | offer a declined provider again (clears its recorded refusal) |
| `q` | close the manager tab |

Update discovery never applies a change. Discovered target versions remain visible
in the table until the exact short-lived plan is approved by clicking **Update** (or
running the selected row action). **Install** likewise starts the approved
installation immediately; neither row action asks for the same approval a second
time. Install/update/uninstall actions are refused for configured, project-local,
and `PATH` providers: karet reports their state but does not claim ownership of
them.

## Managed installations and consent

Managed versions live below the platform data directory in
`language-servers/`. Provider locks serialize concurrent changes; immutable
version directories are activated through an append-only journal only after
archive traversal checks and publisher SHA-256 verification. A torn journal tail
is ignored. Independent providers install on separate background workers, while
the per-provider lock still serializes competing changes to the same provider.
Node providers use a registry-owned, verified active-LTS Node runtime.

`lsp.managedDownloads` controls missing fallbacks:

- `prompt` (default): opening a file performs no network I/O. karet first asks
  permission to discover and install the provider's latest stable version. That
  single approval covers the resulting verified download and activation.

  The prompt is a dialog whose **first** answer is *Not now* — the one `Enter`,
  `Esc`, and any unbound key take — because approving it spends the user's
  bandwidth. A provider that is disabled for the language is asked about
  differently, since saying yes there also turns it on.

  It is raised **at most once** per provider. Two records beside the install
  journals decide that, deliberately kept out of settings: `declined.json`, which
  the *Never ask* answer writes, and the activation history, which reports a
  provider as already decided about even after it is uninstalled. Settings say
  what you want configured; these say what karet has already asked and been told,
  so re-enabling a provider never silently re-triggers a download you refused.
  A declined provider says so in the Language Servers tab; `o` there clears
  the refusal, and it is offered again the next time a file needs it.
- `auto`: the user has pre-authorized discovery and installation.
- `off`: no discovery or download.

Update checks are always explicit. Their discovered exact-version plan expires
after 15 minutes and is rejected if another process changed the active version.
Existing brokers keep their pinned executable until restarted; new brokers use the
new activation.

### Recovering without being asked

karet distinguishes conditions it can fix from conditions only you can. Everything
in the first group recovers on its own, idempotently, and without coordination
between karet instances -- every recovery path re-reads the same install journal
that each instance writes through the same per-provider lock, so two of them
converge rather than fight.

A provider that is simply not installed needs no recovery machinery. Resolution
finds nothing, so no process slot is created, and resolution short-circuits only on
a slot -- so installing it by hand is picked up the next time a file of that
language opens. The check performs no network I/O: it reads settings, the project,
`PATH`, and the install journal. That journal is the one every karet process writes
through the same per-provider lock, so instances converge rather than fight, and
the install prompt is still raised at most once, so re-resolving never re-asks a
question you have answered.

A provider whose *launch* fails in a way no retry can fix -- an explicitly
configured command that is not there, a binary that exits on sight -- reports
`failed` and is not retried for the rest of the session. karet deliberately does
not re-attempt it on each file you open: nothing observable changes between those
opens, so it would re-exec the same broken command and raise the same failure once
per file. Fix it, then use **Restart** in the Language Servers tab.

A completed install starts the provider immediately, for every open document whose
language it serves -- including a companion like Ruff or Biome, which is no
language's default and so used to require reopening the file by hand.

What karet will not do by itself is spend your bandwidth. Downloading a provider
needs explicit approval, once, and update discovery never applies a change: both
live in the Language Servers tab, which is where the badge takes you.

Uninstall first appends a deactivation record, so future resolution immediately
stops selecting that managed version. It then retires language-server connections
only in the requesting editor session. Other karet processes and their shared
brokers keep running. The immutable payload is deleted only after broker endpoint
checks show that no live process still references it; until then the manager
reports `cleanup pending`, and the registry retries reclamation in the background.

## Configuration

```jsonc
{
  "lsp": {
    "enabled": true,
    "managedDownloads": "prompt",
    "servers": {
      "company-rust": {
        "command": "/opt/company/rust-analyzer",
        "args": []
      }
    },
    "languages": {
      "rust": {
        "servers": ["company-rust"],
        "formatter": "company-rust",
        "semanticTokens": null,
        "diagnostics": []
      },
      "python": {
        "servers": ["pyright"],
        "formatter": "ruff",
        "semanticTokens": null,
        "diagnostics": ["pyright", "ruff"]
      }
    }
  }
}
```

Custom executables receive the same broker, supervisor, restart, queue, root, and
document-version protections as built-ins. karet cannot authenticate or update an
arbitrary custom executable; its installation remains the user's responsibility.
