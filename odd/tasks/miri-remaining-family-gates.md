# Miri remaining family gates — ODD feature record
Branch: `refactor/miri-remaining-family` from `main` after PR #3060 squash-merge. Delivery: sequential PR, one slice per family; this document owns the Remaining slice only. Runner: scoped `cargo miri test --lib` (nightly-2026-08-27) plus `scripts/check_miri_lane_invariants.sh`, its harness, `cargo check`, strict clippy, fmt, doc and `ci_fast_gate.sh`.

## Specs

S1. Authorized scope, verbatim: "vamos a implementar el plan en un nuevo worktree". This worktree implements the Remaining family of the plan; MCP and sitemap families stay out of scope here.

S2. Prerequisite, verbatim: "primero mergeamos 3060". PR #3060 was squash-merged to `main` as `e7fd367a` and the queue result was `merged`; this branch is based on that `main`.

S3. Slice definition, verbatim: "Slice 1 — Remaining (sin cambios de CI): syntax_highlight.rs (1 gate / 9 tests, sin FFI ni UB → arbitrar primero) y html_to_markdown.rs (1 / 11, probable lol_html). wikilinks.rs ya corre bajo Miri: úsalo como control. Se conservan html_cleaner.rs, htmd_converter.rs y fragment_markdown.rs: son las tres superficies UB declaradas, con sus markers en UB_SURFACE_ENTRIES. Job: infra-slow: content."

S4. Method, verbatim: "Protocolo común: rama desde main después de mergear #3060, issue propia con status:approved, baseline dispatch antes/después comparando los seis jobs, gates por test con motivo escrito (siempre correr Miri sin el gate antes de decidir), guard + harness + clippy + fmt + fast gate en verde, y un PR por familia."

S5. Scope boundary, verbatim: "antes vamos a dejar planeado la implementacion  Las familias MCP, sitemap y remaining siguen para sus propios PRs." Assumption (not a user requirement): each family gets its own worktree and PR because this repository has no stacked PRs; MCP and sitemap are not touched here.

S6. Decision left open by the plan, verbatim: "Quedan tres decisiones: lane MCP, lane integration tests, y si html_to_markdown.rs necesita una fila nueva en UB_SURFACE_ENTRIES (lo decide el RED)." The third decision is in scope for this slice and must be settled by a RED run, not by assumption.

S7. Applicable checks: scoped Miri for `infrastructure::converter::syntax_highlight::tests::` and `infrastructure::converter::html_to_markdown::tests::` with the same nightly and flags as the Miri jobs; `bash scripts/check_miri_lane_invariants.sh`; `bash scripts/test_miri_lane_invariants.sh`; the four workspace cargo gates; duplication check; `bash scripts/ci_fast_gate.sh`. A new UB surface found by RED must be declared in `UB_SURFACE_ENTRIES`, never silenced.

## Tasks

T1 (S1,S3,S4,S7) — Arbrate `syntax_highlight.rs`: remove the single module gate, run scoped Miri without it, record RED/GREEN, keep tests ungated if all pass or add per-test gates with written reasons if any fails. Route: inline. Evidence: commit + scoped Miri counts.

T2 (S1,S3,S4,S6,S7) — Arbitrate `html_to_markdown.rs`: remove the single module gate, run scoped Miri without it, add a `UB_SURFACE_ENTRIES` row only if RED proves a declared-surface hit, and gate individual tests with written reasons. Keep `wikilinks.rs` untouched as the control. Route: inline. Evidence: commit + RED output + final counts.

T3 (S4,S7) — Run guard, harness, `cargo check`, strict clippy, fmt check, doc, duplication check and fast gate; re-read the diff; record every failed/skipped check. Route: inline. Evidence: command results in Log.

T4 (S2,S5) — Close: commit the slice, open the issue with `status:approved` and its own PR; do not implement MCP or sitemap here. Route: inline after T3. Evidence: issue and PR references in Log.

## Log

L1: "primero mergeamos 3060" and "vamos a implementar el plan en un nuevo worktree". Full planning context from the same turn: "antes vamos a dejar planeado la implementacion  Las familias MCP, sitemap y remaining siguen para sus propios PRs." and "Slice 1 — Remaining (sin cambios de CI): syntax_highlight.rs (1 gate / 9 tests, sin FFI ni UB → arbitrar primero) y html_to_markdown.rs (1 / 11, probable lol_html). wikilinks.rs ya corre bajo Miri: úsalo como control. Se conservan html_cleaner.rs, htmd_converter.rs y fragment_markdown.rs: son las tres superficies UB declaradas, con sus markers en UB_SURFACE_ENTRIES. Job: infra-slow: content."
L2: PR #3060 merged by the queue with method SQUASH; `main` fast-forwarded to `e7fd367a`; old worktree/branch/target removed; new worktree bootstrapped at `/home/xavi/Projects/Rust/webfang-worktrees/refactor-miri-remaining-family`, branch `refactor/miri-remaining-family`, CodeGraph and CodeDB indexed.
L3: Verified inventory before the first edit: `syntax_highlight.rs` has one module gate at line 78 over 9 tests; `html_to_markdown.rs` has one at line 286 over 11 tests; `UB_SURFACE_ENTRIES` declares `html_cleaner.rs~clean_html`, `htmd_converter.rs~HtmdConverter.convert|converter.convert` and `fragment_markdown.rs~with_header`, but not `html_to_markdown.rs`.
