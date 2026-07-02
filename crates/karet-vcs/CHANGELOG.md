# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.1](https://github.com/getkono/karet/compare/karet-vcs-v0.1.0...karet-vcs-v0.1.1) - 2026-07-02

### Added

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
