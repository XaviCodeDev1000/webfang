# Release Process

Two tools cut a release, and each owns exactly one half:

| tool | owns |
|---|---|
| `release-plz` | the version bump, `CHANGELOG.md`, the Release PR, the `v*` tag |
| `release.yml` | the binaries, the archive normalization and the GitHub Release page, triggered by the tag |

The configuration lives in [`release-plz.toml`](https://github.com/XaviCodeDev1000/webfang/blob/main/release-plz.toml)
and the workflow in
[`.github/workflows/release-plz.yml`](https://github.com/XaviCodeDev1000/webfang/blob/main/.github/workflows/release-plz.yml).
This page explains the three decisions that are *not* written in either file: which commits
may cut a version, how to check the filter without touching the remote, and what the support
metadata means.

## Which commits may cut a version

`release-plz` bumps on every commit by default, so a `test:`, `ci:`, `docs:` or plain
`refactor:` commit publishes a tag, a build matrix and a GitHub Release for a change no user
can observe. The workspace sets `release_commits` to fix that:

> the user-facing types always release; **any** type releases when it is breaking.

The exact pattern is in `release-plz.toml` — it is the source of truth and it is pinned by
`scripts/test_release_commits_regex.sh`, which reads it *from the file* and runs a table
against it (a duplicated regex keeps passing while the configuration is broken).

| commit | releases? | why |
|---|---|---|
| `feat:`, `fix(scope):`, `perf:`, `revert:` | yes | user-visible |
| `chore(deps): bump …` | yes | the binary changes and the user cannot rebuild it |
| any type with `!` — `refactor(api)!:`, `docs!:` | yes | a breaking change is user-visible by definition |
| `refactor:`, `refactor(scope):` | no | behaviour-preserving; it ships with the next real release |
| `test:`, `ci:`, `docs:`, `style:`, `chore:` | no | nothing the user can observe |

Two consequences worth knowing:

- **A breaking change is never delayed by a non-breaking type.** The `!` marker is the
  canonical one (with or without a scope), so `refactor(exporter)!:` releases even though
  `refactor:` does not. This repo has shipped breaking refactors that way.
- **Filtered commits are not lost.** They still appear in the `CHANGELOG.md` of the next
  release that does fire, in their own section. The filter decides *when a version is cut*,
  not what the notes say.

## Checking the filter without touching the remote

There is no `--dry-run` for the Release-PR path, but there is a safe pre-check: run the PR
command with a token that cannot possibly work.

```bash
release-plz release-pr --git-token invalid
```

It resolves the versions, logs `no commit matches the 'release_commits' regex` per package
when the window has nothing releasable, and exits **without opening a PR or modifying the
working tree**. A window that *does* match fails later, at the first API call — which is
exactly the signal you want: it means a PR would have been proposed.

**Do not use `release-plz update` as the pre-check.** It mutates the checkout (it rewrites
`Cargo.toml` and the changelog), and on a workspace where every package is filtered it
*writes the new version anyway*: it puts the bumped workspace version into `Cargo.toml`
without updating `Cargo.lock`, leaving a tree that cannot build. That is a behaviour of the
`update` command, not of the PR path — but it makes it the wrong tool for a quick look, and a
dangerous one to run in a working checkout. Measured on release-plz 0.3.168 and 0.3.169 (the
version the GitHub Action installs).

**A probe commit has to touch a file of the crate.** release-plz filters by the files of each
package before it evaluates the commit message, so an empty commit (`git commit --allow-empty`)
reports `no commit matches` no matter what the subject says, and a commit that only touches
`docs/` or `scripts/` does the same for all three packages. To probe the *positive* path,
append a line to a file inside the crate you expect to release (for example
`crates/webfang_core/src/lib.rs`).

Also note that the GitHub Action installs its own release-plz version; a local CLI may be a
different one. The pre-check is a smoke test, not the verdict — the verdict is the next real
run of the workflow.

## A commit that only moves `Cargo.lock` does not cut a version

With `release_commits` active, a commit that changes **only** `Cargo.lock` reports
`no commit matches the 'release_commits' regex` and does not release — even when its subject
matches (`chore(deps):`, or `fix:`). Measured on release-plz 0.3.168 and 0.3.169, with a clean
window (a single lock-only commit on top of `v7.7.1`): without `release_commits` the same
commit produces `next version is 7.7.2`; with the filter, nothing. `release-plz` decides by the
files of each package, and `Cargo.lock` lives at the workspace root, outside every package.

The hole only bites when **no other commit in the window releases**: a window with a `feat:`
or `fix:` releases anyway and carries the lock change with it (that is how #2433 and #2361
shipped). It is not a hypothetical shape, though: with the filter, `test:`, `ci:` and `docs:`
commits accumulate without cutting a version, so a lock-only bump can easily be the only thing
in the window.

**The convention that closes it:** the commit has to touch a **packaged file of the affected
crate**, so the change is attributed to that package. In this workspace that rules out the
root `Cargo.toml`: the dependency requirements live in `[workspace.dependencies]`, and the
root file belongs to no package — raising `libc` there still reports `no commit matches`
(measured on 0.3.168 and 0.3.169). What works is a file inside the crate:

- if the crate declares the dependency with its own version requirement, raise it there
  (a real change: it records the new minimum);
- when the dependency is transitive and the crate only says
  `libc = { workspace = true }`, add a comment in that `Cargo.toml` naming the advisory, e.g.
  `# <crate>: <RUSTSEC-...> patched in the lockfile.` — measured on release-plz 0.3.168 and
  0.3.169: all three packages get the patch version.

The comment is a marker, not a fix: prefer the requirement change whenever the crate actually
owns one.

## Why a Release PR cannot be generated against an untagged `main`

`release-plz` derives the next version from the commits between the newest `v*` tag and
`main`, and in `git_only` mode that tag list is the **local** one. A `main` that is ahead of
its own tag — the version bump merged, the tag seconds away — is a state it cannot read, and
it proposes the version that is already there.

The PR job therefore syncs its checkout to `origin/main` and waits, bounded, for the tag of
the declared version before running `release-plz` (`scripts/await_release_tag.sh`). When the
tag arrives, there is nothing to release; when it does not, the step fails loudly instead of
opening a phantom Release PR. A repository with no `v*` tag at all is still the initial
release and is not awaited.

## Reverts must be conventional

The filter reads the commit subject, and `git revert` writes `Revert "…"` — capital `R`, not
a conventional type — which matches nothing. A revert that must ship has to be recorded as
`revert: …` (lowercase), either by amending the subject or by writing the revert commit by
hand. Otherwise the change it undoes stays published until the next release that fires for
another reason.

## What `released_at` means in `.github/support.json`

`.github/support.json` describes the support lines, and its two date fields answer different
questions:

- `latest` — a fact about what this repository published. It is written by
  `scripts/refresh_support_latest.sh`, the single writer, and it moves on every patch.
- `released_at` — the date the **minor line** went stable. It is written once, during the
  rotation of a minor (`scripts/rotate-stable.sh --pending-publication`), and a patch does
  **not** move it. `scripts/rotate-stable.sh` reads it, so it is a policy field, not a label:
  it is the clock the support window is measured from.

So seeing `latest` at a patch while `released_at` still holds the minor's date is the correct
state, not drift.

## Approving the release PR's linked issue

`Validate PR metadata` checks that every linked issue carries `status:approved`, and it reads
the label from the **issue** through the API. The `pull_request` workflow that runs the check
listens to the PR's own events (`opened`, `synchronize`, `labeled`, …), so approving the issue
is not a PR event: the check does not re-run on its own and the run keeps showing the old
verdict.

If the metadata check is red after the issue was approved, re-run the failed job —
`gh run rerun <run-id> --failed`, or "Re-run jobs" in the UI. Both attempts observed on
#2788 and #2791 were manual re-runs (same `triggering_actor`, event `pull_request`), not label
events.

## `set-version` is not a release flow

`release-plz set-version` exists to correct a version that was not set as expected (for
example a commit that was not prefixed correctly). It is a correction tool, not a way to cut a
release: it does not open a PR, it does not write the changelog entry, and it has not been
exercised in this repository. If a release has to be forced for a window that the filter does
not match, treat that as an incident to investigate — not as a routine path.