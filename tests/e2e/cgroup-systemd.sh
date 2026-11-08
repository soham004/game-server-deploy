#!/usr/bin/env bash
# Hard limits in production conditions: runs the app as a transient systemd service with
# Delegate=yes (as root) on a fresh data folder, then cgroup.e2e.mjs checks a real server gets its
# own control group and limits. Needs root, systemd, cgroup v2, Node 22+ and the internet.
#
#   cargo build -p gsd-app && sudo tests/e2e/cgroup-systemd.sh
#
# The binary is copied to a system folder: the servers' own users can't enter a home folder.
set -u
cd "$(dirname "$0")/../.."
BIN="${GSD_BIN:-${CARGO_TARGET_DIR:-target}/debug/gsd}"
ROOT=/var/tmp/gsd-cgroup-e2e
UNIT=gsd-cgroup-e2e
PORT=18740

[ "$(id -u)" = 0 ] || { echo "run as root (sudo)"; exit 1; }
[ -x "$BIN" ] || { echo "$BIN not found: run cargo build -p gsd-app (or set GSD_BIN)"; exit 1; }

systemctl stop "$UNIT" 2>/dev/null; systemctl reset-failed "$UNIT" 2>/dev/null
rm -rf "$ROOT"; mkdir -p "$ROOT/bin"; chmod 755 "$ROOT" "$ROOT/bin"
install -m 755 "$BIN" "$ROOT/bin/gsd"
cleanup() { systemctl stop "$UNIT" 2>/dev/null; rm -rf "$ROOT"; }
trap cleanup EXIT

systemd-run --quiet --unit="$UNIT" -p Delegate=yes -p KillMode=mixed \
  "$ROOT/bin/gsd" --data-dir "$ROOT/data" --bind "127.0.0.1:$PORT" serve
for _ in $(seq 1 100); do curl -sf "http://127.0.0.1:$PORT/api/v1/health" >/dev/null && break; sleep 0.3; done

E2E_URL="http://127.0.0.1:$PORT" node --test tests/e2e/cgroup.e2e.mjs; RC=$?
if [ $RC -ne 0 ]; then
  echo "--- service log"
  journalctl -u "$UNIT" --no-pager -o cat | tail -40
fi
exit $RC
