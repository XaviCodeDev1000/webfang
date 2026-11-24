# Objective: replace the module-level Miri gate in cli/preflight.rs with per-test gates, recovering pure tests in Miri (core).
# Branch: refactor/miri-preflight-gates from origin/main f1774e09. Delivery: single PR, `Closes #2723`, type:refactor.
# Runner: `bash scripts/check_miri_lane_invariants.sh` + `bash scripts/test_miri_lane_invariants.sh` + scoped `cargo miri test --lib -- cli::preflight:: --test-threads=1`; final check via `workflow_dispatch` of ci.yml.

## Specs

S1 (verbatim, issue #2723, already satisfied by PR #2800 — precondition, not reimplemented): "Extraer los tests de estrategia JS (`#685`/`#787`) a **dos** archivos `#[path]` de ≤500 líneas cada uno, con el patrón que ya usa `application/extraction.rs` para `selector_serialization_tests.rs` y `cli/batch_flush.rs` para `batch_flush_tests.rs`."

S2 (verbatim, issue #2723, already satisfied by PR #2800 — precondition, not reimplemented): "Con eso `preflight.rs` baja ~570 líneas (bien por debajo del baseline) y los archivos nuevos entren bajo el límite."

S3 (verbatim, issue #2723): "Reemplazar el gate de módulo por **gates por test** sobre los que spawnan, actualizando `UB_SURFACE_ENTRIES` para apuntar al archivo hijo correcto."

S4 (verbatim, issue #2723): "Revertir el gate de módulo y verificar con una corrida manual `workflow_dispatch` de `ci.yml`: los ~60 tests deben volver a correr bajo `Miri (core)`."

S5 (verbatim, issue #2723): "Una corrida manual posterior debe mostrar `Miri (core)` con **más** tests corridos que la anterior, no menos: la cuenta sube si los tests puros vuelven al intérprete."

S6 (scope decision, from the accepted suggestion and the user's verbatim acceptance below): keep #2723 scoped to cli/preflight.rs only. The remaining 62 module-level Miri gates across 22 files go to a separate umbrella issue, by family (crawler, MCP, sitemap), each with its own dispatch baseline. Do not mix them in one PR — traceability of which gate was removed and why must be preserved.

## Tasks

T1 (S3) — inline — done in bf7d8f3a — added per-test `#[cfg_attr(miri, ignore)]` gates to the three spawners that lacked them.
T2 (S3) — inline — done in bf7d8f3a — repointed the two stale `UB_SURFACE_ENTRIES` rows from preflight.rs to four child-file entries with discriminating ERE markers; no nominal exemption was needed (markers match only gated spawners).
T3 (S3) — inline — done in bf7d8f3a — `bash -n`, guard and harness green; guard output no longer contains `preflight.rs is gated at the module level`, so it inspects the new entries. `cargo check`, clippy and fmt --check green.
T4 (S4) — inline — done in 8f6a24a2 — replaced the 8 `#[cfg(all(test, not(miri)))]` mod declarations with `#[cfg(test)]`; zero remain in preflight.rs and the guard/harness stayed green.
T5 (S5) — inline — done in 8f6a24a2 — isolated scoped Miri (`RUSTUP_TOOLCHAIN=nightly-2026-08-27`, the job's MIRIFLAGS, `cargo miri test --lib -- cli::preflight:: --test-threads=1`) reports 61 passed, 0 failed, 18 ignored, 2618 filtered out; CARGO_TARGET_DIR confirmed as the worktree's own.
T6 (S5) — inline — pending — `workflow_dispatch` ci.yml against the branch and compare Miri (core) test count with baseline run 43897369587 (1067 passed, 100 ignored). Blocked on explicit push authorization.
T7 (S6) — inline — pending — open the umbrella issue for project-wide Miri gate recovery.

## Log

L1 (user's original request, verbatim): "vamos a revisar en profundida esta issue #2723"

L2 (user, verbatim): "siempre consulta la documentacion oficial antes de proponer soluciones"

L3 (user, verbatim): "hazlo"

L4 (user, verbatim): "esto soluciona todo los tests del proyecto o unicamente lo que menciona la issue ?"

L5 (user, verbatim): "estoy deacuerdo, con tu sugerencia, vamos a hacerlo"

L6: Evidence gathered before this document. The issue's premise is stale: preflight.rs is 311 lines (not 3274) and its grandfathered baseline entry was removed by PR #2800 (commit 8aecddc2, part of #2584). S1/S2 are therefore already done. Current main has 8 module-level `#[cfg(all(test, not(miri)))]` gates in preflight.rs covering 53 tests; 15 already carry per-test gates, so 38 pure tests are excluded from Miri. The two `UB_SURFACE_ENTRIES` rows still point at preflight.rs with markers (`check_js_dependencies_with`, `run_version_probe_with_timeout`) that no longer exist there; `check_ub_attribution` calls `module_gated` and `continue`s, so the vacuity guard never runs — verified live: the guard prints `OK: ... gated at the module level` twice while inspecting nothing. Three spawners lack per-test gates: `tests_js_strategy.rs::hybrid_strategy_ok_without_chromium_feature`, `tests_js_strategy.rs::no_render_build_passes_preflight_with_warning`, `tests_obscura_binary.rs::hybrid_explicit_existing_path_ok_with_empty_path`.

L7: Official documentation consulted before proposing the plan, per L2. Miri README (https://github.com/rust-lang/miri/blob/master/README.md): `cfg(miri)` is set for interpreted code; `#[cfg_attr(miri, ignore)]` is the documented way to skip a test; `cargo miri test filter` and `cargo miri test -- test-suite-filter` forward filters to the harness; Miri has no list of unsupported operations and reports them at runtime. Miri CONTRIBUTING (https://github.com/rust-lang/miri/blob/master/CONTRIBUTING.md): no shims for platform-specific APIs, so Command::spawn gates are permanent. Rust std `Command::spawn` docs: spawn-time errors only; child outcome via ExitStatus. GitHub docs `workflow_dispatch` (https://docs.github.com/en/actions/reference/workflows-and-actions/events-that-trigger-workflows#workflow_dispatch): once a workflow has run at least once it can be dispatched against any branch or tag; ci.yml qualifies. Prior finding #2363: an ignored test cannot arbitrate its own gate, so arbitration requires a scoped run with the gate removed; `cargo miri test --lib` at workspace root unifies features, so `chromium` is ON in the lane.

L8: Scope measurement for L4/L5. Project-wide there are 62 `#[cfg(all(test, not(miri)))]` module gates across 22 files (engine.rs 9, mcp scraping.rs 8, preflight.rs 8, sitemap_parser.rs 7, etc.) plus 282 per-test `cfg_attr(miri, ignore)` gates in src. All three Miri jobs use `cargo miri test --lib`, so integration tests under crates/webfang_core/tests/ are not executed by Miri at all. Preflight is 8/62 (~13%) of the module gates. Some remaining gates are legitimate declared UB surfaces (html_cleaner.rs, htmd_converter.rs, fragment_markdown.rs); others are coarse legacy gates needing their own arbitration.

L9: Baseline for S5 — the most recent green workflow_dispatch of ci.yml is run 43897369587 (2026-11-16, headBranch fix/cross-platform-error-snapshots). Runs 43897375849, 43602674416 and 43307413411 failed and are not valid baselines. Capture the Miri (core) test count from 43897369587 before the comparison in T6.

L10: Risk: item 3 — `UB_SURFACE_ENTRIES` is an internal contract consumed by the guard and its harness; a wrong marker can make the guard vacuous again or falsely red. Mitigation: local guard + its dedicated harness + scoped Miri run + full dispatch count. T2 and T3 land in a separate work unit so the contract change is reviewable alone.

L11: Route: inline for every task. No writer trigger (no independent parallel units, no context backstop); no explorer needed (the map was already built before this document). Delivery strategy `ask-on-risk` with no chain yet: forecast well under 400 authored changed lines. Work units: T1-T3 as one commit (gate completeness + guard attribution), T4-T6 as one commit (module-gate removal + verification evidence), T7 is a GitHub operation (no commit).

L12: Worktree bootstrapped and verified: sibling `refactor-miri-preflight-gates`, its own 5-line .envrc, direnv allowed, identity manifest written, codegraph init + codedb reindex, three isolation checks OK, branch proxy MATCH. Seed was cold (key v1-fb19b930e792d6b7).

L13: Work unit 1 committed as bf7d8f3a "refactor(cli): gate preflight spawn tests individually, repoint UB surfaces" (4 files, 62 insertions, 2 deletions). Work unit 2 committed as 8f6a24a2 "refactor(cli): run preflight pure tests under Miri instead of excluding the module" (2 files, 17 insertions, 14 deletions). Both passed the identity gate and review-gate (no review authority governs either candidate).

L14: The first Miri invocation inherited CARGO_TARGET_DIR pointing at main/miri; it was re-run with the worktree's direnv env loaded and confirmed on refactor-miri-preflight-gates/miri. The isolated result is the one recorded in T5: 61 passed, 0 failed, 18 ignored. Baseline for comparison: run 43897369587, Miri (core) job 133404475522 — 1067 passed, 0 failed, 100 ignored.

L15: Independent verifier (gentle-ai-verify) found no blockers; S3 met, S6 met for code scope, S4/S5 unverified until T6 runs. It reproduced the vacuity fix: baseline f1774e09 with a broken marker exits 0 (module gate short-circuit), changed tree exits 1. Two advisories, both fixed: stale prose in check_miri_lane_invariants.sh still described preflight.rs as a 3274-line module-gated file, and the harness fixtures/RED control still modelled the old module gate — its RED passed because the new child files were absent, not because of the intended violation. Replaced the fixture with four child-file shapes and added eight RED controls (one ungated-spawner + one vacuity per declared entry); guard and harness both exit 0 with all eight controls reported.
