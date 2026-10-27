# ADR 0020: Shared Transport Primitives with Port-Owned Policies for the OpenAI-Compatible Clients

- **Status:** Accepted
- **Date:** 2026-10-27
- **Deciders:** Project owner (issue #2357, `status:approved`)
- **Related issues:** #2357 (this ADR), #789 (LLM-first extraction), #1462 (remote embeddings), #703 (SSRF guard)
- **Supersedes:** —

## Context

The two OpenAI-compatible HTTP clients duplicate their transport in parallel:

| Concern | `client.rs` (`LlmPort`) | `remote_embedding.rs` (`EmbeddingPort`) |
|---|---|---|
| Body cap | 16 MiB (`LLM_MAX_BODY_BYTES`) | 16 MiB (`EMBEDDING_MAX_BODY_BYTES`) |
| SSRF wiring | `ssrf_guard().secure_client(builder)` — plain guard (`OpenAiLlmClient::new`, `build_default_http_client` in `provider.rs`) | `ssrf_guard().secure_client_with_policy(allow_loopback, network_policy)` **plus** a literal-host `entry_gate` with `network_policy.allow_cidrs` |
| Retry | **Zero retries in the transport** — one sequential attempt; errors are classified (`TransientRetriable` for 429/5xx, verified by `rate_limited_maps_to_transient_backoff` / `server_error_maps_to_transient_retriable`) and the caller regenerates records | Loop of `EMBEDDING_MAX_ATTEMPTS = 4`, exponential 1 s→10 s, `max(Retry-After, exponential)` with the double clamp (`retry_after_delay` + `RETRY_AFTER_MAX_MS` = 60 s ceiling), timeouts retried, 401 special-cased, terminal 4xx |
| Metrics | None (token counts travel inside `LlmResponse`) | `EmbeddingRunMetrics` (atomic usage counters) |
| Client builder | Chrome145 + 60 s timeout + 10 s connect | Identical literals, copy-pasted |
| File size | 502 lines | 1820 lines (~880 production, ~940 in-file tests) — violates the ~500-line rule |

One correction to the issue's table: **neither transport owns a `Mutex`.** The
`PoisonError::into_inner()` recoveries of the LLM path live in
`application/llm_extraction.rs` (test fakes), and `RemoteEmbeddingAdapter` holds
only atomics and a `OnceLock`. Poisoning recovery is therefore a repo-wide
convention, not a primitive these two files share — it drops out of scope here.

The retry divergence is **semantic, not accidental**: a completion is cheap to
regenerate at the application layer, while an embedding batch is expensive and
must be defended with its own backoff. The ADR must not "unify" the policies —
it must unify the *mechanisms* while each port keeps its policy.

The body cap is **already shared**: both files call
`domain::body_cap::read_body_capped`. That precedent matters — the codebase
already demonstrated that extracting a transport primitive into `domain/`
works without disturbing either port's error funnel.

## Decision

**Extract shared transport primitives; keep policies, wire shapes, error funnels
and metrics owned by each port.** Concretely:

1. **New shared units inside `crates/webfang_core/src/infrastructure/llm/`** —
   no new crate, no new dependency. Nothing is promoted to `domain/` beyond what
   already lives there (`body_cap`, `ssrf_guard`): `LlmPort` and `EmbeddingPort`
   are untouched and both error funnels keep converging on `ScraperError`.
2. **What becomes shared (stable, policy-free mechanisms):**
   - **HTTP client builder** — one constructor for the Chrome145 + 60 s/10 s
     builder with the SSRF guard applied, parameterized by the policy pair
     `(allow_loopback, Option<&NetworkPolicy>)`. `OpenAiLlmClient::new`,
     `build_default_http_client` and `embedding_http_client` all delegate; the
     `with_http` test hatches stay as-is.
   - **Backoff arithmetic** — `backoff_delay_ms`, `clamp_retry_after`,
     `retry_after_delay` and the `BACKOFF_*`/`RETRY_AFTER_MAX_MS` constants move
     to one unit with their unit tests. These are pure functions; both ports can
     consume them without either inheriting the other's attempt count.
   - **Retry classification** — the status/transport decision (429/5xx and
     timeouts are retriable; 401 is special; other 4xx are terminal; report the
     last observed status) becomes a shared decision function returning a
     `Retry { delay } | Terminal` style verdict. The **loop and attempt budget
     stay in each port**: the embedding adapter keeps its 4-attempt loop; the
     completion adapter keeps zero retries.
3. **What stays port-owned:** wire DTOs (`CompletionRequest` vs `EmbeddingRequest`
   — different schemas), response parsing, error mapping
   (`ScraperError::{Http,Extraction,Validation}` vs `SemanticError::Inference` /
   `ProviderInitError`), attempt counts, and Spanish user-facing messages.
4. **Metrics stay embedding-specific.** `EmbeddingRunMetrics` exists because a
   batch is a budgeted, cost-bearing operation; the completion path carries
   usage inside `LlmResponse`. Generalizing metrics now would be indirection
   without a consumer. Revisit only if a third transport consumer appears.
5. **Fetch guard chain order is preserved by construction.** The shared builder
   covers only client *construction* (stage 3a/3b/3c wiring); pacing, retry
   classification and the capped body read remain ordered per port exactly as
   the AGENTS.md chain mandates: ValidUrl → pacing → timeout → redirect → SSRF
   at the dial → retry classification → body read. A shared classification
   called *before* the capped read does not reorder anything — it is stage 4
   extracted verbatim.
6. **Migration is sequential slices, one PR each, on top of `main`** (this repo
   has no stacked PRs). Zero behavior change per slice: every existing test —
   including `timeout_is_retried_and_recovery_is_served` — stays green
   unmodified, and the retry policy of each port is already pinned by tests
   (embeddings: `status_429_then_success_recovers`, `status_500_exhausts_and_reports_last_status`,
   `status_400_is_terminal_without_retry`, `timeout_is_retried_and_recovery_is_served`;
   completions: the two classification tests cited above).

### Migration plan

| Slice | New unit | Moves | Files touched | Effect on `remote_embedding.rs` |
|---|---|---|---|---|
| 1 | `infrastructure/llm/backoff.rs` | The three backoff/Retry-After functions + constants + their unit tests | `backoff.rs` (new), `remote_embedding.rs`, `mod.rs` | ≈ −170 lines |
| 2 | `infrastructure/llm/http_client.rs` | Unified SSRF-aware builder; `provider.rs` and both constructors delegate | `http_client.rs` (new), `client.rs`, `provider.rs`, `remote_embedding.rs` | ≈ −20 lines |
| 3 | `infrastructure/llm/retry.rs` | `RetryDecision` classification; `post_embeddings` keeps its loop and calls it; wire DTOs + `parse_embedding_body` optionally move to `wire.rs` | `retry.rs` (new), `remote_embedding.rs`, `mod.rs` | ≈ −120 lines |
| 4 | — | In-file wiremock integration tests relocate to `crates/webfang_core/tests/` | `remote_embedding.rs`, new test file(s) | ≈ −940 lines → **file lands well under ~500** |

Each slice carries its own `Closes part of #2357`-style issue; this ADR's issue
closes with the decision, not with the code.

## Trade-offs

| Quality attribute | Generic transport (rejected) | Shared primitives (chosen) | Status quo (rejected) |
|---|---|---|---|
| Deduplication | Total, on paper | The 3 stable mechanisms | None |
| Typed error funnels | Eroded — one funnel must represent both or generics carry the mapping | Untouched per port | Untouched |
| Policy expression | 5 injection points (auth, parse, error-mapping, retry, metrics) for 2 call sites | Policy lives where it is read (the port) | Policies entangled with mechanisms |
| Cost to add a 3rd consumer | Low if it matches the shape | Moderate (re-use primitives, write its loop) | High (copy everything again) |
| Reviewability of each slice | One huge PR | Small mechanical PRs | n/a |

## Consequences

**Positive**
- One builder to keep Chrome145/timeouts/SSRF wiring in sync — the two 16 MiB
  caps stop being independent literals that can drift (both already delegate to
  `read_body_capped`; the constant remains per-port by design since each port
  names its own limit in its error messages).
- `remote_embedding.rs` returns under the ~500-line rule without splitting its
  domain logic, and the pure backoff functions gain a home where the completion
  path could adopt a retry policy later without new archaeology.
- Retry semantics stay test-pinned and identical to today's.

**Negative / costs**
- Three new files in `infrastructure/llm/` and a `mod.rs` update — more units to
  name and navigate.
- The deduplication is bounded: if a third consumer arrives wanting the *same*
  envelope (auth + parse + metrics + 4-attempt loop), the primitives will not
  have anticipated it and a follow-up ADR should revisit granularity then.

**Neutral**
- `domain/` gains nothing new; `ssrf_guard` and `body_cap` are already there.
- No dependency, feature-flag or profile change; no `Cargo.toml` edits.

## Alternatives rejected

1. **A generic `OpenAiTransport<P>` parameterized by auth, parser, error-mapper,
   retry policy and metrics sink.** With exactly two call sites whose divergence
   is deliberate, the parameter set *is* the duplication — relocated into type
   parameters. It also blurs two error funnels that the review rules treat
   separately (`ScraperError` funnel vs `SemanticError`/`ProviderInitError`).
   Rejected until a third consumer with a matching shape exists.
2. **Do nothing until a third consumer appears.** The duplication already costs
   real money: two SSRF wirings that must evolve together (the policy-aware
   variant exists only on the embedding side) and a 1820-line file that cannot
   be split cleanly while it carries transport + parsing + policy + metrics.
   Rejected.
3. **Big-bang unification PR.** Prohibited by the issue and by the repo's
   sequential-delivery rule; would also bury a behavior-preserving refactor
   inside one unreviewable diff. Rejected.

## References

- `crates/webfang_core/src/infrastructure/llm/client.rs` — `OpenAiLlmClient`, `LLM_MAX_BODY_BYTES`, zero-retry `send_completion`, classification tests
- `crates/webfang_core/src/infrastructure/llm/remote_embedding.rs` — `RemoteEmbeddingAdapter`, retry loop (`post_embeddings`), backoff/Retry-After functions, `EmbeddingRunMetrics`
- `crates/webfang_core/src/infrastructure/llm/provider.rs` — `build_default_http_client` (plain-guard builder to be unified)
- `crates/webfang_core/src/domain/body_cap.rs` — `read_body_capped` (the already-shared cap primitive)
- `crates/webfang_core/src/domain/ssrf_guard.rs` — `ssrf_guard`, `secure_client_with_policy`, `is_forbidden_literal_host_with_policy`
- `crates/webfang_core/src/domain/providers.rs` — `ProviderConfig` (`allow_loopback`, `network_policy`)
- `docs/adr/0001-unify-asset-downloader.md` — precedent for structural unification over parameter drilling
- `docs/adr/0003-remote-inference-adapter.md` — origin of the remote adapter and its failure-semantics reuse of `ErrorClass::TransientRetriable`
- Issue: #2357
