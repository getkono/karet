# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.3.0](https://github.com/getkono/karet/compare/karet-vcs-v0.2.2...karet-vcs-v0.3.0) - 2026-07-19

### Added

- *(vcs)* expose the staged diff as a unified patch

### Other

- *(engines)* move oversized unit suites into modules
- Merge pull request #71 from getkono/feat/46-aicommit-integration

## [0.2.1](https://github.com/getkono/karet/compare/karet-vcs-v0.2.0...karet-vcs-v0.2.1) - 2026-07-09

### Added

- *(karet-vcs)* range, merge-base, and upstream diff primitives
- *(karet-session)* lazy GitHub commit-verification behind default 'github' feature
- *(karet-vcs)* commit-vs-parent diff and path-scoped file history
- *(karet-vcs)* rich CommitDetail + signature extraction

## [0.2.0](https://github.com/getkono/karet/compare/karet-vcs-v0.1.0...karet-vcs-v0.2.0) - 2026-07-04

### Added

- *(vcs)* capture commit parents for DAG rendering
- *(app)* remember diff layout for new diffs
- *(vcs,session)* incremental commit-log reconciliation on ref change
- *(karet-vcs)* expose paginated commit log

## [0.1.0](https://github.com/getkono/karet/releases/tag/karet-vcs-v0.1.0) - 2026-07-02

### Added

- *(vcs)* add optional git2 staging backend
- *(vcs)* expose git metadata dirs for watching
- *(vcs)* report conflicts and list files in new untracked dirs
- *(karet-vcs)* gix-backed discovery, selection & change enumeration
- implement core API and introduce session backend

### Fixed

- *(vcs)* always detect staged renames and keep status merge-robust
- resolve relative pathspec against current directory

### Other

- apply diff-resilient rustfmt (cargo +nightly fmt)
- *(vcs,session)* cover linked-worktree staging and fs-event refresh
- document the karet diff viewer; correct karet-diff/karet-vcs READMEs
- update CI and documentation for workspace
