#!/usr/bin/env bash
# Regenerate desktop/src/generated from the Rust types (ADR-0040).
#
# Run from anywhere; the script finds the repository root itself. The output is
# committed so a Rust change shows up as a desktop change in review.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root"

rm -rf desktop/src/generated
mkdir -p desktop/src/generated

# `ts(export)` writes each binding during the test run, into the directory set
# by TS_RS_EXPORT_DIR in .cargo/config.toml.
cargo test -p cowbot-core --features bindings --lib >/dev/null

node desktop/scripts/generate-index.mjs
