# Sources — webfang-worktree-ops

This skill is the actionable layer. The detailed runbook, traps already paid for, and the seed
contract live in Engram; the normative rules live in the repo. Do not copy the runbook here.

## Engram (depth / history / diagnosis)

| Topic | Query | Obs | Scope |
| --- | --- | --- | --- |
| Worktree bootstrap, seeds, cleanup, target dirs, `.envrc` traps | `worktree target dir seed bootstrap envrc` | #2307 | project |
| Workstation build profile (mbx, mold, OOM, timings) | `build profile mbx mold cargo target OOM` | #2306 | personal |
| Contamination, immutable history, hard-deny | `contamination history immutable hard-deny governance` | #2312 | project |

Resolve with `engram search "<query>" --project webfang [--scope personal]` or `mem_search`.

## Repo sources of truth (normative)

- `../../../AGENTS.md` — § Worktree policy: iron rules, placement/naming, identity proxy +
  exception protocol, per-worktree bootstrap, frequent commits, post-merge cleanup, shared vs
  per-worktree resources, reserved namespace, contamination, immutable history.
- `../../../scripts/seed_target.sh` — seed contract (`cold` normal, `refused` not a build).