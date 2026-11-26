## Summary

Move four probe-friendly syscall families in the **Baseline** seccomp profile from the kill tier to errno tiers:

- `memfd_create`, `landlock_create_ruleset`, `landlock_add_rule`, `landlock_restrict_self` → `ENOSYS`, matching the treatment `io_uring_*` and `clone3` already get.
- `ptrace` → `EPERM` in the consolidated EPERM filter, still denied unconditionally.

The **Strict** profile is unchanged and remains the default. `Baseline` stays opt-in via `--seccomp baseline` / `ARAPUCA_SECCOMP_PROFILE=baseline`.

## No new capability

This change grants **no new capability to the sandboxee**. Every syscall listed above was denied before and remains denied after; the only observable difference is that the kernel returns an errno instead of terminating the process. No previously blocked syscall becomes permitted, no new syscall is allowed anywhere in the filter stack, and the default profile is untouched.

## Motivation

Browser runtimes (Chromium, obscura) probe `memfd_create` and the Landlock syscalls at startup. A hard kill surfaces as an unexplained crash; `ENOSYS` keeps the denial while letting the caller take its existing fallback path. `ptrace` remains denied with `EPERM` so a tracer that probes it can degrade instead of being killed.

## Verification

- Applies cleanly to `main` at `f1f9574` (`git apply --check`).
- `cargo check --all-targets` on the patched tree: PASS.
- `seccomp_baseline_blocks_ptrace` updated to accept `exit=1` (EPERM) alongside the previous signal exits; a blocked ptrace is still required.

## Scope

Only `src/seccomp.rs` and `tests/run.rs`. No new dependency, no feature-flag change, no change to Strict.

Upstream PR: _pending_
