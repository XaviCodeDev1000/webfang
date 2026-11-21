#!/usr/bin/env bash
# check_migration_evidence.sh — the PRD cannot lie silently again.
#
# Fail-closed F0 gate. Verifies evidence/mig-2026-01/baseline.json against:
#   * the PRD v2 (no literal facts, placeholders resolved, no orphan RFs),
#   * git history (the measured SHA is HEAD, an ancestor, or the tree of a
#     squash; the only tree change allowed is the baseline itself),
#   * the living policies (Cargo.toml, rust-toolchain.toml, support.json, README).
#
# Unlike a counter gate, this does NOT turn main red every time somebody adds
# a test: counters are F0 evidence, not a contract over the tree. What gets
# compared are the facts that invalidate DECISIONS (RF-23/RF-24/K9/RF-13).
# With evidence/mig-2026-01/TRAIN_OPEN present the drift is blocking; without
# that marker it degrades to a NOTE (train closed).
#
# Wiring: .github/workflows/ci.yml -> job `repo-guards`.
# Harness: scripts/test_migration_evidence.sh (with # NEGATION: markers),
#          wired in the same job.
#
# Exit: 0 coherent · 1 drift/broken evidence · 2 incorrect usage.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="${MIGRATION_ROOT:-$(cd "$SCRIPT_DIR/.." && pwd)}"
AUDITOR="${MIGRATION_AUDITOR:-$SCRIPT_DIR/audit_migration_baseline.py}"
EVIDENCE="${MIGRATION_EVIDENCE:-$ROOT/evidence/mig-2026-01/baseline.json}"
PRD="${MIGRATION_PRD:-$ROOT/evidence/mig-2026-01/PRD-v2.md}"
MARKER="${MIGRATION_TRAIN_OPEN:-$ROOT/evidence/mig-2026-01/TRAIN_OPEN}"

[[ -f $AUDITOR ]] || { echo "::error:: missing auditor $AUDITOR" >&2; exit 1; }
[[ -f $EVIDENCE ]] || { echo "::error:: missing baseline $EVIDENCE (run --write at F0)" >&2; exit 1; }
[[ -f $PRD ]] || { echo "::error:: missing PRD v2 $PRD" >&2; exit 1; }

python3 "$AUDITOR" \
  --root "$ROOT" \
  --check "$EVIDENCE" \
  --prd "$PRD" \
  --marker "$MARKER"
