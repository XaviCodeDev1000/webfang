# Ola 3 — program plan for #2584: the [1000, 1499] tail (T4), parallel execution

Objective: reduce the 25 entries of the `[1000, 1499]` band to zero with four logical lanes in
separate worktrees. Split preparation is parallel; at most TWO implementation agents run at once
because the BAI provider 429s on 3+ concurrent high-thinking sessions (Engram #648). Delivery is
serialized because every split touches the same shared artifacts. Branch of this record:
`docs/file-size-ola3-plan` · worktree `~/Projects/Rust/webfang-worktrees/docs-file-size-ola3-plan`
· base `origin/main@152add03`. Plan record: `odd/tasks/file-size-reduction.md` (T4, governance:
2–3 files per PR). Runner: `cargo nextest` (the same before and after every measurement).

## Specs

**S1 — Band and source of truth.** Ola 3 is the `[1000, 1499]` band of
`scripts/check_file_size.sh`. The live count comes from `file_size_rust.grandfathered` in
`scripts/quality-baselines.json`, never from this prose:

    jq '[.file_size_rust.grandfathered[] | select(. >= 1000 and . <= 1499)] | length' \
      scripts/quality-baselines.json

Verified on `origin/main@152add03`: **25 entries**, 0 entries >2000 (Ola 1 closed), 2 entries in
[1500,2000] (`connect.rs` in flight, `params.rs` declared exception). Every figure below is a
candidate to re-measure before the first cut (policy § Método, S8 of the Ola 2 record).

**S2 — Order by churn, then by class.** The program orders inside a band by churn, bug history
and measured complexity, never by line count. `file_size_preflight.sh --format <path>` is the
single source for the table below: `churn_commits` is `git rev-list --count HEAD -- <path>`
(all history, no `--follow`), and `production` / `inline_tests` are measured by the preflight's
lexer. `needs-review` is a maintainer decision, not a mechanical extraction.

| # | File | Baseline / real | Prod / tests | Churn | Allow | Class |
|---|------|-----------------|--------------|-------|-------|-------|
| 1 | `cli/scrape_flow.rs` | 1444 / 1444 | 726 / 718 | 64 | 0 | needs-review |
| 2 | `application/crawler/discovery.rs` | 1026 / 1026 | 446 / 580 | 63 | 1 | tests-majority |
| 3 | `application/container.rs` | 1071 / 1071 | 1071 / 0 | 60 | 0 | needs-review (ADR-0011) |
| 4 | `cli/args/crawler.rs` | 1375 / 1375 | 508 / 867 | 46 | 0 | tests-majority |
| 5 | `application/http_client/client.rs` | 1317 / 1317 | 608 / 709 | 30 | 0 | tests-majority |
| 6 | `application/crawler/sitemap_discovery.rs` | 1400 / 1400 | 827 / 573 | 28 | 0 | needs-review |
| 7 | `mcp_server/state.rs` | 1131 / 1131 | 565 / 566 | 28 | 0 | tests-majority |
| 8 | `webfang_test_utils/src/lib.rs` | 1458 / 1438 | 798 / 640 | 25 | 0 | needs-review (drift −20) |
| 9 | `infrastructure/ssrf.rs` | 1287 / 1287 | 448 / 839 | 20 | 0 | tests-majority (security) |
| 10 | `cli/batch_flow.rs` | 1250 / 1212 | 898 / 314 | 18 | 0 | needs-review (drift −38) |
| 11 | `domain/options_spec/mod.rs` | 1110 / 1110 | 678 / 432 | 16 | 0 | needs-review |
| 12 | `infrastructure/persistence/sqlite.rs` | 1432 / 1432 | 942 / 490 | 15 | 0 | needs-review |
| 13 | `infrastructure/network/session_pool.rs` | 1156 / 1156 | 551 / 605 | 15 | 0 | tests-majority |
| 14 | `application/crawl_result_repository.rs` | 1097 / 1097 | 548 / 549 | 14 | 0 | tests-majority |
| 15 | `infrastructure/export/record_store.rs` | 1393 / 1393 | 642 / 751 | 14 | 0 | tests-majority |
| 16 | `application/vault_search.rs` | 1076 / 1076 | 403 / 673 | 14 | 0 | tests-majority |
| 17 | `cli/error.rs` | 1117 / 1117 | 442 / 675 | 13 | 0 | tests-majority |
| 18 | `application/crawler/session.rs` | 1006 / 1006 | 628 / 378 | 13 | 0 | needs-review |
| 19 | `infrastructure/export/vector_exporter.rs` | 1252 / 1252 | 594 / 658 | 13 | 0 | tests-majority |
| 20 | `domain/entities/content.rs` | 1155 / 1155 | 513 / 642 | 12 | 0 | tests-majority |
| 21 | `mcp_server/validation.rs` | 1163 / 1163 | 763 / 400 | 12 | 0 | needs-review |
| 22 | `mcp_server/schema_bridge.rs` | 1015 / 1015 | 553 / 462 | 11 | 0 | needs-review |
| 23 | `infrastructure/observability/file_trace_layer.rs` | 1113 / 1113 | 436 / 677 | 10 | 0 | tests-majority |
| 24 | `infrastructure/stream/mod.rs` | 1014 / 1014 | 514 / 500 | 10 | 0 | needs-review |
| 25 | `application/crawler/bounded_sink.rs` | 1103 / 1103 | 649 / 454 | 7 | 0 | needs-review |

13 files are `tests-majority` (mechanical first cut); 12 are `needs-review`.

**S3 — Parallel topology: four lanes, disjoint source paths.** Each lane owns its files and its
own worktree; no two lanes edit the same source path. Own `.envrc` (5 lines, written from the
tree's own name), `direnv allow`, identity manifest, and both indexes per tree. The three
isolation checks (target activation, indexes with root+head = worktree, identity gate) are
mandatory before the first edit.

Concurrency cap (Engram #648): at most two lanes implement at the same time. Docs-only preparation
is cheap and is not counted; an implementation agent is one running preflight, `--simulate`,
splits or cargo verification. On HTTP 429 the agent records it and stops — no retry loop.

| Lane | Batches | Files | Churn sum |
|---|---|---|---|
| L1 — MCP + harness | O3-B1, O3-B2, O3-B9 | 7 | 119 |
| L2 — CLI hot path | O3-B3, O3-B4, O3-B10b | 6 | 165 |
| L3 — Crawler + DI root | O3-B5, O3-B6 | 5 | 171 |
| L4 — Infra core + exports | O3-B7, O3-B8, O3-B10a | 7 | 116 |

Batch contents (2–3 files each, grouped by crate where possible; exact seams are re-measured in
the cut tree before moving anything):

- **O3-B1** — `schema_bridge.rs` + `validation.rs` + `state.rs` (all `webfang_mcp`).
- **O3-B2** — `webfang_test_utils/src/lib.rs` + `cli/error.rs`.
- **O3-B3** — `scrape_flow.rs` + `cli/args/crawler.rs` (the two highest-churn CLI files).
- **O3-B4** — `cli/batch_flow.rs` + `domain/entities/content.rs`.
- **O3-B5** — `crawler/discovery.rs` + `crawler/sitemap_discovery.rs`.
- **O3-B6** — `crawler/session.rs` + `crawler/bounded_sink.rs` + `application/container.rs`.
- **O3-B7** — `infrastructure/ssrf.rs` + `persistence/sqlite.rs`.
- **O3-B8** — `http_client/client.rs` + `stream/mod.rs` + `crawl_result_repository.rs`.
- **O3-B9** — `domain/options_spec/mod.rs` + `application/vault_search.rs`.
- **O3-B10a** — `export/record_store.rs` + `export/vector_exporter.rs`.
- **O3-B10b** — `network/session_pool.rs` + `observability/file_trace_layer.rs`.

11 PRs total. Each batch = own issue + PR with `Closes part of #2584`, `type:refactor`, base
`main`. No stacked PRs (`check-topology.sh`); sequential delivery through the queue.

**S4 — Shared artifacts are the serialization point.** Evidence from the five Ola 2 split PRs:
`quality-baselines.json` in 5/5, golden `real_tree_sites.txt` in 4/5, master doc in 4/5,
`test-inventory.md` in 1/5. The queue does not resolve conflicts and batches require disjoint
files (`scripts/ci_pr_overlap.sh`). Therefore:

1. Agents prepare and verify their batches in parallel; none pushes until its delivery slot.
2. Slots follow lane order (L1 → L2 → L3 → L4); two agents finishing at once use the lower lane
   number first. The waiting agent does NOT push.
2b. Implementation schedule under the two-agent cap: start L1 + L2; when L1's PR merges, start L3;
   when L2's PR merges, start L4. Delivery order stays L1 → L2 → L3 → L4.
3. Before pushing, the agent rebases its **local, unpushed** branch onto the current `main`,
   re-runs the preflight and the full verification on the new base, and re-refreshes the shared
   files in the same commit that produces the split. Pushed history is immutable: no force-push,
   no `reset --hard`, no rewrite of an already-published commit.
4. This record (`odd/tasks/file-size-ola3.md`) is edited only by the agent whose PR is currently
   merging (or by the parent at close) — never concurrently by four agents.
5. One PR through the queue at a time; every shared file lands in the same commit as its split,
   as the policy requires.

**S5 — Risk carve-outs (no mechanical treatment).**
- `ssrf.rs`: security file. Four lenses explicit, parity tests stay together, every security fix
  needs a negative test. Same guard order as always; moving tests must not break parity.
- `container.rs`: the ADR-0011 allowlist matches the path by substring. Moving production into
  `application/container/*` loses the permanent exemption; widening the pattern weakens it.
  Only the test region may be split (the Ola 2 O2-T7 precedent) or the file declared an
  exception — a maintainer decision before the first cut.
- `scrape_flow.rs` (churn 64), `sitemap_discovery.rs`, `sqlite.rs`, `batch_flow.rs`,
  `test_utils/lib.rs`, `stream/mod.rs`, `validation.rs`, `schema_bridge.rs`, `options_spec/mod.rs`,
  `session.rs`, `bounded_sink.rs`: `needs-review`. Each gets a recorded decision in its lane doc
  before any move: glued modules, cohesive declarative unit, high risk or architectural.
- Drift: `batch_flow.rs` 1250 → 1212 (−38) and `test_utils/lib.rs` 1458 → 1438 (−20). Re-measure
  in the cut tree; use descending `--update` or `--remove` according to the real result.
- `crawler/discovery.rs` carries 1 `allow(clippy::too_many_lines)`. Do not add new allows; that
  existing one is F3 input, untouched by this wave.

**S6 — Method per batch.** Split by real seams, never by line arithmetic. Two hats declared in
the PR (`Movido sin cambios` / `Corregido / mejorado`). No second lexer: `split_tests.sh` (F2,
now merged) is the owner of mod-tests extraction with its multiset proof; use it instead of the
hand-verified move. Re-measure spans in the cut tree with a sentinel script; characterize before
moving; test path renames declared; new test files are `*_tests.rs` siblings, never in a `tests/`
directory.

**S7 — Close and gates (no cuts).** `file_size_done.sh <path>` exit 0 per batch. Per PR:
`cargo check --all-targets --all-features`, strict clippy (`-D warnings -W
clippy::cognitive_complexity -W clippy::too_many_lines`), `cargo fmt --all -- --check`,
`RUSTDOCFLAGS=-D warnings cargo doc --workspace --all-features --no-deps`, full nextest suite,
`--list` before/after with the same runner, verbatim diff and `git diff --color-moved`,
visibility-modifier diff, `check_file_size.sh`, `check_test_inventory.sh`, `check_duplication.sh`,
`check_dead_tests.sh`, intra-crate direction, `ci_fast_gate.sh` GREEN with its coverage report
read. Four lenses with explicit result; native cycle when the slice's assessment says `review_due`.

**S8 — F3 boundary.** The phase after F2 is functions/allow-ratchet, with `file_size_rust.functions`
as its reserved key. It is a tooling phase, not a split, and runs in parallel with this wave:
this plan adds no `allow` attributes and does not fix functions. The existing allow in
`discovery.rs` is handed to F3 untouched.

**S9 — Governance.** English-only artifacts, no `CHANGELOG.md`, no `docs/` unless asked, no
`git checkout`/`switch`/`stash`, identity proxy before every commit, frequent commits per step.
Push, PR and merge remain the maintainer's decision for each batch unless explicitly authorized.

## Tasks

| ID | Task | Specs | Route | Status | Evidence |
|----|------|-------|-------|--------|----------|
| O3-T0 | Worktree + bootstrap + indexes + isolation checks + this doc + Engram mirror | S1,S3 | inline | done | base `152add03`, proxy OK |
| O3-T1 | L1 — batches B1, B2, B9 (MCP + harness + domain) | S2,S3,S6,S7 | delegated worker | pending | — |
| O3-T2 | L2 — batches B3, B4, B10b (CLI hot path) | S2,S3,S6,S7 | delegated worker | pending | — |
| O3-T3 | L3 — batches B5, B6 (crawler + DI root) | S2,S3,S5,S6,S7 | delegated worker | pending | — |
| O3-T4 | L4 — batches B7, B8, B10a (infra + exports) | S2,S3,S5,S6,S7 | delegated worker | pending | — |
| O3-T5 | Serialized delivery: rebase-on-local + shared artifacts + one-at-a-time queue | S4 | parent | pending | — |
| O3-T6 | Maintainer decisions for the 12 `needs-review` files and `container.rs` policy | S5 | parent | pending | — |
| O3-T7 | Close: band count from baseline, all batches merged, record updated | S1,S7 | inline | pending | — |

## Log

- **L1 (2026-11-22, verbatim)**: "ok ya estan mergeados vamos a crear el plan" — the owner
  authorizes the Ola 3 plan after the Ola 2 agents' work landed. Immediate antecedent (verbatim):
  "la ola 3 se puede ejecutar de manera simultanea por ejemplo repartirla entre tres o cuatro
  agentes ?".
- **L2 (2026-11-22, evidence)**: `origin/main@152add03` (F2 / #2957 merged), only open PR #2962
  (release v9.2.2). Baseline band [1000,1499] = 25 entries. Per-file preflight measured for all
  25: 13 `tests-majority`, 12 `needs-review`, 1 existing `allow` (`discovery.rs`), 2 drifts
  (`batch_flow.rs` −38, `test_utils/lib.rs` −20). Five Ola 2 split PRs inspected: all touched
  `scripts/quality-baselines.json`, four touched the golden and the master doc, one touched
  `docs/test-inventory.md`.
- **L3 (2026-11-22, decisions)**: four lanes instead of three (25 files / 11 batches / churn sums
  119/165/171/116); parallel preparation, serialized delivery because the shared files conflict
  at merge time and the queue does not resolve conflicts; `needs-review` files require a recorded
  maintainer decision before the first cut; `container.rs` keeps its ADR-0011 exemption and only
  its test region is movable; `ssrf.rs` gets four lenses; F3 stays out of this wave.
- **L4 (2026-11-22, provider constraint surfaced by the Engram mirror)**: Engram #648 records that
  3+ concurrent BAI agents trigger HTTP 429 with no body (shared per-account quota). The plan keeps
  four logical lanes but caps implementation concurrency at TWO: L1 + L2 first, then L3 as L1
  merges and L4 as L2 merges. Docs-only lanes are exempt. On 429, record and stop — never retry-loop.
