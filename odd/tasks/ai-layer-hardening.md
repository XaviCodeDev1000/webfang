# ai-layer-hardening

**Branch:** `feat/ai-layer-hardening` · **Worktree:** `~/Projects/Rust/webfang-worktrees/feat-ai-layer-hardening`
**Goal:** harden the AI layer (`SemanticCleaner` ↔ `EmbeddingPort` ↔ `TextChunker` ↔ `LlmExtractionService`)
so that a provider swap or a malformed response fails loudly instead of degrading silently.
Ten work units, one commit each, English-only artifacts. Native review applies per work-unit commit only
while the user-owned RDD switch is on; a review outcome never authorizes delivery.

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
oversized-block handling (hard-split vs `ChunkTooLarge`), on the budget unit (chars vs bytes), and on
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

## Tasks

- T1 `S9` route: inline — RED first: a mixed-dimension batch must error, not rank. Delete the private
  `cosine_similarity`, route `distances_to_centroid`/`filter_by_relevance` through the domain's
  `cosine_similarity_fail_closed`, and reject a dimension-disagreeing batch. Risk: item 6 — a regression
  here is silent and no current test covers it. Commit 1.
- T2 `S10` route: inline — RED first: multi-byte tests (accented Spanish, CJK, emoji) prove the budget is
  counted in characters in both `split_into_chunks` and `build_sub_chunks`. Decide `chars` as the unit of
  the contract. Contract change: the accepted number changes for non-ASCII text. Risk: item 3. Commit 2.
- T3 `S2` route: inline — make `as_any` a required `EmbeddingPort` method (remove the panicking default);
  every existing impl already overrides it, so this is compile-time enforcement of an existing practice.
  Risk: item 3. Commit 3 (batched with T4 and T5).
- T4 `S16` route: inline — correct the seal doc-comment so it states the invariant it actually protects
  (impl-set control over `Box<dyn SemanticCleaner>` consumers), and add the same one-sentence rationale
  to `TextChunker` that `EmbeddingPort` already carries. Five lines; no seal removal. Risk: none. Commit 3.
- T5 `S7`, `S12` route: inline — stop echoing the parsed model payload into `ScraperError::Extraction`;
  carry the provider's own short message instead. Risk: item 7 (data exposure through logs). Commit 3.
- T6 `S11` route: inline — one classification point: extend the `ScraperError` mapping so a retryable
  remote failure is distinguishable from a local consistency bug, keeping HTTP status and `retry_after` in
  the adapter error and converting once at the boundary. Risk: item 3 (error contract consumed by the CLI
  exit codes) + item 6. Commit 4.
- T7 `S6`, `S9` route: inline — dimension guard at the embedding adapter boundary (the single funnel every
  vector passes: `embed_batch_raw` → `EmbeddingPort::embed_batch`), with a rejection counter and an upper
  bound that stops the bisection from amplifying one bad provider response into `n` remote calls. Ask the
  provider for its width instead of guessing it. Risk: item 1 + item 6. Commit 5.
- T8 `S13` route: inline — persist `model_id` with the vault store vectors and verify it on read in
  `vault_search`, with an actionable warning on mismatch. Requires a schema decision from the human
  (column + migration shape); recorded in the Log before the edit. Risk: item 1. Commit 6.
- T9 `S14` route: inline — one shared `RetryPolicy`/body-cap module that reads the existing constants
  instead of re-declaring them, plus a real retry on the completion path. No new dependency, no invented
  numbers. Risk: item 5 + item 6. Commit 7.
- T10 `S15` route: inline — collapse the duplicate chunking: `SemanticCleanerImpl` depends on
  `TextChunker` and `split_into_chunks` is deleted; decide the empty-input contract once and write it into
  the trait. Check `tools/intra-crate-lint` when the new edge moves. Risk: item 3 + item 6. Commit 8.

Order: T1 → T2 → (T3,T4,T5) → T6 → T7 → T8 → T9 → T10. T1–T5 are one review sitting; from T6 on each
unit moves a seam of the container and gets its own.

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
  paths, and conversational permission does not clear it. Exact commands handed to the human. Until they
  run, builds in this worktree are unisolated and the isolation guard will refuse.
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
- L5: Human decided to review this plan before any code is written. Implementation of T1 is therefore held.
  Four points are flagged in the document as needing a human decision rather than a parent one: the
  empty-input contract that T10 must write once (today `Tokenize` vs `Ok(vec![])`), the schema and
  migration shape for `model_id` in the vault store that T8 needs, confirmation that characters are the
  single budget unit for T2, and whether the seal on `SemanticCleaner` may eventually be removed instead of
  only re-documented (this plan recommends documentation only). Native review per work-unit commit depends
  on the user-owned RDD switch being on for this clone.
- L6: Bootstrap completed by the human (`.envrc` with the five exact lines, `$TREE` substituted and `$HOME`
  literal; `direnv allow`; `.env` copied). Isolation verified with `scripts/check_target_isolation.sh`:
  `reason=own-registered-target`, declared by this tree's own `.envrc`. Commit 8f646367..36219d0a lands this
  mission document and unblocks every later work unit; the identity gate and the review gate both pass. L3
  is superseded: the half it reported as blocked is now done.