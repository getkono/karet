#!/usr/bin/env bash
# Regenerate the README hero (assets/karet.svg) from a real karet frame.
#
# The hero is a capture, not a drawing: `karet --capture` renders the shell into an
# off-screen grid and writes it as truecolor ANSI, and `xtask readme-svg` turns that
# grid into SVG. Every colour, glyph, and column therefore comes from the app's own
# renderer, so the artwork cannot drift from the product without the drift showing.
#
# Deterministic by construction. The frame is captured against a throwaway demo
# repository built here — pinned content, pinned branch, pinned commit identity and
# dates — rather than against the working tree, so re-runs are byte-identical no
# matter which branch, machine, or dirty state you run from. Host state that would
# otherwise leak into the frame (user config and session restore, language servers,
# spell-check, Nerd Font icons, NO_COLOR) is neutralized below.
#
# Requirements: a Rust toolchain. Nothing else — no external SVG tooling.
#
# Usage:
#   mise run svg
#   bash scripts/gen-svg.sh

set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$script_dir/.." && pwd)"
cd "$repo_root"

# Release, so the capture reflects the binary users actually run.
echo "Building karet (release)..."
cargo build --release --package karet
karet="$repo_root/target/release/karet"

scratch="$(mktemp -d)"
trap 'rm -rf "$scratch"' EXIT
# The demo project and the fake home are siblings, never nested: karet writes logs
# and session state under HOME, and anything under the workspace root would show up
# in the file tree.
fixture="$scratch/ropes"
home="$scratch/home"

# --- The demo project -------------------------------------------------------
mkdir -p "$fixture/src" "$fixture/tests" "$fixture/.karet" "$home"

cat >"$fixture/Cargo.toml" <<'TOML'
[package]
name = "ropes"
version = "0.1.0"
edition = "2024"

[dependencies]
TOML

cat >"$fixture/README.md" <<'MD'
# ropes

A small rope buffer: O(log n) splits, joins, and indexed edits.
MD

cat >"$fixture/src/lib.rs" <<'RS'
//! A rope: a balanced tree of string chunks, cheap to split and join.

/// The largest chunk stored in a single leaf, in bytes.
const MAX_LEAF: usize = 512;

/// A rope node: either a run of text or the concatenation of two ropes.
#[derive(Debug, Clone)]
pub enum Rope {
    /// A leaf holding at most [`MAX_LEAF`] bytes.
    Leaf(String),
    /// An internal node, caching the length of its left subtree.
    Branch { left: Box<Rope>, right: Box<Rope>, weight: usize },
}

impl Rope {
    /// Build a rope from `text`, splitting it into leaves as needed.
    #[must_use]
    pub fn new(text: &str) -> Self {
        if text.len() <= MAX_LEAF {
            return Self::Leaf(text.to_owned());
        }
        let mid = text.floor_char_boundary(text.len() / 2);
        let (left, right) = text.split_at(mid);
        Self::Branch {
            left: Box::new(Self::new(left)),
            right: Box::new(Self::new(right)),
            weight: mid,
        }
    }

    /// Total length of the rope, in bytes.
    #[must_use]
    pub fn len(&self) -> usize {
        match self {
            Self::Leaf(text) => text.len(),
            Self::Branch { weight, right, .. } => weight + right.len(),
        }
    }
}
RS

cat >"$fixture/src/main.rs" <<'RS'
fn main() {
    println!("{}", ropes::Rope::new("hello, world").len());
}
RS

cat >"$fixture/src/chunk.rs" <<'RS'
//! Leaf storage: a chunk is a short, immutable run of UTF-8.

/// A single leaf's text, kept under the leaf size limit.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Chunk(String);

impl Chunk {
    /// Wrap `text` as a chunk.
    #[must_use]
    pub fn new(text: impl Into<String>) -> Self {
        Self(text.into())
    }
}
RS

cat >"$fixture/tests/rope.rs" <<'RS'
use ropes::Rope;

#[test]
fn a_short_string_stays_in_one_leaf() {
    assert!(matches!(Rope::new("hello"), Rope::Leaf(_)));
}
RS

cat >"$fixture/.gitignore" <<'TXT'
/target
TXT

# Pin every input the shell reads from configuration:
#   colorTheme  the built-in dark palette, so no theme file is loaded
#   iconStyle   1-cell Unicode glyphs — Nerd Font glyphs are tofu in a browser
#   lsp         off, or a locally-installed rust-analyzer injects diagnostics
#   spellcheck  off, or the host dictionary decides where squiggles land
#   git.blame   off: the inline blame line reads "N months ago", which is relative
#               to the moment of capture and would churn the asset on every run
cat >"$fixture/.karet/setting.jsonc" <<'JSONC'
{
  "workbench": { "colorTheme": "dark", "iconStyle": "unicode" },
  "lsp": { "enabled": false },
  "spellcheck": { "enabled": false },
  "git": { "blame": false }
}
JSONC

# --- The demo history -------------------------------------------------------
export GIT_AUTHOR_NAME="Karet Demo"
export GIT_AUTHOR_EMAIL="demo@karet.invalid"
export GIT_COMMITTER_NAME="$GIT_AUTHOR_NAME"
export GIT_COMMITTER_EMAIL="$GIT_AUTHOR_EMAIL"
export GIT_AUTHOR_DATE="2026-01-01T00:00:00+00:00"
export GIT_COMMITTER_DATE="$GIT_AUTHOR_DATE"

git -C "$fixture" init --quiet --initial-branch=main
git -C "$fixture" add --all
git -C "$fixture" commit --quiet --message "feat: add the rope buffer"

# Leave one tracked file modified, so the Source Control panel and the gutter show
# a real, and always identical, working-tree change.
cat >>"$fixture/src/lib.rs" <<'RS'

impl Default for Rope {
    fn default() -> Self {
        Self::Leaf(String::new())
    }
}
RS

# --- The capture ------------------------------------------------------------
# `env -i` starts from an empty environment, so nothing on this machine — NO_COLOR,
# KARET_ICONS, COLORTERM, a Kitty session — can reach the frame. HOME and the XDG
# directories point at the throwaway home, so no user configuration, restored
# session, or installed language server is found either.
#
# The root is `.` with the working directory set to the project, because the
# breadcrumb renders the tab's path verbatim: an absolute root would print the
# fixture's random mktemp name into the artwork.
echo "Capturing karet..."
(
    cd "$fixture"
    env -i \
        PATH="$PATH" \
        HOME="$home" \
        XDG_CONFIG_HOME="$home/.config" \
        XDG_DATA_HOME="$home/.local/share" \
        XDG_STATE_HOME="$home/.local/state" \
        XDG_CACHE_HOME="$home/.cache" \
        "$karet" \
        --capture \
        --capture-size 120x34 \
        --icons unicode \
        --startup-panel explorer \
        --goto src/lib.rs:20:9 \
        .
) | cargo run --quiet --package xtask -- readme-svg
