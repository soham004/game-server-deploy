#!/usr/bin/env bash
# Serves the app on the real-server data folder for the browser suite (ui.e2e.mjs). Run
# loaders.e2e.mjs first: it installs the servers ("E2E neoforge" is the one the suite drives) and
# prepares the folder the import scenario imports. CurseForge is a local stand-in here
# (lib/mock-curseforge.mjs): the suite has no key for the real one.
#
#   tests/e2e/serve.sh            # Linux, keeps running
#   E2E_URL=http://localhost:18739 node --test tests/e2e/ui.e2e.mjs   # where Chrome or Edge is
set -eu
cd "$(dirname "$0")/../.."
BIN="${GSD_BIN:-${CARGO_TARGET_DIR:-target}/debug/gsd}"
DATA="${E2E_DATA:-$HOME/.cache/gsd-e2e}"
MOCK_PORT="${E2E_MOCK_PORT:-18740}"

node tests/e2e/lib/mock-curseforge.mjs "$MOCK_PORT" &
MOCK=$!
trap 'kill "$MOCK" 2>/dev/null || true' EXIT

GSD_CURSEFORGE_API="http://127.0.0.1:$MOCK_PORT" "$BIN" --data-dir "$DATA" --bind "127.0.0.1:${E2E_PORT:-18739}" serve
