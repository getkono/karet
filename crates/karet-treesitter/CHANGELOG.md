# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
