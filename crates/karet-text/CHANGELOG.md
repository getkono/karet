# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
