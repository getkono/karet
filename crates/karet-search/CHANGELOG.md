# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.8.0](https://github.com/getkono/karet/compare/karet-search-v0.7.1...karet-search-v0.8.0) - 2026-10-27

### Added

- find bar current-match style, search in selection, matches to carets, preserve case ([#465](https://github.com/getkono/karet/pull/465))

### Fixed

- *(search)* never match an empty query, and report replace write failures ([#694](https://github.com/getkono/karet/pull/694))

### Other

- [**breaking**] tighten small-engine surfaces (text, filetype, pdf, search, notebook, markdown, xtask) ([#809](https://github.com/getkono/karet/pull/809))
- *(filetype)* share one binary sniffer with diff and search ([#770](https://github.com/getkono/karet/pull/770))
- *(karet)* list quick-open files through karet_search::walk_files ([#750](https://github.com/getkono/karet/pull/750))
- remove duplicated helpers and dead code in the leaf engine crates ([#717](https://github.com/getkono/karet/pull/717))
- pin karet-search, karet-watch and karet-fuzzy behaviours ([#639](https://github.com/getkono/karet/pull/639))
- run in-file find and search highlights over the rope ([#435](https://github.com/getkono/karet/pull/435))

## [0.6.0](https://github.com/getkono/karet/compare/karet-search-v0.5.0...karet-search-v0.6.0) - 2026-08-29

### Added

- *(search)* expose the reusable compiled matcher

### Other

- *(search)* expose the gitignore-aware text-file walk

## [0.2.0](https://github.com/getkono/karet/compare/karet-search-v0.1.0...karet-search-v0.2.0) - 2026-07-04

### Added

- *(search)* implement literal and regex find/replace

## [0.1.0](https://github.com/getkono/karet/releases/tag/karet-search-v0.1.0) - 2026-07-02

### Added

- *(karet-search)* gitignore-aware streaming workspace search
- *(karet-search)* regex-aware in-file search
- implement core API and introduce session backend

### Other

- apply diff-resilient rustfmt (cargo +nightly fmt)
- update CI and documentation for workspace
