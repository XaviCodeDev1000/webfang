# Miri MCP + integration lanes — ODD feature record
Branch: `refactor/miri-mcp-integration` from `main` at `d9097fce` (after PR #3065 squash-merge). Delivery: one PR for the final blocked block; code slices land before the CI lane slice so each can be reviewed. Runner: scoped `cargo miri test --lib` for MCP, non-`--lib` `cargo miri test -p webfang_core --test cleaning_pipelines_regression` for integration, plus guard/harness and the four workspace cargo gates.

## Specs

S1. Authorization, verbatim: "adelante tienes mi aprobacion" after the proposal "Cuando me digas, lo fusiono y pasamos al último bloque — MCP e integration — que sigue esperando tu decisión sobre el filtro mcp_server:: y una invocación Miri que no sea --lib." This grants the named CI lane change; PR #3065 was merged as `d9097fce` first.

S2. Blocked slice definition, verbatim: "Slice 3 — MCP + regression (bloqueado por decisión tuya): scraping.rs (8 gates / 33 tests, confirmados en líneas 67–82) y cleaning_pipelines_regression.rs (5 gates / ~42 tests). Requiere decidir si abrimos un filtro mcp_server::/selección -p webfang_mcp y una invocación Miri de integration tests."

S3. Method, verbatim: "Protocolo común: rama desde main después de mergear #3060, issue propia con status:approved, baseline dispatch antes/después comparando los seis jobs, gates por test con motivo escrito (siempre correr Miri sin el gate antes de decidir), guard + harness + clippy + fmt + fast gate en verde, y un PR por familia."

S4. Planning context, verbatim: "antes vamos a dejar planeado la implementacion  Las familias MCP, sitemap y remaining siguen para sus propios PRs." The remaining/ and sitemap/ slices are already merged; this document closes only the MCP + integration block.

S5. Remaining decision from the plan, verbatim: "Quedan tres decisiones: lane MCP, lane integration tests, y si html_to_markdown.rs necesita una fila nueva en UB_SURFACE_ENTRIES (lo decide el RED)." The third was resolved in the converter slice by RED; the first two are in scope here.

S6. Verified inventory: `scraping.rs` has eight module gates over eight child modules (33 tests; 16 pure, 17 primitive-reaching; eight existing per-test ignores, nine missing). `cleaning_pipelines_regression.rs` has five module gates over five modules (42 tests; zero per-test Miri gates). Test paths for MCP resolve as `mcp_server::handlers::scraping::<child>::<test>`.

S7. CI facts to preserve: the `miri` job matrix group `core` filter currently contains `application::` and must keep it; `notify.needs` lists `miri` but not `miri-infra-fast`; `MIRI_JOBS=(miri miri-infra-fast miri-infra-slow)` is hard-coded in the guard and its harness, so prefer adding a step to an existing Miri job over creating a new job. No `--skip` may be introduced.

S8. Checks: RED run before each gate decision; every new `cfg_attr(miri, ignore)` carries a written reason; declared UB surfaces stay attributed; guard, harness, `cargo check`, strict clippy, fmt check, doc, duplication ratchet and `ci_fast_gate.sh` green. CI lane change is Risk item 5 (delivery/environment) and gets its own commit.

## Tasks

T1 (S1,S2,S3,S6,S8) — MCP code-only: remove the eight module gates in `scraping.rs`, RED-run `mcp_server::handlers::scraping::`, add written per-test gates to FFI/EnvGuard-reaching tests, keep the eight existing ignores and let pure tests run. Route: inline. Evidence: commit + RED/GREEN counts.

T2 (S2,S3,S6,S8) — Integration code-only: remove the five module gates in `cleaning_pipelines_regression.rs`, RED-run the `--test cleaning_pipelines_regression` target, gate tests that reach `clean_html`/lol_html UB, `CpuBridge`/rayon spawn or other interpreter limits with written reasons, and declare any new UB surface in `UB_SURFACE_ENTRIES`. Route: inline. Evidence: commit + RED/GREEN counts.

T3 (S1,S5,S7,S8) — CI lane slice: add `mcp_server::` to the `miri` core filter and add a non-`--lib` Miri step for `cleaning_pipelines_regression` inside an existing Miri job without touching `notify.needs` or `MIRI_JOBS`. Route: inline, separate commit. Risk: item 5. Evidence: commit + guard/harness + workflow readback.

T4 (S3,S8) — Full verification and diff readback; record failed/skipped checks. Route: inline. Evidence: command results in Log.

T5 (S3,S4) — Issue with `status:approved` and one PR with `Closes #N`; no further families remain. Route: inline after T4. Evidence: issue and PR references in Log.

## Log

L1: "adelante tienes mi aprobacion". Prior turns, verbatim: "primero mergeamos 3060", "vamos a implementar el plan en un nuevo worktree", "ya esta mergeada". Blocked-slice text, verbatim: "Slice 3 — MCP + regression (bloqueado por decisión tuya): scraping.rs (8 gates / 33 tests, confirmados en líneas 67–82) y cleaning_pipelines_regression.rs (5 gates / ~42 tests). Requiere decidir si abrimos un filtro mcp_server::/selección -p webfang_mcp y una invocación Miri de integration tests."
L2: PR #3065 verified `MERGED` at `d9097fce`; `main` fast-forwarded; the sitemap worktree, branch and target were removed; this worktree was bootstrapped with `.envrc`, `.env`, identity manifest and CodeGraph/CodeDB indexes.
L3: Explorer handoff confirmed eight MCP module gates (ordinary directory children, not `#[path]` attributes), 33 tests, eight dead per-test ignores; five integration module gates over 42 tests; CI filter and guard coupling as recorded in S7. No source edits yet.
L4: Implementation evidence: T1 in `9028b729`, T2 in `77af2e4b`, duplication fix in `bd0f4e5a`, CI lane slice in `1013c354`. RED runs: MCP aborted with `TLS_method` FFI at batch_metrics; integration aborted with servo_arc Tree-Borrows UB in bridge and later with tokio `socketpair` in the embedding test. Final scoped Miri: MCP 16 passed / 17 ignored / 0 failed; integration 9 passed / 13 ignored / 0 failed. Two fully-UB integration modules keep a module-level gate with a written reason.
L5: Verification: guard and RED harness green; check, strict clippy, fmt, doc green; duplication 8208 vs baseline 8219 after varying the five SSRF gate reasons (identical reasons had created a new 13-line clone); fast gate 49 PASS / 0 FAIL / 2 SKIP. Independent verifier reported zero blockers, zero change-caused advisories and all S# met; its scratch directory was removed by the parent.
L6: Native RDD: assess escalated item 5 returned `high_risk`; the owner granted review via the consent envelope; lenses risk, readability and reliability reported severe findings; the refuter corroborated R3-002, which claimed the integration step was in the core matrix, but the workflow places it in `miri-infra-slow` with matrix `content` — a reviewer misread corrected by direct workflow readback. Remaining findings were inconclusive/unknown causality and the lifecycle ended with terminal `native_stop_required`: no review approval or receipt exists, and the findings are informational only.
L7: Delivery: issue #3067 with `status:approved`; PR #3068 `refactor(mcp): recover Miri coverage from MCP and integration gates`, body `Closes #3067`. All #3027 families are now delivered; PR #3068 has NOT been merged.
