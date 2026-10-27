# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.8.0](https://github.com/getkono/karet/compare/karet-filetype-v0.7.1...karet-filetype-v0.8.0) - 2026-10-27

### Added

- auto-close brackets and quotes with type-over, pair delete, wrap and smart Enter ([#439](https://github.com/getkono/karet/pull/439))
- toggle line and block comments ([#398](https://github.com/getkono/karet/pull/398))

### Fixed

- *(filetype)* end the banner header block at a CRLF blank line ([#739](https://github.com/getkono/karet/pull/739))
- *(session)* honour the language id passed to OpenDocument ([#698](https://github.com/getkono/karet/pull/698))

### Other

- [**breaking**] tighten small-engine surfaces (text, filetype, pdf, search, notebook, markdown, xtask) ([#809](https://github.com/getkono/karet/pull/809))
- *(filetype)* share one binary sniffer with diff and search ([#770](https://github.com/getkono/karet/pull/770))
- remove duplicated helpers and dead code in the leaf engine crates ([#717](https://github.com/getkono/karet/pull/717))
- correct drifted prose and doc comments in the leaf engine crates ([#692](https://github.com/getkono/karet/pull/692))
- *(filetype)* pin icon-style parsing, classify edges and CRLF banners ([#640](https://github.com/getkono/karet/pull/640))

## [0.6.0](https://github.com/getkono/karet/compare/karet-filetype-v0.5.0...karet-filetype-v0.6.0) - 2026-08-29

### Added

- *(filetype)* classify jupyter notebooks
- *(filetype)* recognize the .graphqls schema extension
- *(app)* mermaid diagrams in the markdown preview

### Other

- Merge remote-tracking branch 'origin/master' into feat/commit-collapse-generated

## [0.4.0](https://github.com/getkono/karet/compare/karet-filetype-v0.3.0...karet-filetype-v0.4.0) - 2026-08-06

### Added

- *(outline)* support structured data formats
- *(outline)* support programming languages
- *(outline)* support shell languages
- *(outline)* support document markup formats
- *(syntax)* add modern language grammars
- *(treesitter)* add LaTeX syntax support

### Other

- *(language)* decouple file type identities

## [0.3.0](https://github.com/getkono/karet/compare/karet-filetype-v0.2.2...karet-filetype-v0.3.0) - 2026-07-19

### Added

- *(karet-filetype)* expose file wrap defaults

## [0.2.1](https://github.com/getkono/karet/compare/karet-filetype-v0.2.0...karet-filetype-v0.2.1) - 2026-07-09

### Fixed

- *(karet-filetype)* don't let the markdown extension skip the binary sniff

## [0.2.0](https://github.com/getkono/karet/compare/karet-filetype-v0.1.0...karet-filetype-v0.2.0) - 2026-07-04

### Added

- *(filetype)* recognize .docx as a distinct file kind

### Other

- Merge branch 'master' into feat/editor-refinements

## [0.1.0](https://github.com/getkono/karet/releases/tag/karet-filetype-v0.1.0) - 2026-07-02

### Added

- *(cbor)* add karet-cbor engine and wire it into session save/load
- *(filetype)* add karet-filetype crate for file-type metadata

### Other

- Merge branch 'feat/cbor'
- apply diff-resilient rustfmt (cargo +nightly fmt)
