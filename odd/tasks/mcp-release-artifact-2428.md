# mcp-release-artifact-2428

Objective: publish the MCP surface (36 tools) as a release artifact with the same
trust chain as the CLI binary — SLSA attestation, keyless cosign signature and
`SHA256SUMS.txt` coverage (issue #2428).
Branch: `worktree-2428-mcp-release-artifact` · Worktree: `~/Projects/Rust/webfang-worktrees/2428-mcp-release-artifact` · Delivery: local branch, no PR yet.

## Status

S1-S5 implemented and validated locally. The build-cost finding (R1) is now
measured, not inferred.

## Specs

- **S1** — `webfang-mcp` and `webfang-mcp-stdio` are built per target in
  `release.yml`, on the same 4-target matrix. **Done.**
- **S2** — `attest` and `sign` cover the new assets; `publish` still refuses
  without an attestation. **Done: the globs covered the new assets already, and
  the completeness inventories now require them.**
- **S3** — `SHA256SUMS.txt` includes the new assets. **Covered by
  `sha256sum webfang-*`; no change needed.**
- **S4** — a post-build smoke test proves the published binary answers
  `tools/list` with 36 tools. **Done: `scripts/smoke-mcp-stdio.sh` +
  `mcp-smoke` job in `release.yml` (`publish` needs it), with
  `scripts/test_smoke_mcp_stdio.sh` covering the pass, the short-count, the
  hang and the missing-binary paths.**
- **S5** — installation docs say which artifact serves MCP. **Done: README,
  `docs/src/installation.md` (new "MCP servers (from the release artifact)"
  subsection), `docs/src/overview.md`, and the now-false row in
  `docs/src/troubleshooting.md`.**

### Inventory wiring

`RELEASE_EXPECTED_ASSETS` (`release.yml`) and `EXPECTED_ASSETS`
(`scripts/ensure-release.sh`) both list the 15 assets and are diffed against
each other by `scripts/test_release_reconcile.sh`. Three fixtures had to move
with them, and each one was a hardcoded count that only surfaced when a harness
ran: the wiring check's `7 7`, the by-id completeness check's `14`, and
`scripts/test_release_squash_dispatch.sh`, which carries its OWN copy of the
complete-release fixture.

## Decisions

- **One archive per binary.** `scripts/check_archive_reproducibility.sh`
  asserts exactly one regular-file entry per archive. Bundling the CLI and MCP
  binaries would fail a guard with its own test suite, so the MCP binaries get
  their own archives: `webfang-mcp-<target>` and `webfang-mcp-stdio-<target>`.
- **`--expect-entry <archive>=<entry>`.** The guard asserted the entry COUNT
  but never the entry NAME, so with three archives out of one loop a mis-paired
  `(binary -> asset)` tuple would ship a normalized archive holding the wrong
  executable, with every invariant green.

## Open findings (review, 4 lenses)

- **R1 — build cost (MEASURED).** `webfang_cli/mcp` is an empty marker that
  forwards nothing, while `webfang_mcp` defaults to `webfang_core/mcp`. Two
  distinct feature sets mean two `webfang_core` fingerprints, so the crate is
  compiled twice per target.

  **Clean-room experiment.** Each scenario used a FRESH `CARGO_TARGET_DIR`
  (nothing cached, BoringSSL rebuilt from scratch), same source tree, `cargo
  check -j 8`, dev profile so no codegen — the cheapest probe that still
  exercises real feature resolution:

  | scenario | invocations | wall time | `webfang_core` compiled |
  |---|---|---|---|
  | A — old shape | `-p webfang_cli --features mcp` then `-p webfang_mcp --bins` | 42.59 s + 15.46 s = **58 s** | **2×** |
  | B — new shape | `--features webfang_cli/mcp -p webfang_cli -p webfang_mcp --bins` | **48.02 s** | **1×** |

  One invocation removes one full `webfang_core` unit and 10 s of the cold
  build, in `check` with no codegen. In release the same crate is compiled with
  LTO and `codegen-units = 1`, so the removed unit is far more expensive than
  the number above suggests — that ratio was NOT measured, because a clean
  four-target release build is not something this worktree pays for.

  The fix is applied: `release.yml`'s build step is the scenario-B command.
- **R2 — the binary list is transcribed three times** (strip, tar, zip).
- **R3 — nothing asserts the shipped binaries were stripped.**

## Adversarial review

A blind review of the full diff returned 0 blockers, 9 warnings and 5
suggestions. Fixed here:

- the release notes said `SHA256SUMS.txt` "lists all four archives" (twelve now),
  never mentioned the MCP archives, and the from-source command still built only
  `-p webfang_cli`;
- the smoke job claimed "only the runner's own target is smoked" when it only
  runs on ubuntu-latest — 1 of 4 targets is executed, and the comment now says so;
- `webfang-mcp` (HTTP) was never executed by any job. It now runs `--version`,
  which is the cheapest proof that the published artifact starts;
- the smoke compared the tool COUNT but not the NAMES, so a binary serving 36
  renamed tools passed. It now compares the fixture's `[tools]` names too, and
  the harness has a renamed-tool case that a count-only check cannot catch;
- the comment claiming the MCP build "reuses the `webfang_core` compilation the
  CLI build already paid for" was false — see R1. The two invocations are now one;
- `test_smoke_mcp_stdio.sh` was not wired into `ci.yml` or `ci_fast_gate.sh`, so
  the smoke's semantics would have rotted silently;
- `--bind` alone is a hard startup error, so the doc example could not run;
- the uninstall section listed only `webfang`, and called the install "one file";
- a duplicated `--expect-entry` silently overwrote the first expectation.

Also applied: `attest` now depends on `mcp-smoke`. Without that edge a binary
that fails the smoke was still attested and written to the Rekor transparency
log before `publish` discarded it — an immutable record of an artifact nobody
would ever download.

## Validation

- `bash scripts/test_archive_reproducibility.sh` → 16 passed, 0 failed.
- `bash scripts/test_smoke_mcp_stdio.sh` → 5 passed, 0 failed.
- `bash scripts/test_release_reconcile.sh` → 35/0 and 24/0, exit 0.
- `cargo check --release --locked --features "webfang_cli/mcp" -p webfang_cli
  -p webfang_mcp --bins -j 8` → the exact release-workflow invocation, cold,
  exit 0 in 1m02s. This is the only RELEASE-profile cargo command run for the
  merged build; the full release build (LTO, four targets) was not attempted
  here.
- `actionlint .github/workflows/release.yml` → no findings beyond the three
  pre-existing SC2012 info warnings in the sign/publish `ls -1` blocks.
- Dry-run of both archive loops against dummy binaries: names, guard acceptance
  and the negative control (swapped pair fails) all verified.
- R1's two clean-room runs were `cargo check` under dedicated
  `CARGO_TARGET_DIR`s outside the worktree, so the worktree's own `target/`
  was never populated and a warm cache cannot be mistaken for the measurement.
- CI on the pushed branch: `Repo guards` (the release hand-off gate, which
  asserts that EVERY checkout in `release.yml` carries `fetch-depth: 0` and
  `fetch-tags: true`) went red on the first push, because the new `mcp-smoke`
  job used a shallow clone. Fixed on the same branch. The two checks still red
  are not this change's: `Security Audit` fails on RUSTSEC-2026-0414
  (pulldown-cmark 0.13.4, advisory published after `main`'s last green run;
  #2433 patches it) and `breaking-surface` cannot read the merge-queue
  configuration with the workflow's token (#2426 fixes the read). This branch
  touches no manifest and no merge-queue code.
