# GitHub REST API description

`api.github.com.json` is vendored verbatim from GitHub's public OpenAPI
description. It is the sole source of truth for generated GitHub wire types.

- Source: <https://raw.githubusercontent.com/github/rest-api-description/03ca9c1cac754ec9b8369dc75de8a8c753c6e087/descriptions-next/api.github.com/api.github.com.json>
- Upstream commit: `03ca9c1cac754ec9b8369dc75de8a8c753c6e087`
- Retrieved: 2026-07-21
- SHA-256: `d88008d8198becda210d59fbe64a6554bcc4c979be2348e2e356638b369eee47`
- OpenAPI: `3.1.0`
- Description version: `1.1.4`
- REST API header version: `2026-03-10`

`surface-operations.json` is the complete reviewed 121-operation repository
workflow surface. `generated-operations.json` contains the 77 operations that
pass spargen's strict audit and compile. `manual-operations.json` records the 44
operations temporarily implemented by typed adapters, together with the real
upstream issue blocking each operation.

`build.rs` verifies the source checksum and manifest partition, derives the
transitive filtered OpenAPI document, and runs pinned spargen 0.2.1. Both the
filtered document and generated Rust are written only to Cargo's `OUT_DIR`.
Generated client code is never tracked and must never be patched manually.

Verified upstream gaps:

- [Union-valued parameters can generate uncompilable Rust](https://github.com/getkono/spargen/issues/45)
- [Remaining typed GitHub `oneOf`/`anyOf` coverage](https://github.com/getkono/spargen/issues/46)
- [Duplicate diagnostics for shared components](https://github.com/getkono/spargen/issues/47)
- [Embedded runtime fails strict consumer lints](https://github.com/getkono/spargen/issues/48)

To refresh the source, replace it with the official JSON from a reviewed upstream
commit, update the checksum here and in `build.rs`, then run the normal format,
lint, and test gates. A regular Cargo build regenerates the client automatically.
