---
name: webfang-worktree-ops
description: "Trigger: worktree, bootstrap, .envrc, target dir, post-merge cleanup, identity proxy, contamination. Run webfang worktree lifecycle safely; complement Engram #2307."
license: Apache-2.0
metadata:
  author: webfang
  version: "1.1"
---

## Activation Contract

Activate when creating or bootstrapping a worktree, writing `.envrc`, choosing a
`CARGO_TARGET_DIR`, committing in a worktree, running post-merge cleanup, handling a
branch/directory mismatch or suspected contamination, or when a worktree's build or
intelligence tools appear to resolve to `main`. Skip for trivial in-place edits.

## Hard Rules

- Sibling dir under `~/Projects/Rust/webfang-worktrees/`; never in-repo, never `/tmp` (tmpfs
  quota kills the build with an empty log).
- `.envrc` is exactly 5 lines, written from the tree's own name with `$TREE` substituted; a
  literal `$TREE` left in the file makes the isolation guard reject the build.
- `.envrc` is **gitignored** (`.gitignore:144`): it is per-worktree and never travels with the
  branch. A worktree without one inherits the parent shell's `CARGO_TARGET_DIR` — almost always
  `main`'s — and silently compiles into main's target. Isolation is never inherited.
- `direnv allow` is part of bootstrap, not optional: without it direnv blocks the rc and the
  worktree builds unisolated. Verify with `direnv export bash` (see Isolation Verification) —
  not with `direnv status`, which prints `Found RC allowed 0` even in `main`.
- `.codegraph/` and the codedb snapshot are **per-worktree**. Missing, both resolve to `main` in
  silence: a green answer from either tool is not evidence that the branch's tree was read.
- One worktree per session. Never `git checkout`/`switch`/`stash` (shared `refs/stash`); use
  `git worktree add`/`remove`/`prune`.
- Identity proxy before every commit: `basename "$PWD"` == `git branch --show-current | tr '/' '-'`.
  A mismatch is a hard stop unless the 3-condition exception protocol holds (external anchor +
  verifiable binding + ancestry); record the trailer.
- `seeds/` and `quarantine/` under `~/.cache/cargo-target/` are reserved; never a build target.
- Contamination: STOP, do not clean up, report worktree + intruder, await human instructions.
- Pushed history is immutable; corrections land on top as a new commit, never reset/rebase.

## Decision Gates

| Situation | Action |
| --- | --- |
| New branch needed | `git worktree add <sibling> -b <type/desc> origin/main`; bootstrap + index |
| Proxy mismatch | Verify exception protocol; if any condition fails, hard stop |
| Merge landed | Run post-merge cleanup from main (verify MERGED → ff-only sync → remove worktree → delete branch → prune → remove target dir by ownership) |
| Operated outside worktree / unexpected stash | Contamination protocol: stop + report, no corrective action |

## Execution Steps

1. Create the sibling worktree off `origin/main`; confirm branch ↔ dir name.
2. Bootstrap `.envrc` (5 lines, `$TREE` substituted) + `direnv allow` + `cp .env` from main.
3. Index: `codegraph init`; `codedb "$PWD" reindex && codedb "$PWD" status` (root + head must match).
4. Verify isolation with the two checks below. A worktree that skips them is unisolated and blind.
5. Optionally `bash scripts/seed_target.sh`; commit after every completed step.
6. On merge: run the post-merge cleanup runbook from main; remove the target dir by ownership
   (standalone `rm` with the literal absolute path, after a moving-window liveness check).

## Isolation Verification (mandatory after bootstrap)

Two checks: content+activation in one, and the indexes.

```bash
W=<abs-worktree>
# 1. CONTENT + ACTIVATION. `direnv export bash` is what the shell hook runs, and it
#    prints the rc's exports only when the rc is ALLOWED. Empty output = blocked.
(cd "$W" && direnv export bash 2>/dev/null) | grep -o "CARGO_TARGET_DIR=\$'[^']*'"
#    must be .../cargo-target/<tree>

# 2. indexes. Each must name the worktree and match its HEAD.
(cd "$W" && codegraph status | head -8)                      # Project: <tree> + Files: N, never "Not initialized"
codedb "$W" status | grep -E 'root|files|head'               # root=<tree>, head == git -C "$W" rev-parse --short HEAD
```

Two traps in the check itself:

- `direnv exec "$W" …` forces the load even when direnv would block the rc, so it proves the
  FILE is right, never that it is ACTIVE.
- `direnv status` is not a discriminator: it prints `Found RC allowed 0` in `main` too, which is
  loaded and working.

| Symptom | Cause | Fix |
| --- | --- | --- |
| `main` recompiles after a worktree build | worktree has no `.envrc`; it inherited main's `CARGO_TARGET_DIR` | write the 5-line `.envrc` from the tree's name + `direnv allow` |
| `.envrc` exists, target is still main's | rc not allowed by direnv (check 1 prints nothing) | `direnv allow` |
| `codegraph explore` returns files that do not exist in the worktree | codegraph not initialized there; the bare name resolved to `main` | `codegraph init`, then restart the MCP connection |
| `codedb status` head ≠ branch head | stale index | `codedb "$W" reindex && codedb "$W" status` |
| intelligence tools "find nothing" in the worktree | both indexes missing | run the bootstrap steps 2-4 |

## Output Contract

Return: worktree path + branch, bootstrap/index status, the isolation verdict (both checks
with their observed values), identity-proxy result per commit, and (for cleanup) the verified
handoff (`git worktree list`, `git branch -vv`, clean `git status`).

## References

- `references/sources.md` — Engram #2307 query + local sources of truth.
- `../../../AGENTS.md` — § Worktree policy (normative rules).
- `../../../scripts/seed_target.sh` — target seed contract.