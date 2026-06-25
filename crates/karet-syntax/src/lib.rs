//! `karet-syntax` — tree-sitter-powered syntactic analysis for karet editors.
//!
//! Produces *data*, not rendering: highlight spans tagged with semantic
//! `TokenId`s, fold regions, bracket pairs and structural selection ranges.
//! Consumers apply a theme (`karet-theme`) and render. This is the crate behind
//! the standalone "highlight a code snippet / code preview" use case.
//!
//! # Responsibilities (to implement)
//! - `highlight` — tree-sitter highlight queries → TokenId-tagged spans + semantic tokens.
//! - `fold` — fold-region computation (functions, blocks, imports, regions).
//! - `bracket` — matching bracket pairs.
//! - `selection` — expand/shrink structural selection ranges.
//!
//! # Internal dependencies
//! - `karet-core` — TokenId, ranges.
//! - `karet-treesitter` — shared parse trees.

// TODO: highlight — highlight queries → TokenId spans + semantic tokens.
// TODO: fold      — fold regions from the parse tree.
// TODO: bracket   — bracket-pair matching.
// TODO: selection — structural (syntax-aware) selection expansion.
