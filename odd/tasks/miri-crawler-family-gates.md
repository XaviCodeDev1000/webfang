# Objective: recover Miri coverage from the crawler-family module-level `not(miri)` gates of #3027.
# Branch: refactor/miri-crawler-gates from origin/main cd15e330. Delivery: single PR, one family per PR, own dispatch baseline. Test runner: scoped `cargo miri test --lib -- <family>::` plus `bash scripts/check_miri_lane_invariants.sh` and `bash scripts/test_miri_lane_invariants.sh`.

## Specs

S1 (verbatim, user): "El slice natural siguiente sería la familia crawler, con su baseline y su PR separado. No lo arranco sin tu ok."

S2 (verbatim, user): "hay que consultar la documentacion oficial antes si no esta en engram para aplicar". Official Miri README consulted: `cfg(miri)` is set for interpreted code; `#[cfg_attr(miri, ignore)]` is the documented way to skip a test; `cargo miri test <filter>` forwards filters to the harness; there is no exhaustive list of unsupported operations (Miri reports them at runtime); Miri has no shims for platform-specific APIs or FFI (networking included). Consequence: gates with a written interpreter-limit reason are legitimate, and every ungated removal must be arbitrated by a scoped run.

S3 (verbatim, issue #3027): "Work one family per PR, sequentially, with its own before/after `workflow_dispatch` baseline". Crawler family scope verbatim: "**Crawler family** — `application/crawler/engine.rs` (9), `crawl_task.rs` (5), `rate_limiter.rs` (3), `rate_limiter/jitter.rs` (2), `export_factory.rs` (2), plus the single-gate files (`url_validator.rs`, `resource_downloader.rs`, `link_extractor.rs`, `sitemap_parser/parse.rs`, `readability.rs`, `dom_inspector_tests.rs`, `extractor/mod.rs`, `author_extractor/mod.rs`)."

S4 (verbatim, issue #3027 acceptance): "- `Miri (core)` runs more tests after each family PR than its own baseline, with 0 UB and 0 failures." "- Every removed module gate is replaced by per-test gates carrying a written reason." "- The static guard and its RED-control harness stay green; no declared surface becomes vacuous."

S5 (verbatim, issue #3027 caveat): "Some module gates are correct and must stay: `domain/html_cleaner.rs`, `infrastructure/converter/htmd_converter.rs` and `application/fragment_markdown.rs` are declared UB surfaces (`clean_html` → `lol_html::rewrite_str`, servo_arc Tree-Borrows UB, servo/stylo#210) where a text marker cannot separate the tests that reach it." These three files are OUT of scope and must not be touched.

S6 (verbatim, issue #3027 closing note): "any further recovery should be tracked as its own family/PR rather than assumed complete by this closure". #3027 is CLOSED (COMPLETED); the PR needs a new issue carrying `status:approved`, never `Closes part of #3027`.

S7 (verbatim, repo record `odd/tasks/miri-ub-lane.md`): "Un test con `#[cfg_attr(miri, ignore = \"…\")]` queda `ignored` bajo Miri: la corrida iba a reportar '2 passed, 1 ignored' **sin ejecutar el brazo de error**, o sea, el arbitraje era lógicamente vacío — la evidencia de un falso verde. La secuencia correcta (sacar el gate, correr, y recién ahí decidir)". Every gate decision needs a scoped run with the gate removed.

S8 (baseline, verified): green `workflow_dispatch` 45663803492 on main — all six Miri jobs success; `Miri (core)` 1148 passed / 0 failed / 120 ignored vs earlier green 1067 / 0 / 100. The latest push to main (run 5693) has Miri jobs `skipped` by path filter and is not a baseline.

S9 (scope measurement, verified by explorer): actual tree has 26 in-scope module gates, not the issue's 27: `crawl_task.rs` has 4 gates (303/307/311/315), not 5. No `UB_SURFACE_ENTRIES` row currently names an in-scope file or marker.

## Tasks

T1 (S1-S3, S6, S9) — inline — done — worktree `refactor-miri-crawler-gates` bootstrapped, three isolation checks green; read-only exploration mapped the 26 gates; new issue with `status:approved` still pending before PR.
T2 (S2, S7) — inline — done — scoped form verified: `RUSTUP_TOOLCHAIN=nightly-2026-08-27` + the lane's five `MIRIFLAGS` + `cargo miri test --lib -- <filter> --test-threads=1` from the direnv-loaded worktree; each gate arbitrated by a RED run with the gate removed and a GREEN rerun after adding it.
T3 (S3, S4, S7) — inline — done — `application/crawler/engine.rs` nine module gates replaced by `#[cfg(test)]`; 24 per-test `cfg_attr(miri, ignore)` with written reasons (23 × `TLS_method` via `session_engine`, 1 × `sigaction` in the ctrl_c test), plus the pilot gate on `tests_pause`. Scoped Miri: 12 passed / 0 failed / 25 ignored; guard and RED-control harness both exit 0.
T4 (S3, S4, S7) — inline — done — 11 module gates removed from `crawl_task.rs` (4), `rate_limiter.rs` (3), `jitter.rs` (2) and `export_factory.rs` (2). Individual arbitration of 119 tests: 118 PASS, 1 wall-clock-driven full jitter sweep TIMEOUT at both 150s and 600s → one per-test gate with written reason. Scoped Miri over the three scopes: 117 passed / 0 failed / 2 ignored.
T5 (S3, S4, S7) — delegated direct worker — done — 9 single-gate files (`url_validator.rs`, `resource_downloader.rs`, `link_extractor.rs`, `sitemap_parser/parse.rs` + its `tests_xml.rs` child, `readability.rs`, `dom_inspector_tests.rs`, `author_extractor/mod.rs`, `src/extractor/mod.rs`); 31 per-test gates with written reasons (TLS_method, servo_arc Tree-Borrows UB, wall-clock budget). All 8 scoped Miri runs exit 0 with 0 failures and 0 UB; guard/harness/check/clippy/fmt all green. `adapters/extractor/mod.rs` was inspected and has no crawler-family module gate; left untouched.
T6 (S4, S5, S8) — inline — done locally; delivery opened — guard + RED-control harness green, `cargo check`/clippy/fmt/doc, `ci_fast_gate.sh` GREEN (48 PASS / 0 FAIL / 2 SKIP), four work-unit commits. Owner authorized issue, push and PR: issue #3059 (`status:approved`), branch pushed, PR #3060 opened with `Closes #3059` and a single `type:refactor` label. PR metadata gates green. Only the before/after `workflow_dispatch` Miri count comparison remains.

Route: inline for every task. No writer trigger (no independent parallel units of behavior; the changes share one guard contract and one arbitration loop). One read-only explorer was used for the map (mapping trigger), per the orchestrator rules. Delivery strategy `ask-on-risk`; forecast well under 400 authored changed lines; single PR, no chain.

## Log

L1 (user, verbatim): "El slice natural siguiente sería la familia crawler, con su baseline y su PR separado. No lo arranco sin tu ok."

L2 (user, verbatim): "hay que consultar la documentacion oficial antes si no esta en engram para aplicar"

L3 (user, verbatim): "OK"

L4: Official docs consulted before any proposal or edit, per L2. Context7 `/rust-lang/miri` README: cfg(miri), cfg_attr(miri, ignore), cargo miri test filters, no supported-operation list, FFI/platform APIs unimplemented. Repo records `odd/tasks/miri-preflight-gates.md` and `odd/tasks/miri-ub-lane.md` read in full; the latter's L7 fixes the arbitration order.

L5: Verified state before planning: issue #3027 CLOSED with a closing note requiring its own family/PR; dispatch 45663803492 green on all six Miri jobs; latest main push has Miri skipped; HEAD/worktree cd15e330; identity gate exit 0; codegraph and codedb both indexed at the worktree root.

L6: Explorer handoff: 26 in-scope module gates; no existing `UB_SURFACE_ENTRIES` row for these files; `engine.rs` gates wrap `#[path]` child files so hidden tests are not in engine.rs; `crawl_task.rs` has 4 gates not 5; `url_validator.rs` in scope is `infrastructure/crawler/url_validator.rs`; `dom_inspector_tests.rs` gates only the DefaultDomInspector module. Triage: obvious removals (`engine/tests_common.rs` with 0 tests), obvious per-test gate conversions (time/wiremock/spawn/FFI modules), and a runtime-arbitration set (jitter, rate_limiter support, crawl_task children, sitemap parse, readability, extractor).

L7: Risk: item 3 — `UB_SURFACE_ENTRIES` is an internal contract consumed by the guard and its harness; a wrong marker can make the guard vacuous or falsely red. Mitigation: guard + harness + scoped Miri + full dispatch comparison.

L8: Next step: capture the pinned scoped-Miri command form from `ci.yml`, check for competing local builds, then start T2 with a RED arbitration run.

L9: T2/T3 evidence. No competing local build was running (only MCP stdio processes; load ≈0.9). Pilot: `tests_pause` needed `tests_common` ungated too (otherwise E0432), then `autoscale_pause_is_judged_by_its_own_watchdog` aborted on `TLS_method`; with its per-test gate the module reported 2 passed / 1 ignored. Full engine arbitration ran 37 tests individually: 13 PASS, 24 FAIL (23 × `TLS_method`, 1 × `sigaction` in `the_default_source_is_a_cancellable_ctrl_c_wait`). After the 24 per-test gates, the whole `application::crawler::engine::` scope reports 12 passed / 0 failed / 25 ignored. Counting caveat: the baseline dispatch's `Miri (core)` includes `application::`, but the infra modules of this family live in other jobs; the final comparison will use all six jobs, not only core.

L10: Work-unit 1 = this document + the engine gates and their 24 per-test reasons + the T2/T3 evidence. Risk item 3 (guard contract) stays contained: no `UB_SURFACE_ENTRIES` row was changed because no declared surface lives in this family; guard and harness prove the contract intact.

L11: Work-unit 2 committed as `d51a4915` "refactor(crawler): run engine tests under Miri with per-test gates" (10 files, 164 insertions, 9 deletions); identity proxy OK, review-gate reported no review authority governing the candidate.

L12: T4 evidence. `crawl_task` 33/33 PASS with no new gates; `rate_limiter` 48 PASS + 1 TIMEOUT (`the_full_measurement_covers_every_cell_and_reproduces` at 150s and again at 600s) → per-test gate for the wall-clock sweep; `export_factory` 37/37 PASS. Combined scope: 117 passed / 0 failed / 2 ignored in 694s, inside the lane's 240-minute budget. Note: the two module gates in `export_factory.rs` carried the reason "tokio_util CancellationToken + time-sensitive drains", but runtime proved every test passes under Miri; reasons were moved only where runtime required them.

L13: Work-unit 2 committed as `f6171188` "refactor(crawler): run crawl-task and limiter tests under Miri" (6 files, 21 insertions, 13 deletions); identity proxy OK, review-gate clean.

L14: T5 delegated to `gentle-ai-worker` (context backstop). It first returned `interaction_required` for a ninth edit surface: `crates/webfang_core/src/infrastructure/crawler/sitemap_parser/tests_xml.rs`, the `#[path]` child of `parse.rs` holding six tests that abort on `TLS_method`. The parent adjudicated and replaced the `## Allowed edit surfaces` block (necessary, in-scope, no competing writer) rather than asking the owner. Final evidence: 81 ungated tests arbitrated individually; 46 PASS, 17 `TLS_method` aborts, 13 servo_arc Tree-Borrows UB, 5 timeouts (4 pass at 600s and stayed ungated; the 600-div `builder_methods` test needed a written budget gate). Eight scoped Miri runs all exit 0; sitemap re-verified by the parent (`0 passed / 6 ignored`, 175s).

L15: Delegated-verification gate: `gentle-ai review assess` → `risk: medium`, `review_due: false` (`under_budget`), so writer self-verification stands and no independent verifier or native review ceremony is required. Parent spot checks: guard exit 0 and sitemap scoped Miri exit 0 with 0 failures. Diff reviewed: 9 files, 132 insertions / 8 deletions, all attribute-only changes plus per-test gates; no production logic or declared UB surface touched.

L16: Work-unit 3 committed as `a5a40bdd`, then amended to `b3981e02` (unpushed, local history only) to include `tools/intra-crate-lint/tests/fixtures/real_tree_sites.txt` in the SAME commit as the source change, per the golden-set contract in `tools/intra-crate-lint/tests/real_tree.rs`. The first `ci_fast_gate.sh` run failed only that step: the golden encodes line numbers and the new `cfg_attr` lines shifted 31 entries. `bash scripts/refresh_real_tree_golden.sh` regenerated it; the targeted intra-crate lint test then passed and the full gate is GREEN (48 PASS / 0 FAIL / 2 SKIP).

L17: Final local evidence: guard exit 0; harness exit 0 with all RED controls; `cargo check`, `cargo clippy --all-targets --all-features -- -D warnings -W clippy::cognitive_complexity -W clippy::too_many_lines`, `cargo fmt --all -- --check` and `env RUSTDOCFLAGS=-D warnings cargo doc --workspace --all-features --no-deps` all exit 0. Scoped Miri results: engine 12 passed / 25 ignored, T4 117 passed / 2 ignored, T5 eight scopes all 0 failed (url_validator 0/11, resource_downloader 5/8, link_extractor 15/16, sitemap 0/6, readability 3/0, dom_inspector 2/14, author_extractor 21/0, extractor 45/20). Total recovered tests vs baseline: the 26 module gates are gone from this family and their pure tests now execute under Miri.

L18: Acceptance criterion "Miri (core) runs more tests after each family PR than its own baseline, with 0 UB and 0 failures" is NOT yet verifiable locally: it requires a `workflow_dispatch` before/after pair on CI. This family spans five Miri jobs (core, domain, infra-fast, infra-slow crawler and infra-slow transport/content), so the comparison must use all six job counts, not core alone. The last known green dispatch is 45663803492 (1067→1148 core passed in the earlier slice).

L19: Owner authorized "crea la issue con label approved y pushea & ABRE LA PR". Native review was mandatory: base-diff assess returned `high_risk`/`review_due: true` on a `process_boundary` signal from engine.rs; START returned the typed consent/v3 envelope, relayed losslessly, and the owner selected "Review this change" (`granted`). Binaries ran unchanged: lineage `review-cb15e5d4cd1f1f10`, lenses `review-risk` + `review-reliability`, both approved with zero findings, acknowledged via `gentle-ai.review-acknowledged/v1` (`authority: burned`). Then: issue #3059 created with `status:approved`, `git push -u origin refactor/miri-crawler-gates`, PR #3060 opened (`Closes #3059`, one `type:refactor` label). Run 45743797103: `Validate PR metadata` success, `breaking-surface` success, `Auto-label harness (offline)` success. The PR's own Miri jobs are `skipping` by design (they only run on schedule/workflow_dispatch), so the acceptance count still needs an explicit dispatch.

L20: Work-unit 4 committed as `796bd753` "docs(ci): close crawler Miri gate recovery record" before delivery. Scratch under /tmp/opencode (T5 arbitration and Miri logs) was removed after verification.

L21: CI fix after PR #3060: `Code quality (machete + duplication ratchet)` failed at 8231 duplicated lines vs baseline 8219; `CI Gate` failed only as the rollup of that job. jscpd clone diff against `cd15e330` isolated two new intra-file clones in `url_validator.rs`, created by the repeated four-line `cfg_attr` blocks between near-identical `filter_invalid_patterns` tests. The attribute was compacted to one line and the near-identical arrange was replaced by a shared `filter_invalid_patterns_for` helper. Local measurement is now 8185 (baseline 8219, zero new clones); `check_duplication.sh` exit 0. `cargo fmt --check`, `cargo check`, strict clippy, guard, harness and scoped Miri (`url_validator`: 0 passed / 11 ignored, unchanged) all exit 0. Gate reasons remain written and specific.
