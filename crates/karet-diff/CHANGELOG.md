# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.6.0](https://github.com/getkono/karet/compare/karet-diff-v0.5.0...karet-diff-v0.6.0) - 2026-08-22

### Added

- *(diff)* say why a file has no hunks instead of painting nothing

### Fixed

- *(diff)* bound the intra-line LCS and detect binary in diff_files
- *(diff)* reproduce original bytes when rebuilding a patch
- *(diff)* parse every block shape git emits

## [0.1.0](https://github.com/getkono/karet/releases/tag/karet-diff-v0.1.0) - 2026-07-02

### Added

- *(karet-diff)* port the unified-diff parser + scope extraction
- *(karet-diff)* port diff model + imara-diff engine, align, intraline, patch
- implement core API and introduce session backend

### Other

- apply diff-resilient rustfmt (cargo +nightly fmt)
- document the karet diff viewer; correct karet-diff/karet-vcs READMEs
- update CI and documentation for workspace
