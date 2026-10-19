#!/usr/bin/env bash
# End-to-end smoke test in a sandbox. Never touches the real sessions folder:
# HOME is overridden so config defaults resolve inside the sandbox.
set -euo pipefail

cd "$(dirname "$0")/.."
cargo build --quiet
BIN="$PWD/target/debug/meld"

SANDBOX="$(mktemp -d)"
trap 'rm -rf "$SANDBOX"' EXIT
export HOME="$SANDBOX"

ROOT="$SANDBOX/Library/Application Support/Claude/claude-code-sessions"
mkdir -p "$ROOT/acct-a/11111111-1111-4111-8111-111111111111" "$ROOT/acct-b/11111111-1111-4111-8111-111111111111"
echo '{"sessionId":"x"}' > "$ROOT/acct-a/11111111-1111-4111-8111-111111111111/local_x.json"
echo '{"sessionId":"y"}' > "$ROOT/acct-b/11111111-1111-4111-8111-111111111111/local_y.json"
touch -t 202001010000 "$ROOT"/acct-*/11111111-1111-4111-8111-111111111111/*.json

echo "== doctor ==";  "$BIN" doctor
echo "== scan ==";    "$BIN" scan
echo "== status =="; "$BIN" status
echo "== accounts =="; "$BIN" accounts
echo "== diff ==";    "$BIN" diff
echo "== dry-run =="; "$BIN" sync --dry-run
test ! -f "$ROOT/acct-a/11111111-1111-4111-8111-111111111111/local_y.json" || { echo "FAIL: dry-run wrote"; exit 1; }
echo "== sync ==";    "$BIN" sync
test -f "$ROOT/acct-a/11111111-1111-4111-8111-111111111111/local_y.json" || { echo "FAIL: y not mirrored"; exit 1; }
test -f "$ROOT/acct-b/11111111-1111-4111-8111-111111111111/local_x.json" || { echo "FAIL: x not mirrored"; exit 1; }
echo "== idempotency =="
"$BIN" diff | grep -q "Nothing to sync" || { echo "FAIL: not idempotent"; exit 1; }
echo "== restore ==";
echo 'garbage' > "$ROOT/acct-a/11111111-1111-4111-8111-111111111111/local_x.json"
"$BIN" restore
grep -q sessionId "$ROOT/acct-a/11111111-1111-4111-8111-111111111111/local_x.json" || { echo "FAIL: restore"; exit 1; }
echo "== new account folder =="
mkdir -p "$ROOT/acct-c"
"$BIN" sync
test -f "$ROOT/acct-c/11111111-1111-4111-8111-111111111111/local_x.json" || { echo "FAIL: new account not filled"; exit 1; }

echo "SMOKE TEST PASSED"
