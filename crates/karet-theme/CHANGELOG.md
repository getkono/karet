# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
