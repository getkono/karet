# Language support and language servers

This is the canonical support matrix and precedence policy for karet. Changes to
the built-in catalog in `karet-session/src/lsp.rs`, the managed catalog in
`karet-session/src/lsp_registry/catalog.rs`, or the grammar registry in
`karet-treesitter/src/registry.rs` must update this document in the same change.

## The built-in experience

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

The managed fallback is intentionally smaller than the recognition catalog:

| Language | Default providers | Managed by karet | Tree-sitter |
|---|---|---:|---:|
| Rust | **rust-analyzer** | yes | yes |
| JavaScript, TypeScript, JSX, TSX | **typescript-language-server**; **Biome** diagnostics/formatting when a Biome config exists | yes (TypeScript provider) | yes |
| Python | **Pyright** intelligence/type checking + **Ruff** diagnostics/formatting | yes (both) | yes |
| TeX / LaTeX | **texlab** | yes | yes |
| C / C++ | **clangd** | project/PATH | yes |
| C# | **Microsoft.CodeAnalysis.LanguageServer** | project/PATH | yes |
| Go | **gopls** | project/PATH | yes |
| Java | **jdtls** | project/PATH | yes |
| Zig | **zls** | project/PATH | yes |
| Astro | **Astro language server** | project/PATH | yes, with injections |
| Svelte | **svelte-language-server** | project/PATH | yes, with injections |
| Vue | **vue-language-server** | project/PATH | yes, with injections |
| YAML | **yaml-language-server** | project/PATH | yes |
| XML / SVG | **lemminx** (`xml`) | project/PATH | yes |
| HTML | vscode-html-language-server | project/PATH | yes |
| CSS / Sass / Less | vscode-css-language-server | project/PATH | yes |
| JSON | vscode-json-language-server | project/PATH | yes |
| Shell / Bash | bash-language-server | project/PATH | yes |
| Ruby | ruby-lsp | project/PATH | when compiled in |
| PHP | phpactor | project/PATH | when compiled in |
| Swift | sourcekit-lsp | project/PATH | when compiled in |
| Scala | metals | project/PATH | when compiled in |
| Lua | lua-language-server | project/PATH | when compiled in |
| Haskell | haskell-language-server | project/PATH | when compiled in |
| OCaml | ocamllsp | project/PATH | when compiled in |
| Erlang | elp | project/PATH | when compiled in |
| Dart | `dart language-server` | project/PATH | when compiled in |
| R | languageserver | project/PATH | when compiled in |
| Clojure | clojure-lsp | project/PATH | when compiled in |
| TOML | taplo | project/PATH | yes |
| Pkl | pkl-lsp | project/PATH | when compiled in |
| Protobuf | `buf beta lsp` | project/PATH | when compiled in |
| GraphQL | graphql-lsp | project/PATH | when compiled in |
| PowerShell | PowerShell Editor Services | project/PATH | when compiled in |
| Markdown | marksman | project/PATH | yes, with injections |
| reStructuredText | esbonio | project/PATH | when compiled in |
| Dockerfile | docker-langserver | project/PATH | when compiled in |
| CMake | neocmakelsp | project/PATH | when compiled in |

“Project/PATH” is still built-in support: selection, lifecycle, synchronization,
diagnostics, and editor features work without configuration when the conventional
executable is present. It does not mean karet downloads that third-party tool.

## Capability ownership and overlap

Only one provider owns a capability that produces edits or navigation results.
Diagnostics are the exception: independent diagnostic layers are merged by
provider, path, and document version, then sorted and deduplicated.

| Capability | Owner and behavior |
|---|---|
| Parsing, syntax colours, folds, brackets, structural selection, injections | Tree-sitter, always the baseline |
| Completion, hover, definition, symbols, rename, signature help, code actions, inlay hints | first capable LSP in the language's ordered `servers` list |
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
Requests made during an outage receive an empty response rather than hanging.
Both protocol and per-server command queues are bounded at 256 messages.

Normal shutdown sends `shutdown`/`exit`. Forced editor or broker death closes the
supervisor lease, which kills and reaps the entire server process group. Stale
broker leases are time-bounded and replaced on a subsequent connection attempt.

## Managed installations and consent

Managed versions live below the platform data directory in
`language-servers/`. Provider locks serialize concurrent changes; immutable
version directories are activated through an append-only journal only after
archive traversal checks and publisher SHA-256 verification. A torn journal tail
is ignored. Node providers use a registry-owned, verified active-LTS Node runtime.

`lsp.managedDownloads` controls missing fallbacks:

- `prompt` (default): opening a file performs no network I/O. karet first asks
  permission to discover release metadata, then displays the exact provider,
  version, transition, and known download size. Applying that short-lived plan is
  a separate confirmation.
- `auto`: the user has pre-authorized discovery and exact-plan application.
- `off`: no discovery or download.

Update checks are always explicit. An approved plan expires after 15 minutes and
is rejected if another process changed the active version. Existing brokers keep
their pinned executable until restarted; new brokers use the new activation.

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
