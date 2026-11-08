#!/usr/bin/env bash
# Builds the app under another name, slug and port (D-017) and checks the binary uses them, and
# that an invalid slug fails the build. Uses its own target folder, so the normal build stays.
#
#   scripts/check-rebrand.sh
set -euo pipefail
cd "$(dirname "$0")/.."
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-target}/rebrand"

APP_NAME="Blocky Panel" APP_SLUG=blocky APP_DEFAULT_PORT=9123 cargo build -q -p gsd-app
bin="$CARGO_TARGET_DIR/debug/gsd"
[ "$("$bin" brand name)" = "Blocky Panel" ] || { echo "wrong name: $("$bin" brand name)"; exit 1; }
[ "$("$bin" brand slug)" = "blocky" ] || { echo "wrong slug"; exit 1; }
[ "$("$bin" brand port)" = "9123" ] || { echo "wrong port"; exit 1; }
[ "$("$bin" brand env-prefix)" = "BLOCKY" ] || { echo "wrong env prefix"; exit 1; }

errors=$(mktemp)
trap 'rm -f "$errors"' EXIT
if APP_SLUG="Bad Slug" cargo build -q -p gsd-brand 2>"$errors"; then
  echo "an invalid slug should fail the build"; exit 1
fi
grep -q "APP_SLUG must match" "$errors" || { cat "$errors"; exit 1; }
echo "Rebranding works."
