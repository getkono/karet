# Managed language servers

karet owns the built-in language-server toolchain instead of resolving it from
`PATH`. The machine-local registry currently covers:

| Documents | Provider |
|---|---|
| Rust | rust-analyzer |
| JavaScript / TypeScript | TypeScript Language Server |
| Python | Pyright |
| TeX / LaTeX | texlab |

Installations are shared by every karet instance for the current OS user. They
live below the platform data directory in `language-servers/`; each provider has
immutable version directories and an append-only activation journal. Concurrent
instances serialize changes with a provider lock, download into a staging
directory, verify publisher SHA-256 metadata, and activate only a complete
installation. A torn journal tail is ignored. Node-based providers use a
registry-owned, checksum-verified Node LTS runtime, so they do not depend on a
system Node or npm. The TypeScript provider also installs an explicitly versioned
TypeScript runtime rather than falling back to a workspace or global copy.

Opening a matching document never performs network I/O. If its provider is
missing, karet asks for typed `install` approval. That approval permits discovery
and installation of the latest stable provider; declining leaves language
features disabled without repeated spawning.

Updates are also never automatic:

1. Run **Language Servers: Check for Updates…** from the command palette. This is
   the only update-check path and is an explicit metadata network request.
2. karet displays every installed-to-target version transition.
3. Type `update` to approve that exact, short-lived plan. If another instance
   changes an active version first, the plan is rejected and must be checked
   again.
4. An existing session keeps its old process until the user types `restart`.
   New processes use the newly activated version immediately.

This means a machine that runs continuously sees no background checks, downloads,
or surprise toolchain changes.

## Process ownership

Installations are shared; live LSP processes are deliberately not. Each session
starts a provider lazily for the workspace and closes it as soon as the last
matching document closes. Sharing a live process would mix workspace roots and
unsaved buffer state between editor instances.

Every language-server process, and the related external LaTeX compiler workflow,
is launched through a hidden karet supervisor. The supervisor owns the real
process group (a Windows Job Object on Windows), and the editor's stdin pipe is
its lifetime lease:

- normal close performs the LSP shutdown handshake, then drops the lease;
- cancellation or Rust task teardown kills the supervisor and its process group;
- `SIGKILL`, abort, or a crash closes the pipe, so the surviving supervisor kills
  and reaps the whole server tree.

Consequently, even if every visible karet instance crashes, no language server is
left running. The same ownership path is used for configured custom LSP commands
and external LaTeX builds.

## Custom providers

`lsp.servers` entries remain an escape hatch for languages or versions outside
the built-in registry. Their command and arguments are user-supplied, but their
process tree is still supervisor-owned and idle-shutdown rules still apply.
Because karet cannot authenticate an arbitrary executable, installation and
upgrades for a custom entry remain the user's responsibility.
