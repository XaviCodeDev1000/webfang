#!/usr/bin/env bash
# smoke-mcp-stdio.sh — prove the PUBLISHED MCP binary answers tools/list.
#
# WHY THIS EXISTS (webfang#2428): the release now ships webfang-mcp-stdio as an
# artifact, and "the archive contains a file" is not "the file works". This runs
# the EXTRACTED binary the way a desktop MCP client does — JSON-RPC over stdin —
# and compares the advertised tool count against the committed public-surface
# fixture, which is the same contract the Rust suite asserts.
#
# The expected surface is READ FROM THE FIXTURE — both the count and the tool
# NAMES — never transcribed here. A second copy of "36" is a number that stops
# being true the day the surface changes, and a count alone would let a binary
# that renamed every tool pass while the product drifted away from the contract.
#
# USAGE
#   bash scripts/smoke-mcp-stdio.sh <path-to-webfang-mcp-stdio>
#
# ENV
#   SMOKE_READ_TIMEOUT_SECS  per-read deadline, default 30. The harness lowers it
#                            to prove the hang path fails instead of blocking.
#
# Fail-closed: a missing binary, a missing fixture, no initialize answer, no
# tools/list answer, a response without result.tools, or a count that differs
# from the fixture are all FAILURES.
#
# STYLE: user-facing errors are Spanish (repo convention for this scripts/
# family); internal output is English.

set -euo pipefail

export LC_ALL=C

usage() {
    cat <<'USAGE'
smoke-mcp-stdio.sh — drive the published webfang-mcp-stdio binary over stdio
(initialize -> notifications/initialized -> tools/list) and assert the advertised
tool count matches crates/webfang_mcp/tests/fixtures/mcp_public_surface.tsv.

usage:
  bash scripts/smoke-mcp-stdio.sh <path-to-webfang-mcp-stdio>

Fail-closed: a missing binary, a missing fixture, an unanswered request or a
count mismatch is a FAILURE, never a skip.
USAGE
}

if [[ $# -ne 1 ]]; then
    usage >&2
    exit 2
fi

BIN="$1"
if [[ ! -f "$BIN" ]]; then
    echo "::error::'$BIN' no existe: no hay binario publicado que probar."
    exit 1
fi
if [[ ! -x "$BIN" ]]; then
    echo "::error::'$BIN' no es ejecutable: el artefacto se publicaría sin poder arrancar."
    exit 1
fi

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
FIXTURE="$ROOT/crates/webfang_mcp/tests/fixtures/mcp_public_surface.tsv"
if [[ ! -f "$FIXTURE" ]]; then
    echo "::error::falta el fixture '$FIXTURE': sin él no hay contra qué comparar la superficie servida, y este smoke test NO se salta."
    exit 1
fi

exec python3 - "$BIN" "$FIXTURE" "${SMOKE_READ_TIMEOUT_SECS:-30}" <<'PY'
import json
import os
import select
import subprocess
import sys
import time

binary, fixture, timeout_s = sys.argv[1], sys.argv[2], float(sys.argv[3])

# The fixture IS the public-surface contract: the header carries the count and
# the [tools] section carries the NAMES. Checking only the count would let a
# binary that renamed or duplicated tools pass, so both are read here.
expected = None
expected_names = []
section = None
with open(fixture, "r", encoding="utf-8") as handle:
    for raw in handle:
        line = raw.rstrip("\n")
        if line.startswith("# tools:"):
            expected = int(line.split(":", 1)[1].split()[0])
            continue
        if line.startswith("[") and line.endswith("]"):
            section = line[1:-1]
            continue
        if section != "tools" or not line or line.startswith("#"):
            continue
        expected_names.append(line.split("\t", 1)[0])
if expected is None or not expected_names:
    print("::error::el fixture '{}' no declara '# tools: N' y una sección [tools].".format(fixture))
    sys.exit(1)

proc = subprocess.Popen(
    [binary],
    stdin=subprocess.PIPE,
    stdout=subprocess.PIPE,
    # stderr carries tracing logs, never JSON-RPC. Null it so the child can
    # never block on an unread log buffer and turn a healthy server into a hang.
    stderr=subprocess.DEVNULL,
    bufsize=0,
)

_pending = b""


def send(payload):
    proc.stdin.write((json.dumps(payload) + "\n").encode("utf-8"))
    proc.stdin.flush()


def read_line(deadline):
    """One line, or None once the deadline passes or stdout closes."""
    global _pending
    while b"\n" not in _pending:
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            return None
        ready, _, _ = select.select([proc.stdout], [], [], remaining)
        if not ready:
            return None
        chunk = os.read(proc.stdout.fileno(), 65536)
        if not chunk:
            return None
        _pending += chunk
    line, _, _pending = _pending.partition(b"\n")
    return line.decode("utf-8", "replace").strip()


def wait_for_id(target, budget):
    """The response carrying that id; notifications and blank lines are skipped.

    The deadline is per request, not one budget shared by the whole exchange:
    a slow initialize must not eat the tools/list budget and then be reported
    as a tools/list failure.
    """
    deadline = time.monotonic() + budget
    while True:
        line = read_line(deadline)
        if line is None:
            return None
        if not line:
            continue
        try:
            message = json.loads(line)
        except json.JSONDecodeError:
            continue
        if isinstance(message, dict) and message.get("id") == target:
            return message


try:
    send({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-03-26",
            "capabilities": {},
            "clientInfo": {"name": "release-smoke", "version": "1"},
        },
    })
    handshake = wait_for_id(1, timeout_s)
    if handshake is None:
        print("::error::el binario no respondió 'initialize' en {:g}s: el artefacto publicado no habla MCP.".format(timeout_s))
        sys.exit(1)
    if "result" not in handshake:
        # A JSON-RPC error reply is an ANSWER, but not a successful handshake.
        print("::error::'initialize' respondió con error en vez de resultado: {}".format(json.dumps(handshake)[:200]))
        sys.exit(1)

    send({"jsonrpc": "2.0", "method": "notifications/initialized"})
    send({"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}})

    listed = wait_for_id(2, timeout_s)
    if listed is None:
        print("::error::el binario no respondió 'tools/list' en {:g}s.".format(timeout_s))
        sys.exit(1)

    tools = (listed.get("result") or {}).get("tools")
    if not isinstance(tools, list):
        print("::error::la respuesta de 'tools/list' no trae result.tools: {}".format(json.dumps(listed)[:200]))
        sys.exit(1)

    if len(tools) != expected:
        print("::error::el binario publica {} tools y el fixture declara {}: el artefacto no sirve la superficie documentada.".format(len(tools), expected))
        sys.exit(1)

    advertised = {tool.get("name") for tool in tools if isinstance(tool, dict)}
    wanted = set(expected_names)
    missing = sorted(wanted - advertised)
    extra = sorted(advertised - wanted)
    if missing or extra:
        print("::error::la superficie servida no coincide con el fixture — faltan {} y sobran {}.".format(
            missing[:10] or "ninguna", extra[:10] or "ninguna"))
        sys.exit(1)

    print("smoke OK: {} tools servidas por {} (conteo y nombres coinciden con el fixture)".format(len(tools), binary))
finally:
    try:
        proc.stdin.close()
    except Exception:
        pass
    try:
        proc.wait(timeout=5)
    except Exception:
        proc.kill()
PY
