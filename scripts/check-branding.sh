#!/usr/bin/env bash
# Fails if the default product name or web port is written anywhere except where it is declared
# (crates/gsd-brand/build.rs). Everything else must use the gsd_brand constants (D-017).
#
# Allowed exceptions: planning notes (claude/), deployment examples (deploy/), lock and build output.
set -euo pipefail
cd "$(dirname "$0")/.."

declaration=crates/gsd-brand/build.rs
name=$(sed -n 's/^const DEFAULT_NAME: &str = "\(.*\)";$/\1/p' "$declaration")
port=$(sed -n 's/^const DEFAULT_PORT: u16 = \([0-9]*\);$/\1/p' "$declaration")

if [[ -z "$name" || -z "$port" ]]; then
  echo "check-branding: could not read the defaults from $declaration" >&2
  exit 2
fi

matches=$(grep -rnI \
  --exclude-dir={target,.git,claude,deploy,gsd-brand,node_modules} \
  --exclude=Cargo.lock \
  --exclude=check-branding.sh \
  -e "$name" -e "\b$port\b" . || true)

if [[ -n "$matches" ]]; then
  echo "Hard-coded branding found. Use gsd_brand::{NAME, DEFAULT_PORT} (or {{APP_NAME}} in HTML) instead:" >&2
  echo "$matches" >&2
  exit 1
fi
echo "check-branding: ok"
