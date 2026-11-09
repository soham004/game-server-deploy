#!/usr/bin/env bash
# The test API key in `.env` must never ship: checks that git and Docker both ignore that file, and
# that no tracked file carries a key. Reads no secret and prints none.
#
#   scripts/check-secrets.sh
set -euo pipefail
cd "$(dirname "$0")/.."

for name in .env .env.local crates/gsd-mods/.env; do
  git check-ignore -q --no-index "$name" || {
    echo "check-secrets: git doesn't ignore $name" >&2
    exit 1
  }
done
for pattern in '.env' '**/.env' '.env.*' '**/.env.*'; do
  grep -qxF -- "$pattern" .dockerignore || {
    echo "check-secrets: .dockerignore lacks $pattern" >&2
    exit 1
  }
done
if [ -n "$(git ls-files -- '.env' '.env.*' '**/.env' '**/.env.*')" ]; then
  echo 'check-secrets: an env file is tracked by git' >&2
  exit 1
fi
# CurseForge keys are bcrypt-shaped: $2a$10$ and 53 more characters.
if git grep -qIE '\$2[aby]\$[0-9]{2}\$[./A-Za-z0-9]{53}' -- ':!Cargo.lock'; then
  echo 'check-secrets: a tracked file holds what looks like a CurseForge key' >&2
  exit 1
fi
echo 'check-secrets: ok'
