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
| O3-T1 | L1 — batches B1, B2, B9 (MCP + harness + domain) | S2,S3,S6,S7 | delegated worker | in_progress | B1 impl done; B1 PR #2975 queued; B2 impl running |
| O3-T2 | L2 — batches B3, B4, B10b (CLI hot path) | S2,S3,S6,S7 | delegated worker | impl done, awaiting slot | B3 impl done (5633/5633, both --remove) |
| O3-T3 | L3 — batches B5, B6 (crawler + DI root) | S2,S3,S5,S6,S7 | delegated worker | pending | starts when B1's PR merges |
| O3-T4 | L4 — batches B7, B8, B10a (infra + exports) | S2,S3,S5,S6,S7 | delegated worker | pending | starts when B3's PR merges |
| O3-T5 | Serialized delivery: rebase-on-local + shared artifacts + one-at-a-time queue | S4 | parent | in_progress | B1 rebased onto 04a34612 (release merged), re-verified fail=0, PR #2975 |
| O3-T6 | Maintainer decisions for the 12 `needs-review` files and `container.rs` policy | S5 | parent | done | owner pre-authorized full execution ("implementar el plan completo"); decisions recorded per batch issue; container.rs = test-region-only split |
| O3-T7 | Close: band count from baseline, all batches merged, record updated | S1,S7 | inline | pending | — |
| O3-T8 | B10a real splits: `export/record_store.rs` + `export/vector_exporter.rs` | S2,S5,S6,S7 | delegated worker | impl + native review approved | worktree `refactor-ola3-b10a` @ `34795f54`; 5633 tests green; RDD risk+resilience approved/acknowledged (lineage `review-6528a9f1193da9a6`) |
| O3-T9 | #2970 remainder: `stream/mod.rs` + `crawl_result_repository.rs` + `client.rs` <500 | S2,S5,S6,S7 | delegated worker | impl + native review approved | worktree `refactor-ola3-b8-remainder` @ `023b3a09`; 5633 tests green; RDD risk+resilience approved/acknowledged (lineage `review-4b04120e1d71e13a`) |

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
- **L5 (2026-11-22, B1 delivery cycle)**: origin/main moved to `04a34612` (release v9.2.2, #2962 merged) before delivery; B1 rebased (12 commits, clean), full re-verification on the new base: fail=0 (check/clippy/fmt/doc/nextest 5633/size guards/direction). Native review: `review_due=true (high_risk)` from assess, but START failed twice with the typed `lens_context_budget_exceeded` (4628-line slice; single-lens probe also fails) — the native transaction can only isolate the tail of a built branch, so the outside view ran as two independent lens reviewers (risk, readability) over the full PR diff: **both approved, zero blockers** (one minor pre-existing finding: stale `metrics.rs::ensure_global_subscriber` doc comment carried verbatim from base). Consent for reviews and merges: owner's standing authorization ("ejecutar reviews sin mi permiso y mergear las pr's si todo esta verde"). PR #2975 opened (`type:refactor`, `Closes #2963` + `Closes part of #2584`). First queue attempt rejected: **Repo guards failed** — `check_concurrency_lints.sh` keys its CC-L2 exemptions by path in `scripts/concurrency_lint_allowlist.txt`; the `state.rs|limits.` row did not follow the code into `state/limits.rs`. Fixed in `dd53cfa7` (same exemption, same reason, migrated row), guard green locally. **Lesson: every delivery verification must include `bash scripts/check_concurrency_lints.sh`** — the local gate's coverage report does not cover Repo guards, and any path-keyed allowlist breaks silently when a split moves code.
- **L6 (2026-11-22, B2 delivery)**: B2 (#2976) rebased onto post-B1 main (`c98ba07f`) — one shared-artifact conflict (`quality-baselines.json`) resolved via the ratchet script over the base version, not by hand; lens reviews (risk + readability) approved with 2 minors, both fixed pre-push (stale grandfathered doc in `redaction_network.rs`; L6 numbers corrected to +28 sites / five renames). MERGED green on first queue attempt. Post-merge done; band 25→20; issue #2964 auto-closed.
- **L7 (2026-11-22, B3 rebase + review)**: B3 rebased onto `cecfd1b8`; master-doc conflict resolved by keeping B2's L6 and renumbering B3's entry to L7 (union, verbatim text). Lens reviews approved: risk 2 minors (test-helper widening narrower as `pub(super)` — applied; stale base hash in L7 — corrected via L8 in the master doc), readability minors = cosmetic follow-ups (re-export block grouping in `scrape_flow.rs`; orphaned `--timeout-secs` doc on `parse_selector`; dead underscore bindings; line-number citations carried from base — all pre-existing or cosmetic, NOT fixed in this slice).
- **L8 (2026-11-22, CONCURRENCY RULE CORRECTION — real cause of the 429)**: B9's worker died mid-task with `user concurrency limit exceeded` while TWO workers AND TWO lens reviewers ran simultaneously (4 BAI sessions). Engram #648's constraint is about concurrent **LLM sessions** (shared per-account provider quota), not cargo processes. Corrected rule for the rest of the wave: **at most TWO LLM sessions total at any instant** (worker+worker, worker+reviewer, or reviewer+reviewer — never a third); local `cargo` verification runs (bash, no LLM) do NOT count and may overlap freely. B9 was resumed with its context intact and finished its remaining ratchet/artifacts/verification solo. Reviewers are now scheduled only when at most one worker runs.
- **L9 (2026-11-22, the intra-crate direction blocker + its fix)**: B9's PR (#2978) failed CI on `check_intra_crate_direction.sh`: the extractor (`tools/intra-crate-lint`) decides test regions per-file from item attributes, so every `*_tests.rs` sibling this program extracts (gate lives at the parent's `#[cfg(test)] #[path] mod` declaration) was scanned as production code — B9's `vault_search_support_tests.rs` inline-importing `crate::infrastructure::obsidian::VaultFsReader` failed ADR-0010 strict mode, and B5's crawler test files carried the same latent violation. The allowlist was not an option (ADR-0012-B cap 5, 2 permanent entries, per-symbol removal conditions). Fix as a dedicated PR (#2980, issue #2979): the extractor now follows `#[cfg(test)]` mod DECLARATIONS (explicit `#[path]` or default sibling) and excludes the declared child file whole; pinned by 3 new tool unit tests + the harness; golden regenerated (−225 rows of earlier waves' test-file sites). MERGED green first attempt. Checker verified green on both B9's and B5's trees. **Lesson: the guard battery a delivery must run locally includes `check_intra_crate_direction.sh` (added to the delivery script) — path/shape-keyed guards break when the program's own convention moves code.**
- **L10 (2026-11-22, premature-push lesson)**: B9 was pushed once (#2978) before its final S4.3 rebase; when the extractor fix landed on main, the sanctioned rebase made the local branch diverge from the pushed candidate. Force-push is hard-deny, so the failed candidate was closed with a superseded note (#2978), its remote branch deleted, and the rebased lineage pushed fresh (#2981). **Rule: push only when the delivery slot is open AND main has been re-fetched within minutes of the push; never push a branch that still has a known rebase coming.**
- **L11 (2026-11-22, delivery cycle B9/B4/B10b)**: B9 merged as #2981 (superseded #2978 after the sanctioned rebase; L10 lesson). B4 merged as #2982 after three CI round-trips that each named a distinct path-keyed guard: (1) the cfg(windows) cross-lint — `batch_spool_tests.rs`'s `CliExit` import is used only by the `#[cfg(unix)]` permission test → gated `#[cfg(unix)]`; (2) the data-flow doc guard — `docs/src/data-flow.md` cites `.webfang-batch-capture.jsonl` by FILE PATH, the constant moved to `batch_spool.rs` → citation followed the code (guard-demanded docs/ edit, not a voluntary one); (3) the golden shifted by post-regen doc commits → regenerated once more. **The delivery battery now runs: check, clippy, fmt, doc, nextest, size guards, dependency direction, concurrency lints, intra-crate direction, data-flow doc, and check_windows_platform.sh when the target is installed.** B10b queued as #2983. container.rs disposition resolved: measured 1071/0 — its test region was ALREADY path-declared (`application/container/tests.rs`, O2-T7 precedent), so the only permitted treatment is a no-op; file untouched, recorded on #2968 as the maintainer-exception-list candidate (S5's declared alternative). B6 implemented: session.rs 1006→445, bounded_sink.rs 1103→445, both --remove. All 11 batches are now implemented; 5 merged, 6 in delivery.
- **L12 (2026-11-22, Engram mirror refresh)**: merged 6 PRs (B1/B2/B3/B9/B4 + extractor fix); band 14→12 in flight; independent lens reviews (risk+readability) approved for B4, B9, B10b with only minor doc-figure findings, all corrected via L-numbered log entries (L8/L11/L13 pattern). Remaining deliveries in lane order: B10b (queued #2983) → B5 → B7 → B6 → B8 → B10a.
- **L13 (2026-11-22, verbatim, continuation authorization)**: "seguimos con la implementación real de los splits que faltan —B10a completo más lo pendiente de #2970—". After B5/B7/B6/B8-landing, main is `919a70a4`; band [1000,1499] is 7. Two independent units remain and run in parallel under the two-LLM-session cap: O3-T8 (B10a: `record_store.rs` 1393, `vector_exporter.rs` 1252; existing worktree already holds only the 2-line `pub(super)` helper widening `3da9147c`) and O3-T9 (#2970 remainder: `stream/mod.rs` 1014, `crawl_result_repository.rs` 1097, `client.rs` 627→<500; PR #2994 already landed the first `client.rs` test extraction with `Closes part of #2970`). Both workers: local commits allowed, NO push / PR / merge (parent owns serialized delivery per S4). `stream/mod.rs` keeps the poisoned-mutex policy intact (recovered writes must not become silent drops or panics). Shared artifacts (`quality-baselines.json`, golden `real_tree_sites.txt`, master doc) are updated in each tree for local gates; the parent reconciles them at integration. Parent-created worktree `refactor-ola3-b8-remainder` bootstrapped and indexed this session; B10a indexes refreshed (codedb head was stale at `2592333a`, now `3da9147c`).
- **L14 (2026-11-22, both units delivered + native review closed)**: O3-T8 (`34795f54`): record_store.rs 1393→454 (+ record_store_file.rs 215, two themed test siblings 468/316), vector_exporter.rs 1252→497 (+ vector_exporter_header.rs 121, two test siblings 279/403); all parts <=500, `--remove` in the same commits, jscpd duplicated lines 8195 ≤ 8219. O3-T9 (`023b3a09`): client.rs 627→473 (+ client_response.rs 175), crawl_result_repository.rs 1097→350 (+ writer 231, two test siblings 343/206), stream/mod.rs 1014→364 (+ record.rs 168, stream_tests.rs 497); all <=500, `--remove`, jscpd 8131 ≤ 8219. `split_tests.sh` multiset proofs carried in every extraction; `--list` multiset invariant held (declared test-name renames only). Both workers: full pre-commit gate rc=0, strict clippy rc=0, nextest 5633 passed/8 skipped, `ci_fast_gate.sh` GREEN (PASS=47 FAIL=0 SKIP=2) with coverage tail read, all path-keyed guards green. `check_data_flow_doc.sh` was pre-existing red in the #2970 tree (doc-only assertion, unchanged from base); #2996 later fixed false claims in NOTICE/README on main.
- **L15 (2026-11-22, native RDD)**: `gentle-ai review assess` returned `review_due=true (high_risk)` for both candidates; RDD enabled by global; owner granted candidate-scoped consent via typed `consent/v3` envelopes. Lenses `review-risk,review-resilience` (stored-data contracts, retry/error paths, visibility widenings). #2970 lineage `review-4b04120e1d71e13a` → **approved**, zero blockers, 4 advisory pre-existing warnings (R4-001..004 in `crawl_result_repository_writer.rs` / `client_response.rs`), acknowledged and authority burned. B10a lineage `review-6528a9f1193da9a6` → **approved**, zero findings in both lenses, acknowledged and authority burned. Relay hiccup `opencode_task_not_completed` was recovered by re-querying the exact bound STATUS and relaunching only reoffered slots; no defect report (not a Gentle AI typed envelope). Native review is informational: push/PR/merge remain separate maintainer decisions.
- **L16 (2026-11-22, main moved during the wave)**: while both units were being verified, `origin/main` advanced to `f0b524c2` with `20c75b79` (B10b #2983 merged) and `f0b524c2` (false-claims doc fix #2996). Both feature branches are now based on older main and MUST be rebased onto current main before any push (S4.3); shared artifacts (`quality-baselines.json`, golden `real_tree_sites.txt`, master doc) will conflict with B10b's versions and must be reconciled in the rebase commit, then local gates and native review re-run on the new target. Main baseline band is 5 entries: 1071 (container exception), 1097, 1393, 1252, 1014 — exactly the four removable ones our two units delete, so after both land only the documented `container.rs` exception remains. An untracked `odd/tasks/docs-consolidation.md` exists in main from another session; not touched by this mission.
- **L17 (2026-11-22, delivery authorized and executed)**: owner said "adelante si" to rebase + reconcile + open both PRs. B10a rebased onto `origin/main@f0b524c2` → head `a39b8e16`; master-doc conflicts resolved by union/renumbering (B10a L14/L15 → L16/L17) and `quality-baselines.json` by removing all three entries (B10b's two + B10a's two). B8 rebased → head `c5dc3a96`; master-doc B8 L14 → L16. Golden regeneration reported "already matches the tree" in both. Post-rebase gates all green: check/clippy/fmt/doc/nextest/14 guards/`ci_fast_gate.sh` PASS=47 FAIL=0 SKIP=2. Fresh native review on the new targets: B10a lineage `review-a5a258b012f152bb` approved (5 advisory pre-existing warnings in `record_store_file.rs` / `vector_exporter_header.rs`), B8 lineage `review-b154dcf019e81ec7` approved (zero findings); both acknowledged and authority burned after new candidate-scoped consent. Pushed `refactor/ola3-b10a` + `refactor/ola3-b8-remainder` (local-only before; no force-push). Opened **PR #2998** (`Closes #2972`, `Closes part of #2584`) and **PR #2999** (`Closes #2970`, `Closes part of #2584`), both base `main`, `type:refactor` auto-applied, MERGEABLE, CI checks running at open time. The two PRs touch the same shared artifacts, so the queue must merge them sequentially; the second will need a fresh rebase + re-review after the first lands. Merge remains a maintainer decision.
- **L18 (2026-11-23, merges + issue + cleanup)**: owner authorized "abrimos issue para 5 warnings advisory pre-existing ... y mergea las pr's abiertas". Opened issue **#3000** (`type:chore`) tracking the 5 B10a advisory findings (HeaderScanWindow, NoFsyncOnWrite, AppendCrashRecoveryGap, LockSentinelWorldReadable, MigrationBackupTimestampCollision) with locations and disposition criteria. Merged **PR #2998** at 2026-11-23T01:11:12Z and **PR #2999** at 2026-11-23T01:41:56Z, both by the queue with all required checks green. #2999 became CONFLICTING after #2998 landed (master doc); resolved with a **merge commit** (`a3a06b3d`, not a rebase — the branch was already pushed), renumbering B8's L16 → L18, then pushed normally and re-ran all gates (green) before merging. A fresh native review on that post-merge target returned terminal **escalated / inconclusive** (`R4-MissingTestCfg`, unknown causality, no correction offered); the flagged `#[cfg(test)]` + `#[cfg(not(miri))]` pair is the split_tests.sh-required equivalent of the original `#[cfg(all(test, not(miri)))]` and had been approved by the two prior reviews on the same source diff. Per contract the review is informational and its stop never approves delivery; merge proceeded on the owner's explicit authorization plus green CI. Post-merge runbook completed for both: main ff-only synced to `491e77b2`, both worktrees removed, both branches deleted, prune run, both target dirs removed by literal path after liveness check. Clean final state.
