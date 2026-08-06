# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.4.0](https://github.com/getkono/karet/compare/karet-text-v0.3.0...karet-text-v0.4.0) - 2026-08-06

### Added

- *(karet)* explain manual language-server requirements
- *(karet)* report existing application logs
- *(karet)* persist application diagnostics
- *(lsp)* manage shared language server toolchains
- *(session)* resolve EditorConfig per document

### Other

- Merge pull request #178 from getkono/fix/open-cli-path-153
- *(readme)* capture the hero image from the real app
- *(readme)* add deterministic hero artwork
- unify pre-push and workflow checks
- *(lsp)* document server manager controls
- document the external LaTeX workflow
- explain optional spell checking

## [0.2.2](https://github.com/getkono/karet/compare/karet-text-v0.2.1...karet-text-v0.2.2) - 2026-07-10

### Other

- Merge branch 'master' into feat/53-markdown-preview
- pin Rust toolchains to exact versions

## [0.2.1](https://github.com/getkono/karet/compare/karet-text-v0.2.0...karet-text-v0.2.1) - 2026-07-09

### Fixed

- track unsaved state by text content

### Other

- *(readme)* refresh karet positioning
- Merge branch 'master' into test/startup-smoke-and-cursor-research
- Merge branch 'master' of github.com:getkono/karet
- restructure design principles and testing guidance

## [0.2.0](https://github.com/getkono/karet/compare/karet-text-v0.1.0...karet-text-v0.2.0) - 2026-07-04

### Added

- *(text)* expose content_fingerprint for change detection

### Other

- *(editor,core)* back EditorState with CursorState

## [0.1.0](https://github.com/getkono/karet/releases/tag/karet-text-v0.1.0) - 2026-07-02

### Added

- *(cbor)* add karet-cbor engine and wire it into session save/load
- *(filetype)* add karet-filetype crate for file-type metadata
- *(session)* wire live document store with undo/redo and fs watching
- *(karet-text)* read-only file loading & coordinate conversion
- implement core API and introduce session backend

### Other

- Merge branch 'feat/cbor'
- apply diff-resilient rustfmt (cargo +nightly fmt)
- document the karet diff viewer; correct karet-diff/karet-vcs READMEs
- add MSRV section to README
- update CI and documentation for workspace
- initialize project
