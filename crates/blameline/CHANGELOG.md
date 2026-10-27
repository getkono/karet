# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [3.0.0](https://github.com/getkono/karet/compare/blameline-v2.0.4...blameline-v3.0.0) - 2026-10-27

### Fixed

- *(blameline)* do not panic on a non-ASCII commit hash in short_hash ([#684](https://github.com/getkono/karet/pull/684))

### Other

- *(treesitter)* [**breaking**] make LanguageId unforgeable and name grammars by typed constants ([#796](https://github.com/getkono/karet/pull/796))
- *(blameline,session)* [**breaking**] carry author time as data and number lines from 1 ([#783](https://github.com/getkono/karet/pull/783))
- correct drifted prose and doc comments in the leaf engine crates ([#692](https://github.com/getkono/karet/pull/692))
- run tests under the feature subsets CI only checked ([#674](https://github.com/getkono/karet/pull/674))
- tighten karet-theme, karet-markdown and blameline tests ([#647](https://github.com/getkono/karet/pull/647))

## [2.0.4](https://github.com/getkono/karet/compare/blameline-v2.0.3...blameline-v2.0.4) - 2026-10-01

### Other

- updated the following local packages: karet-treesitter

## [2.0.3](https://github.com/getkono/karet/compare/blameline-v2.0.2...blameline-v2.0.3) - 2026-09-24

### Other

- updated the following local packages: karet-treesitter

## [2.0.2](https://github.com/getkono/karet/compare/blameline-v2.0.1...blameline-v2.0.2) - 2026-09-01

### Other

- updated the following local packages: karet-treesitter

## [2.0.1](https://github.com/getkono/karet/compare/blameline-v2.0.0...blameline-v2.0.1) - 2026-08-29

### Other

- *(blameline)* use the workspace tempfile version

## [1.1.0](https://github.com/getkono/karet/compare/blameline-v1.0.4...blameline-v1.1.0) - 2026-08-06

### Added

- *(vcs)* switch inline blame to line-only click-to-detail model

## [1.0.4](https://github.com/getkono/karet/compare/blameline-v1.0.3...blameline-v1.0.4) - 2026-07-19

### Other

- updated the following local packages: karet-core, karet-treesitter

## [1.0.3](https://github.com/getkono/karet/compare/blameline-v1.0.2...blameline-v1.0.3) - 2026-07-10

### Other

- updated the following local packages: karet-core, karet-treesitter

## [1.0.2](https://github.com/getkono/karet/compare/blameline-v1.0.1...blameline-v1.0.2) - 2026-07-09

### Other

- updated the following local packages: karet-core, karet-treesitter

## [1.0.1](https://github.com/getkono/karet/compare/blameline-v1.0.0...blameline-v1.0.1) - 2026-07-04

### Other

- update Cargo.toml dependencies

## [1.0.0](https://github.com/getkono/karet/releases/tag/blameline-v1.0.0) - 2026-07-02

### Added

- *(blameline)* add semantic git blame crate with karet integration

### Other

- apply diff-resilient rustfmt (cargo +nightly fmt)
