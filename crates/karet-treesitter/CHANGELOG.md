# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.6.0](https://github.com/getkono/karet/compare/karet-treesitter-v0.5.0...karet-treesitter-v0.6.0) - 2026-08-22

### Added

- *(treesitter)* extend graphql injection to typescript and marker templates

### Other

- Merge remote-tracking branch 'origin/master' into feat/seam-view
- justify the remaining bare allows

## [0.4.0](https://github.com/getkono/karet/compare/karet-treesitter-v0.3.0...karet-treesitter-v0.4.0) - 2026-08-06

### Added

- *(editor)* add semantic inline macros
- *(outline)* support structured data formats
- *(outline)* add layered web languages
- *(outline)* support programming languages
- *(outline)* support shell languages
- *(outline)* support document markup formats
- *(outline)* support build languages
- *(outline)* support query and schema languages
- *(outline)* add tree-sitter symbol fallback
- *(syntax)* add modern language grammars
- *(treesitter)* add LaTeX syntax support

### Other

- *(language)* decouple file type identities

## [0.3.0](https://github.com/getkono/karet/compare/karet-treesitter-v0.2.2...karet-treesitter-v0.3.0) - 2026-07-19

### Added

- *(karet-treesitter)* expose semantic structure queries
- *(karet-treesitter)* expose syntax-error line ranges

### Other

- *(engines)* move oversized unit suites into modules
- *(config)* document semantic sticky scroll

## [0.2.2](https://github.com/getkono/karet/compare/karet-treesitter-v0.2.1...karet-treesitter-v0.2.2) - 2026-07-10

### Added

- *(karet-treesitter)* inject markdown into Rust doc comments
- *(karet-treesitter)* layered parsing of injected languages
- *(karet-treesitter)* injection query registry and language-name resolver

### Other

- describe language injection across the crate docs
- *(karet-treesitter)* build the line index once per layered parse
- *(karet-treesitter)* expand injected layers breadth-first

## [0.2.0](https://github.com/getkono/karet/compare/karet-treesitter-v0.1.0...karet-treesitter-v0.2.0) - 2026-07-04

### Added

- *(syntax)* language-agnostic tree-sitter fold regions

## [0.1.0](https://github.com/getkono/karet/releases/tag/karet-treesitter-v0.1.0) - 2026-07-02

### Added

- *(filetype)* add karet-filetype crate for file-type metadata
- *(session)* wire live document store with undo/redo and fs watching
- *(karet-treesitter)* parse host + grammar registry + extension detection
- implement core API and introduce session backend

### Other

- apply diff-resilient rustfmt (cargo +nightly fmt)
- update CI and documentation for workspace
