# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
