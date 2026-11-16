#!/usr/bin/env bash
# check_migration_evidence.sh — el PRD no puede volver a mentir en silencio.
#
# Gate fail-closed de F0. Verifica evidence/mig-2026-01/baseline.json contra:
#   * el PRD v2 (sin hechos literales, placeholders resueltos, RFs no huérfanos),
#   * la historia de git (el SHA medido sigue siendo ancestro de HEAD),
#   * las políticas vivas (Cargo.toml, rust-toolchain.toml, support.json, README).
#
# A diferencia de un gate de contadores, esto NO pone rojo main cada vez que
# alguien agrega un test: los contadores son evidencia de F0, no un contrato
# sobre el árbol. Lo que se compara son los hechos que invalidan DECISIONES
# (RF-23/RF-24/K9/RF-13). Con evidence/mig-2026-01/TRAIN_OPEN presente la
# deriva es bloqueante; sin ese marcador degrada a NOTE (tren cerrado).
#
# Wiring: .github/workflows/ci.yml -> job `repo-guards`.
# Harness: scripts/test_migration_evidence.sh (con marcadores # NEGATION:),
#          también wired en el mismo job. Ambos exigidos por #2056.
#
# Exit: 0 coherente · 1 deriva/evidencia rota · 2 uso incorrecto.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="${MIGRATION_ROOT:-$(cd "$SCRIPT_DIR/.." && pwd)}"
AUDITOR="${MIGRATION_AUDITOR:-$SCRIPT_DIR/audit_migration_baseline.py}"
EVIDENCE="${MIGRATION_EVIDENCE:-$ROOT/evidence/mig-2026-01/baseline.json}"
PRD="${MIGRATION_PRD:-$ROOT/evidence/mig-2026-01/PRD-v2.md}"
MARKER="${MIGRATION_TRAIN_OPEN:-$ROOT/evidence/mig-2026-01/TRAIN_OPEN}"

[[ -f $AUDITOR ]] || { echo "::error:: falta el auditor $AUDITOR" >&2; exit 1; }
[[ -f $EVIDENCE ]] || { echo "::error:: falta el baseline $EVIDENCE (corré --write en F0)" >&2; exit 1; }
[[ -f $PRD ]] || { echo "::error:: falta el PRD v2 $PRD" >&2; exit 1; }

python3 "$AUDITOR" \
  --root "$ROOT" \
  --check "$EVIDENCE" \
  --prd "$PRD" \
  --marker "$MARKER"
