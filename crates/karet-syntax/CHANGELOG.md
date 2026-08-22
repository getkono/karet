# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.6.0](https://github.com/getkono/karet/compare/karet-syntax-v0.5.0...karet-syntax-v0.6.0) - 2026-08-22

### Added

- *(syntax)* detect color literals for inline swatches
- *(syntax)* expose codetag occurrences, not just their tint

### Other

- point shadowed crate readmes at their local files

## [0.4.0](https://github.com/getkono/karet/compare/karet-syntax-v0.3.0...karet-syntax-v0.4.0) - 2026-08-06

### Added

- *(karet)* explain manual language-server requirements
- *(karet)* report existing application logs
- *(karet)* persist application diagnostics
- *(lsp)* manage shared language server toolchains
- *(treesitter)* add LaTeX syntax support

### Other

- Merge pull request #166 from getkono/feat/inline-macros-152
- Merge pull request #165 from getkono/feat/config-language-outlines-137
- Merge pull request #164 from getkono/feat/layered-web-outlines-133
- Merge pull request #163 from getkono/feat/programming-language-outlines-132
- Merge pull request #162 from getkono/feat/shell-language-outlines-135
- Merge pull request #161 from getkono/feat/document-markup-outlines-138
- Merge pull request #160 from getkono/feat/build-language-outlines-136
- Merge pull request #159 from getkono/feat/query-schema-outlines-134
- Merge pull request #158 from getkono/feat/treesitter-outline-fallback-131
- *(readme)* capture the hero image from the real app
- *(readme)* add deterministic hero artwork
- unify pre-push and workflow checks
- *(lsp)* document server manager controls
- document the external LaTeX workflow
- explain optional spell checking

## [0.3.0](https://github.com/getkono/karet/compare/karet-syntax-v0.2.2...karet-syntax-v0.3.0) - 2026-07-19

### Added

- *(karet-syntax)* derive semantic block hierarchies
- *(karet-syntax)* detect semantic comment blocks

### Other

- *(config)* document semantic sticky scroll

## [0.2.2](https://github.com/getkono/karet/compare/karet-syntax-v0.2.1...karet-syntax-v0.2.2) - 2026-07-10

### Added

- *(karet-syntax)* translate highlight spans across an edit
- *(karet-syntax)* highlight injected languages across layers

### Other

- Merge branch 'master' into feat/53-markdown-preview
- pin Rust toolchains to exact versions
- *(karet-syntax)* use `?` for the dotted-capture fallback
- describe language injection across the crate docs

## [0.2.1](https://github.com/getkono/karet/compare/karet-syntax-v0.2.0...karet-syntax-v0.2.1) - 2026-07-09

### Other

- *(readme)* refresh karet positioning
- restructure design principles and testing guidance

## [0.2.0](https://github.com/getkono/karet/compare/karet-syntax-v0.1.0...karet-syntax-v0.2.0) - 2026-07-04

### Added

- *(editor,session,app)* render + toggle code folds
- *(syntax)* language-agnostic tree-sitter fold regions

## [0.1.0](https://github.com/getkono/karet/releases/tag/karet-syntax-v0.1.0) - 2026-07-02

### Added

- *(filetype)* add karet-filetype crate for file-type metadata
- *(karet-syntax)* tree-sitter highlighter (single-parse, no tree-sitter-highlight)
- implement core API and introduce session backend

### Other

- *(release)* publish karet-fileview and its dependency chain
- *(release)* automate releases, enforce conventional commits, document versioning
- apply diff-resilient rustfmt (cargo +nightly fmt)
- document the karet diff viewer; correct karet-diff/karet-vcs READMEs
- add MSRV section to README
- update CI and documentation for workspace
- initialize project
