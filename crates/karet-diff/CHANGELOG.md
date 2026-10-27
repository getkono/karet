# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.8.0](https://github.com/getkono/karet/compare/karet-diff-v0.7.1...karet-diff-v0.8.0) - 2026-10-27

### Added

- *(editor)* a change-marker click lands the diff on the clicked hunk ([#455](https://github.com/getkono/karet/pull/455))

### Fixed

- *(diff)* place a zero-context removal of an unterminated last line on that line ([#771](https://github.com/getkono/karet/pull/771))
- *(diff)* emit a file mode line so added and removed hunk patches apply ([#735](https://github.com/getkono/karet/pull/735))
- *(diff)* return a parse error instead of panicking on malformed headers ([#676](https://github.com/getkono/karet/pull/676))

### Other

- *(editor,diff,lsp,session)* [**breaking**] drop the remaining karet-core re-exports ([#808](https://github.com/getkono/karet/pull/808))
- *(diff)* [**breaking**] encapsulate prepared-diff pairing and hunk header, move painters under view ([#790](https://github.com/getkono/karet/pull/790))
- *(filetype)* share one binary sniffer with diff and search ([#770](https://github.com/getkono/karet/pull/770))
- extend opt-in mutation testing to the pure, fast modules ([#681](https://github.com/getkono/karet/pull/681))
- *(diff)* [**breaking**] remove the DiffStrategy, diff and detect_strategy stubs ([#713](https://github.com/getkono/karet/pull/713))
- correct drifted prose and doc comments in the leaf engine crates ([#692](https://github.com/getkono/karet/pull/692))
- *(diff)* reuse prepared intraline pairs in side-by-side painting ([#673](https://github.com/getkono/karet/pull/673))
- *(diff)* ungate row-to-hunk geometry into a layout module ([#656](https://github.com/getkono/karet/pull/656))
- *(diff)* pin engine flags, hunk merging and patch round trip ([#627](https://github.com/getkono/karet/pull/627))

### Changed

- **Breaking:** `PreparedDiff`'s fields are private, so its intra-line pairs can only be the ones derived from its diff; read them through `diff()`, `intraline(hunk, row)`, `old_tokens()` and `new_tokens()`, and adjust a `FileDiff` before `PreparedDiff::new` rather than after. Deserializing (feature `serde`) rejects a payload whose pair table does not match its hunks
- **Breaking:** `Hunk::header` is a method built from the hunk's counts and scope rather than a stored string, so it cannot disagree with them; a parsed header now reads `@@ -a,b +c,d @@ scope` with counts spelled out (git's `-3` shorthand becomes `-3,1`), and patches write the same form
- The ratatui painters live at `karet_diff::view` (`view::DiffPalette`, `view::unified_lines`, `view::side_by_side_lines`, `view::pad_diff_lines`), matching `karet-graph` and `karet-markdown`; the crate-root paths remain as deprecated aliases for one release
- `TokenSpan` is now defined in `karet-core`; `karet_diff::TokenSpan` re-exports it, so existing imports keep compiling

## [0.6.0](https://github.com/getkono/karet/compare/karet-diff-v0.5.0...karet-diff-v0.6.0) - 2026-08-29

### Added

- *(karet-diff)* expose the copyable content of a painted row
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
