---
name: webfang-file-splits
description: "Trigger: split files, file size, 500 lines, OLAS, ratchet, too_many_lines, jscpd cx. Apply webfang's split policy aligned with Rust/expert guidance."
license: Apache-2.0
metadata:
  author: webfang
  version: "1.0"
---

## Activation Contract

Activate when splitting a Rust file, reducing a grandfathered file in the size ratchet, sizing a
new module, or deciding whether a ~500-line file needs a split. Skip for non-source docs or a
one-off formatting edit.

## Hard Rules

- No official Rust source sets a file line limit. The Rust Book (ch7) asks only to group related
  functionality and limit the detail kept in your head. 500 is webfang's local guardrail/trigger,
  not Rust doctrine.
- The only official numeric thresholds are function-level: `clippy::too_many_lines` 100 and
  `clippy::cognitive_complexity` 25, both already enabled in webfang CI. Closure is co-equal:
  lines <=500 + those lints clean with no new suppressions + `jscpd --complexity` `cx` not
  increased. A 450-line file with short functions beats a 300-line file with a 180-line function.
- Split by real seams, never line arithmetic. A split that breaks cohesion is worse than the
  large file.
- Two hats, never mixed: mechanical move vs semantic change. The PR declares `Movido sin cambios`
  and `Corregido / mejorado`; semantic changes are separated, bounded (~20-30% of the diff) or
  moved to another PR.
- Behavior preserved: `refactor(scope)`, tests green between work units, snapshots intact, and
  ratchet descent (`--remove`) in the same PR as the split that produces it.
- Net-zero for grandfathered files: growing one by a line fails the guard. A single declarative,
  cohesive unit (enum, state machine, table, exhaustive `match` with minimal logic) may stay
  grandfathered with the reason in the baseline `notes`, decided per file in its mission.
- Use pinned `jscpd` 5.4.0 via `scripts/check_duplication.sh`, never `npx`.
- Before editing, run `codegraph_explore` for callers and blast radius. If the seam asks for a new
  crate, that is an architecture decision in its own mission/PR (Google Rust Style Guide:
  decide crate boundaries before submodules).

## Decision Gates

| Situation | Action |
| --- | --- |
| <=500 lines, cohesive, function lints clean | No split; record the check |
| >500 but one declarative/cohesive unit | Grandfather with a `notes` rationale in its mission |
| Seam = stuck modules / multi-responsibility function / giant match | extract module / extract function / table |
| Finding while moving | blocking: stop or separate PR with a regression test; small local: separate listed commit; speculative/broad: new issue |
| Semantic change above ~30% of the diff | Split the delivery or extract the correction |

## Execution Steps

1. Read the baseline entry from `scripts/quality-baselines.json`; name the real driver and seam;
   ask the crate-boundary question first.
2. Characterize behavior with existing tests/snapshots (or a minimal characterization test). No
   red-to-green safety net, no split.
3. Extract mechanically, one work-unit commit per seam, tests green between commits.
4. Verify the triple: line guard, function lints, `jscpd --complexity` (`cx` must actually drop).
5. Close in the ratchet: `--remove` when all parts are <=500, `--update` for smaller but still
   >500 parts.
6. Peer review with four lenses (logic, efficiency, security, maintainability); an explicit
   "no findings" is valid. Governance: own issue + PR `Closes part of #2584`, `type:refactor`,
   base `main`, sequential delivery.

## Output Contract

Return: the driver and seam chosen, evidence per lens (red-to-green / negative test / measurement
/ intact snapshots), the three verification results (lines, function lints, `cx`), the ratchet
action (`--remove` / `--update`), and the PR's two declaration lists.

## References

- `references/sources.md` — Engram #3000/#3001 and repo sources of truth.
- `../../../odd/tasks/file-size-reduction.md` — OLAS program (checked-out file is the source of truth).
- `../../../scripts/check_file_size.sh`, `../../../scripts/quality-baselines.json`,
  `../../../scripts/check_duplication.sh`.
