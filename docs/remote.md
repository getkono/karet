# Split sessions

karet is normally one process. It can also be two: a **backend** holding the
documents, git, language servers and the files themselves, and a **client** that
renders. This document is the reference for that split — why it exists, what
crosses the connection, what each side owns, and how to run it.

## Why

A terminal multiplexer forwarding a remote pane forwards the *screen*. Every
keystroke waits for a round trip before a character appears, because the terminal
grid is authoritative on the far side. On a fast link that is fine. On a slow one
it makes an editor unusable, and no amount of tuning fixes it — the round trip is
the design.

Forwarding the *session* instead inverts that. The editor draws on the machine the
user is sitting at, so typing, scrolling and repainting are answered locally. Only
edits and derived data cross the gap, and they cross asynchronously: a slow link
costs freshness, never responsiveness.

## Running it

```bash
# By hand. The command supplies the connection; karet supplies none.
karet --client-exec "ssh dev-box karet --serve /srv/repo"

# Against a socket, typically one a multiplexer forwards.
karet --client /run/user/1000/karet/some.sock

# The backend half, speaking the protocol on stdin/stdout.
karet --serve /srv/repo
```

`--no-split` forces a single local process. Outside a multiplexer that can host a
client, that is what karet does anyway.

**karet ships no transport.** `serve` and `connect` take any
`AsyncRead`/`AsyncWrite` pair and never learn where it came from — a pipe, a
socket, the stdio of an `ssh` invocation, a channel a multiplexer forwards. There
is no TLS stack, no `known_hosts`, no connection UX, because supplying a stream is
a solved problem owned by something else. Authentication, encryption and host
verification are `ssh`'s or the multiplexer's, and they are better at them.

## The shape

```
        CLIENT HOST                            WORKSPACE HOST
        ───────────                            ──────────────
 karet --client                            karet --serve
   ├─ ratatui render, keymap, carets          ├─ Session
   ├─ replica TextBuffer per document         │   documents · undo · workers
   ├─ mints DocSnapshot locally               ├─ vcs · search · seam · spell
   └─ Arc<dyn Backend> = RemoteBackend        ├─ filesystem worker
              │                               └─ LSP ─▶ the shared broker
              └──── CBOR frames ──────────────▶      (per server + repo root)
```

The client rebuilds the `DocSnapshot` stream the renderer already draws from, so
**no rendering code knows which mode it is in**. That is what made the split
additive rather than a rewrite: `crates/karet/src/ui/**` needed no changes.

## What crosses, and how often

| | Direction | When |
|---|---|---|
| A keystroke | client → backend | per edit, ~100 bytes |
| Its acknowledgement | backend → client | per edit, a version |
| Highlight spans | backend → client | per recompute, **scoped to the viewport** |
| Folds, blocks, decorations | backend → client | only when a producer recomputed them |
| Document text | backend → client | on open, on resync, and on a backend-originated edit — as an *edit*, not a document |
| File bytes (images, PDFs, hex) | backend → client | on open, in 1 MiB chunks |
| Directory listings | backend → client | one per directory the tree reveals |

Three properties keep the steady state small:

- **Highlights are viewport-scoped.** A view resolves them per rendered line, so
  spans outside it are never read. `Command::SetViewport` bounds them to the
  visible window plus a 200-line margin, so an ordinary scroll needs no round trip
  at all.
- **Only differences travel.** The `Arc`s in a snapshot are the session's own, so
  pointer identity settles "unchanged" without comparing spans. A keystroke's
  update carries a highlight slice and little else.
- **The client's own edits are never echoed back.** The backend tracks which
  versions the client produced, so text is sent only when the *backend* moved it —
  a format-on-save, an LSP rename, a reload, an undo. When it did, one edit is
  derived rather than the document resent.

## The replica

The client holds a replica of each open document: derived, discardable, never
authoritative. It exists so a keystroke can be echoed without waiting for the
network — the client applies its own edit immediately and the backend's
acknowledgement confirms a version it already reached.

A replica can diverge, if a backend edit arrives that does not fit the text the
client has. When that happens the client discards it and asks for the document
again (`ClientFrame::Resync`); the backend forgets what it believed the client had
and describes it from scratch. Divergence is recoverable by design — an early
version was not, and a document that diverged stayed blank for the rest of the
session.

## Who owns what

| | Backend | Client |
|---|---|---|
| Files, git, language servers | ✓ | |
| Documents, undo history | ✓ | |
| Workspace config (`.editorconfig`, project settings, linter and formatter config) | ✓ | |
| Search, seam index, spell scan | ✓ | |
| Theme, icon style | | ✓ |
| Keymap, terminal capabilities, graphics protocol | | ✓ |
| Tabs, panes, carets, scroll | | ✓ |
| Image decoding, PDF rasterization | | ✓ |

Settings split by *what they describe*. The backend resolves the configuration
that describes the code, including layers a client cannot even read. It has no
standing to say which theme suits the terminal the user is looking at, so a client
keeps those keys across the backend's configuration.

Media is decoded by the client because rendering it needs the client's cell grid,
graphics protocol and DPI — and because a PNG is far smaller than the pixels it
becomes.

The backend announces its roots and its configuration on attach, before anything
that depends on them. A client's own working directory says nothing about which
workspace it is rendering.

## The wire

CBOR (`ciborium`) behind a length-prefixed frame:

```
[u32 BE length][u8 codec: 0 = raw, 1 = deflate][body]
```

Compression is decided per frame above 4 KiB and recorded in the tag, so a
highlight payload pays for deflate and a keystroke acknowledgement does not.

**CBOR because self-describing was the deciding property.** The two ends run on
different machines and nobody upgrades both at once, so a field added to an
existing message must decode on an older peer. The protocol version is a *floor*,
not an exact match — a newer peer is accepted, and `karet` versions are exchanged
for diagnostics but never gate a connection.

The honest limit: serde cannot decode an enum variant it has never heard of. A
frame that fails to decode is therefore **skipped, not fatal** — a peer speaking a
newer protocol loses the feature the older one cannot name, not the session.

One command deliberately cannot cross: `Command::GithubLogin` refuses to
serialize. A token is authenticated on the host that holds the repository, and the
type enforces that rather than trusting the transport.

## Reattaching

The backend outlives its client. Events carry a monotonic sequence number and a
bounded replay ring, so a reattaching client presents what it last saw and is
either replayed from there or told to resynchronize.

A client also checkpoints its view state — tabs, panes, carets — as an opaque
blob the backend stores and never interprets. The client owns the meaning; the
backend owns the durability, because a client process does not outlive its
connection.

## Multiplexer integration

Inside a terminal multiplexer that supports **split-app panes**, none of the above
needs configuring: `karet` in a pane asks the multiplexer to host `karet --client`
on the machine with the display and to forward a channel back.

karet offers that only to a multiplexer that has **declared** a split-app contract
revision this build speaks, through `KMUX_SPLIT_APP`. Nothing is inferred — not
from a version number, and not from what `kmux help` prints. The contract covers
how the client half is spawned and how it is handed its endpoint, so acting on a
guess would engage a calling convention that has not been agreed and fail after
the editor had already given up its local session. Since no kmux exports the
variable yet, this path is **inert today**: nothing is spawned on the startup path
and karet runs locally.

karet talks to [kmux](https://github.com/getkono/kmux) only through the `kmux`
command-line tool and the documented `KMUX_*` environment variables. That is a
licence boundary, not a preference: kmux is `AGPL-3.0-only OR
LicenseRef-Commercial` with every crate unpublished, karet is `MIT OR Apache-2.0`,
and `xtask publish-closure` gates releases on the difference. Shelling out also
decouples the release cadences — kmux pins an exact protocol version between its
own halves, and karet should not inherit that.

Every failure there means "run locally": no multiplexer, one without split-app
support, a refusal, a timeout. The fallback is exactly the editor the user would
otherwise have had, so none of them is worth an error message.

The multiplexer side is tracked at
[getkono/kmux#201](https://github.com/getkono/kmux/issues/201). Two things are
still open there and are deliberately not guessed at in karet: how the hosted
client is told which endpoint to connect to, and how a pane's startup flags (the
file to open, `--goto`, `--command`) reach it. Until both are settled and kmux
declares its revision, `--client-exec` is the way to run a split session.

## Known gaps

- **The review store and log files stay on the client.** Per-user state, not
  workspace state — but a review begun on one machine is not visible from another.
- **`--seam-query` and `--capture` are local-only.** Both are one-shot answers
  with nobody watching a screen, so a split would make nothing faster.
- **Path canonicalization is lexical on a client.** `fs::canonicalize` cannot
  resolve a path that is not on this machine, so tab de-duplication compares
  lexically. Two spellings of one file that differ only through a symlink would
  open twice.
- **No reconnection UI.** A dropped connection ends the client. The backend
  survives, and reattaching resumes the session, but nothing yet retries
  automatically or shows a "reconnecting" state.
