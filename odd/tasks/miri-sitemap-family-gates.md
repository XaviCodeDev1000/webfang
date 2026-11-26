# Miri sitemap family gates — ODD feature record
Branch: `refactor/miri-sitemap-family` from `main` at `b0b03582` (after PR #3062 squash-merge). Delivery: sequential PR; this document owns the sitemap library slice only, not the integration-test or MCP slices. Runner: scoped `cargo miri test --lib` (nightly-2026-08-27) plus the Miri lane guard/harness and the four workspace cargo gates.

## Specs

S1. Authorization, verbatim: "vamos a implementar el plan en un nuevo worktree". Continuation after the owner reported, verbatim: "ya esta mergeada" — PR #3062 was merged as `b0b03582` and the sitemap slice now has its own worktree.

S2. Slice definition, verbatim: "Slice 2 — Sitemap lib: sitemap_parser.rs (7 gates / 24 tests, job infra-slow: crawler). Primero tests_resolve_url y tests_support puros, luego config/fetch/compression/waf con RED→GREEN. tests_xml.rs ya quedó sin gate en el slice crawler."

S3. Method, verbatim: "Protocolo común: rama desde main después de mergear #3060, issue propia con status:approved, baseline dispatch antes/después comparando los seis jobs, gates por test con motivo escrito (siempre correr Miri sin el gate antes de decidir), guard + harness + clippy + fmt + fast gate en verde, y un PR por familia."

S4. Scope boundary, verbatim: "antes vamos a dejar planeado la implementacion  Las familias MCP, sitemap y remaining siguen para sus propios PRs." Assumption (not a user requirement): this worktree covers `sitemap_parser.rs` lib tests only; `crates/webfang_core/tests/cleaning_pipelines_regression.rs` is an integration target excluded by `--lib` and needs its own lane decision, and `webfang_mcp` needs its own PR.

S5. Verified inventory: `sitemap_parser.rs` has seven module-level `not(miri)` attributes over six `#[path]` child modules plus the `use tests_support as test_support` line. Children: `tests_resolve_url.rs` (4 pure tests), `tests_config.rs` (11, `SitemapParser::new` → `build_client` → wreq/btls), `tests_fetch.rs` (3, wiremock + parser build, one non-Miri `#[ignore]` for real DNS), `tests_compression.rs` (3, wiremock + async-compression), `tests_waf.rs` (3, wiremock + tokio + `LogBuffer`), `tests_support.rs` (helpers only), `tests_xml.rs` (already ungated in the crawler slice).

S6. Checks: the Miri job that selects this scope is `infra-slow: crawler` with filter `-- infrastructure::crawler:: -- infrastructure::network::`. Every existing per-test attribute must be preserved with its reason; any new `cfg_attr(miri, ignore)` must name the concrete interpreter limit found by RED. Guard, harness, `cargo check`, strict clippy, fmt check, doc, duplication ratchet and `ci_fast_gate.sh` must be green.

## Tasks

T1 (S1,S2,S3,S5,S6) — Remove the seven module gates in `sitemap_parser.rs`, run scoped Miri without them, arbitrate each child: pure tests ungated, FFI/interpreter-limited tests gated with written reasons. Keep `tests_xml.rs` and the non-Miri network `#[ignore]` unchanged. Route: inline. Evidence: commit + RED output + final counts.

T2 (S3,S6) — Run guard, harness, check, strict clippy, fmt check, doc, duplication check and fast gate; re-read the diff and record failed/skipped checks. Route: inline. Evidence: command results in Log.

T3 (S3,S4) — Commit the slice, open its own issue with `status:approved` and its own PR with `Closes #N`; do not touch MCP or the integration regression file here. Route: inline after T2. Evidence: issue and PR references in Log.

## Log

L1: "ya esta mergeada" after "vamos a implementar el plan en un nuevo worktree". Planning context, verbatim: "Slice 2 — Sitemap lib: sitemap_parser.rs (7 gates / 24 tests, job infra-slow: crawler). Primero tests_resolve_url y tests_support puros, luego config/fetch/compression/waf con RED→GREEN. tests_xml.rs ya quedó sin gate en el slice crawler."
L2: PR #3062 verified `MERGED` at `b0b03582`; `main` fast-forwarded; the previous worktree, branch and target were removed; this worktree was bootstrapped with `.envrc`, `.env`, identity manifest and CodeGraph/CodeDB indexes.
L3: Inventory read from the current tree before the first edit: seven `not(miri)` attributes at the `sitemap_parser.rs` test-module boundary; child file names and counts as recorded in S5.
