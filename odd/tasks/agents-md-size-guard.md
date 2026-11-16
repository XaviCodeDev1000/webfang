# Feature: AGENTS.md size/structure guard (Slice 2 of 2)

**Issue:** #2839 — requires the owner's `status:approved` before the PR is opened.
**Labels:** `type:chore`
**Branch:** `chore/agents-doc-guard` (worktree `~/Projects/Rust/webfang-worktrees/chore-agents-doc-guard`), base `origin/main` @ 3783d59d (post-#2834).
**Delivery route:** sequential slices — Slice 1 (#2832 / PR #2834, MERGED) pruned AGENTS.md to 647 lines / 40,957 bytes; this slice adds the fail-closed guard that makes further growth red. No stacked PRs.
**Scope:** guard + two harnesses + baseline key + CI wiring + fast-gate mirrors + coverage note + this record. No AGENTS.md content change, no `docs/` change, no `CHANGELOG.md` change, no Rust change.

## Objective

Engram obs #368: *"Sin un gate que falle, un doc largo no se auto-achica. Hace falta un guard,
no una buena intención."* The router regrew 740 → 862 lines after #2208 because nothing
evaluated its size. The guard was deferred as Fase 2 of the routerization spec (obs #2317,
`odd/tasks/agents-md-routerization.md` T5) and never built. Owner-approved scope
(AskUserQuestion, 2026-11-16): **size + structure** — line/byte budget, paired fences, zero
line-number citations. No prose/fact checking.

## Design

- **`scripts/check_agents_doc.sh`** — fail-closed (`set -euo pipefail`, `err()` →
  `::error::` + exit 1, Spanish messages), sibling of `check_docs_claims.sh`:
  1. **Size budget** from the new `agents_doc` key in `scripts/quality-baselines.json`
     (`max_lines: 650`, `max_bytes: 40960` — the owner-approved ceiling; main measures
     647 / 40,957). Budget lives in the JSON, not the script, so an agent cannot silently
     raise it to pass. `--update max_lines=N [max_bytes=M]` is DESCENDING-only (the
     `file_size_rust` ratchet philosophy, #516); raising is a manual maintainer-reviewed
     JSON edit. One embedded python block owns every baseline read/write, statuses
     machine-readable, any non-OK is a red gate.
  2. **Fence balance** — count of `^``` ` lines must be even.
  3. **Zero line-number citations** — extension-anchored regex
     (`[A-Za-z0-9_./-]+\.(rs|sh|bash|toml|md|json|ya?ml|txt|tsv):[0-9]+`) so `#2307`,
     `v8.0.0`, `127.0.0.1:8080`, `:443` never match; per-line
     `<!-- line-cite:allow -->` marker for intentional edge cases.
  4. **Fail-closed inputs** — missing AGENTS.md, missing/unreadable/invalid baseline → red.
- **`scripts/test_agents_doc.sh`** — semantics harness, 16 cases on mktemp fixtures:
  healthy green; one NEGATION per check (oversize lines, oversize bytes, odd fences, line
  citation); allow-marker green; no false positives; four fail-closed inputs; budget pin
  (650 / 40960 — a silent upward JSON edit breaks the pin); `--update` descending applied,
  upward refused untouched, unknown key refused.
- **`scripts/test_agents_doc_wiring.sh`** — wiring harness (structure of
  `test_docs_claims_wiring.sh`): `repo-guards` runs all three scripts; the gate step carries
  `# guard-required` (blocking from day one, no advisory phase — the budget is already met
  on main); `repo-guards` unfiltered/unconditional; `gate` needs `repo-guards`;
  `ci_fast_gate.sh` mirrors the gate in ≥2 lanes and runs both harnesses. Verified RED on
  the unwired tree before wiring (non-vacuous).

## Wiring

- `.github/workflows/ci.yml` `repo-guards`: 3 steps beside the other doc guards (after the
  docs-claims trio). Satisfies the guard-coverage meta-guard (#2056): wired guard + wired
  `scripts/test_agents_doc.sh` harness + `# NEGATION:` markers + mutation caught (green
  intact, red against an `exit 0` stub). NOT added to `guard_coverage_baseline.txt` —
  required from day one.
- `scripts/ci_fast_gate.sh`: `run_guard` mirror in `lane_docs` (markdown side) and
  `lane_fmt_and_guards` (baseline/code side); triggered harness block in `lane_ci`
  (`scripts/**` is the only change class that can break the guard/harnesses, and
  `lane_docs` never runs for it).
- `scripts/ci_fast_gate_coverage.tsv`: `repo-guards` partial note names the new guard and
  where it runs; `scripts/test_ci_gate_structure.sh` stays green (23 assertions, 7
  self-tests).

## Verification

- `bash scripts/check_agents_doc.sh` → OK (647 / 40,957 under 650 / 40,960; 14 fences; zero
  citations).
- `bash scripts/test_agents_doc.sh` → 16 passed, 0 failed.
- `bash scripts/test_agents_doc_wiring.sh` → 7 FAIL before wiring, all ok after.
- `bash scripts/test_guard_coverage.sh` → new guard wired, marker present, mutation caught.
- `bash scripts/test_ci_gate_structure.sh` → green.
- shellcheck (shared CI ignore list) clean on the three new scripts; actionlint clean on
  ci.yml.
- `bash scripts/ci_fast_gate.sh` → GREEN (docs+ci lanes); coverage report read.
- No Rust changed: the cargo pre-commit quartet is not applicable to this diff; the ci lane
  carries the bash/workflow validation instead.

## Log

- L1 (2026-11-16): worktree bootstrapped off 3783d59d; templates studied
  (`check_docs_claims.sh`, `test_docs_claims*.sh`, `check_file_size.sh` ratchet,
  `test_guard_coverage.sh`, fast-gate lanes); issue #2839 opened.
- L2 (2026-11-16): guard + baseline key + both harnesses written; semantics 16/16; wiring
  red-then-green; ci.yml + fast-gate + coverage.tsv wired; meta-guard mutation caught.
