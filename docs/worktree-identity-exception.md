# Worktree identity exception

A directory/branch mismatch is denied unless a valid owner-issued attestation exists in the
per-worktree git dir. The attestation replaces **only** the basename proxy. The four commit
trailers are audit evidence, not the key that unlocks the gate.

## Quick path

1. In an interactive terminal, the owner runs:
   `scripts/githooks/identity-attestation.sh --branch <branch> --evidence <external-source>`
   with `OWNER_IDENTITY_ATTEST_TOKEN` set. The command requires a TTY and a typed confirmation.
2. Commit normally. `identity-gate.sh` validates the three conditions and binds the exact staged
   tree (`git write-tree`) into the record.
3. `scripts/githooks/post-commit` consumes the record only when `HEAD^{tree}` matches the bound
   tree. It marks the record `spent` before deleting it, so a failed delete cannot be reused.

## The three conditions

| # | Condition | Satisfied by |
| --- | --- | --- |
| 1 | Externally anchored expectation | The intended branch is known from outside this worktree **before** committing: PR metadata (`gh pr view --json headRefOid,headRefName`), task metadata, an upstream ref, or explicit human instruction. A branch that exists only inside the worktree proves nothing. |
| 2 | Verifiable binding | `git symbolic-ref --short HEAD` and `git worktree list` agree on path → branch. `git rev-parse @{u}` is an extra signal when an upstream exists. |
| 3 | Ancestry, not equality | Pre-commit HEAD is at, or a descendant of, the expected tip/base. Strict `HEAD == tip` breaks after the first commit of a sequence. |

Any failed condition is a hard stop.

## The four audit trailers

Put this block in the commit body when an exception is used:

```text
Worktree-Identity-Exception: directory-name mismatch
Intended-Branch: <branch>
Expected-Head: <sha>
Evidence: <external source> + git symbolic-ref + git worktree list
```

The gate does **not** parse these trailers. They exist so a later reader can audit the exception
without reconstructing the chat. The authority is the attestation file described below.

## Attestation record

The file lives at `$(git rev-parse --absolute-git-dir)/webfang-identity-attestation`: the
per-worktree git dir, never the shared git common dir and never the versioned tree.

| Field | Written by | Meaning |
| --- | --- | --- |
| `version` | issuer | Record format, currently `1`. |
| `branch` | issuer | Intended branch; must equal the HEAD branch at validation. |
| `anchored_head` | issuer | Commit used for the ancestry check (`merge-base --is-ancestor`). |
| `evidence` | issuer | External source of the expectation; must be non-empty. |
| `expires_at` | issuer | Unix expiry; a default of 900 seconds applies. |
| `nonce` | issuer | Identifier copied to the consumption audit log. |
| `tree` | `identity-gate.sh` | Exact staged tree bound at pre-commit time. |
| `spent` | `post-commit` | Set before delete; a spent record is rejected on the next commit. |

## What the exception never bypasses

- `.worktree-identity` manifest (exact branch recorded at bootstrap).
- `.envrc` declaration and the `CARGO_TARGET_DIR` guard.
- Canonical `main` root on `main`, or a non-`main` branch in the main checkout.
- Detached HEAD, rebase and cherry-pick states.

The issuer requires TTY + token + typed confirmation as a **policy barrier**, not cryptography:
any process running as the owner can write the file. Validation therefore checks the record's
content, never its provenance.

## Checklist

- [ ] Intended branch is anchored outside the worktree before committing.
- [ ] `git symbolic-ref --short HEAD` and `git worktree list` agree.
- [ ] Pre-commit HEAD is at or descendant of the expected SHA.
- [ ] The four trailers are in the commit body for audit.
- [ ] A later commit in the same worktree has a fresh attestation.

## Next step

The router entry is `AGENTS.md` § Identity-exception protocol; the historical full text is Engram
#3201 (its sentence claiming the gate already implemented the override is stale).
