# CI / Tests / PRs Playbook — learned from 7 elite Rust repos

> Date: 2026-09-08. Motivation: internal audit — 2744 test functions, 19 jobs in
> `ci.yml`, ~33 min of wall-clock for a green merge, high merge friction
> (`strict: true` with no merge queue). This doc is the adaptation guide: what to copy,
> what to adapt and what NOT to copy from each repo.
>
> How each piece of data was obtained: `gh api repos/<repo>/contents/.github/workflows/<file>`
> (raw YAML) + `gh run list` (real wall-clocks) + `raw.githubusercontent.com`
> (CONTRIBUTING, PR templates, CI scripts). Chaotic network that day: `api.github.com`
> intermittent, DeepWiki MCP and JINA API down — worked with `gh` + raw.

## 0. Our baseline (the "before")

| Metric | Value (2026-09-08) |
| ------- | ------------------ |
| Green PR wall-clock (main, run 34274635094) | **~33 min (1999 s)** |
| Jobs in `ci.yml` | 19 |
| Most expensive job | Coverage **1610 s** (~27 min) |
| 2nd most expensive | Tests AI integration **1163 s** (~19 min, **required**) |
| Required checks (branch protection) | `Validate PR metadata`, `cargo-mutants (PR diff)`, `CI Gate`, `Tests (AI integration)` |
| `strict` (requires being up to date with main) | **true** — every merge to main invalidates open PRs and re-triggers 33 min |
| Merge queue | **Does not exist** (loose `auto_merge: true`) |
| Tests | ~2744 functions (`#[test]`/`#[tokio::test]`/`#[rstest]` in `crates/`+`tests/`) |
| Mutants on PR | **hard gate** (`mutants-pr`, required) |

## 1. Compared wall-clocks (real green runs, `gh run list`)

| Repo | PR (push/PR event) | Queue/main | Main CI jobs | Note |
| ---- | ------------------- | --------- | ------------ | ---- |
| wasmtime | **8–9 min** | 25–34 min (merge queue) | 35 | FAST_MATRIX on PR, FULL in queue |
| rust-analyzer | **7–8 min** (fast red: 2 min) | 11–13 min (queue) | 12 | `conclusion` aggregator |
| ripgrep | **9–10 min** | 10–11 min (schedule) | 5 | matrix 18 configs on ubuntu+cross |
| ruff | 11–16 min | 11 min (main) | 31 | nextest + `determine_changes` |
| typst | 15–17 min | 4–5 min* | 7 | *push to branch; matrix 3 platforms |
| tokio | 20–28 min (fast red: 2 min) | — | 46 | `basics` gate |
| bevy | ~24 min | 21–29 min (queue) | 18 | 595 open PRs, merge queue |
| **webfang** | **~33 min** | — (no queue) | 19 | required includes mutants + AI 19 min |

Conclusion: **nobody** puts mutants or a 19-min integration as required on a PR.
Everyone separates the *fast PR gate* from *heavy verification in queue/main/schedule*.

## 2. Universal patterns (all 7 agree — copy without hesitation)

1. **A single aggregator check as required.** wasmtime (`ci-status`, `if: always()`),
   ruff (`required-checks-passed`, accepts `skipped`), rust-analyzer (`conclusion`,
   with an explicit comment about the `skipped == success` trap), typst (`tests`,
   citing the branch protection trick). We already have `CI Gate` — good, but it must
   aggregate a SMALL SET, not everything.
2. **Fuzz = compiles only on PR.** ripgrep (`cargo check` in `fuzz/`), typst
   (`cargo fuzz build --dev`), tokio (`check-fuzzing`). Nobody runs fuzz on a PR.
3. **Bounded Miri, never the whole suite on a PR.** typst: ONE target
   (`cargo miri test -p typst-library test_miri`). tokio: lib/test/doc splits.
   wasmtime: opt-in by `prtest:miri` token.
4. **`cancel-in-progress` on PRs** (all of them). We already have it.
5. **Pinned action SHAs + `rust-cache`/`Swatinem` with `save-if: main-only`**
   (ruff). Cheap and stabilizing.
6. **Expensive checks report, they do not block.** ruff: fuzz, ecosystem, benches,
   codspeed out of required. bevy: miri and validations out of the critical path.

## 3. Techniques per repo (what → evidence → adaptation → effort)

### 3.1 wasmtime — total tiering (PRIORITY 1 for us)

- **FAST_MATRIX vs FULL_MATRIX** (`ci/build-test-matrix.js`, 440 lines):
  PR runs **1 job** (Linux x86_64); the merge queue and main run the full
  matrix. Measured result: PR 8–9 min, queue 25–34 min.
- **Opt-in by commit token**: `prtest:full`, `prtest:miri`, `prtest:capi`,
  `prtest:debug`, `prtest:platform-checks` in the commit message trigger
  heavy suites on demand. Auto-trigger by path (`fuzz/` → nightly,
  `Cargo.lock` → audit, `gc` → gc-zeal, `cranelift/codegen/src/isa/<x>` →
  that ISA's config).
- **Sharding by crates**: 3 generic buckets (round-robin over
  `cargo metadata`) + singleton buckets for the slow crates + sub-buckets
  by `--test` for the dominant crate. Duplicated dependency build, parallelized
  execution (pays off on slow targets).
- **Hourly triage** (`triage.yml`): path labeler + `subscribe-to-label`
  (@-mentions subscribers) + `label-messager` (canned replies).
- **`file-issue-on-error`**: a heavy build (dispatch/schedule) that fails
  **automatically opens an issue**. Failures in the heavy tier are never
  lost in silence.
- **Native merge queue** (`merge_group` in `main.yml`; queues visible as
  `gh-readonly-queue/...` in `gh run list`).
- Adaptation: own `scripts/build-test-matrix.{js,py}` with
  `FAST = [unit+clippy+fmt]` / `FULL = everything`; tokens `citest:full|miri|ai`;
  move coverage+AI-integration+mutants-weekly to the queue/main/schedule tier.
  Effort: **M**.

### 3.2 ruff — thousands of tests without pain (PRIORITY 1)

- **`determine_changes`** (`ci.yaml`): real merge-base (`git fetch --unshallow`
  + `git diff MERGE_BASE...HEAD -- <pathspecs>`) with per-area flags
  (parser/linter/formatter/code/fuzz/ty/playground/benchmarks/release). Each
  heavy job carries `if: <flag> || main`. Docs-only compiles nothing.
- **Escape hatch by label**: `no-test` / `no-build` on the PR skips jobs.
  (Discipline: only the maintainer sets it.)
- **nextest profile `ci`** (`.config/nextest.toml`): `slow-timeout 1s`
  (flags slow ones), `terminate-after 60s` (guillotines deadlocks),
  `fail-fast = false`, final summary of slow ones. Officially recommended in its
  CONTRIBUTING (`cargo nextest run`).
- **insta anti-sprawl**: `cargo insta test --unreferenced reject` — an orphan
  snapshot **fails CI**. Our gitignored `.snap.new` + manual review is
  compatible; we lack the automatic reject.
- **Dogfooding as a tier**: `ty` checks itself against its own repo in
  CI. Our equivalent: `webfang` scraping its own versioned fixtures.
- **codspeed**: benches with a regression gate (instrumented + walltime).
  Our `benches.yml` exists — evaluate whether it gates or only reports.
- **Large runners + mold + `line-tables-only`**: `depot-ubuntu-22.04-16`,
  `setup-mold`, `CARGO_PROFILE_DEV_DEBUG=line-tables-only` (backtraces without
  the cost of full debuginfo). We already have `DEBUG=0`+no-incremental in
  mutants; generalize.
- **Minimum required**: 11 jobs (`fmt, clippy, hawk, test-linux, test-wasm,
  msrv, shellcheck, scripts, prek, docs, python-package`). Everything else
  reports.
- Adaptation: `determine_changes` with areas `core/ai/mcp/cli/docs/ci`;
  own nextest `ci` profile; `--unreferenced reject`; required = aggregator
  over ~6 jobs. Effort: **S–M**.

### 3.3 tokio — feature matrix without explosion (PRIORITY 2)

- **`basics` gate**: dumb aggregator (`needs: [clippy, fmt, docs, minrust]`,
  `run: exit 0`) that **everything** hangs off (`needs: basics`). Red in 2
  measured minutes (run 34258764834). The cheapest possible fail-fast.
- **loom outside the PR CI**: separate `loom.yml` workflow; on a PR it runs only
  with the `R-loom-blocking` label, always on push to master. Generalizable
  pattern: *heavy tool = separate workflow + opt-in label + always on main*.
- **MSRV as a policy, not as a matrix**: documented (≥6 months, bumps only on
  minor) + one `minrust` job. No combinatorics.
- Copyable extras: `check-spelling` (typos), `semver-checks`, `check-readme`
  (synced docs), documented LTS.
- Adaptation: our `feature-matrix` (265 s) becomes: required `basics` job
  + full matrix only in queue/main; loom-style for our concurrency tests if they
  appear. Effort: **S**.

### 3.4 typst — lean snapshot culture (PRIORITY 2)

- **One required**: aggregator `tests` job (trick
  `orgs/community/discussions/4324`). A single name in branch protection.
- **`testit` harness**: `suite/` (inputs `.typ`, N tests per file via
  `--- name attrs ---`) + `ref/` (references) + **`hashed references`**
  (32 bytes per test, anti-bloat) + `store/` (live output, gitignored, with
  auto-regen from the commit that pinned the hash). HTML report with diffs on
  failure. Flags `--stages eval,paged,html` (speed in dev) and `--update`
  (explicit bless).
- **Policy "assertions > reference images"** + a 20 KiB cap per reference image
  (the `large` attribute as a shameful exception).
- **`checks` anti-masking**: clippy per crate with
  `--no-default-features` (feature unification hides errors) +
  `git diff --exit-code` (fresh generated docs).
- **PR culture** (CONTRIBUTING): small PRs, discuss design BEFORE in an
  issue/Discord, green CI before the first review, **close stalled PRs**
  waiting on the contributor, explicit no-AI policy.
- Adaptation: think of our behavioral corpus as a `suite/` with stages
  (fast: parse-only vs full: network-mock); size cap on snapshots;
  `git diff --exit-code` for generated files; policy for closing our own
  old PRs. Effort: **M** (harness) / **S** (policies).

### 3.5 ripgrep — single-maintainer efficiency (PRIORITY 1 in spirit)

- **Matrix 18 configs, 9–10 min**: everything on `ubuntu-latest` with a
  **pinned** `cross` binary (compiling cross is expensive; the binary is not).
  Only occasional native mac/win. Explicit cost comments ("this almost doubles
  the runtime, skipped on cross").
- **`rgtest!(r<issue>)`**: every regression test carries the issue number in its
  name (`tests/regression.rs`, 1744 lines). Total traceability with no extra
  tooling.
- **5 jobs, 271 lines**: test, wasm, rustfmt, docs, fuzz (build-only).
  Minimal permissions documented in the YAML itself.
- Adaptation: pin `cross` if we expand targets; name regression tests
  `r<issue>`; prune `ci.yml` with the scissors of "what confidence does this job
  add per minute?". Effort: **S**.

### 3.6 bevy — PR scale (selective copy, PRIORITY 3)

- **Native merge queue** + a **60-label** taxonomy
  (`C-*` category, `A-*` area, `P-*` priority, `O-*` OS, `S-*` status,
  `D-*` difficulty, `M-*`, `X-Needs-SME`). `S-Ready-For-Final-Review` =
  merge signal; `S-Needs-Triage` = inbox.
- **Bots per label** (`action-on-PR-labeled.yml`, via `pull_request_target`
  without checking out untrusted code): require migration-guide / release-note
  depending on the label. Copyable pattern for "this PR touches X → require Y".
- **CI-as-code**: `tools/ci` crate (`cargo run -p ci -- lints`); caches
  pre-warmed by schedule (`update-caches.yml`); own composite actions.
- **DO NOT copy**: the process volume (595 PRs, areas, SMEs) is for an org.
  From bevy we keep: merge queue + 6–8 labels + a checklist bot.
  Effort: **S–M**.

### 3.7 rust-analyzer — QA and triage (PRIORITY 2–3)

- **`conclusion` with the documented trap**: `skipped == success` for GitHub,
  hence `if: !cancelled()` + manual check with `jq`, + an uppercase warning to
  add every new job to `needs`. Copy the comment, not just the code.
- **`cargo codegen --check`**: fresh generated files as a job (our equivalent:
  `git diff --exit-code` after regenerating).
- **`analysis-stats`**: performance telemetry IN CI (analyzer timings).
  Our equivalent: budget assertion on critical benches.
- **`cancel-if-matrix-failed`**: cancels the matrix if an axis fails.
- **Feature freeze as a backlog tool** (CONTRIBUTING announces a freeze on
  assists). Our `FREEZE_FEATURES` already exists (Gate 0) — align the narrative.
- Green PRs in **7–8 min**, red ones in **2 min**. Effort: **S**.

## 4. What we do NOT copy

- bevy's process scale (areas, SMEs, 60 labels, layered review).
- Exotic matrices (wasmtime/tokio QEMU/SDE/wasm-tier3) until we have users
  asking for them.
- `ty`/`mdtest`/ruff's ecosystem-infra (conformance over an external corpus) —
  a future idea, not phase 1.
- Daily fuzz / OSS-Fuzz on a PR (everyone removes it from the merge path).

## 5. Adaptation backlog (phases)

**Phase 0 — S quick wins (one afternoon, no redesign):**
1. nextest `ci` profile (`slow-timeout`, `terminate-after`, slow summary).
2. `cargo insta test --unreferenced reject` in the snapshot job.
3. `save-if: main` in caches + generalized `mold`/`line-tables-only`.
4. `r<issue>` naming for new regression tests.
5. Anti-trap `skipped==success` comment in our `CI Gate` (ra style).

**Phase 1 — tiers (the big cut, M):**
6. `determine_changes` by area (`core/ai/mcp/cli/docs/ci`) + `no-test` label.
7. FAST on PR (fmt, clippy, check, fast units); FULL (coverage, AI-integration,
   feature-matrix, mutants-weekly, miri, sanitizers, benches-gate) in
   merge queue / main / schedule.
8. Remove `cargo-mutants` and `Tests (AI integration)` from required; required =
   aggregator over ~6 fast jobs.
9. Tokens `citest:full|miri|ai` for targeted opt-in.

**Phase 2 — merge queue (S–M, unlocks `strict` painlessly):**
10. Enable native merge queue + `merge_group` in workflows; remove the
    need for manual "update branch" (the `merge-when-green.sh` script
    still works as an active wait).
11. `file-issue-on-error` for the schedule/dispatch tiers.

**Phase 3 — test pyramid (M, the pending audit):**
12. Classify the 2744 tests (unit / integration-mock / behavioral-corpus /
    slow-redundant) and set a budget per level.
13. File-driven corpus with stages (`testit` idea), size cap on snapshots,
    assertions-first policy.
14. `cargo codegen --check` / `git diff --exit-code` for generated files.

**Phase 4 — PR/issue culture (continuous, S):**
15. 6–8 labels (`needs-triage`, `ready`, areas…) + a per-label checklist bot
    (bevy style, via `pull_request_target`).
16. CONTRIBUTING guide: small PRs, design-first, green-CI-before-review,
    closing stalled PRs.
17. Light scheduled triage (path labeler).

## 6. Reproduce this study

```bash
# Workflow inventory
gh api repos/<owner>/<repo>/contents/.github/workflows --jq '.[].name'
# Raw CI
gh api repos/<owner>/<repo>/contents/.github/workflows/<file> --jq '.content' | base64 -d
# Wall-clocks (retry: the network to api.github.com is intermittent)
gh run list -R <owner>/<repo> --workflow=<file> --limit 8 \
  --json databaseId,event,headBranch,createdAt,updatedAt,conclusion
# Labels / recent merged / dir listing
gh label list -R <owner>/<repo> --limit 60 --json name
gh pr list -R <owner>/<repo> --state merged --limit 5 --json number,title,mergedAt
gh api repos/<owner>/<repo>/contents/<dir> --jq '.[].name'
# Loose files without the API (reliable channel when gh fails):
curl -s "https://raw.githubusercontent.com/<owner>/<repo>/<branch>/<path>"
```