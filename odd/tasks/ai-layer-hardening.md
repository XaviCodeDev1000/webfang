# ai-layer-hardening

**Branch:** `feat/ai-layer-hardening` · **Worktree:** `~/Projects/Rust/webfang-worktrees/feat-ai-layer-hardening`
**Goal:** harden the AI layer (`SemanticCleaner` ↔ `EmbeddingPort` ↔ `TextChunker` ↔ `LlmExtractionService`)
so that a provider swap or a malformed response fails loudly instead of degrading silently.
Eleven work units in nine commits — T11 lands first (Commit 1) and T3–T5 share Commit 4 — English-only
artifacts. Native review applies per work-unit commit only while the user-owned RDD switch is on; a review
outcome never authorizes delivery.

**Grounding (human decision):** every spec below is either the human's verbatim requirement (S1–S8) or a
finding we verified in this session against the worktree tree (S9–S16). Repository ADRs and prose are
context, never authority: where a doc-comment contradicts the code, the code wins and the comment becomes
a task (it does, in T4 and T2). No worker re-derives scope from `docs/adr/`.

## Specs

S1 (user, verbatim) "siento que que la parte de AI en WebFang está mal diseñada o con fricciones de diseño, es una señal importante para revisar antes de presentarla"

S2 (user, verbatim) "Puertos con métodos por defecto peligrosos o asimetrías de sealing: En arquitectura de puertos/adaptadores, algunos ports están sellados para proteger invariantes (como SemanticCleaner), mientras otros quedan abiertos (TextChunker, EmbeddingPort) o exponen métodos auxiliares con comportamientos por defecto sorpresivos (como as_any haciendo panic si no se implementa). Revisar estos contratos evita fallos en tiempo de ejecución al cambiar de proveedor o backend."

S3 (user, verbatim) "Mezcla de responsabilidades entre limpieza, chunking, embeddings y orquestación: Si la limpieza semántica, la segmentación de texto, las llamadas a modelos/embeddings y la lógica de handlers MCP comparten supuestos implícitos (tamaños, formatos, errores o estado), cualquier ajuste en un proveedor rompe otros flujos."

S4 (user, verbatim) "Frontera de datos y prompt injection: En flujos que alimentan modelos o devuelven resultados a agentes, es crítico garantizar que nunca se envíe HTML crudo ni credenciales por canales no previstos, y que el contenido externo/no confiable quede tratado como datos y no como instrucciones."

S5 (user, verbatim) "Acoplamiento a proveedores o timeouts/reintentos inconsistentes: Si las llamadas a LLM o embeddings no comparten límites estrictos de tiempo, manejo uniforme de errores tipados y trazabilidad limpia (sin filtrar secretos ni payloads sensibles), la fiabilidad cae rápido bajo concurrencia."

S6 (user, verbatim) "Verificar qué ocurre ante entradas vacías/malformadas, respuestas lentas o fallos de red/API, y eliminar cualquier panic evitable en rutas de producción o adapters."

S7 (user, verbatim) "Confirmar que spans, logs, archivos de traza y errores neutralicen fragmentos no confiables y no expongan claves o contenido sensible."

S8 (user, verbatim) "lo que necesito es que me crees un plan compeleto odd en un worktree para implementar todas la mejoras y soluciones que mejoran nuesto sistema mencionadas en esta sesion"

S9 (derived, verified by parent) The relevance filter in `infrastructure/semantic_cleaner.rs` uses a private
`cosine_similarity` that `zip`s vectors of unequal length without checking, so a mismatched embedding is
truncated instead of rejected. `distances_to_centroid` derives the centroid width from the first vector
and `filter_by_relevance` validates only the count, never the dimension.

S10 (derived) One budget is measured in two units: `SemanticCleaner::max_chars` is documented as CHARACTERS
and compared against `String::len` (bytes) in `split_into_chunks` and again in
`LlmExtractionService::build_sub_chunks`, while `MarkdownChunker::chunk_text` compares `chars().count()`.
The cleaner's own test asserts the byte length as "chars", pinning the defect by test.

S11 (derived) `SemanticError::Inference` collapses 401/407/body-cap/transport into a single `String`, which
classifies `InternalFatal` in `ScraperError`; a retryable remote 5xx is therefore type-indistinguishable
from a local consistency bug, and each consumer re-flattens it independently (`elastic_ingestion` wraps it
as `ScraperError::ingestion`, `vault_search` as `ScraperError::Semantic`).

S12 (derived) `LlmExtractionService::build_sub_chunks` renders the parsed model payload into
`ScraperError::Extraction`, which the aggregate paths then log with `warn!(error = %e, ...)`. The MCP AI
handlers receive that already-mutilated string; a handler can never be better than the error it is fed.

S13 (derived) `NoteChunkVector` persists no `model_id`, and `vault_search` compares vectors against the
query embedding only. A same-width model swap is undetectable, exactly as the `EmbeddingPort::model_id`
docstring warns.

S14 (derived) Remote policy literals are mirrored, not shared: the body-cap constant is duplicated, the
backoff bounds are re-declared, and the attempt ceiling differs between the embedding path and the
downloader. The completion path has no retry at all: `llm/client.rs` maps every non-2xx to
`ScraperError::Http` and `build_sub_chunks` issues a single sequential attempt per sub-chunk.

S15 (derived) Two independent chunking implementations diverge on separator (`\n\n` vs `\n`), on
oversized-block handling (hard-split vs `ChunkTooLarge`), on the budget unit (chars vs bytes — S10), and on
empty input (`Tokenize` vs `Ok(vec![])`). `LlmExtractionService` is injected both and re-chunks what the
cleaner already chunked.

S16 (derived) `SemanticCleaner::max_chars` and `SemanticCleaner::is_ready` have no production reader; the
cleaner keeps the budget in its own field. The seal on `SemanticCleaner` states an invariant
("memory safety or caching invariants") that the contract does not contain: there is no `unsafe` and no
cache. `EmbeddingPort` is deliberately open (external adapters must be able to implement it) and that
rationale is written in its doc.

S17 (out of scope, decided against in review) No new `EmbeddingError` expansion
(`DimensionMismatch`/`NonFiniteValue`/`ZeroVector`); no `http_status()` or `retry_after` in the domain; no
jitter through an added dependency; no blue/green dual-write index migration (this is a CLI, not a live
service); no query-adaptive chunking (the pipeline has no queries at ingest); no `thiserror` migration;
no adoption of benchmark token counts (512) as a character budget — the benchmark numbers are token-based
and would import the very unit defect this mission removes.

S18 (out of scope) Re-embedding existing stores; near-duplicate dedup; renaming `odd/tasks` Spanish
prose; touching `CHANGELOG.md` (single writer is the Release PR); touching `docs/`.

## Spec ↔ task coverage

No spec is left without an owner, and no task quietly widens another. A row with no decision is an open
spec and blocks its commit.

| Spec | Owner | Note |
| --- | --- | --- |
| S1, S8 | this document | Intent: reviewing before presenting is the deliverable; no code task. |
| S2 | T3, T4 | Panicking default removed; sealing rationale made true. |
| S3 | T10 | Cross-cutting by nature; the union of T1/T2/T10 is where the shared assumptions become owned contracts. Satisfied only by that union — its own row set goes stale in each unit's commit. |
| S4 | T11 | The data/instruction boundary itself — no raw HTML and no credential on an unplanned channel, page text treated as data, never as prose. |
| S5 | T9 (primary), T6, T11 | Shared limits, uniform typed errors, clean traces; T9 completion retries remain OPEN pending provider semantics. |
| S6 | T3 (panics), T10 (empty/malformed), T1 (malformed dims), T6 (network/API), T9 (slow responses), T11 (credential leaks) | Decomposed; each owner names the failure it closes. |
| S7 | T5, T11 | Redaction; T11 pins the serialized sink output against page markers and credentials, not just field names. |
| S9 | T1, T7 | Truncated ranking; the guard that stops one bad response becoming `n` calls. |
| S10 | T2 | One budget, one unit, four consumers (cleaner, chunker, profile display, completion sub-chunks). |
| S11 | T6 | Retryability distinct from a local consistency bug. |
| S12 | T5 | The payload stops travelling in error text. |
| S13 | T8 | `model_id` persisted and checked on read. |
| S14 | T9 | Shared policy literals and a real retry on completion. |
| S15 | T10 | Two chunkers become one. |
| S16 | T4 | The seal keeps a true sentence. |
| S17, S18 | this document | Rejected and out-of-scope decisions, so no worker re-litigates them. |

## Tasks

- T11 `S4`, `S7`, `S5` route: inline — the data/instruction boundary, asserted at the two places content
  leaves the process (embedding request, completion request). RED first on the REAL path the CLI accepts:
  HTML → `extract_text_content` (`legible` primary, `strip_html_tags` fallback) → the real
  `SemanticCleanerImpl` → chunker → captured requests at BOTH model clients. Today's
  `request_bodies_never_carry_html_tags` feeds pre-cleaned chunks through `FakeCleaner`, so extending it
  with raw HTML alone would not exercise this boundary: replace that fake with the real cleaner, or add a
  sibling test that does, and never count the fake as proof. (a) no raw HTML reaches either client — a tag
  sequence from the input document is absent from the captured bodies; (b) credential channel single —
  the key appears only in `Authorization` for its intended endpoint, never in body, URL, error string or
  trace event; (c) REQUEST STRUCTURE — page text travels in a separate `user` `ChatMessage` and the system
  instruction is pinned as a constant; this proves shape, NOT that the model treats the text as inert data
  or is immune to prompt injection — no such semantic guarantee is claimed; (d) sinks — assert on the
  SERIALIZED JSONL output of `FileTraceLayer` (and every other sink that actually exists) that a known page
  marker and the credential never appear; a field-name allowlist is insufficient because VALUES may carry
  untrusted text. Verified in code: `build_request` already separates the user message and
  `EmbeddingRequest` carries text in `input`; keep that shape and add no provider-specific instruction
  channel. This task does NOT add tracing fields to `EmbeddingPort` (that would reopen T3/T4) and does NOT
  change the empty-input contract (T10). Risk: item 2 (what leaves the process, and on which channel) +
  item 6 (no test exercises (a)-(d) on the real path; the existing one uses `FakeCleaner`). Commit 1.
  **DONE** — `crates/webfang_core/tests/ai_boundary_test.rs::boundary_assertions_on_the_real_path`
  pins (a)-(d) with wiremock at BOTH clients, the real `SemanticCleanerImpl` and the serialized
  `FileTraceLayer` JSONL. RED was real: `legible` returns a readability HTML fragment, so raw
  `<div>/<p>/<span>` reached the embedding client; the `FakeCleaner` test could never see it. Fix in
  `extract_text_content`: the primary branch strips tags with `strip_tags_keep_layout` (preserves
  newlines for T10), the fallback keeps `strip_html_tags`. Verified: focused test PASS;
  `infrastructure::semantic_cleaner` + `cleaning_pipelines_regression` PASS; check/clippy/fmt/doc
  gate PASS. Risk: item 2 (what leaves the process) + item 6 (no test covered the real path) —
  independent RDD review applies to Commit 1 (`0b1de600`; native outcome recorded in L13).
- T1 `S9` route: inline — RED first: a mixed-dimension batch must error, not rank. Delete the private
  `cosine_similarity`, route `distances_to_centroid`/`filter_by_relevance` through the domain's
  `cosine_similarity_fail_closed`, and reject a dimension-disagreeing batch. Risk: item 3 (the filter
  decides which chunks survive for every caller — export, batch, MCP clean, vault — so a stricter batch
  decision changes their output) + item 6 (no existing test pins reject-vs-degrade on mixed dimensions).
  Commit 2. **DONE** — RED observed on `mixed_dimension_batch_is_rejected_not_ranked` (the mixed batch
  was ranked), then GREEN after adding the batch width check and routing `distances_to_centroid`
  through the domain `cosine_similarity_fail_closed`; the private zip-based cosine was deleted.
  Verification: full `infrastructure::semantic_cleaner` suite PASS, `ai_boundary_test` PASS.
  Native assess: `medium` / `under_budget` (no review due); Commit 2 SHA `0f2bb2aa`.
- T2 `S10` route: inline — RED first: multi-byte tests (accented Spanish, CJK, emoji) prove the budget is
  counted in characters in `split_into_chunks` and in `build_sub_chunks`. Decide `chars` as the unit of the
  contract: the accepted number changes for non-ASCII text, and that is the contract change.
  There is a THIRD budget consumer, and it is live: `ModelProfile::max_chars()` derives a character budget
  from `context` × chars-per-token and its own doc-comment calls it the per-profile input for the chunk
  budget, yet its only production call site is the display helper in `cli/commands/blocks.rs:217`; the
  cleaner is in fact built from `AiConfig::max_chars` (`cli/parse.rs:54` →
  `webfang_cli/src/main/ai.rs:94,118`), and `max_chars_explicit` already exists in `cli/args/ai.rs`. Two
  budgets, one printed and one governing: the operator reads a number that chunks nothing.
  A FOURTH consumer was missing from that accounting: `LlmExtractionService::extract` derives its sub-chunk
  budget from `LlmConfig::max_tokens * CHARS_PER_TOKEN` (the hardcoded `8` in `application/llm_extraction.rs`),
  independent of both numbers above. S17 already rejects importing token counts as a character budget, so
  this derivation is part of the same defect, not a separate concern. The tokens→chars derivation in
  `ModelProfile::max_chars()` followed by a byte comparison in the chunkers is that same unit defect one
  layer up, not a second amplification; no newtype is added — the unit is fixed at the comparison sites.
  State explicitly in the spec: `chars().count()` counts Rust Unicode SCALAR values, not visual graphemes
  and not provider tokens; the character budget is the chunker's rejection guard, not an exact guarantee
  that the input fits the provider's tokenized context.
  First step of this task is a verification, not an edit: establish whether options-spec resolution already
  derives `AiConfig::max_chars` from the profile when the flag is absent. Either outcome ends the same
  way — ONE governing character budget in `chars`, and the printed one is the governing one: the resolved
  budget derives from `ModelProfile::max_chars()`, an explicit `--max-chars` wins, and that same resolved
  number is carried into the completion path as an explicit `char_budget` field on `LlmConfig` (set by the
  composition root). `LlmConfig::max_tokens` returns to being purely the per-completion OUTPUT budget and
  `CHARS_PER_TOKEN` stops setting a chunk size. If the derivation already exists, this task degrades to the
  unit fix plus a test proving printed == splitting == sub-chunk budget. Risk: item 3 — `LlmConfig` is a
  consumed contract and the sub-chunk count changes for existing callers; the commit body states it.
  Commit 3 (unit + contract), Commit 3b (profile-derived resolution, see L14). **PART 1 DONE** —
   multibyte RED→GREEN in both chunkers, `char_budget` carried explicitly, `CHARS_PER_TOKEN` deleted;
   full semantic_cleaner and llm_extraction suites, `ai_boundary_test` and clippy PASS.
   **T2b DONE (Commit 3b `60d8e414`)** — a single `governed_max_chars` resolver in
   `domain/model_profile.rs`; the run (`__main` after config load), `show` and `doctor` all call it, so
   printed == governing == `char_budget`. Explicit `--max-chars` still wins; absent, the budget is
   `ModelProfile::max_chars()` of the default profile under `[[model_profiles]]` overrides; a bad
   override fails closed with exit 78. RED observed (four resolver tests failed to compile with no
   function), GREEN after the resolver. `LlmConfig` still has no production constructor — that seam
   is tracked separately; the contract is pinned by `completion_char_budget_is_the_governing_resolution`.
- T3 `S2` route: inline — make `as_any` a required `EmbeddingPort` method (remove the panicking default);
  every in-tree impl already overrides it, so this is compile-time enforcement of an existing practice.
  Risk: item 3 — `EmbeddingPort` is deliberately open so external adapters may implement it, and a required
  method is a source-breaking change for any impl outside this workspace. Seven in-tree impls carry it, so
  the workspace compiles unchanged; the commit body states the breaking change. Commit 4 (with T4, T5).
  **DONE** — `as_any` is required; three test-only impls that lacked it were completed (CLI
  `CountingEmbedder`, vault `StubEmbedding`, inspector `MockEmbedder`). Source-breaking for external
  adapters, stated in the commit body.
- T4 `S16` route: inline — correct the seal doc-comment so it states the invariant it actually protects
  (impl-set control over `Box<dyn SemanticCleaner>` consumers), and add the one-sentence rationale
  `EmbeddingPort` already carries to `TextChunker`. Five lines; no seal removal. Risk: none. Commit 4.
  **DONE** — seal doc now states implementor-set control over `Box<dyn SemanticCleaner>` consumers and
  says explicitly there is no `unsafe` and no cache; `TextChunker` records why it stays open.
- T5 `S7`, `S12` route: inline — stop echoing the parsed model payload into `ScraperError::Extraction`;
  carry the provider's own short message instead. Risk:   item 3 — the error text and the snapshots pinning it
  change. Not item 2: the payload is page content, not a credential; T11 owns the credential channel.
  Commit 4. **DONE** — `merge_records` no longer interpolates the parsed value into
  `ScraperError::Extraction`; `extraction_error_never_echoes_the_model_payload` pins it.
- T6 `S11` route: inline — one classification point, using the taxonomy that already exists.
  `ScraperError::classify` maps `Http` 5xx → `TransientRetriable`, `Http` 429 → `TransientBackoff`, other
  4xx → `PermanentFatal`, and `RateLimited(u64)` → `TransientBackoff`; the defect is that
  `SemanticError::Inference` is an unstructured `String`, so `ScraperError::Semantic(inner) => inner.classify()`
  sends a retryable remote 5xx to `InternalFatal`, indistinguishable from a local consistency bug.
  Convert ONCE at the adapter boundary into existing `ScraperError` variants: 5xx, 401/407 →
  `Http { status, url }`; 429 → `RateLimited(retry_after_secs)`; timeout/transport → `Network`. The body cap
  is a LOCAL read limit (`HttpError::BodyTooLarge`), never a remote 413: keep its existing local path and
  `PermanentFatal` classification; do not synthesize a server status that was never observed. Status and
  `retry_after` stay in the adapter error while it computes backoff. Do NOT add
  `RemoteTransient`/`RemotePermanent`/`LocalConsistency` variants or expand `SemanticError`: that duplicates
  `ErrorClass` and contradicts the "no over-engineered variants" decision. RED first: one test per mapping
  row asserting the resulting `ErrorClass`, plus a test pinning that a local consistency failure stays
  `InternalFatal`. Risk: item 3 (error contract consumed by the CLI exit codes) + item 6 (no existing test
  pins the retryability of an AI remote failure). Commit 5.
- T7 `S6`, `S9` route: inline — dimension guard at the embedding adapter boundary, the single funnel every
  vector passes (`embed_batch_impl` in `infrastructure/llm/remote_embedding.rs`). The width comes from the
  CONTRACT, never from the data: `EmbeddingPort::embedding_dim()` is documented as declared by the concrete
  adapter, and `RemoteEmbeddingAdapter` already has a configured `pinned_dim` plus `probe_dim()` that
  compares the pin with the served width — no `/models` call and never the first vector of a batch.
  Validate EVERY vector against that width; add a rejection counter to `EmbeddingRunMetrics`; abort the
  whole batch above an explicit ceiling. A dimension-invalid response MUST NOT come back as a generic `Err`
  that `embed_batch_raw` treats like any batch failure — `Err(_) if contents.len() > 1` currently bisects and
  would issue more remote calls. Require a terminal path matched before that arm, or validate the SUCCESSFUL
  result inside `embed_batch_raw` before the bisection branch is reachable. If `embedding_dim() == 0`
  (provider declared no width), cut BEFORE any provider request and fail closed — never adopt a measured
  width. State whether the ceiling is per batch or per run, and when the counter increments. RED first:
  mixed-dimension batch, rejection counter, abort-at-threshold, zero-width, and a test asserting the MAXIMUM
  OUTBOUND CALLS to a provider that keeps returning bad dimensions — returning an error and incrementing a
  counter alone does not prove the remote calls stopped. Risk: item 6 — no existing test covers a
  mixed-dimension batch, and a guard that rejects valid vectors fails silently on every page. Explicitly
  NOT item 1: the store is rebuildable by re-running the export, so a false rejection is recoverable rather
  than unrecoverable data. Commit 6.
- T8 `S13` route: inline — persist `model_id` with the vault store vectors and verify it on read in
  `vault_search`. The human's schema decision is CLOSED and recorded in L10: option A (NULL = unknown),
  with this spec. Schema: bump `SCHEMA_VERSION` to 3 and add `note_chunks.model_id TEXT` nullable; change
  `migrate_to_current` to apply ALL pending steps by checking each column's real existence
  (`spans_json`, then `model_id`) instead of using `PREVIOUS_SCHEMA_VERSION` as a single selector — a v1
  database gets both columns, a v2 database gets only `model_id`. Run the checks, the `ALTER`s and the
  `PRAGMA user_version` stamp inside ONE transaction, stamping the version last. Never backfill legacy
  rows (S17 excludes re-embedding). Write: `VaultSearchService::index_note` calls
  `EmbeddingPort::model_id()` once per note and passes `model_id: &str` as an argument to
  `index_note_transactional`; do NOT widen the public `ChunkToPersist` tuple (risk item 3).
  `NoteChunkVector` gains `model_id: Option<String>` (`None` = legacy unknown) and `load_all_vectors`
  selects the new column. Read policy, in this order: an EMPTY eligible set returns zero results (not a
  model mismatch); `NULL` rows are NEVER backfilled — they are included by width for compatibility and
  produce ONE aggregated actionable re-index warning; a known `model_id` equal to the active one is
  included; a known `model_id` different from it is skipped with a warning. Fail closed for a model mismatch
  ONLY when eligible known-model rows remain, ALL of them are incompatible, and NO eligible `NULL` row would
  be included by option A — unknown rows keep compatibility and win over the fail-closed arm. Existing
  dimension behaviour stays. RED first: v1→v3 and v2→v3 migration (including rollback-on-failure), an empty
  index, a NULL-only index included with the aggregated warning, a `model_id` round-trip, a same-width
  different-model mismatch with an incompatible model and no NULL rows, and a mixed NULL+known case that
  must NOT fail closed. Scope boundary (document, do not implement): the general `chunks`
  table has the same gap and is out of T8's scope. Risk: item 1 (schema/format change and migration over
  stored user data). Commit 7, or the next free slot if it lands earlier per the delivery order below —
  later task commit numbers are updated in the same commit.
- T9 `S14` route: inline — one shared `RetryPolicy`/body-cap module that reads the existing constants
  (`LLM_MAX_BODY_BYTES`, the downloader's values) instead of re-declaring them. Verify and test the timeouts
  that ALREADY exist — `CONNECT_TIMEOUT = 10 s` and `REQUEST_TIMEOUT = 60 s` in
  `infrastructure/llm/http_client.rs`, shared by both AI clients — and invent no values; this task adds no
  new total-deadline number. The embedding path's bounded attempts, backoff and `Retry-After` handling
  (seconds and HTTP-date) already exist and are pinned by tests: reuse them.
  OPEN / BLOCKED SUB-DECISION — completion-path retries: `send_completion` is a single `POST
  /chat/completions`, a non-idempotent operation. Retrying it requires the concrete provider's documented
  semantics or an idempotency mechanism, and neither is known yet. T9 therefore does NOT claim to satisfy
  the completion retry policy: that part stays EXPLICITLY OPEN and blocks the commit until the provider is
  known and a safe repeat is confirmed. The rest of T9 — shared policy/body cap and timeout verification —
  remains implementable. No new dependency. Risk: item 6 — an embedding attempt-count regression is visible
  only as latency and no test catches it. Commit 8.
- T10 `S15`, `S3` route: inline — collapse the duplicate chunking: `SemanticCleanerImpl` depends on
  `TextChunker` and `split_into_chunks` is deleted. The four S15 divergences are NOT interchangeable
  details; each is decided in the Log before the edit, with the test that pins it:
  1. **separator** — `\n` (every line) vs `\n\n` (blank-line block). Decide from the output of
     `extract_text_content`, not from `strip_html_tags`: the fallback collapses ALL whitespace with
     `split_whitespace().join(" ")` and therefore never contains `\n`, while the primary `legible` path
     returns Markdown with newlines. Use an INLINE HTML fixture in the test (no new `test/fixtures/` file);
     if a paragraph from `extract_text_content` can contain `\n`, `\n\n` is the correct unit and the current
     splitter is shredding paragraphs; if it never does, both are equivalent. Pin with that fixture, not
     with opinion, and do not import the token-based benchmark numbers as a character verdict.
  2. **oversized block** — today `semantic_cleaner.rs` errors `ChunkTooLarge` and `clean_respects_max_chars`
     (`infrastructure/semantic_cleaner.rs:499`) pins exactly that, while `MarkdownChunker::split_oversized`
     hard-splits. Default: keep strict (the error is the page-level degradation signal) and put hard-split
     behind an explicit policy value; if the consolidated chunker adopts hard-split instead, rewrite that
     test deliberately and describe it as a behaviour change, never as a refactor.
  3. **byte spans** — `split_into_chunks` spans point at the cleaned text it received; `MarkdownChunker`
     spans point at its own input and chunks join on `\n\n`. If the cleaner passes the cleaned text the
     spans stay comparable, but the joined chunk text — and therefore the offsets — change. Requirements:
     the consolidated chunker's chunk text reconstructs its source from the spans. Promote
     `assert_spans_reconstruct` out of `text_chunker`'s private `#[cfg(test)]` module into the shared test
     harness (`webfang_test_utils`) and use it on the cleaner path; porting the assertion there is allowed
     only if promotion would widen production code. Re-derive, do not assume intact, the downstream
     locators (chunk_index + char ranges, the #2429 provenance). UNVERIFIED here: the audit that
     would have said which bytes the spans point at was cancelled; verify before deciding.
  4. **empty input** — empty input is a VALUE, not a failure: `SemanticCleaner::clean("")` → `Ok(vec![])`
     and `TextChunker::chunk_text("")` → `Ok(vec![])`. Whoever cannot proceed with zero chunks owns that
     decision in its own terms; `llm_extraction` may still fail the page, but with its own reason, not the
     chunker's `Tokenize`. This row implements the pending human call recorded in L5 (parent's
     recommendation), and its commit rewrites any test that pins the old value, describing it as a
     behaviour change.
  S3 acceptance: this task also delivers the closed table of assumptions that changed owner — one row per
  item above plus the budget unit (T2) and the dimension (T1). A row with no decision is an open spec and
  blocks the commit as committed. Check `tools/intra-crate-lint` when the new edge moves. Risk: item 3 +
  item 6. Commit 9.

Delivery order is this list's order: T11 first (boundary assertions are the oracle the budget and chunking
changes must not break), then T1 and T2 one commit each, then the batched contract-hygiene commit
(T3, T4, T5), then T6 and T7, each moving a seam of the container, then T9, then T10, which closes the
feature. T8 sits outside the streak but is NOT blocked by a human decision (L10 closed it): its effective
place is anywhere after T6, in the next free commit slot, with later task numbers updated in that commit.
T9's completion-retry sub-decision remains open as recorded in its task. No work unit or commit count
changes in this revision.

## Log

- L1: Opening request of this session, verbatim: "siento que que la parte de AI en WebFang está mal
  diseñada o con fricciones de diseño, es una señal importante para revisar antes de presentarla: en
  sistemas de extracción y agentes MCP, una capa de AI mal acoplada suele generar fugas de contexto,
  comportamientos impredecibles en downcasting/puertos o contratos frágiles entre dominio, embeddings,
  limpieza semántica y handlers MCP." Closing question, verbatim: "¿Qué parte de la AI crees que esta mal
  diseñada hoy: los puertos/adaptadores (SemanticCleaner / EmbeddingPort / clientes LLM), el pipeline de
  limpieza y chunking, o los handlers MCP de AI y su manejo de errores/contexto, ...etc ?"
  (Spanish is a verbatim quotation, not new prose; see S2–S5 and S8 for the remaining quoted requirements.)
- L2: Follow-up, verbatim: "lo que necesito es que me crees un plan compeleto odd en un worktree para
  implementar todas la mejoras y soluciones que mejoran nuesto sistema mencionadas en esta sesion".
  Worktree `feat-ai-layer-hardening` created off `origin/main`; this mission document is the first write.
- L3: Bootstrap state — identity manifest written for `feat/ai-layer-hardening`, `codegraph init` done,
  `codedb` reindexed against the worktree root (head matches). The `.envrc`/`.env` half of the per-worktree
  bootstrap could not be executed by this session: the host safety policy blocks any command naming those
  paths, and conversational permission does not clear it. Exact commands handed to the human.
  (Superseded by L6.)
- L4: Source verification of the 2026 research the human supplied (via the Jina CLI). Held up: arXiv
  2605.03346, arXiv 2504.16318, agentmemory #247/#248/#256, arXiv 2605.22834, arXiv 2608.00824, RFC 3323
  (`impl_restriction`, nightly only), `C-NEWTYPE`. Did not hold up: "Qdrant PR #9058" (the real record is
  issue #9045, about empty vectors, and #2557 on async dimension validation was closed `not_planned`);
  agentmemory #1373 says the width is guessed and must be *asked from the server*, not that a guard
  without accounting is the villain; `agentmemory #455` is unverified (#469 is the verifiable one);
  "Idiomatic Rust (Manning, octubre 2026)" is Brenden Matthews, 2024. Consequence for this plan: the
  benchmark sources the human cited for collapsing the duplicate chunking recommend the strategy
  `MarkdownChunker` already implements, so T10 is justified by *deleting* the second implementation, not by
  unifying two peers — no shared conformance test is needed once only one implementation remains.
- L5: Human decided to review this plan before any code is written. Implementation is therefore held.
  Decisions still the human's, not the parent's: the empty-input contract (today `Tokenize` in
  `MarkdownChunker` vs `Ok(vec![])` in `split_into_chunks`; parent recommends `Ok(vec![])`, T10 records it
  as a behaviour change), the schema and migration shape for `model_id` in the vault store (T8), and
  whether the seal on `SemanticCleaner` may eventually be removed instead of only re-documented (parent
  recommends documentation only, T4). Characters as the single budget unit is accepted by the review and
  the unit fix proceeds on it (T2). Native review per work-unit commit depends on the user-owned
  RDD switch being on for this clone.
- L6: Bootstrap completed by the human (`.envrc` with the five exact lines, `$TREE` substituted and `$HOME`
  literal; `direnv allow`; `.env` copied). Isolation verified with `scripts/check_target_isolation.sh`:
  `reason=own-registered-target`, declared by this tree's own `.envrc`. Commit 8f646367..36219d0a lands this
  mission document. The identity gate and the review gate both pass, **but only inside a shell that has
  loaded this tree's environment**: `eval "$(direnv export bash)"` must precede every non-interactive
  commit, or the gate denies with `CARGO_TARGET_DIR` pointing at `main`. This entry does not clear any
  unit to start — the review itself happened and is recorded in L7, but the human authorization to begin
  implementation (L5) and the T8 schema decision remain open.
- L7: The human reviewed this plan against the tree (not against the document) and confirmed the forensics
  of S9, S10 and S2: the truncating `zip`, the byte budget compared against `chars().count()` in
  `MarkdownChunker`, the `panic!` default on `as_any`, and the existence of
  `cosine_similarity_fail_closed` in the domain and of `tools/intra-crate-lint`. Five defects were
  corrected in place:
  1. **S4 had no owner** — nothing in T1–T10 built the data/instruction boundary the human asked about
     (T5 removes an echo on the way out; it does not guard the way in). Added **T11** as Commit 1, holding
     the four boundary assertions ((a) no raw HTML, (b) single credential channel, (c) page text is data,
     not instruction, (d) trace fields are an allowlist). S4 is not otherwise mappable, so no other task
     claims it.
  2. **Invalid risk lines** — T5 cited "item 7" and T7 cited "item 1"; the taxonomy has six items. T5 is
     item 3 (the error text and its snapshots change; the echoed payload is page content, not a secret),
     T7 is item 6 (no existing test covers a mixed-dimension batch; not item 1, the store is rebuildable
     by re-running the export). T1 (item 3 on the filter's consumers + item 6), T3 (item 3: the port is
     deliberately open, a required method is source-breaking outside this workspace) and T9 (item 6:
     attempt-count regressions show up as latency only) were also restated.
  3. **L6 contradicted L5 and the header promised the wrong count** — "unblocks every later work unit"
     is false while T8 waits on the human schema call; now L6 says gates pass but the T8 decision and this
     review stay open. The header now reads eleven work units in nine commits, and each task names its
     commit.
  4. **T2 ignored a third budget** — `ModelProfile::max_chars()`, verified live only in the display
     helper `cli/commands/blocks.rs:217` (`profile.max_chars()`), while the cleaner receives
     `AiConfig::max_chars` (`cli/parse.rs:54` → `webfang_cli/src/main/ai.rs:94,118`) and
     `max_chars_explicit` already exists (`cli/args/ai.rs`). A unit fix that leaves two numbers with one
     printed and one governing is the same defect one layer up; T2 now carries it, with a verification
     step first (does options-spec resolution already derive the budget from the profile?). S16 records
     the confirmed dead accessors (`SemanticCleaner::max_chars`/`is_ready`) with no production reader.
  5. **T10 decided too much and too little** — it fixed the empty-input contract while silently changing
     separator, oversized-block policy and byte spans. All four divergences are now written into T10 with
     a decision, and the witness this review already established: `clean_respects_max_chars`
     (`infrastructure/semantic_cleaner.rs:499`) pins `ChunkTooLarge`, so the strict-vs-hard-split choice is
     a behaviour change that T10 must either keep strict or rewrite the test for, never describe as a
     refactor. Separator and span rows start with a verification, since the audit that would have settled
     them was cancelled and remains unverified.
- L8: Human's second instruction, verbatim: "si ajustalo". Six residuals from the re-review were corrected
  in place, all checked against the worktree before writing:
  1. T2 had missed a FOURTH budget: `LlmExtractionService::extract` derives `char_budget` from
     `LlmConfig::max_tokens * CHARS_PER_TOKEN` (hardcoded `8`), independent of `AiConfig::max_chars` and
     `ModelProfile::max_chars()`. Decision recorded in T2 and in the S10 coverage row: one governing
     character budget in `chars`, derived from the profile with explicit `--max-chars` winning, carried
     into the completion path as an explicit `char_budget` field on `LlmConfig`; `max_tokens` becomes
     output-only and `CHARS_PER_TOKEN` stops setting a chunk size. This changes a consumed contract (risk
     item 3) and is stated as such in the task.
  2. L6 said the human review was still open while L7 records it as done. L6 now distinguishes review
     done (L7) from authorization to implement (L5) and the T8 schema call, both still open.
  3. T8's commit number now floats to the next free slot if it lands before T7, with later numbers
     renumbered in the same commit, matching the delivery order.
  4. Removed the stray token "Kipling" from L7 item 1.
  5. T10 no longer claims to "reuse" a private helper: `assert_spans_reconstruct` must be promoted to
     `webfang_test_utils` before it is used on the cleaner path.
   6. T11 no longer claims no test covers (a): `request_bodies_never_carry_html_tags` is extended with
      raw-HTML input instead of duplicating it.
  Implementation is still not authorized by this entry; the T8 schema decision remains the human's.
- L9: Human's third instruction, decisive sentence verbatim: "Sí. Aplica los cuatro ajustes aceptados con
  esas correcciones y reescribe T6 para usar la taxonomía existente de `ScraperError` (conversión en el
  borde del adapter a `Http`/`RateLimited`/`Network`, sin agregar variantes nuevas)." This entry grants
  edits to this document only; the T8 schema decision stays the human's and implementation stays gated by
  L5. Corrections applied after code verification:
  1. T7 now takes the width from `EmbeddingPort::embedding_dim()` — the contract, already implemented by
     `pinned_dim` + `probe_dim()` — instead of `/models` or the first vector. Every vector is validated in
     `embed_batch_impl`, rejections are counted in `EmbeddingRunMetrics`, the batch aborts above a ceiling
     so bisection cannot amplify it, and `embedding_dim() == 0` fails closed.
  2. T2 records that the tokens→chars→bytes chain is one unit defect, not two, and adds no newtype.
     Verified: `AiConfig::default()` uses `DEFAULT_MAX_CHARS`; `max_chars_explicit` only feeds
     `subcommand_args`.
  3. T10's separator row now targets `extract_text_content` → `legible` with an inline fixture:
     `strip_html_tags` collapses all whitespace and can never carry `\n`. The four divergences are decided
     here before the edit: separator by that fixture, oversized block stays strict (`ChunkTooLarge`, the
     behaviour pinned by `clean_respects_max_chars`), byte spans must reconstruct the source via the
     promoted `assert_spans_reconstruct`, empty input is `Ok(vec![])` (L5). No new fixture files.
  4. T11's (c) is now recorded as VERIFIED in code: `build_request` uses a separate `user` `ChatMessage`
     and `EmbeddingRequest.input`; only (b) and (d) remain pinning tests.
   5. T6 rewritten to the existing `ErrorClass` taxonomy. No new error variants: 5xx / 401 / 407 / 413 →
      `Http { status, url }`, 429 → `RateLimited(retry_after_secs)`, timeout/transport → `Network`; status
      and `retry_after` remain in the adapter error. The bug is `SemanticError::Inference` → `InternalFatal`
      through `ScraperError::Semantic(inner) => inner.classify()`, not a missing variant.
- L10: T8 schema decision, human's words verbatim: "Decisión: Opción A — NULL = desconocido, con
  refinamientos de migración" and "Sí, aplica esta especificación en el commit de T8." The decision is
  recorded in T8 before the edit, as that task requires. Verified in the worktree:
  `SCHEMA_VERSION = 2`, `PREVIOUS_SCHEMA_VERSION = 1`, and `enforce_schema_version` accepts only 0/1/2
  with a single `spans_json` ALTER, so a naive v3 bump with `PREVIOUS = 2` would reject existing v1
  databases. The migration therefore checks each column's existence and applies every pending step.
  Other closed choices: `model_id TEXT` nullable on `note_chunks` (not `notes`, because
  `load_all_vectors` reads per chunk and mixed-model indexes are possible); `model_id()` called once per
  note and passed as a `&str` argument to `index_note_transactional`, without widening the public
  `ChunkToPersist` tuple; no backfill of legacy NULL rows (S17); read policy NULL → include with a
  re-index warning, individual known mismatch → skip with warning, wholesale mismatch → fail closed,
  dimension behaviour unchanged. The general `chunks` table has the same gap and is explicitly out of
  T8's scope. Options B (fail closed until re-index) and C (stamp the current model on old rows) were
  rejected: B breaks an existing CLI workflow, C lies about provenance.
- L11: Human's fourth instruction, verbatim opening: "Sí. **Aplica las siete correcciones y commitea
  únicamente el plan**; la revisión del código que compartiste basta, no hace falta otra investigación
  externa." Also verbatim: "No hagas cambios de implementación ni presentes el commit del plan como
  autorización para empezar a implementarlos." Companion-review corrections applied after code
  verification, all document-only:
  1. T8 read policy disambiguated: empty eligible set → zero results; `NULL` never backfilled, included by
     width with ONE aggregated warning; fail-closed only with eligible known-model rows, all incompatible
     and no eligible `NULL` rows. Migration checks, `ALTER`s and `user_version` stamp run in one transaction,
     version last.
  2. T7: `embedding_dim() == 0` cuts before any provider request; a dimension-invalid response cannot return
     as a generic `Err` that triggers `embed_batch_raw` bisection — terminal path or validation of the
     `Ok` result; test asserts MAXIMUM outbound calls, and the ceiling scope and counter increment are
     stated.
  3. T9: existing `CONNECT_TIMEOUT = 10 s` / `REQUEST_TIMEOUT = 60 s` are kept, no invented values;
     completion retries stay EXPLICITLY OPEN/BLOCKED pending the concrete provider and a safe `POST` repeat
     (contract or idempotency). T9 does not claim the completion retry policy is satisfied.
  4. T11/S7: test the real HTML path through `extract_text_content` → `legible` and the real cleaner with
     captured requests at both clients; `FakeCleaner` is not proof; (c) pins request structure only, with
     no claim of semantic prompt-injection immunity; assert on serialized `FileTraceLayer` JSONL for page
     markers and credentials, not just field names.
  5. Delivery order: removed the stale "waits on the human schema decision named in L5"; T8's effective
     place is anywhere after T6 in the next free commit slot.
  6. T6: local body cap keeps its local `PermanentFatal` path; no synthesized remote `Http 413`.
  7. T2: `chars().count()` counts Unicode scalars, not graphemes or tokens; the character budget is a
     chunker guard, not an exact provider token limit.
   No work unit or commit count changes. This entry is a plan edit, not authorization to implement T1–T11.
- L12: Human's implementation authorization, verbatim: "vamos a implementar el odd task que esta en
  \"~/Projects/Rust/webfang-worktrees/feat-ai-layer-hardening\"". This supersedes the L5/L11 hold;
  implementation is now in progress in worktree `feat-ai-layer-hardening` (branch matches directory,
  target isolation `own-registered-target`, codedb reindexed to HEAD `3255ac90`). Environment note:
  every non-interactive command must start with `eval "$(direnv export bash)"` or the identity gate
  denies with `CARGO_TARGET_DIR` pointing at main. T11 was executed RED-first on the real path: the
  first run leaked `<div id="readability-page-1" class="page"><p>…<span>…</span></p></div>` into the
  embedding request body, proving the old `FakeCleaner` test was not an oracle for S4. Fixed at the
  extraction boundary with a layout-preserving tag stripper for the `legible` primary branch. The
  two `EnvGuard`s initially stacked in the test deadlocked on the shared `ENV_LOCK` (one guard for
  both variables is the fix); wiremock's loopback is spelled `localhost` so `ssrf_gate` stays armed.
  Commit 1 holds T11; its SHA is recorded at the next task transition.
- L13: Native RDD review of Commit 1 (`0b1de600`, lineage `review-06c2ce3c9aa4f569`, lenses risk +
  reliability) was granted by the human and ran to completion. The final capture escalated with
  `unknown_causality` on findings R3-1 and R3-2; exact-lineage STATUS returned `action: stop`,
  `reason_code: native_stop_required`, `replayability: manual_action_required`. The transaction is
  TERMINAL: no scoped correction was offered, no authority was burned, and the escalation neither
  approves nor blocks delivery — maintainer inspection of the lineage is the only review action,
  and ordinary repository policy still owns commit/push/PR. R3-1/R3-2 details stay with the review
  authority; they are not silently reclassified here. Commit 1 SHA: `0b1de600`.
- L14: T2 executed as two slices. First (Commit 3) fixes the unit defect and the consumed
  `LlmConfig` contract: `split_into_chunks` and `build_sub_chunks` now compare `chars().count()`
  (Unicode scalars, not bytes, graphemes or tokens); `extract` takes the explicit `char_budget`
  resolved by the composition root; `CHARS_PER_TOKEN` was deleted; `max_tokens` is output-only.
  RED observed on `split_into_chunks_counts_characters_not_bytes` (25 bytes > 12 chars) and on
  `sub_chunk_budget_counts_characters_not_bytes` once `extract` stopped honoring the derived 800.
  Remaining slice **T2b** (next free slot, Commit 3b; later task numbers update when it lands):
  derive the governing budget from `ModelProfile::max_chars()` when `--max-chars` is absent, and
  prove the printed value in `show`/`doctor` is that same governing value carried into
  `char_budget`. No newtype, no benchmark token counts as a character budget (S17).
- L15: Native RDD review of Commit 3 (`997c4237`, lineage `review-c97064a20ba2b8fe`, lenses risk +
  reliability) granted by the human. Risk lens admitted `completed`; reliability escalated with
  `unknown_causality` on R3-1 and exact-lineage STATUS returned terminal `native_stop_required`.
  As with L13, no correction was offered and no authority was burned; the escalation neither
  approves nor blocks delivery. Commit 3 SHA recorded here; implementation of the remaining units
  continues under the human's L12 authorization.
- L16: Commit 4 batches T3+T4+T5 as planned. `as_any` became required on `EmbeddingPort` (three
  test-only impls completed); the `SemanticCleaner` seal doc now states the invariant it actually
  protects and `TextChunker` records why it stays open; the LLM extraction error stops echoing the
  parsed model payload. Verification: new payload regression test PASS, `cargo check --all-targets
  --all-features` PASS, clippy/fmt PASS. No native review (medium contract-hygiene slice);
  Commit 4 SHA `060f3cb2`.
- L17: Session handoff (implementation authorized by L12, still in force). **Landed:** Commit 1
  `0b1de600` (T11, boundary fix + real-path oracle), Commit 2 `0f2bb2aa` (T1, dimension fail-closed),
  Commit 3 `997c4237` (T2 part 1, character unit + `char_budget`), Commit 4 `060f3cb2` (T3+T4+T5).
  Tree clean; branch `feat/ai-layer-hardening` matches directory; all four commits passed clippy and
  their focused suites; the full pre-commit gate passed on Commit 1. **Native RDD:** Commits 1 and 3
  were `high_risk` (the diff touched `ai_boundary_test.rs`), granted by the human, and both escalated
  with `unknown_causality` (L13: R3-1/R3-2; L15: R3-1) to terminal `native_stop_required` — no
  correction offered, no authority burned, delivery stays under ordinary repository policy. Commits 2
  and 4 assessed `medium`/`under_budget` (no review due). **Not landed:** T2b (profile-derived
  governing budget, see L14), T6, T7, T8, T9, T10. Next action: resume with T2b, then T6
  (classify remote AI errors at the adapter boundary — note `ScraperError::Semantic(inner) =>
  inner.classify()` sends every `SemanticError::Inference` to `InternalFatal`; change the boundary,
  not the taxonomy), then T7. T8 is HIGH RISK (schema/migration over stored user data): do not start
  it until a fresh session can give the transaction and rollback tests full attention. T9's
   completion-retry sub-decision remains OPEN pending concrete provider semantics. Reindex codedb and
   verify `eval "$(direnv export bash)"` before any command in a new session.
- L18: Session resumed with the exact handoff of L17. Bootstrap verified first: `mem_current_project`
  → `webfang` (cwd matches), `eval "$(direnv export bash)"`, worktree HEAD `e833786c`, clean tree,
  branch `feat/ai-layer-hardening` matches directory, codedb reindexed (`root` = worktree,
  `head e833786c18f6`). **T2b landed as Commit 3b `60d8e414`** (`fix(ai): derive one governing
  character budget from the model profile`, 8 files / 300 insertions / 70 deletions). Verification:
  RED → GREEN on four domain resolver tests; `cli::commands` lib tests 53 PASS;
  `show_provider_identity_test` 32 PASS; `cli_binary_test` budget/profile tests PASS;
  `ai_boundary_test`, `max_chars_bound_test`, llm_extraction suite PASS; full `nextest run
  -p webfang_core -p webfang_cli` 4902 passed / 8 skipped; check + clippy all-targets/all-features
  `-D warnings` + fmt --check + doc `-D warnings` PASS (rustdoc emitted an internal ICE in
  `write_shared` for several crates but cargo still exited 0 — toolchain quirk, not a lint finding).
  Old assertions that said "no run consumes the profile" were rewritten, not bypassed: the profile
  now governs a budget but is still NOT the run identity. Native assess over `e833786c..HEAD`
  reported `high_risk` (review_due true) — review lifecycle pending for this commit. Next: T6, then
  T7; T8 stays in its own session. Follow-up recorded, not fixed here: `show --help` still describes
  `[[model_profiles]]` as historical metadata that no run consumes; that help string and its
  snapshots need a separate truthfulness edit.
- L19: Native review of the T2b candidate (lineage `review-11a9184b7464d4f9`, target
  `sha256:7551723324bc42e78deb5a8fd9ea4a06cf49a46d990e66d08aa83edf428cde07`, lenses risk +
  resilience + readability + reliability) was granted by the human and ran to completion. All four
  captures were admitted `completed`; the refuter corroborated `R1-profile-budget-overflow-ub` (with
  `probe unavailable on opencode`), and the final capture escalated `R2-1` with
  `unknown_causality`. Exact-lineage STATUS returned `action: stop`,
  `reason_code: native_stop_required`, `replayability: manual_action_required`. The transaction is
  TERMINAL: no scoped correction was offered, no authority was burned, and the escalation neither
  approves nor blocks delivery — maintainer inspection is the only review action, ordinary repository
  policy still owns commit/push/PR. The native findings are not reclassified here. Parent observation
  for the maintainer, not a reclassification: Rust's `f64 as usize` saturates since 1.45 and
  `ModelProfile::max_chars` already documents that it saturates, so the claimed overflow-UB does not
  match current Rust semantics; the pre-existing unbounded budget value remains a separate question.
- L20: **T6 landed as Commit 5 `e9978183`** (`fix(ai): classify remote AI failures at the adapter
  boundary`, 5 files / 346 insertions / 29 deletions, new `error/ai_failure.rs`). The embedding
  adapter appends a stable machine tag to the human prose of `SemanticError::Inference` (observed
  status, dialed endpoint, honored Retry-After; or `transport`). The single
  `From<SemanticError> for ScraperError` conversion reads that tag: 5xx and 401/407 → `Http`,
  429 → `RateLimited(retry_after_secs)`, timeout/transport → `Network`. Untagged local consistency
  failures — including the local body cap — keep the `Semantic` path. No new `ScraperError` or
  `SemanticError` variants; `SemanticError` itself is not expanded. RED observed (six tests failed
  to compile without the tag module), GREEN after wiring. Verification: `error::` lib suite 271
  PASS, `ai_error_propagation` 14 PASS, `ai_boundary_test` PASS, `remote_embedding_resilience_test`
  8 PASS, `ai_proxy_407_test` 4 PASS, full `nextest run -p webfang_core -p webfang_cli` 4909 passed
  / 8 skipped, check + clippy all-targets/all-features `-D warnings` + fmt --check + doc
  `-D warnings` PASS. All newly authored strings are English; the pre-existing Spanish operator
  messages were preserved verbatim inside the tagged wrappers, not translated or newly written.
