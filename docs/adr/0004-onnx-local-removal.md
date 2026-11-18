# ADR 0004: Retire local ONNX; remote embeddings as the only AI path

- **Status:** Accepted — implemented and shipped. The removal was merged in #2072 (`e9b7217a`) and released as **v3.0.0** (tag over `63d912ae`, 2026-10-20). The *Addendum 2026-10-19* at the end records outcome 6a, the threshold decision and the shim follow-up.
- **Date:** 2026-10-08 (revised 2026-10-09: step 6 arbitration mechanism and Q6 resolution — mechanism precision, decision (a) with shelter (c) does not change; revised 2026-10-11: factual correction — the 0.75 of Tier 2 is two numbers with opposite roles — and record of condition 2 resolved as migrate, #1975; revised 2026-10-16: the breaking marker moves from step 5 to step 6 — read *Amendment 2026-10-16* under *Compatibility* —, `--clean-ai` is bound to `--embedding-provider`, the recovery tag and the objection channel become concrete, #1998)
- **Deciders:** Project owner
- **Supersedes:** ADR 0003 §5 ("Local ONNX goes dormant, not deleted")
- **Scope of the supersession:** §5, plus the claims in the 0003 §Context that became obsolete: "Port defined, never injected" has been false since `--extract-with-llm` injects a real `dyn LlmPort` into the Container (`webfang_cli/src/main.rs:721-759`, wired in #1493). It is not `resolve_remote_embedding` (`:687`): that function returns the `RemoteEmbeddingAdapter` from the embeddings slot, not an `LlmPort`. The rest (§1–4, 6, 7, trade-offs, alternatives) remains in force.
- **Related issues:** #1915 (decision), #1917 (first review, closed by #1918), #1919 (re-scope), #1930 (arbitration mechanism review + Q6), #1975 (0.75 Tier 2 correction + condition 2 resolved), #1997 (step 5 objection channel, open), #1998 (breaking marker amendment), #1999 (run-time notice + threshold notice + migration guide)
- **Working papers:** AI-inference audit outside the repo (5 passes, HEAD `3ac5599`, 47 FIN findings); the key numbers go inline below so this ADR stands on its own.

## Context

ADR 0003 left ONNX dormant behind `--features ai` as an offline fallback. Dormancy is the worst of the three options —**delete, sleep, isolate**—: the tree is paid for in CI (7 `--all-features` invocations in 5 jobs —clippy:464, build+test:568,569, doc:870,874, mcp:1159, coverage:1496—; a virgin `check -p webfang_ai --features ai` = 1m44s + 1.3 GB on a local machine, without tests: a ceiling, not a figure), `hf-hub` is outside `[features] ai` (`crates/webfang_ai/Cargo.toml:25` vs `:55`) so **every** build that includes `webfang_ai` drags in `reqwest` 0.12.28 (verified: `cargo tree -p webfang_ai` without features) —an unconditional violation of the `wreq`-only policy—, 390 MB of models, and code that rots without an owner. Baseline note: the audit ran on `3ac5599`; this ADR is merged over `main` at the time, so the distance grows with every commit — what fixes the scope is the audit SHA, not how many commits are missing.

**Premise correction (review #1919): the remote path already exists and is independent of ONNX.** `RemoteEmbeddingAdapter` implements `EmbeddingPort` (`remote_embedding.rs:454`, with `embed_batch`, tolerant `usage: Option` at `:73` and bounded retry), is resolved with `--embedding-provider` / `WEBFANG_EMBEDDING_PROVIDER` (`options_spec/llm.rs:66-79`) and `--extract-with-llm` injects the port into the Container (`main.rs:721-759`) without an `ai` gate. The real gap is much smaller than described before: **remote embeddings for the cleaner** —`SemanticCleanerImpl` is built from `InferenceEngine` and erased to `dyn InferenceEngine` (`main.rs:660-676`), still ONNX-only—. This ADR is re-scoped accordingly: it is not "how we get to remote" but **what gets deleted, what breaks and how it is deprecated**.

## Decision

**Delete ONNX from the workspace**, with external shelter only with an owner and an abandonment criterion (see below). Mandatory sequence (the deletion is the last step):

1. MVP PRs 1–8 + **real endpoint config** (the env vars of ADR 0003 §1 do not exist in code —0 hits outside the ADR itself—; the real config is `ProvidersConfig` + `AuthSource`: make the documented path exist before simplifying it) + **B2** (CIDR allowlist for LAN — **Q6 resolved, option (b)**: `NetworkPolicy{mode, allow_cidrs, allow_hosts}` per provider, with link-local/unspecified/multicast/broadcast/reserved **always** blocked even when in the allowlist (FIN-017: without this a naive allowlist opens `169.254.169.254`), and `100.64.0.0/10` blocked by default with explicit opt-in; the first delivery is B1, `AuthSource::None`, which is independent of Q6) + **C1–C3** (`--max-chars`, this ADR, regenerated docs with the explicit list of Affected surface).
2. **Tranche D**: rewrite `SemanticCleanerImpl` over the existing `EmbeddingPort` (the seam exists; what is pending is the cleaner, still generic over `InferenceEngine` at `:215`) + migration + batch/bisection + cache + opt-in relative. The `chars_per_token`/`matryoshka` of the profiles are consumed by steps 2–3, so the profile schema version is pinned here.
3. Tier 2 with an explicit destination (verified: reimplementable over the endpoint, it only consumes text→vectors; threshold 0.75 to be recalibrated per model).
4. Own tranche **easy configuration**: self profiles + `connect`/`doctor`/`show` + manual adjustment (or a sibling ADR if it grows).
5. **Deprecation release** (see Compatibility): announces, does not remove capability —the shims keep working—; with the git recovery tag created before publishing it, already done: **`onnx-tree/pre-step5`** (commit `38ab6a31`, see *Reversibility*). It is a **minor without a breaking marker**: the announcement is made by the run-time notice and the release notes, not by semver (*Amendment 2026-10-16*). The objection channel the notice names is **#1997**. This step does not improve `main`, it prepares it: it is declared as such explicitly.
6. Outcome per the *Review condition* (below), with distinct criteria per branch: **6a full deletion** (if the technical criterion is met and the objection signal records no active dependency) or **6b external crate** (if there are claims or real demand: with a named owner, minimal CI and an abandonment criterion —no activity or releases in 6 months means archived—).

## Owner conditions (blocking step 6)

1. **M2 really works**: add the `AuthSource::None` variant (today only Keyring/EncryptedFile/Env, `auth_source.rs:55-70`); loopback is already allowed (`is_permitted_loopback`, `ssrf_guard.rs:231`); B2 adds the CIDR allowlist, not the loopback. Gate: the **technical criterion** of the *Review condition* —the *M2 acceptance criteria* (below) plus the PR-8 wiremock suite and the 401 test with and without credentials— and a **reproducible record** archived in the PR that enables step 6 (command + `--trace-file` + summary with backend/model/dim) run against real Ollama and vLLM. "Manually tested" without a record does not count; the record is re-runnable by anyone. The `usage` reported by Ollama and vLLM remains unverified and is **optional, never blocking**. LM Studio: best effort, no guarantee.
2. **Explicit Tier 2**: migrate over the endpoint or document the removal. Never in silence. **Resolved 2026-10-11: migrate over the endpoint**, with the complete egress package (§Tier 2 Egress) —documented removal is discarded because it silently degrades `--adaptive-selectors` to Tier 1, exactly what this condition prohibits.
3. **Loss of zero-config full offline is accepted**: M3 covers no-inference; `--clean-ai` survives (ungated, see Compatibility) but will require the user's endpoint. It is declared in the README.
4. **Rewrite**: `SemanticCleanerImpl` (1096 LOC) over `EmbeddingPort`; `--max-tokens → --max-chars` with `chars_per_token` per profile (default 3: better than 4, insufficient for CJK —the real fallback is the degraded path on 400/413, never the estimate); 400/413 degrades with splitting/bisection.
5. **Published deprecation**: the step 5 release came at least one minor before the deletion.

## Compatibility (announced break)

Surface, all verified under the `ai` gate today (`options_spec/ai.rs:77,102,127`):

- **The `ai` feature disappears without a shim.** Cargo features resolve at compile time: `cargo build --features ai` will fail at resolution, so here there are only release notes.
- **Flags that survive, ungated.** `--clean-ai` (changes semantics: ONNX → remote endpoint; it is the cleaner flag of step 2, **not** a shim) and `--offline`. For them to live, the `ai` gate must be removed and they must move to the default build: a shim behind the feature being deleted does not exist, in any language.
- **Which knob resolves `--clean-ai` after the deletion: `--embedding-provider`** (env `WEBFANG_EMBEDDING_PROVIDER`, flag > env). Decided 2026-10-16. It is the same knob that already fills the vault slot and that **today resolves without the `ai` feature** (`main.rs:980-987`), so the path already exists and there is no need to invent one. It is not `--tier2-embedding-provider`: the §Tier 2 Egress prohibits an embeddings slot from inheriting another slot's resolution, and the inspector's is explicit by design and requires its own `--tier2-consent`. It is not a new flag: it would be one more shim to deprecate inside the same window in which five are already being retired. **Gap today:** `build_ai_cleaner` (`main.rs:873-978`) lives under `#[cfg(feature = "ai")]` and resolves only the local ONNX port, so this knob is the specification of step 6, not an existing capability.
- **Shims** (useful migration error, ≥2 minor releases, also ungated): `--ai-model`, `--max-tokens` —which is also renamed to `--max-chars`, condition 4—, and the envs `WEBFANG_MAX_TOKENS`, `AI_MODEL_ID`/`WEBFANG_AI_MODEL_ID`, `WEBFANG_AI_ENGINE`.
- **Precedence the shims replace**, so the migration message is exact: `WEBFANG_AI_MODEL_ID` > legacy `AI_MODEL_ID` (`webfang_ai/.../compat.rs:34-38`); the CLI flag wins over the env (`webfang_cli/src/main.rs:602`).

`--offline` keeps its semantics (loopback allowed, public remote → exit 78). A **minor** release announces all of this beforehand (run-time notice + release notes —work PRs do not touch `CHANGELOG.md`, release-plz writes it—); the deletion arrives two minors later, and it is **that** commit that carries the breaking marker. Never a generic `ConfigError` nor silence.

**Correction 2026-10-21 (#2166): `--offline` rejects ALL embeddings providers, loopback included.** The previous paragraph says that "`--offline` keeps its semantics (loopback allowed, public remote → exit 78)"; that sentence records a promise the implementation **never** kept. The `--offline` guard (the `offline` branch of `build_embedding_provider`, `crates/webfang_core/src/cli/llm_wire.rs`) fires **before** inspecting the endpoint, so it rejects any resolved provider —a public remote one or one on `127.0.0.1`— with exit 78 (`EXIT_CONFIG`); the loopback exception that step 5 announced referred to the local ONNX model, which step 6 removed, and without that path `--offline` is only good for runs without semantic cleaning. **Owner decision (2026-10-21): the guard does not change; the published texts are corrected** —the per-run notice, the guard message, the `allow_loopback` line of `webfang doctor` and the migration guide—, and a test is added that pins the loopback + `--offline` → exit 78 without a dial. The original decision is **not** rewritten: the paragraph above stays as the record of what was announced at the time, and this correction is the one that starts telling the truth.

### Amendment 2026-10-16 — the breaking marker belongs to step 6 (#1998)

The previous wording of this section asked for the `BREAKING CHANGE:` trailer in the commit of the prior release, "so that both the version bump and the ⚠️ section are generated". In this repo the breaking marker —title `!` or `BREAKING CHANGE:` footer— **triggers the bump**, so read literally step 5 came out as MAJOR (3.0.0) and not as a minor. That contradicted the owner's condition 5 ("the step 5 release came at least one minor before the deletion") and *Reversibility*, which assumes shims with a ≥2 minor release window.

**Decision:** step 5 is a minor **without** a marker; the marker travels in the step 6 commit, which is where the change is real and where the ⚠️ section belongs. The cost is that the ⚠️ section of the CHANGELOG appears one release after the first mention, not in it.

**Caveat, and it is not cosmetic:** the bump rule comes from the policy documented in `AGENTS.md`, not from a measurement in this repo —since `v2.0.0` there is no breaking commit in the history (`git log v2.0.0..HEAD --grep='!:' -E` returns nothing), so the rule never ran here. Verifying it is a step of the deprecation release: if cutting the minor turns out to be a MAJOR, this amendment falls and the notice is rewritten as a 3.0.0 announcement.

## Orphaned default threshold

The 0.3 is calibrated for Granite and lives in four places (`options_spec/ai.rs:37`, `threshold_config.rs:49`, `semantic_cleaner_impl.rs:124`, `relevance_scorer.rs:333`); with the only calibrated model retired, any remote retains differently with the same number. Mandatory mitigation with an **exit criterion**: while the threshold is the default and the model is not calibrated, notice once per run + `threshold_used` in the summary; the mitigation ends at the announced flip with a cap of 2 minor releases —if there is no golden set by then, the owner decides (documented permanent notice or keeping 0.3 with signed risk) and records it here. Without a dated exit, the orphan becomes permanent.

**Correction 2026-10-21 (#2167): the 0.3 no longer lives in four places.** The previous paragraph enumerates them (`options_spec/ai.rs:37`, `threshold_config.rs:49`, `semantic_cleaner_impl.rs:124`, `relevance_scorer.rs:333`): three of those files belong to the `webfang_ai` crate, which this same ADR deleted in step 6, so the list describes a tree that no longer exists —and the surviving line (`options_spec/ai.rs`) is no longer the one with the default. The **four real places** of the default are `domain/options_spec/ai.rs:47` (`DEFAULT_THRESHOLD`, the single source of the value), `domain/options_spec/ai.rs:76` (the spec default, which is a string), `config.rs:59` and `application/crawl_options.rs:37` (the `Config` and `AiConfig` defaults).

The reasoning of this section does not change because of that, and it is what sustains it: what matters is that the default is **not calibrated** for any living provider, and that remains true. What changes is where the number lives. The original decision is **not** rewritten: the paragraph above stays as the record of what was written at the time.

**Correction 2026-10-11 (#1975): the 0.75 of Tier 2 is two numbers, not one.** The previous text said that "it is not similarity but `ErrorHintConfig::semantic_threshold`" — incomplete. (1) `ErrorHintConfig::semantic_threshold = 0.75` (`extraction_quality.rs:29`, next to `lexical_escalation_lower_bound: 0.70`) gates nothing: its only consumer is `semantic_drift_factor` (`structural_score.rs:56-64`), which uses it as the normalization denominator `1 − (0.75 − t)/0.75` and returns a neutral 1.0 when Tier 2 did not run. (2) `AdaptiveSelectorOptions::semantic_threshold = 0.75` (`adaptive_engine/mod.rs:99`) is the actual similarity gate: it decides Repaired/Degraded (`:532`) and is passed as the inspector's threshold (`webfang_cli/src/main.rs:588`), which also filters by it. The "threshold 0.75 to recalibrate per model" of step 3 refers to (2); recalibrating (1) is decoupling a hint denominator — a different change.

## Review condition

The decision between full deletion (6a) and shelter (6b) is arbitrated by **two verifiable signals**, without depending on external discovery processes or telemetry.

**1. Technical criterion (gate of step 6).** Evaluated when step 5 is reached:

- The *M2 acceptance criteria* (below) pass green against **real Ollama and vLLM**, not just wiremock, in at least one documented environment.
- The PR-8 wiremock suite (401 —with and without credentials—, 429 with `Retry-After` in seconds and as a date, 5xx, timeout, malformed response, batch order, body read error, DNS) is green.
- On 429, the corrected rule: `Retry-After` is honored in **both** forms allowed by RFC 9110 §10.2.3 — `delay-seconds` and an HTTP date. Previously the HTTP date was silently discarded and fell back to exponential backoff, so the client retried earlier than the server asked; today the conversion is a pure function with `now` as a parameter, and a header that is present but unusable emits a structured `warn!` instead of degrading without a trace.
- The B2 seam between the name lookup and the policy verdict is covered **with a deterministic resolver**: `ValidatingResolver` receives the lookup through a seam (`NameSource`, production = `GaiNameSource`, the same `getaddrinfo` that was previously called inline), so `Resolve::resolve` is exercised end to end with a fixed answer set —accepts a declared host on an allowlisted network, rejects the undeclared one with the `allow_hosts` class, poisons the mixed set with the SSRF class, skips the scan with validation disarmed and propagates a lookup failure without changing it—. Before this, both halves were tested separately (`fail_closed_scan` with synthetic answer sets, and real `resolve` only with literals or with `localhost` denied), so a failure in how `gai_lookup` joined them would not have been observable. A real LAN resolution is not reproducible in CI (it depends on the machine's resolver and routing), hence the seam instead of a network E2E.

If both are met, step 6 may proceed, subject to signal 2. If either fails, a minor release is postponed and the gap is recorded in this ADR.

**2. Objection signal (between step 5 and step 6).** It is passive: it collects no data and opens no egress.

- The deprecation release publishes the notice (in the run and in the CHANGELOG, see Compatibility) pointing to a pinned issue or discussion. **Pinned 2026-10-16: #1997** (`chore: canal de objeción — migración desde ONNX local`). It is an issue and not a discussion because the repo has Discussions disabled (`has_discussions: false`), and it **cannot** be the issue that the notice PR closes: `pr-validation.yml` requires every PR to close an issue, and GitHub would also close the objection channel on merge. The notice PR references #1997 with `Refs`, and closes its own work issue (#1999).
- After **2 minor releases**: if no user claims ONNX dependency, step 6 proceeds as full deletion; if there are claims, the crate moves to shelter (c) "unsupported" and the cases are recorded here.
- **Accepted limitation:** the absence of claims **does not prove** absence of use —some users do not read notices—. That is why the deletion is reversible from the step 5 git tag (see *Reversibility*), and that asymmetry is a risk this ADR declares, not hides.

**Interviews:** they are **non-blocking input**. If they exist when step 5 is reached, they inform the decision; if not, the owner decides with the two previous signals and records it here. This section **supersedes** the line of the `10_DECISION_OWNER_Q1.md` document that declared them arbiters between (a) and (c): that file is gitignored and is a historical record of a decision at the time, so it is not edited —the correction stays in the normative document, which is where whoever reads later will look.

**Future measurement:** if real usage is ever to be measured, it requires its own ADR —unit of measurement per user or per single run, declared destination and consent, per M4—. This ADR instruments nothing.

**Maintenance exception:** if the signals show that M2-endpoint is not a viable path **and** there is real demand for ONNX, the owner may assign an owner to the external crate and records it here. It is an explicit exception to "unsupported".

## Model profiles

models.dev is not enough (no dim/batch/prefixes; `limit.output` ≠ dimension —mistral 3072 vs 1024 real—; no Granite-embedding; no local runtimes). Own versioned profiles: `id → {schema_version, dim, batch_max, query_prefix, passage_prefix, context, chars_per_token, matryoshka: bool}`, living in the repo next to `ProvidersConfig` (defaults) with overrides in the user's config file; the same PR that touches the schema migrates the existing profiles. models.dev only refreshes pricing (MIT license, with notice). Defensive rules: `dimensions` **omitted by default** (non-Matryoshka vLLM rejects it with 400), `usage` always optional. `NetworkPolicy`/`allow_cidrs` do not exist as a type (0 hits): B2 creates them.

## Tier 2 Egress (new path, explicit)

The inspector goes from local to sending DOM fragments to a third party: it enters the consent by provider+endpoint hash and the spend counter. The Tier 2 slot is explicit by design: without `--tier2-embedding-provider` the local pool is used, and it **never** adopts by default the vault's embeddings provider —the default resolution of the vault slot is not inherited here (#1981, #1977). Observability of the jump (iron rule: without a trace it is not done): the spans already exist —`remote_embedding.rs:456-460` in `embed` (`provider_id`/`model`/`dim`) and `:482-487` in `embed_batch` (the three plus `count`=texts)— and they must be extended with a propagated `CorrelationId` + a per-run summary with tokens/retries/degraded.

## Vault migration

Confirmed: there is no `model_id` in the schema or DTOs (0 hits in infrastructure+application); if the new profile also gives 384, mixing with Granite vectors is undetectable. The fail-closed extended to `model_id` (`vault_search.rs:130-148`, current message preserved) **cannot fire** on the vaults that never had a `model_id` —which are exactly the affected ones—; opening them is allowed with an explicit notice ("legacy", reindexing recommended), accepting the indeterminacy. The only thing that really closes it is **re-embedding**, whose cost with a BYO key is paid by the user: it goes to Consequences, it is not hidden.

## Affected surface (explicit deliverable of C1–C3, not "regenerated docs")

- Feature gate: `cli/preflight.rs:1404` (`check_clean_ai_feature`) and `:1410` (`check_clean_ai_feature_with`) disappear with the feature, along with their tests.
- Tests: `crates/webfang_ai/tests/` (9 targets, most ort-dependent: `ai_integration`, `batched_inference_smoke`, `session_pool_sizing`, `semantic_cleaner_pipeline`, …) are removed with the feature; `crates/webfang_core/tests/adaptive_selectors_gate_test.rs` only mentions the gate in a doc comment (`:12`), so that line is adjusted instead of rewriting the file; the insta snapshots of `--clean-ai` change and are reviewed in the same PR.
- Docs and policy: `AGENTS.md` (matrix with `webfang_ai`, `AI feature` section, `hf-hub`, `ort`), `scripts/check_dependency_direction.sh`, `docs/test-inventory.md`, `COMPATIBILITY-MATRIX.md`, `README.md` (model sections, `ai` feature, cache and crate).
- Observability: remote jump fields (above) + per-run summary.

## Consequences

- Positive: the ONNX tree closes in CI; `wreq`-only in the default build —with a caveat: `--all-features` keeps `reqwest` via `chromiumoxide` (`Cargo.toml:158`, `chromium` feature in `core/Cargo.toml:33`), so the "real" claim holds for default, not for all-features—; M2 as the main path when the technical criterion is met and the objection signal records no active dependency.
- Negative: full offline with inference is lost; cost per use and **vault re-embedding cost** pass to the user (BYO key); network latency in the hot path; **the objection signal may underestimate real usage** —accepted risk, mitigated by reversibility from the step 5 git tag—.
- Neutral: M3 exists and is solid; the deletion, if nobody uses the external crate, is trivial.

## M2 acceptance criteria (condition 1, verifiable)

- Ollama `127.0.0.1:11434` without a key: `--clean-ai` works, 0 invented `Authorization`.
- vLLM on a LAN with `allow_cidrs` (B2): works; `100.64.0.0/10` rejected by default.
- `--offline` + remote: exit 78 —already implemented (`EXIT_CONFIG`, `cli/error.rs:36`; `llm_wire.rs:141-147`): it is a regression guard, not a new criterion.
- Reproducible record against real Ollama and vLLM archived before step 6.
- PR-8 wiremock suite green **plus the 401 test with and without credentials**: with `AuthSource::None`, a 401 from a provider without credentials must be distinguishable from a 401 due to invalid credentials. That test does not exist yet —it is a B1 deliverable, not a pre-existing criterion—.
- **Amendment on "stream truncation"** (see *Review condition* §1): the scenario, as it was enumerated, is not expressible in the code and therefore cannot be covered. The adapter has no streaming read path —it uses `read_body_capped`, a read capped at 16 MiB—, so there is no point where a stream could be truncated midway, and wiremock offers no primitive to abort a body in progress. The verifiable criterion that replaces it is the **body read error mapping**: a read that fails is translated to `SemanticError::Inference` with the Spanish message naming the endpoint, and is never silence nor `panic`.

## Reversibility

If the deletion fails, the ONNX tree is recovered from `onnx-tree/pre-step5` — a **non-`v*`** tag, created on 2026-10-16 before publishing the deprecation release, pointing to commit `38ab6a31` (one commit after `v2.11.0`), which is the last state of `main` with the complete ONNX implementation. The non-`v*` name is deliberate: `release.yml:34-35` triggers on `tags: v*` and `RELEASE_TAG_NAME_RE` (`scripts/release-tag-trust.sh:50`) is `^v[0-9]+\.[0-9]+\.[0-9]+(-rc\.[0-9]+)?$`, so that tag is inert for the release pipeline. Step 5 does not touch ONNX code, so pointing to the pre-release commit loses nothing. Caveat: it is not drop-in —if steps 1–4 touched `webfang_ai`, the tag returns old code that may not fit the tree; the tag is a reference point for manual recovery, not an automatic restore.

## Alternatives rejected

1. **Keep it dormant (ADR 0003 §5).** Costs CI, unconditionally drags in `reqwest` and rots ownerless code in exchange for a fallback nobody tests. Rejected.
2. **Isolate without deleting (crate in the workspace).** Maintains upkeep and the feature matrix for nothing once M2 works. Only acceptable as external shelter with an owner and a defined abandonment. Rejected as a final state.
3. **Rewrite `webfang_ai`.** The crate is correct; it is misplaced as the default. Rejected (precedent ADR 0003).

## References

- `crates/webfang_core/src/domain/embedding_port.rs:40` — the seam exists
- `crates/webfang_core/src/infrastructure/llm/remote_embedding.rs:454,73` — remote adapter + optional `usage`
- `crates/webfang_core/src/domain/options_spec/llm.rs:66-79` — `--embedding-provider` exists
- `crates/webfang_cli/src/main.rs:721-759,660-676,687` — injection without the `ai` gate; cleaner still ONNX-only
- `crates/webfang_ai/src/infrastructure_ai/semantic_cleaner_impl.rs:215` — pending rewrite
- `crates/webfang_ai/Cargo.toml:25,55` — `hf-hub` outside `[features] ai`
- `Cargo.toml:158`, `core/Cargo.toml:33` — `chromiumoxide` also drags in `reqwest`
- `crates/webfang_core/src/domain/auth_source.rs:55-70` — no `None` variant
- `crates/webfang_core/src/domain/ssrf_guard.rs:231` — loopback already allowed
- `crates/webfang_core/src/cli/error.rs:36`, `cli/llm_wire.rs:141-147` — `--offline` already implemented
- `crates/webfang_core/src/domain/extraction_quality.rs:29` — the hint's 0.75: drift factor denominator, does not gate
- `crates/webfang_core/src/application/adaptive_engine/mod.rs:99,532` + `crates/webfang_cli/src/main.rs:588` — the real 0.75 similarity gate for Tier 2 (Repaired/Degraded + inspector threshold)
- `crates/webfang_core/src/application/structural_score.rs:56-64` — the only consumer of the hint's 0.75 (normalization)
- `crates/webfang_core/src/application/vault_search.rs:130-148` — fail-closed by dimension
- `docs/adr/0003-remote-inference-adapter.md:35` — declared env vars, never implemented
- `crates/webfang_core/src/cli/preflight.rs:1404,1410` — the feature gate that disappears
- `crates/webfang_core/src/domain/options_spec/ai.rs:77,102,127` — the three flags under the `ai` gate (and their envs: `WEBFANG_MAX_TOKENS`, `WEBFANG_OFFLINE`)
- `crates/webfang_ai/src/infrastructure_ai/relevance_scorer.rs:333` — fourth place of the 0.3
- `crates/webfang_core/src/infrastructure/llm/remote_embedding.rs:456-460,482-487` — remote jump spans
- `crates/webfang_cli/src/main.rs:602` — `--ai-model` precedence over env
- Issues: #1915 (decision), #1917 (first review, closed by #1918), #1919 (re-scope)

**Correction 2026-10-21 (#2167, #2168): some references no longer resolve.** This list is the inventory of the evidence the decision was made on, and it stays as it was; what is corrected is its current state. The pointers to the `webfang_ai` crate (`semantic_cleaner_impl.rs`, `relevance_scorer.rs`, `Cargo.toml`) point to a tree this ADR deleted in step 6: they no longer resolve. Two of the live references moved due to the slices that closed this ADR —the lines of `options_spec/ai.rs` and of `cli/llm_wire.rs`— and their line citations became stale; the current locations of the 0.3 sites are in the correction of the *Orphaned default threshold* section, and the current semantics of `--offline` are in the *Compatibility* section and its dated correction (#2166).

## Addendum 2026-10-19 — step 6 closure

### Outcome: 6a, full deletion

The *Review condition* arbitrates 6a (full deletion) versus 6b (external shelter) by signal 2. #1997 was open since 2026-10-16 and when the deletion was executed it had **0 comments**: there is no claim or declared ONNX dependency. **6a** was executed in #2072 (`e9b7217a`).

The limitation this ADR already declared remains in force and is not rewritten: the absence of claims **does not prove** absence of use. The mitigation is the same one the ADR fixed, the `onnx-tree/pre-step5` tag, and not a guarantee.

### Orphaned default threshold: documented permanent notice

The announced flip happened and there is no golden set. Of the two exits this section leaves to the owner, the first is taken: **documented permanent notice**.

- The per-run notice and `threshold_used` in the summary keep being emitted; the value does not move (0.3).
- The risk —a remote endpoint retains differently with the same number— is declared in the migration guide, not only in this ADR.
- **Dated exit:** when cutting **3.2.0**, the same release that retires the shims, the owner reviews the decision. If by then there is a golden set, it is recalibrated; if not, the notice is renewed for another window or the risk of keeping 0.3 is signed. A single review point for the two open windows, instead of two.

**Correction 2026-10-21 (#2167, #2168): the 3.2.0 anchor became unreachable and the review becomes condition-based.** `.github/support.json` declares 4.0 STABLE · 3.0 MAINTENANCE · 2.13 EOL, so no future release can cut a 3.2.0: the review date just written cannot arrive, and with it the "dated exit" that this section required falls —the orphan would become permanent through the very mechanism this ADR prohibits.

The underlying decision **does not change**: it remains **documented permanent notice**. The per-run notice and `threshold_used` in the summary are emitted, the value stays at 0.3, and its risk —a remote endpoint retains differently with the same number— is declared in the migration guide, not only in this ADR. What is corrected is the **trigger** of the review: it becomes **condition-based**, with the condition the ADR already fixed —**when a golden set exists** for the remote provider in use, the threshold is recalibrated; while it does not exist, the permanent notice and the signed 0.3 are maintained—, and the owner may reopen it earlier if a reason appears. The original decision is **not** rewritten: the paragraph above stays as the record of what was announced at the time, and this correction is the one that starts telling the truth.

Without that review the orphan becomes permanent, which is exactly what this ADR prohibits.

### Follow-up: the shim window does not close with this ADR

This ADR closes the **deletion**. The shims (`--ai-model`, `--max-tokens`, `WEBFANG_MAX_TOKENS`, `AI_MODEL_ID` / `WEBFANG_AI_MODEL_ID`, `WEBFANG_AI_ENGINE`) remain alive and their retirement is its own slice, with its breaking marker and its issue: **#2119** (2026-10-20).

The anchor is **3.2.0**. The window of the *Compatibility* section asked for "≥2 minor releases" from the v2.12.0 announcement: 2.13.0 (minor) and 3.0.0 (major, the deletion) passed. Counting them as the two releases of the window is the reading that keeps the promise made to the user without stretching it to a third release.

**Correction 2026-10-21 (#2119, #2168): the shims are retired and their anchor became unreachable.** The previous paragraph says that the shims "remain alive": their retirement is the slice **#2119** (`refactor(cli)!:`) — `--ai-model` and `--max-tokens` stop parsing (standard clap error, exit 64), their envs are ignored silently, and the `onnx_deprecation` notice that announced them goes with them. The anchor was **3.2.0** and it **never arrived**: 4.0.0 was published before that minor existed, so the retirement date was unreachable and the window closes **early**.

What is met is the promise of the *Compatibility* section, which was "≥2 minor releases" from the v2.12.0 announcement: 2.13.0 (minor) and 3.0.0 (major, the deletion) passed. The early closure does not break that promise —the count of releases that sustained it was already closed, and what is moved up is the date of an anchor the release process was never going to reach. That is why the anchor is not re-anchored to another date: a future date would repeat the same failure when that release comes early, and what is needed is a trigger that does not depend on the release calendar.

The original decision is **not** rewritten: the paragraph above stays as the record of what was announced at the time, and this correction is the one that starts telling the truth.

### Deletion residue (2026-10-20)

Step 6 closed when the engine stopped existing, not when the tree was tidy. The post-release local audit found four units of residue; one is already closed and three remain open:

- **#2104 (closed, #2108)** — `SemanticError` kept six variants without a producer (`ModelLoad`, `Download`, `CacheValidation`, `OfflineMode`, `InvalidThreshold`, `InvalidCharsPerToken`) and `Inference` rendered as "Error ejecutando inferencia ONNX". Retired.
- **#2105 (open)** — `LocalEmbeddingPort` and `Tier2EmbeddingPort::Local` are not constructed in production.
- **#2106 (open)** — `show` and `doctor` still print `granite-97m`, a retired local model.
- **#2107 (open)** — comments asserting that the Cargo `ai` feature exists.

**Correction 2026-10-21 (#2168): the state of that list changed.** #2105 and #2106 were closed (their slices landed: `Tier2EmbeddingPort` collapsed to a newtype, and `show`/`doctor` reporting the embeddings provider actually resolved). #2107 was split: half of the comments were fixed in #2122, and the part that retired the `--enable-ai` flag from the two MCP binaries is the slice #2123. The list above stays as the record of 2026-10-20, which is what it was; this correction is the one that updates it.

None of them is an acceptance criterion of this ADR, and that is why they do not block its closure. They are recorded here so that "ADR-0004 closed" is not read as "no trace of ONNX remains in the tree".

### Process note: the native review did not cover step 6

The native review was left *exhausted* over this range: `lens_context_budget_exceeded` with 20,706 and 15,927 lines against a diff of 14,534 —deleting a crate of that size overflows the lens budget. The verdict came from an independent verifier (recorded in L8 of the unit's feature doc). It is noted so that a unit of this size does not assume coverage by the native reviewer: the absence of findings there is not evidence of the absence of defects.

### Correction of Amendment 2026-10-16

The caveat of that amendment asked to verify the bump rule "when cutting the minor", because since `v2.0.0` no breaking commit had executed it. It was verified, and the result is not what the amendment foresaw:

- The step 6 commit used the title `feat!(ai):`, which is **not** Conventional Commits: the `!` goes after the scope, `feat(ai)!:`.
- With that form release-plz recognizes neither the type nor the break and computes a patch (2.13.1). Measured with `release-plz update` over the same tree: `feat(ai):` → 2.14.0; `fix(cli)!:` or a `BREAKING CHANGE:` footer → 3.0.0.
- **The amendment does not fall.** The repo rule (a breaking marker produces a major) is met; what was wrong was the form of the marker, not the rule.

Operational corollary: this repo squash-merges, so a `BREAKING CHANGE:` footer written in a branch commit is buried in the body of the squash commit. The marker that survives is the one in the **PR title**.

### Delivery

- **Implementation:** PR #2072 → `e9b7217a` (squash), 2026-10-19.
- **Release:** **v3.0.0**, tag over `63d912ae` (2026-10-20). It is 3.0.0 and not 2.14.0 because the removal is a break; the number got there through an explicit declaration commit (#2077, `chore(release)!:` merged with `--merge`), not through the deletion commit: the squash discards the `!` from the PR title and release-plz ignores the `BREAKING CHANGE:` footer for the version calculation, although git-cliff does read it to group the CHANGELOG. The failure class was recorded in #2081 and closed in #2085.
- **Support window:** rotated in #2082 — 3.0 STABLE, 2.13 MAINTENANCE, 2.12 EOL.

### Deviation: the major shipped without `release/3.0` and without an RC

This addendum tells the delivery; it must also tell what was **not** done, because the clean record above suggests a path that was not the one taken.

`AGENTS.md` asks two things when cutting a MAJOR, and they are of a different nature:

- **`release/X.Y` is a hard rule, without conditions:** "MAJOR → always cuts `release/X.Y`". **It was not cut.** It is a deviation.
- **The RC is discretionary, but this case is one of them:** "`vX.Y.0-rc.N` ... only when the maintainer declares a stabilization window (state migrations, export format, major AI (embedding-provider) changes)". The retirement of the embeddings provider is exactly one of the listed cases, and nobody declared a window. It is not a broken rule: it is a decision that was not taken.

Why it happened, in order:

1. `release-plz` was broken since #2072 —`release-plz.toml` still named the deleted crate—, so there was no possible tag until #2077.
2. The 3.0.0 number did not come from the deletion commit but from a declaration commit merged with `--merge` (#2077): the squash discards the `!` and release-plz ignores the `BREAKING CHANGE:` footer to compute the version (#2081).
3. The `support.json` rotation must happen immediately after the tag: any other PR between the tag and the rotation fails `Validate PR metadata`. Cutting `release/3.0` as a prior step required inserting a PR exactly into the window that does not admit it.

**Real consequence:** the major had no *soak*. The first published build of 3.0.0 was the final one. What was verified before announcing it: `Release-plz release` green and the 14 assets of the GitHub Release present —evidence that the release was published completely, not that the major was stable.

**What is reversible and what is not.** `release/3.0` can still be cut from the tag if a maintenance line for 3.0 is ever needed. A `v3.0.0-rc.N` **cannot** be backfilled: the soak only works before publishing.

**Root cause of point 2, closed:** #2085 makes `merge-when-green.sh` refuse to squash-merge when the title or a commit declares a break. The next major declaration does not depend on whoever merges remembering to pass `--merge`.

## Glossary of references (opaque IDs)

The work IDs (audit outside the repo, gitignored) mean: Q1(a)=delete, Q1(c)=external shelter; B2=CIDR allowlist; C1–C3=docs and `--max-chars`; FIN-024=silent Tier 2; PR-4/5/6=MVP traceability and counters; M1–M4=product models; D6a=opt-in relative threshold; Q4/Q6/Q7=questions to the owner (golden set, M2 scope, default). Everything load-bearing is inline above; the glossary is only an index.