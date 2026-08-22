# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.6.0](https://github.com/getkono/karet/compare/karet-editor-v0.5.0...karet-editor-v0.6.0) - 2026-08-22

### Added

- *(editor)* hit-test the gutter marker column
- *(core)* a color-swatch decoration carrying its own color
- *(ui)* drag, page and step the scrollbars
- *(editor)* report the last render's scroll extents

### Other

- split the files that outgrew the code-line ceiling
- point shadowed crate readmes at their local files
- Merge branch 'master' into feat/goto-definition-187

## [0.4.0](https://github.com/getkono/karet/compare/karet-editor-v0.3.0...karet-editor-v0.4.0) - 2026-08-06

### Added

- *(karet)* explain manual language-server requirements
- *(karet)* report existing application logs
- *(karet)* persist application diagnostics
- *(lsp)* manage shared language server toolchains
- *(editor)* render diagnostic underlines
- *(editor)* exempt structural lines from soft wrapping
- *(karet)* honor resolved indentation and tab widths
- *(editor)* model merge conflict decorations
- *(vcs)* switch inline blame to line-only click-to-detail model
- *(app)* add git workflows and live blame

### Other

- Merge pull request #168 from getkono/feat/text-selection-ergonomics-141
- *(readme)* capture the hero image from the real app
- *(readme)* add deterministic hero artwork
- unify pre-push and workflow checks
- *(lsp)* document server manager controls
- document the external LaTeX workflow
- explain optional spell checking

## [0.3.0](https://github.com/getkono/karet/compare/karet-editor-v0.2.2...karet-editor-v0.3.0) - 2026-07-19

### Added

- *(karet-editor)* render sticky semantic headers
- *(karet-editor)* support wrapped and overflow viewports

### Other

- *(karet-editor)* separate editor state and rendering
- *(config)* document semantic sticky scroll

## [0.2.2](https://github.com/getkono/karet/compare/karet-editor-v0.2.1...karet-editor-v0.2.2) - 2026-07-10

### Added

- *(karet-theme)* colors and text emphasis for markup scopes

### Other

- Merge branch 'master' into feat/53-markdown-preview
- pin Rust toolchains to exact versions

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
