# Sources — webfang-ci-gate

This skill is the actionable layer. The coverage runbook (which `gate.needs` jobs each lane
covers, the deliberate exceptions, the #2548 false-green incident) lives in Engram; the gate
mechanics live in the repo scripts. Do not copy the runbook here.

## Engram (depth / history / diagnosis)

| Topic | Query | Obs | Scope |
| --- | --- | --- | --- |
| ci_fast_gate coverage report, lanes, partial/never | `ci_fast_gate coverage report gate.needs lane partial` | #2980 | project |
| PR gates, pre-commit, merge, batch, CHANGELOG | `merge-when-green batch merge pre-commit changelog` | #2309 | project |
| Delivery gates: hooks, push to main, cloud verification | `delivery gates hooksPath main-push verify cloud timeout` | #2316 | project |

Resolve with `engram search "<query>" --project webfang [--scope personal]` or `mem_search`.

## Repo sources of truth (normative)

- `../../../scripts/ci_fast_gate.sh` — the lane-aware local gate (docs / ci / code / full).
- `../../../scripts/ci_fast_gate_coverage.tsv` — coverage declaration; mirrors `gate.needs`.
- `../../../scripts/test_ci_gate_structure.sh` — fails when the TSV drifts from `gate.needs`.
- `../../../AGENTS.md` — § Commands (full verification gate) and § Project rules (local gate
  mirrors `gate.needs`, deliberate exceptions by class).
- `../../../.github/workflows/pr-validation.yml` — PR rules CI enforces.