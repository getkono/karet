# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.8.0](https://github.com/getkono/karet/compare/karet-core-v0.7.1...karet-core-v0.8.0) - 2026-10-27

### Added

- fold regions from LSP textDocument/foldingRange ([#777](https://github.com/getkono/karet/pull/777))
- LSP document links and document colors ([#763](https://github.com/getkono/karet/pull/763))
- AI inline completion as ghost text via LSP inlineCompletion ([#727](https://github.com/getkono/karet/pull/727))
- re-pull diagnostics on refresh and across inter-file dependencies ([#693](https://github.com/getkono/karet/pull/693))
- call hierarchy and type hierarchy views ([#670](https://github.com/getkono/karet/pull/670))
- *(theme)* dedicated theme roles for occurrence highlights ([#663](https://github.com/getkono/karet/pull/663))
- semantic tokens layered over tree-sitter highlighting ([#511](https://github.com/getkono/karet/pull/511))
- fold commands and per-language fold queries ([#505](https://github.com/getkono/karet/pull/505))
- *(core)* decoration foreground/modifier styles, priority, interval index ([#504](https://github.com/getkono/karet/pull/504))
- LSP snippets with tabstop and placeholder sessions ([#500](https://github.com/getkono/karet/pull/500))
- pull diagnostics, related information and codeDescription links ([#494](https://github.com/getkono/karet/pull/494))
- render LSP code lens ([#483](https://github.com/getkono/karet/pull/483))
- find bar current-match style, search in selection, matches to carets, preserve case ([#465](https://github.com/getkono/karet/pull/465))
- *(editor)* virtual lines above and below buffer lines ([#466](https://github.com/getkono/karet/pull/466))
- *(editor)* a change-marker lane of its own in the gutter ([#452](https://github.com/getkono/karet/pull/452))
- go to symbol in workspace through quick-open's # mode and Ctrl+T ([#447](https://github.com/getkono/karet/pull/447))
- completion overhaul — textEdit ranges, auto-import, resolve + docs, trigger/commit chars, incomplete lists ([#436](https://github.com/getkono/karet/pull/436))
- richer inlay hints — per-kind styling, label parts, resolve, toggle ([#429](https://github.com/getkono/karet/pull/429))
- *(editor)* [**breaking**] unified column-anchored virtual text in layout and hit-testing ([#415](https://github.com/getkono/karet/pull/415))
- code actions and quick fix menu with a gutter lightbulb ([#413](https://github.com/getkono/karet/pull/413))
- rename symbol (F2) and a session-level workspace-edit apply path ([#402](https://github.com/getkono/karet/pull/402))

### Fixed

- *(core)* clamp LineIndex::line_col for offsets inside a char or past the end ([#740](https://github.com/getkono/karet/pull/740))
- *(editor)* grapheme-cluster-aware motions, deletes and widths ([#457](https://github.com/getkono/karet/pull/457))
- *(core,editor)* multi-caret motion collapses before moving and keeps every caret ([#414](https://github.com/getkono/karet/pull/414))

### Other

- *(dap,session)* [**breaking**] one neutral debugger frame model with typed ids ([#793](https://github.com/getkono/karet/pull/793))
- *(diff)* [**breaking**] encapsulate prepared-diff pairing and hunk header, move painters under view ([#790](https://github.com/getkono/karet/pull/790))
- *(core)* [**breaking**] trim karet-core to shared vocabulary and make its constructors honest ([#785](https://github.com/getkono/karet/pull/785))
- *(core)* define HighlightKind and DocumentHighlight once in karet-core ([#780](https://github.com/getkono/karet/pull/780))
- *(core)* [**breaking**] move single-consumer formatter and lens-row types out of karet-core ([#747](https://github.com/getkono/karet/pull/747))
- extend opt-in mutation testing to the pure, fast modules ([#681](https://github.com/getkono/karet/pull/681))
- remove duplicated helpers and dead code in the leaf engine crates ([#717](https://github.com/getkono/karet/pull/717))
- *(core)* move shift_span and merge_spans into karet-core ([#702](https://github.com/getkono/karet/pull/702))
- correct drifted prose and doc comments in the leaf engine crates ([#692](https://github.com/getkono/karet/pull/692))
- *(seam)* curate karet-seam's public surface and protect SeamIndex invariants ([#661](https://github.com/getkono/karet/pull/661))
- *(editor)* drop the karet-syntax dependency by moving SemanticBlock(s) into karet-core ([#662](https://github.com/getkono/karet/pull/662))
- tighten karet-treesitter and karet-syntax assertions ([#653](https://github.com/getkono/karet/pull/653))
- *(core)* constrain token, serde-wire and cursor-normalize contracts ([#642](https://github.com/getkono/karet/pull/642))

### Added

- `TokenSpan`, a line-relative token run (byte offsets within one line plus a `TokenId`), moved here from `karet-diff` so per-line token tables (prepared diffs, source previews) share one core type; its serialized shape is unchanged

## [0.7.1](https://github.com/getkono/karet/compare/karet-core-v0.7.0...karet-core-v0.7.1) - 2026-10-01

### Added

- mark uncommitted lines in the editor gutter ([#311](https://github.com/getkono/karet/pull/311))

## [0.7.0](https://github.com/getkono/karet/compare/karet-core-v0.6.1...karet-core-v0.7.0) - 2026-09-24

### Added

- *(editor)* render inlay hints as column-accurate virtual text
- *(core)* give language-server capabilities a neutral model

## [0.6.0](https://github.com/getkono/karet/compare/karet-core-v0.5.0...karet-core-v0.6.0) - 2026-08-29

### Added

- *(core)* a stopped-line theme role
- *(core)* add the diagnostic-formatter interop trait
- *(core)* a color-swatch decoration carrying its own color
- *(core)* add an underline decoration kind
- *(theme)* add scrollbar track and thumb theme roles

## [0.4.0](https://github.com/getkono/karet/compare/karet-core-v0.3.0...karet-core-v0.4.0) - 2026-08-06

### Added

- *(lsp)* implement document symbol requests
- *(vcs)* switch inline blame to line-only click-to-detail model
- *(session)* add async repository and blame contracts

## [0.3.0](https://github.com/getkono/karet/compare/karet-core-v0.2.2...karet-core-v0.3.0) - 2026-07-19

### Added

- *(karet-core)* add a semantic-comment token

## [0.2.2](https://github.com/getkono/karet/compare/karet-core-v0.2.1...karet-core-v0.2.2) - 2026-07-10

### Added

- *(theme)* add strikethrough support for markup
- *(karet-core)* doc-comment and markup token scopes

## [0.2.1](https://github.com/getkono/karet/compare/karet-core-v0.2.0...karet-core-v0.2.1) - 2026-07-09

### Added

- *(karet-theme)* verified/unverified VCS badge roles

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
