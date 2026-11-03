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
  it paints spans only for the lines it shows: its window, and the sticky-scroll
  headers of the blocks it is inside. `DocumentCommand::SetViewport` bounds them
  to the visible window plus a 200-line margin (and those headers, however far
  above it they are). Bracket matching reads further: up to 10,000 lines past
  the view it asks whether a bracket sits in a string or comment, so the string
  and comment spans that contain a bracket travel within that reach too. A scroll that stays inside the window last published
  republishes nothing, so an ordinary scroll needs no round trip at all, and
  `DocumentCommand::ClearViewport` withdraws a closed view's range. Only a
  client of a remote backend declares viewports; a local session keeps
  whole-document highlights, which cost it nothing to share.
- **Only differences travel.** The `Arc`s in a snapshot are the session's own, so
  pointer identity settles "unchanged" without comparing spans; a narrowed
  highlight slice keeps its `Arc` across republishes while its spans are the
  same. A keystroke's update carries a highlight slice and little else.
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
session. Until that description arrives the client holds no replica, and an
update carrying no full text — one in flight when the replica was discarded — is
dropped rather than applied to nothing.

The backend can also refuse a keystroke: it moved the document (an undo, a
format, a reload) before the edit arrived. The client numbers its optimistic
edits with the document's own counter, so a second keystroke stacked on the
refused one can name exactly the version the backend's move produced, while
describing text the backend never held. Two rules keep such an edit out. The
backend forwards one client edit to the session at a time, so a stacked edit is
never applied before the backend knows whether the one beneath it was. And each
command reports `seen`, the newest document description the client's
presentation layer had taken when the command was composed (stamped as it is
sent, not when the connection writes it, since the connection may already hold
a re-description the editor has not drawn): after a refusal, an edit of that
document composed before the complete re-description is dropped, and the
client does not echo it onto the re-described replica either. The keystroke is lost; the re-description puts the tab back on the
backend's text.

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

**Not implemented yet.** A served session lives for exactly one connection:
`karet --serve` creates the `Session` when a client attaches and drops it when the
connection ends, so there is nothing for a later client to resume. The protocol
leaves room for it — events carry a monotonic sequence number, `Attach` names the
last one a client saw, and a client may checkpoint its view state as an opaque
blob — but this build's backend retains no frames to replay, always answers an
attach with `resumed: false`, and always sends no view state. A checkpoint is
accepted and kept for the connection's lifetime, never replayed. The client, in
turn, always attaches as fresh (sequence 0).

A frame is capped at 64 MiB of uncompressed body, the same quantity on both ends.
An event too large for one frame is skipped rather than ending the connection;
when it answered a request, an error notification takes its place on that request
id. A command too large for one frame is answered the same way by the client,
and an edit refused so asks for its document again, since the client's own text
already holds it.

Each end writes its frames from a queue beside its read loop, never from inside
it, so a large frame written each way at once — a big paste against a file
chunk — cannot leave both ends waiting for the other to read.

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
[getkono/kmux#201](https://github.com/getkono/kmux/issues/201). karet's request
ends the hosted command at a bare `--client`, whose value is the socket to
connect to: only kmux knows the client-side end of the channel it forwards, so
revision 1 has kmux append that path. How a pane's startup flags (the file to
open, `--goto`, `--command`) reach the hosted client is still open there. Until
kmux declares its revision, `--client-exec` is the way to run a split session.

## Known gaps

- **The review store and log files stay on the client.** Per-user state, not
  workspace state — but a review begun on one machine is not visible from another.
- **`--seam-query` and `--capture` are local-only.** Both are one-shot answers
  with nobody watching a screen, so a split would make nothing faster.
- **A client takes no startup paths.** A PATH argument, `--open`, `--preview`,
  `--split`, `--goto` and `--diff` name files on the machine they are typed on,
  which is not the workspace's, so a client refuses them before taking the
  terminal. Open files from the editor once it attaches.
- **A client connects before it takes the terminal.** A `--client-exec` command
  that asks the user something — an `ssh` password, a host key, a second factor
  — asks on the plain terminal, and is not cut off. A backend that never greets
  is ended with Ctrl+C. A handshake that fails is reported with the last lines
  the command wrote to stderr (captured throughout, so nothing it writes later
  is drawn over the editor).
- **Path canonicalization is lexical on a client.** `fs::canonicalize` cannot
  resolve a path that is not on this machine, so tab de-duplication compares
  lexically. Two spellings of one file that differ only through a symlink would
  open twice.
- **No resume, no reattach, no reconnection UI.** A dropped connection ends the
  backend's session with it: unsaved edits there are covered only by the
  backend's crash-recovery swaps. The client tells the user the backend is gone
  and refuses further commands, but nothing retries, resumes from a sequence
  number, or restores checkpointed view state.
