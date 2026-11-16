---
name: webfang-worktree-ops
description: "Trigger: worktree, bootstrap, .envrc, target dir, post-merge cleanup, identity proxy, contamination. Run webfang worktree lifecycle safely; complement Engram #2307."
license: Apache-2.0
metadata:
  author: webfang
  version: "1.0"
---

## Activation Contract

Activate when creating or bootstrapping a worktree, writing `.envrc`, choosing a
`CARGO_TARGET_DIR`, committing in a worktree, running post-merge cleanup, or handling a
branch/directory mismatch or suspected contamination. Skip for trivial in-place edits.

## Hard Rules

- Sibling dir under `~/Projects/Rust/webfang-worktrees/`; never in-repo, never `/tmp` (tmpfs
  quota kills the build with an empty log).
- `.envrc` is exactly 5 lines, written from the tree's own name with `$TREE` substituted; a
  literal `$TREE` left in the file makes the isolation guard reject the build.
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
4. Optionally `bash scripts/seed_target.sh`; commit after every completed step.
5. On merge: run the post-merge cleanup runbook from main; remove the target dir by ownership
   (standalone `rm` with the literal absolute path, after a moving-window liveness check).

## Output Contract

Return: worktree path + branch, bootstrap/index status, identity-proxy result per commit, and
(for cleanup) the verified handoff (`git worktree list`, `git branch -vv`, clean `git status`).

## References

- `references/sources.md` — Engram #2307 query + local sources of truth.
- `../../../AGENTS.md` — § Worktree policy (normative rules).
- `../../../scripts/seed_target.sh` — target seed contract.