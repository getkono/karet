# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.1](https://github.com/getkono/karet/compare/karet-editor-v0.2.0...karet-editor-v0.2.1) - 2026-07-09

### Added

- *(karet-editor)* expose caret geometry

### Other

- *(readme)* refresh karet positioning
- restructure design principles and testing guidance

## [0.2.0](https://github.com/getkono/karet/compare/karet-editor-v0.1.0...karet-editor-v0.2.0) - 2026-07-04

### Added

- *(editor,app)* multi-cursor add / next-occurrence / Alt-click
- *(editor,app)* complete keyboard text-selection vocabulary
- *(editor,session,app)* render + toggle code folds

### Other

- *(editor,core)* back EditorState with CursorState

## [0.1.0](https://github.com/getkono/karet/releases/tag/karet-editor-v0.1.0) - 2026-07-02

### Added

- *(editor)* add read-only rendering mode
- *(filetype)* add karet-filetype crate for file-type metadata
- *(karet)* editor caret, click-to-position & text selection
- *(karet-editor)* read-only editor widget render
- implement core API and introduce session backend

### Other

- *(release)* publish karet-fileview and its dependency chain
- *(fileview)* add runnable render-any-file example
- *(release)* automate releases, enforce conventional commits, document versioning
- apply diff-resilient rustfmt (cargo +nightly fmt)
- document the karet diff viewer; correct karet-diff/karet-vcs READMEs
- add MSRV section to README
- update CI and documentation for workspace
- initialize project
