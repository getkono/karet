# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.8.0](https://github.com/getkono/karet/compare/karet-editor-v0.7.1...karet-editor-v0.8.0) - 2026-10-27

### Added

- *(app)* open a source link from the caret and paint source links as OSC 8 ([#789](https://github.com/getkono/karet/pull/789))
- *(editor)* word wrap at a column and indent wrapped continuation rows ([#666](https://github.com/getkono/karet/pull/666))
- *(core)* decoration foreground/modifier styles, priority, interval index ([#504](https://github.com/getkono/karet/pull/504))
- multi-cursor select all occurrences, skip occurrence, line-end carets and cursor undo ([#501](https://github.com/getkono/karet/pull/501))
- pull diagnostics, related information and codeDescription links ([#494](https://github.com/getkono/karet/pull/494))
- indentation guides, whitespace rendering, opt-in rainbow brackets ([#485](https://github.com/getkono/karet/pull/485))
- auto-indent from tree-sitter indent queries ([#469](https://github.com/getkono/karet/pull/469))
- *(editor)* virtual lines above and below buffer lines ([#466](https://github.com/getkono/karet/pull/466))
- *(editor)* a change-marker lane of its own in the gutter ([#452](https://github.com/getkono/karet/pull/452))
- expand and shrink selection along the syntax tree ([#450](https://github.com/getkono/karet/pull/450))
- editor.inlineDiagnostics setting, undercurl diagnostics, merged-pane signs ([#446](https://github.com/getkono/karet/pull/446))
- show diagnostics in place — gutter signs, end-of-line messages, tag styling ([#441](https://github.com/getkono/karet/pull/441))
- auto-close brackets and quotes with type-over, pair delete, wrap and smart Enter ([#439](https://github.com/getkono/karet/pull/439))
- Tab and Shift+Tab indent and outdent in the editor ([#434](https://github.com/getkono/karet/pull/434))
- highlight matching bracket and jump to bracket ([#427](https://github.com/getkono/karet/pull/427))
- *(editor)* [**breaking**] unified column-anchored virtual text in layout and hit-testing ([#415](https://github.com/getkono/karet/pull/415))
- toggle line and block comments ([#398](https://github.com/getkono/karet/pull/398))

### Fixed

- *(text)* strip every rope line break in the line accessors ([#741](https://github.com/getkono/karet/pull/741))
- *(editor)* grapheme-cluster-aware motions, deletes and widths ([#457](https://github.com/getkono/karet/pull/457))
- *(editor)* vertical and line motions — goal column, document ends, pages, folds, wrap, smart Home ([#454](https://github.com/getkono/karet/pull/454))
- *(editor)* gutter lanes — chevron folds, line numbers select lines ([#448](https://github.com/getkono/karet/pull/448))
- honour editor.rulers, scrollOff, lineNumbers and cursorLine ([#437](https://github.com/getkono/karet/pull/437))
- map carets through external edits; undo/redo restore the full selection set in the issuing view ([#430](https://github.com/getkono/karet/pull/430))
- *(karet)* keep every caret through multi-caret edit batches ([#426](https://github.com/getkono/karet/pull/426))
- *(karet)* multi-click and modifier semantics ([#425](https://github.com/getkono/karet/pull/425))
- *(editor)* Ctrl+D reaches every occurrence and matches whole words from a caret ([#421](https://github.com/getkono/karet/pull/421))
- *(editor)* paint selected line breaks and blank lines ([#418](https://github.com/getkono/karet/pull/418))
- *(core,editor)* multi-caret motion collapses before moving and keeps every caret ([#414](https://github.com/getkono/karet/pull/414))
- *(editor)* click hit-testing precision ([#407](https://github.com/getkono/karet/pull/407))
- *(editor)* a click never scrolls visible text ([#403](https://github.com/getkono/karet/pull/403))

### Other

- *(editor,diff,lsp,session)* [**breaking**] drop the remaining karet-core re-exports ([#808](https://github.com/getkono/karet/pull/808))
- *(text,editor)* move the word model and buffer slicing into karet-text ([#797](https://github.com/getkono/karet/pull/797))
- *(core)* [**breaking**] trim karet-core to shared vocabulary and make its constructors honest ([#785](https://github.com/getkono/karet/pull/785))
- *(editor)* [**breaking**] encapsulate scroll and per-frame geometry state ([#737](https://github.com/getkono/karet/pull/737))
- extend opt-in mutation testing to the pure, fast modules ([#681](https://github.com/getkono/karet/pull/681))
- remove duplicated helpers and dead code in the leaf engine crates ([#717](https://github.com/getkono/karet/pull/717))
- *(editor)* [**breaking**] rename the selection-extending shorthands to extend_* ([#707](https://github.com/getkono/karet/pull/707))
- correct drifted prose in protocol, VCS, seam and widget crates ([#690](https://github.com/getkono/karet/pull/690))
- *(editor)* drop the karet-syntax dependency by moving SemanticBlock(s) into karet-core ([#662](https://github.com/getkono/karet/pull/662))
- render-test karet-editor, karet-widgets and karet-fileview gaps ([#625](https://github.com/getkono/karet/pull/625))
- *(editor)* cap row_window doubling on zero-width-only lines ([#495](https://github.com/getkono/karet/pull/495))
- *(editor)* bound per-row work on very long lines ([#475](https://github.com/getkono/karet/pull/475))
- remove whole-document copies per keystroke ([#419](https://github.com/getkono/karet/pull/419))

### Changed

- The word model moved to `karet_text::words`: `word_bounds`, `class_run_at`, `identifier_at`, `previous_word_boundary`, `next_word_boundary` and `line_span` remain here as deprecated forwarding paths for one release, and `cluster_bounds` is now a re-export of `karet_text::words::cluster_bounds`
- `selection_text` is deprecated in favour of `karet_text::TextBuffer::slice_range`, which reads the range off the buffer instead of requiring a copy of its text

## [0.7.0](https://github.com/getkono/karet/compare/karet-editor-v0.6.1...karet-editor-v0.7.0) - 2026-09-24

### Added

- *(editor)* render inlay hints as column-accurate virtual text

### Fixed

- *(editor)* give the horizontal reveal one answer when a hint straddles the margin
- *(editor)* re-check a reveal against the frame that paints it
- *(editor)* reveal the caret horizontally as the motion happens
- *(editor)* scroll horizontally by the cells the caret is drawn in
- *(editor)* draw the caret at a hint's anchor before the hint
- honour dynamic registration, and close the review's mapping gaps
- *(editor)* map a click against the row's origin, not the column's

## [0.6.0](https://github.com/getkono/karet/compare/karet-editor-v0.5.0...karet-editor-v0.6.0) - 2026-08-29

### Added

- *(editor)* hit-test the gutter marker column
- *(core)* a color-swatch decoration carrying its own color
- *(ui)* drag, page and step the scrollbars
- *(editor)* report the last render's scroll extents

### Other

- split the files that outgrew the code-line ceiling
- point shadowed crate readmes at their local files
- Merge branch 'master' into feat/goto-definition-187

## [0.4.0](https://github.com/getkono/karet/compare/karet-editor-v0.3.0...karet-editor-v0.4.0) - 2026-08-06

### Added

- *(karet)* explain manual language-server requirements
- *(karet)* report existing application logs
- *(karet)* persist application diagnostics
- *(lsp)* manage shared language server toolchains
- *(editor)* render diagnostic underlines
- *(editor)* exempt structural lines from soft wrapping
- *(karet)* honor resolved indentation and tab widths
- *(editor)* model merge conflict decorations
- *(vcs)* switch inline blame to line-only click-to-detail model
- *(app)* add git workflows and live blame

### Other

- Merge pull request #168 from getkono/feat/text-selection-ergonomics-141
- *(readme)* capture the hero image from the real app
- *(readme)* add deterministic hero artwork
- unify pre-push and workflow checks
- *(lsp)* document server manager controls
- document the external LaTeX workflow
- explain optional spell checking

## [0.3.0](https://github.com/getkono/karet/compare/karet-editor-v0.2.2...karet-editor-v0.3.0) - 2026-07-19

### Added

- *(karet-editor)* render sticky semantic headers
- *(karet-editor)* support wrapped and overflow viewports

### Other

- *(karet-editor)* separate editor state and rendering
- *(config)* document semantic sticky scroll

## [0.2.2](https://github.com/getkono/karet/compare/karet-editor-v0.2.1...karet-editor-v0.2.2) - 2026-07-10

### Added

- *(karet-theme)* colors and text emphasis for markup scopes

### Other

- Merge branch 'master' into feat/53-markdown-preview
- pin Rust toolchains to exact versions

## [0.2.1](https://github.com/getkono/karet/compare/karet-editor-v0.2.0...karet-editor-v0.2.1) - 2026-07-09

### Added

- *(karet-editor)* expose caret geometry

### Other

- *(readme)* refresh karet positioning
- restructure design principles and testing guidance

## [0.2.0](https://github.com/getkono/karet/compare/karet-editor-v0.1.0...karet-editor-v0.2.0) - 2026-07-04

### Added

- *(editor,app)* multi-cursor add / next-occurrence / Alt-click
- *(editor,app)* complete keyboard text-selection vocabulary
- *(editor,session,app)* render + toggle code folds

### Other

- *(editor,core)* back EditorState with CursorState

## [0.1.0](https://github.com/getkono/karet/releases/tag/karet-editor-v0.1.0) - 2026-07-02

### Added

- *(editor)* add read-only rendering mode
- *(filetype)* add karet-filetype crate for file-type metadata
- *(karet)* editor caret, click-to-position & text selection
- *(karet-editor)* read-only editor widget render
- implement core API and introduce session backend

### Other

- *(release)* publish karet-fileview and its dependency chain
- *(fileview)* add runnable render-any-file example
- *(release)* automate releases, enforce conventional commits, document versioning
- apply diff-resilient rustfmt (cargo +nightly fmt)
- document the karet diff viewer; correct karet-diff/karet-vcs READMEs
- add MSRV section to README
- update CI and documentation for workspace
- initialize project
