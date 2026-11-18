# Sources — webfang-worktree-ops

This skill is the actionable layer. The detailed runbook, traps already paid for, and the seed
contract live in Engram; the normative rules live in the repo. Do not copy the runbook here.

## Engram (depth / history / diagnosis)

| Topic | Query | Obs | Scope |
| --- | --- | --- | --- |
| Worktree bootstrap, seeds, cleanup, target dirs, `.envrc` traps | `worktree target dir seed bootstrap envrc` | #2307 | project |
| Isolation gaps: missing/unallowed `.envrc`, indexes resolving to `main` | `worktree isolation envrc direnv codegraph codedb` | #3144 | project |
| Recurring env failures: `/tmp` leaks, false rustdoc ICE (mbx), CI state | `env failures tmp rustdoc mbx` | #3141 | project |
| Workstation build profile (mbx, mold, OOM, timings) | `build profile mbx mold cargo target OOM` | #2306 | personal |
| Contamination, immutable history, hard-deny | `contamination history immutable hard-deny governance` | #2312 | project |

Resolve with `engram search "<query>" --project webfang [--scope personal]` or `mem_search`.

## Activation source of truth (non-Engram)

- `direnv export bash` in the worktree — must print the rc's exports (e.g.
  `CARGO_TARGET_DIR=$'.../<tree>'`); empty output means the rc is blocked, then `direnv allow`.
  `direnv status` is NOT a discriminator: it prints `Found RC allowed 0` in `main` too, which is
  loaded and working.
- `codedb <abs-worktree> status` — `root` must be the worktree and `head` its `git rev-parse --short HEAD`.
  The codedb CLI is positional-first; `status` with a trailing path acts on the cwd project.
- `codegraph status` with the worktree as cwd — `Project:` must name the worktree; `Not initialized`
  means it silently falls back to `main`.

## Repo sources of truth (normative)

- `../../../AGENTS.md` — § Worktree policy: iron rules, placement/naming, identity proxy +
  exception protocol, per-worktree bootstrap, frequent commits, post-merge cleanup, shared vs
  per-worktree resources, reserved namespace, contamination, immutable history.
- `../../../scripts/seed_target.sh` — seed contract (`cold` normal, `refused` not a build).