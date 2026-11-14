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
workflow surface. `generated-operations.json` lists the operations spargen
generates: since spargen 0.6.0 that is all 121, because the generator gaps that
used to block 44 of them (spargen#45, #46, #47, #48) are fixed upstream.
`manual-operations.json` is therefore empty. It stays as the place to record any
future operation spargen rejects, together with the upstream spargen issue
blocking it; `build.rs` requires that link and checks that the three lists
partition the surface.

An operation in `generated-operations.json` can still be called through a
hand-written adapter in `GitHubClient` that decodes only the fields karet reads
(issues, search, comments, workflows, timelines, and others do this today).
Moving those adapters onto the generated methods is a separate change.

`build.rs` verifies the source checksum and manifest partition, derives the
transitive filtered OpenAPI document, and runs pinned spargen 0.6.0 with
`CargoIntegration::Required`, so spargen's consumer-manifest dependency audit
(E023) always runs and fails the build if `Cargo.toml` falls below a runtime
dependency floor. Both the filtered document and generated Rust are written only
to Cargo's `OUT_DIR`. Generated client code is never tracked and must never be
patched manually.

To refresh the source, replace it with the official JSON from a reviewed upstream
commit, update the checksum here and in `build.rs`, then run the normal format,
lint, and test gates. A regular Cargo build regenerates the client automatically.
