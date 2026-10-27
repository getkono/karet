# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.8.0](https://github.com/getkono/karet/compare/karet-fileview-v0.7.1...karet-fileview-v0.8.0) - 2026-10-27

### Fixed

- *(app)* chunk the graphical caret's Kitty transmit at the 4096-byte limit ([#775](https://github.com/getkono/karet/pull/775))

### Other

- *(fileview)* [**breaking**] one path per item and a public hex row_count ([#762](https://github.com/getkono/karet/pull/762))
- *(fileview)* build Kitty transmit and place escapes in the image API ([#733](https://github.com/getkono/karet/pull/733))
- share karet-fileview's human_size byte formatter with the app ([#712](https://github.com/getkono/karet/pull/712))
- correct drifted prose in protocol, VCS, seam and widget crates ([#690](https://github.com/getkono/karet/pull/690))
- render-test karet-editor, karet-widgets and karet-fileview gaps ([#625](https://github.com/getkono/karet/pull/625))

### Added

- `hex::row_count(bytes)`, the hex dump's row count without building a `HexView`, so a consumer clamps scroll from the crate's own row geometry

### Changed

- **Breaking:** the crate root no longer re-exports `HexView`; import it as `karet_fileview::hex::HexView`, so every item has one path (its owning module, and `viewer` for the `karet-filetype` routing items)

## [0.7.1](https://github.com/getkono/karet/compare/karet-fileview-v0.7.0...karet-fileview-v0.7.1) - 2026-10-01

### Added

- *(markdown)* render curated HTML and local images in the preview ([#297](https://github.com/getkono/karet/pull/297))

## [0.6.0](https://github.com/getkono/karet/compare/karet-fileview-v0.5.0...karet-fileview-v0.6.0) - 2026-08-29

### Added

- *(karet-fileview)* drag to select and copy in the hex dump
- preview notebooks through the document-conversion seam

### Other

- point shadowed crate readmes at their local files

## [0.3.0](https://github.com/getkono/karet/compare/karet-fileview-v0.2.2...karet-fileview-v0.3.0) - 2026-07-19

### Fixed

- keep the unpublished DOCX engine out of the published dependency closure

## [0.2.2](https://github.com/getkono/karet/compare/karet-fileview-v0.2.1...karet-fileview-v0.2.2) - 2026-07-10

### Added

- *(karet-fileview)* gate raster image rendering behind features

### Other

- Merge branch 'master' into feat/53-markdown-preview
- pin Rust toolchains to exact versions
- document optional format features + binary-size analysis

## [0.2.1](https://github.com/getkono/karet/compare/karet-fileview-v0.2.0...karet-fileview-v0.2.1) - 2026-07-09

### Other

- *(readme)* refresh karet positioning
- restructure design principles and testing guidance

## [0.2.0](https://github.com/getkono/karet/compare/karet-fileview-v0.1.0...karet-fileview-v0.2.0) - 2026-07-04

### Added

- *(fileview)* render PDF pages via Kitty graphics

## [0.1.0](https://github.com/getkono/karet/releases/tag/karet-fileview-v0.1.0) - 2026-07-02

### Added

- *(fileview)* compose FileDoc prepare + FileView dispatch
- *(fileview)* scaffold karet-fileview, relocating file-view primitives
- *(filetype)* add karet-filetype crate for file-type metadata

### Other

- Merge branch 'feat/cbor'
- *(release)* publish karet-fileview and its dependency chain
- *(fileview)* add runnable render-any-file example
- *(release)* automate releases, enforce conventional commits, document versioning
- document the karet diff viewer; correct karet-diff/karet-vcs READMEs
- add MSRV section to README
- initialize project
