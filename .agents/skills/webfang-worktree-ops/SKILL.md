---
name: webfang-worktree-ops
description: "Trigger: worktree, bootstrap, .envrc, identity gate, manifest, target dir, orphan sweep, post-merge cleanup, contamination. Run webfang worktree lifecycle safely; complement Engram #2307."
license: Apache-2.0
metadata:
  author: webfang
  version: "1.2"
---

## Activation Contract

Activate when creating or bootstrapping a worktree, writing `.envrc` or the identity manifest,
choosing a `CARGO_TARGET_DIR`, committing in a worktree, diagnosing a fail-closed identity gate,
classifying or collecting orphan target dirs, running post-merge cleanup, handling a
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
  It is also enforced by `scripts/githooks/identity-gate.sh`, wired **before** `review-gate.sh` in
  `pre-commit` and fail-closed. Bootstrap writes the ignored per-tree `.worktree-identity` via
  `scripts/worktree_identity.sh`; without it feature-worktree commits are denied. In non-interactive
  shells load the tree env (`eval "$(direnv export bash)"`) or an inherited `CARGO_TARGET_DIR` is
  denied. A mismatch is a hard stop unless a valid owner-issued attestation exists
  (`../../../docs/worktree-identity-exception.md`); the four trailers are audit only.
- Four reserved identities under `~/.cache/cargo-target/`: `seeds/`, `seeds-webfang/`,
  `quarantine/`, `quarantine-webfang/` — canonical identity, never a build target, never collected.
- `scripts/sweep_orphan_targets.sh` is dry-run by default. Its live registry is the
  `CARGO_TARGET_DIR` declarations in live worktrees' `.envrc`, never worktree root paths. It
  protects the four reserved identities, uses `lsof` and newest-file mtime, and rechecks all guards
  at delete time. `--execute` requires `SWEEP_CONFIRM_TOKEN` from the owner; never run it ad hoc.
- Contamination: STOP, do not clean up, report worktree + intruder, await human instructions.
- Pushed history is immutable; corrections land on top as a new commit, never reset/rebase.

## Decision Gates

| Situation | Action |
| --- | --- |
| New branch needed | `git worktree add <sibling> -b <type/desc> origin/main`; bootstrap + index + identity manifest |
| Proxy mismatch | Verify the owner attestation and its three conditions (`../../../docs/worktree-identity-exception.md`); if any fails, hard stop |
| Commit denied by identity gate | Write the manifest with `scripts/worktree_identity.sh`; load direnv in non-interactive shells. Never bypass the hook |
| Orphan target dirs after merges | Run the sweep dry-run; reserve the four identities; execute only with explicit owner token |
| Merge landed | Run post-merge cleanup from main (verify MERGED → ff-only sync → remove worktree → delete branch → prune → remove target dir by ownership) |
| Operated outside worktree / unexpected stash | Contamination protocol: stop + report, no corrective action |

## Execution Steps

1. Create the sibling worktree off `origin/main`; confirm branch ↔ dir name.
2. Bootstrap `.envrc` (5 lines, `$TREE` substituted) + `direnv allow` + `cp .env` from main +
   `scripts/worktree_identity.sh` (writes the ignored manifest; the shared pre-commit gate needs it).
3. Index: `codegraph init`; `codedb "$PWD" reindex && codedb "$PWD" status` (root + head must match).
4. Verify isolation with the checks below. A worktree that skips them is unisolated and blind.
5. Optionally `bash scripts/seed_target.sh`; commit after every completed step.
6. On merge: run the post-merge cleanup runbook from main; remove the target dir by ownership
   (standalone `rm` with the literal absolute path, after a moving-window liveness check).
7. To reclaim space, run `scripts/sweep_orphan_targets.sh` in dry-run and read the JSON report;
   execute only after explicit owner authorization and `SWEEP_CONFIRM_TOKEN`.

## Isolation Verification (mandatory after bootstrap)

Three checks: content+activation, the indexes, and the identity gate.

```bash
W=<abs-worktree>
# 1. CONTENT + ACTIVATION. `direnv export bash` is what the shell hook runs, and it
#    prints the rc's exports only when the rc is ALLOWED. Empty output = blocked.
(cd "$W" && direnv export bash 2>/dev/null) | grep -o "CARGO_TARGET_DIR=\$'[^']*'"
#    must be .../cargo-target/<tree>

# 2. indexes. Each must name the worktree and match its HEAD.
(cd "$W" && codegraph status | head -8)                      # Project: <tree> + Files: N, never "Not initialized"
codedb "$W" status | grep -E 'root|files|head'               # root=<tree>, head == git -C "$W" rev-parse --short HEAD

# 3. identity gate: exit 0 from a direnv-loaded shell with the manifest present.
(cd "$W" && eval "$(direnv export bash)" >/dev/null 2>&1 && scripts/githooks/identity-gate.sh)
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
| commit denied: missing identity manifest | bootstrap skipped `scripts/worktree_identity.sh` | write the manifest in that worktree; never bypass the hook |
| commit denied: `CARGO_TARGET_DIR` mismatch | non-interactive shell inherited main's target | `eval "$(direnv export bash)"`, then retry |

## Output Contract

Return: worktree path + branch, bootstrap/index status, the isolation verdict (three checks with
their observed values), manifest and identity-gate result per commit, sweep dry-run report when
space was reclaimed, and (for cleanup) the verified handoff (`git worktree list`, `git branch -vv`,
clean `git status`).

## References

- `references/sources.md` — Engram #2307 query + local sources of truth.
- `../../../AGENTS.md` — § Worktree policy (normative rules).
- `../../../scripts/seed_target.sh` — target seed contract.
- `../../../scripts/worktree_identity.sh` — writes the ignored `.worktree-identity` manifest.
- `../../../scripts/githooks/identity-gate.sh` — fail-closed pre-commit identity gate.
- `../../../scripts/sweep_orphan_targets.sh` + `../../../scripts/lib/worktree_paths.sh` — orphan sweep.