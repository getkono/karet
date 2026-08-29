# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.6.0](https://github.com/getkono/karet/compare/karet-theme-v0.5.0...karet-theme-v0.6.0) - 2026-08-29

### Added

- *(core)* a stopped-line theme role
- *(theme)* add scrollbar track and thumb theme roles

### Other

- *(theme)* remove the unimplemented tmtheme claims
- point shadowed crate readmes at their local files

## [0.4.0](https://github.com/getkono/karet/compare/karet-theme-v0.3.0...karet-theme-v0.4.0) - 2026-08-06

### Added

- *(karet)* explain manual language-server requirements
- *(karet)* report existing application logs
- *(karet)* persist application diagnostics
- *(lsp)* manage shared language server toolchains

### Fixed

- *(karet)* keep loading commit views responsive

### Other

- *(readme)* capture the hero image from the real app
- *(readme)* add deterministic hero artwork
- unify pre-push and workflow checks
- *(lsp)* document server manager controls
- Merge remote-tracking branch 'origin/master' into series/19-feat-61-latex
- Merge branch 'master' into series/18-feat-97-spellcheck

## [0.3.0](https://github.com/getkono/karet/compare/karet-theme-v0.2.2...karet-theme-v0.3.0) - 2026-07-19

### Added

- *(karet-core)* add a semantic-comment token

## [0.2.2](https://github.com/getkono/karet/compare/karet-theme-v0.2.1...karet-theme-v0.2.2) - 2026-07-10

### Added

- *(theme)* add strikethrough support for markup
- *(karet-theme)* colors and text emphasis for markup scopes

### Other

- *(karet-theme)* add strikethrough field to Emphasis initializers
- Merge branch 'master' into feat/53-markdown-preview
- pin Rust toolchains to exact versions

## [0.2.1](https://github.com/getkono/karet/compare/karet-theme-v0.2.0...karet-theme-v0.2.1) - 2026-07-09

### Added

- *(karet-theme)* verified/unverified VCS badge roles

### Other

- *(readme)* refresh karet positioning
- Merge branch 'master' into feat/commit-view

## [0.2.0](https://github.com/getkono/karet/compare/karet-theme-v0.1.0...karet-theme-v0.2.0) - 2026-07-04

### Added

- *(core,theme,widgets,app)* explorer highlights track active/focused editors
- *(theme)* add Muted + file-icon category roles
- *(karet)* hover highlight in explorer and source control

## [0.1.0](https://github.com/getkono/karet/releases/tag/karet-theme-v0.1.0) - 2026-07-02

### Added

- *(filetype)* add karet-filetype crate for file-type metadata
- *(karet-theme)* built-in dark theme, contrast, vscode loader, ratatui view
- implement core API and introduce session backend

### Other

- *(release)* publish karet-fileview and its dependency chain
- *(release)* automate releases, enforce conventional commits, document versioning
- apply diff-resilient rustfmt (cargo +nightly fmt)
- document the karet diff viewer; correct karet-diff/karet-vcs READMEs
- add MSRV section to README
- update CI and documentation for workspace
- initialize project
