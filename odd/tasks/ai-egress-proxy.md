# Mission: ai-egress-proxy — ADR-0021 (+ ADR-0020 Slice 2 dependency)

Implement ADR-0021 "Explicit Per-Provider Forward Proxy for AI Egress" (docs-only PR #2870,
`docs/adr/0021-explicit-ai-egress-proxy.md` — the ADR text there is the normative spec) plus
the ADR-0020 Slice 2 dependency the owner chose to land first. Sequential delivery: one PR per
phase on `main` (no stacked PRs). Test runner: `cargo nextest`. Local gate:
`bash scripts/ci_fast_gate.sh` (with direnv env) before every push — tree-derived artifacts
(golden set, test inventory, doc claim tables) travel with the tree (lesson #2547 L7).

## Specs

- S1: user request — "vamos a implementar la el ADR que esta en la pr #2870".
- S2: sequencing decision (owner, 2026-11-17): "Aterrizar ADR-0020 Slice 2 primero" — P0 lands
  ADR-0020 Slice 2 as its own behavior-preserving PR (`Closes part of #2357`) before ADR-0021
  Slice 2.
- S3 (P0, ADR-0020 Slice 2 verbatim): "`infrastructure/llm/http_client.rs` — Unified SSRF-aware
  builder; `provider.rs` and both constructors delegate"; "Zero behavior change per slice:
  every existing test — including `timeout_is_retried_and_recovery_is_served` — stays green
  unmodified".
- S4 (P1, ADR-0021 Slice 1 verbatim): domain types "`ProxyConfig`, `ProxyAuth`, `ProxySpec`
  (`Named` | `Inline`), `ProxyRegistry`, `ValidatedProxy` (scheme `http`/`https`, no userinfo,
  literal-IP gate + FIN-017), `EndpointPolicy`, `ClientSecurityProfile`, and
  `SsrfGuard::secure_client_with_profile`. Unit-tested in domain (Miri-clean, no network).
  Adding `proxy` to `ProviderConfig` also requires updating the `shape` row **and every
  line-pinned claim** in the manifest of `docs/architecture/ai-providers-design.md` audited by
  `scripts/check_ai_providers_doc.sh` (C2/C3/C6), not just the field list."
- S5 (P2, ADR-0021 Slice 2 verbatim): "`DefaultSsrfGuard::secure_client_with_profile`
  implementation (`.redirect(dest)` → `.no_proxy()` → `.proxy(...)` →
  `.dns_resolver(proxy_or_dest)`), shared LLM HTTP client builder (ADR-0020 Slice 2), 407
  classification, and symmetric wiring into both `RemoteEmbeddingAdapter` and
  `OpenAiCompatibleProvider` (plus `webfang_cli` / `llm_wire`)."
- S6 (P3, ADR-0021 Slice 3): `print_proxy_posture` (ambient env always ignored + effective
  proxy: origin sanitized, provenance, proxy network policy, auth source label, DNS-delegation
  notice); tracing fields `proxy_enabled`/`proxy_origin`/`proxy_id`; credentials never in logs
  or `--trace-file`; `docs/src/{data-flow,installation}.md` + `scripts/check_data_flow_doc.sh`
  claims manifest in lockstep.
- S7 (P4, ADR-0021 Slice 4): E2E harness with wiremock as observable forward proxy
  (absolute-form for `http://` targets only — no CONNECT coverage): ambient `HTTP_PROXY`/
  `HTTPS_PROXY` ignored (0 requests at ambient proxy); configured proxy without opt-in
  rejected pre-socket (0 requests); configured proxy with opt-in routes the AI request
  (`received_requests() == 1`); client-side guarantees under proxy: literal-IP `302` into the
  proxy's own subnet (or `169.254.169.254`) stopped before a second proxy request, 16 MiB body
  cap triggers, `429` backoff pacing fires before retry.
- S8 (ADR-0021 cross-cutting invariants): ambient env vars (`HTTP_PROXY`/`HTTPS_PROXY`/
  `ALL_PROXY`/`NO_PROXY`) unconditionally ignored (`.no_proxy()` first everywhere); no CLI
  `--proxy` flag; zero userinfo in `proxy.url` (exit 78 → `proxy.auth`); `proxy = null`
  explicit → deserialization error; duplicate `[[proxies]]` ids / unknown `proxy = "id"` →
  `CliExit::ConfigError` (exit 78) with Spanish message; SOCKS (`socks5`/`socks5h`) rejected
  at `ValidatedProxy::try_new` with specific Spanish diagnostic; 407 terminal (zero retries),
  Spanish error distinguishing configured vs anonymous proxy auth, never logging credentials;
  crawl/scrape/`WreqDownloader`/`FetchRouter` stay direct-only; user-facing errors in Spanish,
  tracing fields in English.

## Tasks

- T1 (S3): P0 — unified `infrastructure/llm/http_client.rs` builder; `client.rs`, `provider.rs`,
  `remote_embedding.rs` delegate — inline — branch `refactor/llm-shared-http-client` — commit
  `94a1c226` (+ mission doc `74314acd`); golden set 2262→2264 and ai-providers doc pins travel
  in the same commit.
- T2 (S4): P1 — domain types + `SsrfGuard::secure_client_with_profile` declaration + ProviderConfig
  `proxy` field + ai-providers-doc claims — inline — branch `feat/ai-proxy-domain-types` — commit
  `5cb6641a` (golden refresh + ai-providers manifest C2/C3/C6 + baseline descents travel in the
  same commit). NOTE: the trait method's infrastructure impl travels with the declaration (a
  declaration without impl does not compile); P2 keeps the shared-builder path, adapter wiring,
  407 classification and CLI/registry glue.
- T3 (S5): P2 — profile-aware shared LLM builder + resolved proxy auth + symmetric wiring into
  `RemoteEmbeddingAdapter` / `OpenAiCompatibleProvider` / `llm_wire` / `webfang_cli` + 407 terminal
  classification + `[[proxies]]` config load and `ProxyRegistry` resolution — inline — branch
  `feat/ai-proxy-transport-wiring` — TBD. The guard method itself already landed in T2 and is NOT
  reimplemented here.
- T4 (S6): P3 — doctor/show posture + tracing + disclosure docs — inline — branch
  `feat/ai-proxy-diagnostics` — delivered (see L16), unpublished.
- T5 (S7): P4 — E2E harness — inline — branch `feat/ai-proxy-e2e-harness` — TBD.

Delivery forecast ≈ 1200–1500 authored lines across 5 PRs → strategy `ask-on-risk` (default):
when publishing beyond the first slice, ask the owner once for the chain strategy.

## Log

- L1: user request — "vamos a implementar la el ADR que esta en la pr #2870".
- L2: verified state: PR #2870 open (docs-only, ADR-0021 refined); issue #2551 `status:approved`
  + `type:feature`; `infrastructure/llm/http_client.rs` NOT on main (checked `ls`); no open PRs
  for ADR-0020 slices; Engram #3016 documents the review objections already folded into the ADR.
- L3: owner sequencing decision via ask → land ADR-0020 Slice 2 first (S2). Worktree
  `refactor-llm-shared-http-client` @ origin/main f3ae9823; .envrc 5-line canonical form with
  `export` (plain assignment is NOT captured by direnv 2.38.2 — first bootstrap attempt exported
  only PATH); `seed: cold` (no published seed); codegraph init + codedb reindex OK (root=head=f3ae9823).
- L4: prior-art found: `odd/tasks/proxy-posture.md` (mission #2500 → PR #2547) — the direct-egress
  pin this ADR amends. `.no_proxy()` choke point: `DefaultSsrfGuard::secure_client*`
  (`infrastructure/ssrf.rs`); doctor posture line + docs sections already exist (to be updated by
  P3, not created).
- L5 (T1 verification, 2026-11-17): zero-behavior-change exception to RED documented (refactor;
  behavior pinned by existing tests, unmodified). Observed: cargo check OK · clippy
  --all-targets --all-features -D warnings OK · fmt --check OK · rustdoc -D warnings OK · llm
  unit tests 54/54 (incl. the 4 ADR-pinned transport tests) · new builder tests 2/2 ·
  remote_embedding_resilience 8/8 · ci_fast_gate first run FAIL=2 (intra-crate golden set
  2262→2264; ai-providers doc C2 line pins) → both refreshed per #2547 L6/L7 and re-run:
  **fast-gate GREEN, PASS=45 FAIL=0 SKIP=2** (skips declared: pinned-lint changed-scope,
  windows — no cfg(windows) touched). Coverage: NOT-covered jobs (toolchain, artifact-guards,
  test-full, security) untouched by this change. Commits: `94a1c226` (refactor + artifacts),
  `74314acd` (mission doc). PR gates pre-checked: #2357 CLOSED but carries `status:approved`;
  validator checks labels only, `Closes part of #2357` passes.
- L6: awaiting owner decision on push + PR (push/PR are human decisions; #2357 linkage ready).
- L7 (T2 verification, 2026-11-17): RED exception documented (new domain types + additive trait
  method; behavior covered by 20 new unit tests: proxy_config 16, config_serde 4). Observed:
  fmt --check OK · clippy --all-targets --all-features -D warnings OK · rustdoc -D warnings OK ·
  proxy_config 16/16 · config_serde 4/4 · providers 18/18 · ssrf_guard 47/47 · infra ssrf 31/31 ·
  connect 35/35 · llm_wire 21/21 · llm 52/52 · resilience 8/8 · auth_source_none 13/13 ·
  ssrf_policy_e2e 7/7 · real_tree 2/2 · ai-providers drift gate OK · file-size ratchet OK
  (baselines LOWERED: providers.rs 593→546, connect.rs 1974→1874, remote_embedding.rs
  1825→1773 — pure-move extractions: ssrf_guard/registry.rs, infrastructure/ssrf/name_source.rs,
  domain/config_serde.rs, three test-fixture submodules, twin-fixture dedupe in
  auth_source_none_test.rs). Design deviations recorded: `ValidatedProxy::try_new` takes the
  `ProxyConfig` (catalog form) plus `try_inline(&ProxyDefinition)` for the provider-inline form
  (the ADR's single-ctor signature cannot serve both provenances); the `wreq::Proxy` matcher is
  built inside `try_new` so wiring stays infallible; proxy auth stays unresolved in Slice 1
  (resolution + basic_auth install is Slice 2).
- L8 (DEFECTS + root cause): (1) `proxy_config.rs` was corrupted twice by in-place awk streaming
  rewrite (self-truncation) and by an unrestored stub from a doc-ICE bisection probe — recovered
  from context each time; lesson enforced: commit after every verified step, never run in-place
  awk rewrites, back up BOTH halves of a file split. (2) The rustdoc "ICE" (scoped-tls panic)
  was the mbx shim bug (obs #2967, upstream mr-boxington#966): deterministic via mbx, clean via
  real cargo with the same pinned toolchain; ci_fast_gate's doc step reports it as failed, so
  the gate is run with `PATH="$HOME/.cargo/bin:$PATH"` (real cargo, same target dir).
- L9 (rebase onto post-P0 main, 2026-11-17): P0 merged as PR #2878 (main `5d044bba`) before T2
  published. Rebased `feat/ai-proxy-domain-types` from `1fece35b` onto `5d044bba` — commits were
  unpushed, so no history rewrite on a public SHA. Add/add conflict on this mission doc resolved
  by union (P0 T1 delivery + T2 verification/log); content conflicts on the ai-providers doc
  claim pins and the intra-crate golden set resolved against the rebased tree (golden regenerated
  with `scripts/refresh_real_tree_golden.sh`, pins refreshed from the merged symbols). T1 stays
  DELIVERED; T2 stays unpublished pending owner push/PR decision.
- L10 (post-rebase verification, 2026-11-17): full re-run on the rebased tree with the worktree's
  own env (`CARGO_TARGET_DIR=~/.cache/cargo-target/feat-ai-proxy-domain-types`, real cargo via
  `PATH="$HOME/.cargo/bin:$PATH"` — the inherited shell pointed at main's target dir and the mbx
  shim). Observed: cargo check OK · clippy --all-targets --all-features -D warnings
  (-W cognitive_complexity -W too_many_lines) OK · fmt --check OK · rustdoc -D warnings OK ·
  `scripts/check_ai_providers_doc.sh` OK (30 claim, 8 shape, 5 impl, 1 absent, 47 prose cites,
  26 § refs) · ci_fast_gate **GREEN PASS=45 FAIL=0 SKIP=2** (declares pinned-lint changed-scope +
  windows; nextest integration 4923/4923 passed) · golden set regenerated, no drift. Branch
  `feat/ai-proxy-domain-types` @ 2 unpushed commits (feature `5cb6641a` + this log), based on main
  `5d044bba`, identity proxy OK, clean status, nothing pushed. NOT-covered CI jobs (toolchain,
  artifact-guards, merge-metrics,
  test-full, security) untouched by this change; windows cross-target skipped (no cfg(windows) arm
  touched).
- L11 (publish, 2026-11-17): owner explicitly authorized push + PR. Pushed
  `feat/ai-proxy-domain-types` (remote @ `65879bc1`) and opened PR #2884 against `main` — title
  `feat(ai): ADR-0021 Slice 1 — proxy domain types with type-separated SSRF policies`, body carries
  `Closes part of #2551` (issue OPEN, labels `status:approved` + `type:feature`). Two unpushed
  commits at push time → squash uses the PR title as the changelog line. CI checks pending at
  creation; auto-label job applies `type:feature` from the title. Merge remains a maintainer
  decision; next slice T3 starts from `main` after #2884 lands.
- L12 (scope decision, 2026-11-17): T3 starts from `main f1f42951` (v9.1.0 release #2892, which
  absorbed the redundant Windows-cross-lint fix PR #2894). Exploration showed the ADR's declaration
  surface (Decision 1, `ConfigDefaults` top-level `[[proxies]]`) was never wired: no production
  `ProxyRegistry` construction exists and `ConfigDefaults` has no `proxies` field, so a
  `ProxySpec::Named` could never resolve at runtime. Decision: fold the minimal catalog load +
  per-provider resolution into T3 — without it the builder/wiring is unreachable and T5's E2E
  harness cannot drive a configured proxy. This is recorded as an assumption of S5, not a new user
  requirement; it keeps the ADR's slice boundary intact.
- L13 (T3 map): `DefaultSsrfGuard::secure_client_with_profile` already implemented at
  `infrastructure/ssrf.rs` with the mandated order (destination redirect policy → `.proxy(...)` →
  proxy-or-destination resolver). Remaining: `build_guarded_http_client` profile variant + basic
  auth resolution from `ProxyAuth.password: AuthSource`; both adapters' constructors; the
  production completion path in `webfang_cli/src/main.rs` (it bypasses `llm_wire`); 407 terminal
  branches mirroring the 401 handler in `remote_embedding.rs`. RED tests go next to
  `plain_guard_client_builds` in `http_client.rs` and beside `status_400_is_terminal_without_retry`
  in `remote_embedding.rs`.
- L14 (T3 delivered, 2026-11-17) — commit `6d83f05a`, branch `feat/ai-proxy-transport-wiring`,
  based on main `f1f42951`. RED→GREEN: 5 profile-builder tests (3 new) + 2 407 integration tests
  in `tests/ai_proxy_407_test.rs` (moved out of the grandfathered parent to keep the ratchet).
  Implementation: `ValidatedProxy::with_basic_auth` installs the resolved `AuthSource` credential
  on the prebuilt matcher; `build_llm_client(config, registry)` returns client + proxy-auth
  provenance; both adapters get `new_with_registry` / `with_http_and_proxy_auth`; `llm_wire` gets
  `_with_registry` variants; `main.rs` loads `[[proxies]]` via `proxy_wire`; 407 is terminal with
  Spanish configured-vs-anonymous messages and zero credential logging. File-size ratchet held by
  splitting 407 diagnostics/wiring into `remote_embedding/{proxy_auth,proxy_wiring}.rs`,
  `llm_wire/proxy.rs` and `webfang_cli/src/proxy_wire.rs`; `remote_embedding.rs` baseline lowered
  1773→1714. Tree-derived artifacts refreshed in the same commit (golden set, ai-providers pins,
  test inventory, baseline). Verification: cargo check/clippy --all-targets --all-features -D
  warnings/fmt --check/rustdoc -D warnings OK · `check_ai_providers_doc.sh` OK (30 claim, 8 shape,
  5 impl, 47 cites) · **ci_fast_gate GREEN PASS=45 FAIL=0 SKIP=2** (windows cross-target skipped
  locally — no mingw-gcc/lib.exe; CI covers it). Windows cross-lint risk: no `cfg(windows)` arm
  touched, but the new integration test file uses `#[cfg_attr(test, allow(unwrap/expect))]` like
  its siblings. T3 unpublished pending owner push/PR decision; T4/T5 remain.
- L15 (completion 407 closed, 2026-11-17): owner asked to close the S8 gap before publishing.
  `OpenAiCompatibleProvider` now carries proxy-auth provenance (no secret) via
  `with_http_and_proxy_auth`; its `LlmPort::send_completion` maps the raw terminal
  `ScraperError::Http{407}` to a Spanish `ScraperError::Extraction` that distinguishes a
  configured `proxy.auth` credential from an anonymous proxy, with zero retries and no
  credential in logs. Four integration tests in `ai_proxy_407_test.rs` (2 embedding + 2
  completion) pass, each asserting `received_requests() == 1`. Doc pin for `provider.rs::new`
  refreshed (124→127) in the same commit. Full gate re-run: GREEN PASS=45 FAIL=0 SKIP=2.
  This completes S8's 407 contract on both AI ports.
- L16 (T4 delivered, 2026-11-17): branch `feat/ai-proxy-diagnostics` from main `d4825976`
  (PR #2897 squash). `blocks.rs` now renders `proxy_posture_lines(provider, registry)` as a pure
  function: the two #2500 direct-egress lines stay verbatim when no proxy is configured; with one
  it prints the sanitized origin, provenance (`[[proxies]]` id vs inline), proxy-dial policy
  counts, auth-source label (`AuthSource::label()`, never the user/secret) and the DNS-delegation
  notice. Wired into all 3 `doctor.rs` sites and into `show.rs` (the ADR names both). The shared
  builder emits `proxy_enabled` / `proxy_origin` / `proxy_id` structured fields on both egress
  paths. `docs/src/data-flow.md` + `installation.md` updated with the opt-in proxy declaration;
  the data-flow claims manifest gained 3 `contains` and 3 `code` claims and is green (13/11).
  `ai-providers-design.md` doctor pins refreshed 59→64 / 72→79. Two new unit tests in `blocks.rs`
  pin the legacy wording and the no-secret guarantee. Gate GREEN PASS=45 FAIL=0 SKIP=2; rustdoc
  -D warnings OK. Tree-derived golden set refreshed in the same commit. Unpublished pending
  owner decision.
