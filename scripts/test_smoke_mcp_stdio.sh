#!/usr/bin/env bash
# test_smoke_mcp_stdio.sh
#
# Semantics harness for scripts/smoke-mcp-stdio.sh (webfang#2428). The smoke
# script is the post-build gate that proves the PUBLISHED MCP binary answers
# tools/list with the count the public-surface fixture declares. It runs at
# release time, so the only way to know it fails for the right reason is to
# drive it here, offline, against a fake server that speaks the protocol.
#
# What this harness proves (mktemp fixtures, python3 only; no network, no cargo):
#
#   1. a server advertising exactly the fixture's surface PASSES;
#   2. a server advertising one tool fewer FAILS, and the message names both
#      numbers — a gate that fails without saying why is a gate nobody can act on;
#   3. a server with the RIGHT COUNT and one renamed tool FAILS. This is the
#      case a count-only check cannot see, and the fixture is a name contract,
#      not a number contract;
#   4. a server that never answers FAILS on the deadline instead of hanging;
#   5. a missing binary FAILS before anything is spawned.
#
# The expected count is read from the same fixture the smoke script reads, so
# this harness does not carry a second copy of the number either.
#
# Usage: bash scripts/test_smoke_mcp_stdio.sh

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
GATE="$SCRIPT_DIR/smoke-mcp-stdio.sh"
FIXTURE="$SCRIPT_DIR/../crates/webfang_mcp/tests/fixtures/mcp_public_surface.tsv"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

if ! command -v python3 >/dev/null 2>&1; then
  echo "test_smoke_mcp_stdio.sh: python3 not available; this harness needs it to build the fake server" >&2
  exit 2
fi

EXPECTED="$(sed -n 's/^# tools: \([0-9][0-9]*\).*/\1/p' "$FIXTURE" | head -1)"
if [[ -z "$EXPECTED" ]]; then
  echo "test_smoke_mcp_stdio.sh: cannot read '# tools: N' from $FIXTURE" >&2
  exit 2
fi

passed=0
failed=0

ok() {
  printf '  ok   %-52s %s\n' "$1" "${2:-}"
  passed=$((passed + 1))
}

bad() {
  printf '  FAIL %-52s %s\n' "$1" "$2"
  if [ -n "${3:-}" ]; then
    printf '%s\n' "$3" | sed 's/^/        /' | head -20
  fi
  failed=$((failed + 1))
}

# --- fixtures ----------------------------------------------------------------
# A fake stdio MCP server: answers initialize and tools/list with the tool NAMES
# the fixture declares, so the harness exercises the same contract the real
# binary is held to. `mode` degrades that surface deliberately:
#   full     - every fixture name, unchanged
#   short    - one name fewer (the count check must catch it)
#   renamed  - same count, one name replaced (only the NAME check can catch it)
cat >"$WORK/fake-server.py" <<'PY'
import json
import sys

fixture, mode = sys.argv[1], sys.argv[2]

names = []
section = None
with open(fixture, "r", encoding="utf-8") as handle:
    for raw in handle:
        line = raw.rstrip("\n")
        if line.startswith("[") and line.endswith("]"):
            section = line[1:-1]
            continue
        if section != "tools" or not line or line.startswith("#"):
            continue
        names.append(line.split("\t", 1)[0])

if mode == "short":
    names = names[:-1]
elif mode == "renamed":
    names = names[:-1] + ["renamed_tool"]

for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    message = json.loads(line)
    method = message.get("method")
    if method == "initialize":
        print(json.dumps({
            "jsonrpc": "2.0",
            "id": message["id"],
            "result": {
                "protocolVersion": "2025-03-26",
                "capabilities": {},
                "serverInfo": {"name": "fake", "version": "0"},
            },
        }), flush=True)
    elif method == "tools/list":
        print(json.dumps({
            "jsonrpc": "2.0",
            "id": message["id"],
            "result": {"tools": [{"name": name} for name in names]},
        }), flush=True)
PY

# A server that accepts the connection and never answers: the hang path.
cat >"$WORK/silent-server.py" <<'PY'
import time

time.sleep(300)
PY

cat >"$WORK/fake-good" <<EOF
#!/usr/bin/env bash
exec python3 "$WORK/fake-server.py" "$FIXTURE" full
EOF

cat >"$WORK/fake-short" <<EOF
#!/usr/bin/env bash
exec python3 "$WORK/fake-server.py" "$FIXTURE" short
EOF

cat >"$WORK/fake-renamed" <<EOF
#!/usr/bin/env bash
exec python3 "$WORK/fake-server.py" "$FIXTURE" renamed
EOF

cat >"$WORK/fake-silent" <<EOF
#!/usr/bin/env bash
exec python3 "$WORK/silent-server.py"
EOF

chmod +x "$WORK/fake-good" "$WORK/fake-short" "$WORK/fake-renamed" "$WORK/fake-silent"

OUT=""
RC=0
run_gate() { # <args...>
  OUT="$(SMOKE_READ_TIMEOUT_SECS="${SMOKE_READ_TIMEOUT_SECS:-30}" bash "$GATE" "$@" 2>&1)"
  RC=$?
}

echo "smoke-mcp-stdio semantics harness (fixture declares $EXPECTED tools)"

# --- 1. the happy path --------------------------------------------------------
# NEGATION: a gate that rejects everything is not a gate; this is the control
# for every FAIL case below.
run_gate "$WORK/fake-good"
if [ "$RC" -eq 0 ]; then
  ok "server advertising the fixture count passes" "rc=0"
else
  bad "server advertising the fixture count passes" "expected PASS, got rc=$RC" "$OUT"
fi

# --- 2. one tool short fails, and says both numbers ---------------------------
# NEGATION: the whole point of the gate is the count comparison. A server that
# answers but advertises a different surface must never be published.
run_gate "$WORK/fake-short"
if [ "$RC" -eq 0 ]; then
  bad "server one tool short fails" "expected FAIL, got rc=0" "$OUT"
elif ! printf '%s' "$OUT" | grep -qF "$((EXPECTED - 1))"; then
  bad "server one tool short fails" "the failure does not name the served count" "$OUT"
elif ! printf '%s' "$OUT" | grep -qF "$EXPECTED"; then
  bad "server one tool short fails" "the failure does not name the expected count" "$OUT"
else
  ok "server one tool short fails" "rc=$RC"
fi

# --- 2b. the right count with a renamed tool still fails ----------------------
# NEGATION: this is exactly what a count-only comparison would wave through. The
# fixture is the public-surface contract, and a binary serving 36 differently
# named tools has drifted from it as surely as one serving 35.
run_gate "$WORK/fake-renamed"
if [ "$RC" -eq 0 ]; then
  bad "a renamed tool fails even at the right count" "expected FAIL, got rc=0" "$OUT"
elif ! printf '%s' "$OUT" | grep -qF "renamed_tool"; then
  bad "a renamed tool fails even at the right count" "the failure does not name the offending tool" "$OUT"
else
  ok "a renamed tool fails even at the right count" "rc=$RC"
fi

# --- 3. a silent server fails on the deadline, never hangs --------------------
# NEGATION: the release job has its own timeout, but a gate that BLOCKS until it
# is killed reports nothing. The deadline has to be the gate's own.
SMOKE_READ_TIMEOUT_SECS=2 run_gate "$WORK/fake-silent"
if [ "$RC" -eq 0 ]; then
  bad "a silent server fails on the deadline" "expected FAIL, got rc=0" "$OUT"
elif ! printf '%s' "$OUT" | grep -qiF "initialize"; then
  bad "a silent server fails on the deadline" "the failure does not name the unanswered request" "$OUT"
else
  ok "a silent server fails on the deadline" "rc=$RC"
fi

# --- 4. the fail-closed paths -------------------------------------------------
run_gate "$WORK/does-not-exist"
if [ "$RC" -ne 0 ]; then
  ok "a missing binary fails" "rc=$RC"
else
  bad "a missing binary fails" "expected FAIL, got rc=0" "$OUT"
fi

echo
echo "smoke-mcp-stdio semantics harness: $passed passed, $failed failed"
[ "$failed" -eq 0 ]
