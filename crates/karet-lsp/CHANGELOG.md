# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.8.0](https://github.com/getkono/karet/compare/karet-lsp-v0.7.1...karet-lsp-v0.8.0) - 2026-10-27

### Added

- fold regions from LSP textDocument/foldingRange ([#777](https://github.com/getkono/karet/pull/777))
- LSP document links and document colors ([#763](https://github.com/getkono/karet/pull/763))
- AI inline completion as ghost text via LSP inlineCompletion ([#727](https://github.com/getkono/karet/pull/727))
- re-pull diagnostics on refresh and across inter-file dependencies ([#693](https://github.com/getkono/karet/pull/693))
- call hierarchy and type hierarchy views ([#670](https://github.com/getkono/karet/pull/670))
- linked editing, on-type formatting and willSaveWaitUntil ([#660](https://github.com/getkono/karet/pull/660))
- semantic tokens layered over tree-sitter highlighting ([#511](https://github.com/getkono/karet/pull/511))
- highlight occurrences of the symbol under the caret ([#506](https://github.com/getkono/karet/pull/506))
- LSP snippets with tabstop and placeholder sessions ([#500](https://github.com/getkono/karet/pull/500))
- pull diagnostics, related information and codeDescription links ([#494](https://github.com/getkono/karet/pull/494))
- render LSP code lens ([#483](https://github.com/getkono/karet/pull/483))
- expand and shrink selection along the syntax tree ([#450](https://github.com/getkono/karet/pull/450))
- go to symbol in workspace through quick-open's # mode and Ctrl+T ([#447](https://github.com/getkono/karet/pull/447))
- Format Document and Format Selection commands ([#445](https://github.com/getkono/karet/pull/445))
- completion overhaul — textEdit ranges, auto-import, resolve + docs, trigger/commit chars, incomplete lists ([#436](https://github.com/getkono/karet/pull/436))
- richer inlay hints — per-kind styling, label parts, resolve, toggle ([#429](https://github.com/getkono/karet/pull/429))
- LSP protocol plumbing — progress, messages, configuration, watched files ([#422](https://github.com/getkono/karet/pull/422))
- find references and go to declaration, type definition and implementation ([#417](https://github.com/getkono/karet/pull/417))
- code actions and quick fix menu with a gutter lightbulb ([#413](https://github.com/getkono/karet/pull/413))
- rename symbol (F2) and a session-level workspace-edit apply path ([#402](https://github.com/getkono/karet/pull/402))

### Fixed

- *(session)* clear pulled diagnostics when a server withdraws pull support ([#701](https://github.com/getkono/karet/pull/701))

### Other

- *(editor,diff,lsp,session)* [**breaking**] drop the remaining karet-core re-exports ([#808](https://github.com/getkono/karet/pull/808))
- *(core)* [**breaking**] trim karet-core to shared vocabulary and make its constructors honest ([#785](https://github.com/getkono/karet/pull/785))
- *(core)* define HighlightKind and DocumentHighlight once in karet-core ([#780](https://github.com/getkono/karet/pull/780))
- correct drifted prose in protocol, VCS, seam and widget crates ([#690](https://github.com/getkono/karet/pull/690))
- pin karet-jsonrpc's queue/encode/timeout contracts and karet-lsp's gating ([#645](https://github.com/getkono/karet/pull/645))
- incremental LSP document sync ([#468](https://github.com/getkono/karet/pull/468))

## [0.7.0](https://github.com/getkono/karet/compare/karet-lsp-v0.6.1...karet-lsp-v0.7.0) - 2026-09-24

### Added

- *(lsp)* [**breaking**] scope dynamic registrations to their documentSelector
- *(lsp)* declare inlay hints to the server, and hear it ask for a refresh
- *(lsp)* expose the server's formatting capability
- *(jsonrpc)* let a caller await a connection's death

### Fixed

- *(lsp)* fail open on a document-selector pattern karet cannot use
- *(lsp)* [**breaking**] format with the indentation the buffer is actually in

### Other

- *(lsp)* say a document must be opened before asking what it supports
- put drain_stderr's comment back and narrow what close() logs
- say which server answers, and what the client really declares
- Merge remote-tracking branch 'origin/master' into feat/language-intelligence
- describe what formatOnSave actually does

## [0.6.0](https://github.com/getkono/karet/compare/karet-lsp-v0.5.0...karet-lsp-v0.6.0) - 2026-08-29

### Added

- *(lsp)* say why a language server failed to launch
- *(lsp)* a generic request and notification escape hatch

### Fixed

- *(supervisor)* make a broker prove who it is before its silence is a verdict
- *(lsp)* report the reason a launch failed, and keep a slow server
- *(session)* re-derive managed launch arguments, and tighten installed modes
- *(session)* pin TypeScript to 5 and give Astro its SDK path

### Other

- *(lsp)* [**breaking**] seal LspSpec so the next field is additive
- *(jsonrpc)* harden the extracted core against review findings
- *(jsonrpc)* extract the protocol-agnostic correlation actor
- Merge remote-tracking branch 'origin/master' into feat/seam-view
- *(lsp)* only clone a notification for the raw fan-out when someone listens
- justify the remaining bare allows

## [0.4.0](https://github.com/getkono/karet/compare/karet-lsp-v0.2.2...karet-lsp-v0.4.0) - 2026-08-06

### Added

- *(lsp)* complete typed protocol operations
- *(lsp)* manage shared language server toolchains
- *(lsp)* implement document symbol requests
- *(karet-lsp)* completion requests mapped to core models
- *(karet-lsp)* implement the JSON-RPC transport and client lifecycle

### Fixed

- *(karet-lsp)* fail requests issued on a dead connection fast

### Other

- *(lsp)* split oversized runtime modules
- release

## [0.1.0](https://github.com/getkono/karet/releases/tag/karet-lsp-v0.1.0) - 2026-07-02

### Added

- *(session)* wire live document store with undo/redo and fs watching
- implement core API and introduce session backend

### Other

- apply diff-resilient rustfmt (cargo +nightly fmt)
- update CI and documentation for workspace
