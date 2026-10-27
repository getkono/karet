# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.8.0](https://github.com/getkono/karet/compare/karet-text-v0.7.1...karet-text-v0.8.0) - 2026-10-27

### Added

- cap undo history and persist it across restarts ([#467](https://github.com/getkono/karet/pull/467))

### Fixed

- *(text)* clamp utf16_to_line_col's column to the line content ([#748](https://github.com/getkono/karet/pull/748))
- *(text)* strip every rope line break in the line accessors ([#741](https://github.com/getkono/karet/pull/741))
- map carets through external edits; undo/redo restore the full selection set in the issuing view ([#430](https://github.com/getkono/karet/pull/430))

### Other

- [**breaking**] tighten small-engine surfaces (text, filetype, pdf, search, notebook, markdown, xtask) ([#809](https://github.com/getkono/karet/pull/809))
- *(syntax,text,treesitter)* [**breaking**] name shared vocabulary only from karet-core ([#806](https://github.com/getkono/karet/pull/806))
- *(text,editor)* move the word model and buffer slicing into karet-text ([#797](https://github.com/getkono/karet/pull/797))
- extend opt-in mutation testing to the pure, fast modules ([#681](https://github.com/getkono/karet/pull/681))
- remove duplicated helpers and dead code in the leaf engine crates ([#717](https://github.com/getkono/karet/pull/717))
- correct drifted prose and doc comments in the leaf engine crates ([#692](https://github.com/getkono/karet/pull/692))
- *(text)* make karet-text's save, apply and line-break tests fail when the behaviour breaks ([#632](https://github.com/getkono/karet/pull/632))
- run in-file find and search highlights over the rope ([#435](https://github.com/getkono/karet/pull/435))
- remove whole-document copies per keystroke ([#419](https://github.com/getkono/karet/pull/419))

### Added

- `karet_text::words`: the grapheme-aware word model (`word_at`, `word_bounds`, `class_run_at`, `identifier_at`, `previous_word_boundary`, `next_word_boundary`, `line_span`, and the cluster primitives `cluster_bounds`, `clusters`, `previous_cluster`, `next_cluster`), moved here from `karet-editor` so headless consumers share it
- `TextBuffer::slice_range(Range)`: the text within a line/column range, read off the rope without copying the whole document

## [0.7.1](https://github.com/getkono/karet/compare/karet-text-v0.7.0...karet-text-v0.7.1) - 2026-10-01

### Fixed

- *(session)* align gutter-marker line splitting with the editor buffer ([#314](https://github.com/getkono/karet/pull/314))
- *(session)* undo a save's rewrites back to the caret, as one step ([#299](https://github.com/getkono/karet/pull/299))

## [0.6.0](https://github.com/getkono/karet/compare/karet-text-v0.5.0...karet-text-v0.6.0) - 2026-08-29

### Fixed

- *(text)* fingerprint content with a fixed algorithm

### Other

- point shadowed crate readmes at their local files

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
