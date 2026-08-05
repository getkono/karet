#!/usr/bin/env bash

# Regenerate the README hero from the deterministic xtask-owned SVG source.
# No live repository, terminal, font, clock, or host state enters the output, so
# repeated runs are byte-identical.

set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$script_dir/.." && pwd)"

cd "$repo_root"
cargo run --quiet --package xtask -- readme-svg
