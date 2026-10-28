# mcp-release-artifact-2428

Objective: publish the MCP surface (36 tools) as a release artifact with the same
trust chain as the CLI binary — SLSA attestation, keyless cosign signature and
`SHA256SUMS.txt` coverage (issue #2428).
Branch: `worktree-2428-mcp-release-artifact` · Worktree: `~/Projects/Rust/webfang-worktrees/2428-mcp-release-artifact` · Delivery: local branch, no PR yet.

## Status

Slice 1 implemented and validated locally. Slices 2-4 open. No `cargo build` has
been run: the build-cost finding below is inferential and unmeasured.

## Specs

- **S1** — `webfang-mcp` and `webfang-mcp-stdio` are built per target in
  `release.yml`, on the same 4-target matrix. **Done.**
- **S2** — `attest` and `sign` cover the new assets; `publish` still refuses
  without an attestation. **Covered by glob, but the completeness inventory does
  not yet require the new assets — see S2-gap.**
- **S3** — `SHA256SUMS.txt` includes the new assets. **Covered by
  `sha256sum webfang-*`; no change needed.**
- **S4** — a post-build smoke test proves the published binary answers
  `tools/list` with 36 tools. **Open.**
- **S5** — installation docs say which artifact serves MCP. **Open;
  `docs/src/installation.md:278` currently states the opposite.**

### S2-gap

`RELEASE_EXPECTED_ASSETS` (`release.yml`) and `EXPECTED_ASSETS`
(`scripts/ensure-release.sh`) still list only the seven pre-existing assets.
The new archives are signed and published by glob, but a release that lost them
would still pass the completeness gate. These two lists must change together —
`scripts/test_release_reconcile.sh` diffs them — and its hardcoded
"complete release" fixtures must gain the new names.

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

- **R1 — build cost (inferential, unmeasured).** `webfang_cli/mcp` is an empty
  marker that forwards nothing, while `webfang_mcp` defaults to
  `webfang_core/mcp`. Two distinct feature sets mean two `webfang_core`
  fingerprints, so BoringSSL is rebuilt per target. A single invocation
  (`-p webfang_cli -p webfang_mcp --features webfang_cli/mcp`) would unify them.
- **R2 — the binary list is transcribed three times** (strip, tar, zip).
- **R3 — nothing asserts the shipped binaries were stripped.**

## Validation

- `bash scripts/test_archive_reproducibility.sh` → 15 passed, 0 failed.
- `actionlint .github/workflows/release.yml` → no findings beyond the three
  pre-existing SC2012 info warnings in the sign/publish `ls -1` blocks.
- Dry-run of both archive loops against dummy binaries: names, guard acceptance
  and the negative control (swapped pair fails) all verified.
