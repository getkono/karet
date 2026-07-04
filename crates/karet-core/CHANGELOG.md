# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.0](https://github.com/getkono/karet/compare/karet-core-v0.1.0...karet-core-v0.2.0) - 2026-07-04

### Added

- *(core)* neutral GraphView model for visualizations
- *(core,theme,widgets,app)* explorer highlights track active/focused editors
- *(theme)* add Muted + file-icon category roles
- *(karet)* hover highlight in explorer and source control
- *(core)* add Notification model, NotificationKind, and severity_role

### Other

- *(viz)* document the visualization suite and dependable requests
- *(editor,core)* back EditorState with CursorState

## [0.1.0](https://github.com/getkono/karet/releases/tag/karet-core-v0.1.0) - 2026-07-02

### Added

- implement core API and introduce session backend

### Other

- apply diff-resilient rustfmt (cargo +nightly fmt)
- update CI and documentation for workspace
