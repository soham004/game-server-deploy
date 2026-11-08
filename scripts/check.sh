#!/usr/bin/env bash
# Everything CI checks on Linux, in one go: formatting, clippy, all tests, branding, and the
# frontend's syntax and unit tests. Stops at the first failure.
#
#   scripts/check.sh
set -euo pipefail
cd "$(dirname "$0")/.."

cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
bash scripts/check-branding.sh
find web/js -name '*.js' -print0 | xargs -0 -n1 node --check
node --test 'tests/web/*.test.mjs'
echo "All checks passed."
