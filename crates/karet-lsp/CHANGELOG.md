# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.6.0](https://github.com/getkono/karet/compare/karet-lsp-v0.5.0...karet-lsp-v0.6.0) - 2026-08-22

### Added

- *(lsp)* a generic request and notification escape hatch

### Other

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
