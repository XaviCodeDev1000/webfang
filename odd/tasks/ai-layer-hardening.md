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
| S5 | T9 (primary), T6, T11 | Shared limits, uniform typed errors, clean traces. |
| S6 | T3 (panics), T10 (empty/malformed), T1 (malformed dims), T6 (network/API), T9 (slow responses), T11 (credential leaks) | Decomposed; each owner names the failure it closes. |
| S7 | T5, T11 | Redaction, plus an assertion that span fields stay an allowlist. |
| S9 | T1, T7 | Truncated ranking; the guard that stops one bad response becoming `n` calls. |
| S10 | T2 | One budget, one unit, three consumers. |
| S11 | T6 | Retryability distinct from a local consistency bug. |
| S12 | T5 | The payload stops travelling in error text. |
| S13 | T8 | `model_id` persisted and checked on read. |
| S14 | T9 | Shared policy literals and a real retry on completion. |
| S15 | T10 | Two chunkers become one. |
| S16 | T4 | The seal keeps a true sentence. |
| S17, S18 | this document | Rejected and out-of-scope decisions, so no worker re-litigates them. |

## Tasks

- T11 `S4`, `S7`, `S5` route: inline — the data/instruction boundary, asserted at the two places content
  leaves the process (embedding request, completion request). RED first, built from the same raw HTML the
  CLI accepts: (a) no raw HTML reaches either model client — assert on the captured request body that a
  tag sequence from the input document is absent, and locate the single strip site
  (`content_processing::aggressive::strip_html_tags`, imported by `infrastructure/semantic_cleaner.rs`)
  before claiming it is the only door; (b) the credential channel is single — the key appears only in the
  `Authorization` header of its intended endpoint, in no body, URL, error string or trace event;
  (c) untrusted page text is data — it travels in a delimited data field of the completion request and is
  never interpolated into the instruction prose, and the instruction text is pinned as a constant that
  cannot absorb page content; (d) tracing stays an allowlist — provider/model/dim/count/request_id/lengths
  only, with a test that a known page substring never appears in the emitted span fields.
  Honest precondition: (c) and (d) were NOT verified before this task was written; the read-only audit that
  would have confirmed the current request shape and span fields was cancelled by the human. Point (d) may
  land as a pinning test with no code change (the design already looks right), or as an interim pin on the
  recording macro's field set; point (c) is the only part with standing behaviour risk, and it must not
  grow a provider-specific instruction channel on the way. This task does NOT add tracing fields to
  `EmbeddingPort` (that would reopen T3/T4) and does NOT change the empty-input contract (T10).
  Risk: item 2 (what leaves the process toward an external model, and on which channel) + item 6 (no
  existing test asserts any of (a)-(d)). Commit 1.
- T1 `S9` route: inline — RED first: a mixed-dimension batch must error, not rank. Delete the private
  `cosine_similarity`, route `distances_to_centroid`/`filter_by_relevance` through the domain's
  `cosine_similarity_fail_closed`, and reject a dimension-disagreeing batch. Risk: item 3 (the filter
  decides which chunks survive for every caller — export, batch, MCP clean, vault — so a stricter batch
  decision changes their output) + item 6 (no existing test pins reject-vs-degrade on mixed dimensions).
  Commit 2.
- T2 `S10` route: inline — RED first: multi-byte tests (accented Spanish, CJK, emoji) prove the budget is
  counted in characters in `split_into_chunks` and in `build_sub_chunks`. Decide `chars` as the unit of the
  contract: the accepted number changes for non-ASCII text, and that is the contract change.
  There is a THIRD budget consumer, and it is live: `ModelProfile::max_chars()` derives a character budget
  from `context` × chars-per-token and its own doc-comment calls it the per-profile input for the chunk
  budget, yet its only production call site is the display helper in `cli/commands/blocks.rs:217`; the
  cleaner is in fact built from `AiConfig::max_chars` (`cli/parse.rs:54` →
  `webfang_cli/src/main/ai.rs:94,118`), and `max_chars_explicit` already exists in `cli/args/ai.rs`. Two
  budgets, one printed and one governing: the operator reads a number that chunks nothing.
  First step of this task is a verification, not an edit: establish whether options-spec resolution already
  derives `AiConfig::max_chars` from the profile when the flag is absent. Either outcome ends the same
  way — ONE governing budget, and the printed one is the governing one: the resolved budget derives from
  `ModelProfile::max_chars()`, and an explicit `--max-chars` wins. If the derivation already exists, this
  task degrades to the unit fix plus a test proving printed == splitting. Risk: item 3. Commit 3.
- T3 `S2` route: inline — make `as_any` a required `EmbeddingPort` method (remove the panicking default);
  every in-tree impl already overrides it, so this is compile-time enforcement of an existing practice.
  Risk: item 3 — `EmbeddingPort` is deliberately open so external adapters may implement it, and a required
  method is a source-breaking change for any impl outside this workspace. Seven in-tree impls carry it, so
  the workspace compiles unchanged; the commit body states the breaking change. Commit 4 (with T4, T5).
- T4 `S16` route: inline — correct the seal doc-comment so it states the invariant it actually protects
  (impl-set control over `Box<dyn SemanticCleaner>` consumers), and add the one-sentence rationale
  `EmbeddingPort` already carries to `TextChunker`. Five lines; no seal removal. Risk: none. Commit 4.
- T5 `S7`, `S12` route: inline — stop echoing the parsed model payload into `ScraperError::Extraction`;
  carry the provider's own short message instead. Risk: item 3 — the error text and the snapshots pinning it
  change. Not item 2: the payload is page content, not a credential; T11 owns the credential channel.
  Commit 4.
- T6 `S11` route: inline — one classification point: extend the `ScraperError` mapping so a retryable
  remote failure is distinguishable from a local consistency bug, keeping HTTP status and `retry_after` in
  the adapter error and converting once at the boundary. Risk: item 3 (error contract consumed by the CLI
  exit codes) + item 6. Commit 5.
- T7 `S6`, `S9` route: inline — dimension guard at the embedding adapter boundary, the single funnel every
  vector passes (`embed_batch_raw` → `EmbeddingPort::embed_batch`), with a rejection counter and an upper
  bound that stops the bisection amplifying one bad provider response into `n` remote calls. Ask the
  provider for its width (`/models` or the endpoint's declared dim) instead of guessing it. Risk: item 6
  — no existing test covers a mixed-dimension batch, and a guard that rejects valid vectors fails silently
  on every page. Explicitly NOT item 1: the store is rebuildable by re-running the export, so a false
  rejection is recoverable rather than unrecoverable data. Commit 6.
- T8 `S13` route: inline — persist `model_id` with the vault store vectors and verify it on read in
  `vault_search`, with an actionable warning on mismatch. Requires the human's schema decision (column +
  migration shape); recorded in the Log before the edit. Risk: item 1 (a schema/format change and a
  migration over stored user data). Commit 7.
- T9 `S14` route: inline — one shared `RetryPolicy`/body-cap module that reads the existing constants
  (`LLM_MAX_BODY_BYTES`, the downloader's values) instead of re-declaring them, plus a real retry on the
  completion path. No new dependency, no invented numbers. Risk: item 6 — an attempt-count regression is
  visible only as latency and no test catches it. Commit 8.
- T10 `S15`, `S3` route: inline — collapse the duplicate chunking: `SemanticCleanerImpl` depends on
  `TextChunker` and `split_into_chunks` is deleted. The four S15 divergences are NOT interchangeable
  details; each is decided in the Log before the edit, with the test that pins it:
  1. **separator** — `\n` (every line) vs `\n\n` (blank-line block). Decide from the observed shape of
     `strip_html_tags` output on a real sample: if a paragraph can contain `\n`, `\n\n` is the correct unit
     and the current splitter is shredding paragraphs; if it never does, both are equivalent. Pin with a
     fixture, not with opinion, and do not import the token-based benchmark numbers as a character verdict.
  2. **oversized block** — today `semantic_cleaner.rs` errors `ChunkTooLarge` and `clean_respects_max_chars`
     (`infrastructure/semantic_cleaner.rs:499`) pins exactly that, while `MarkdownChunker::split_oversized`
     hard-splits. Default: keep strict (the error is the page-level degradation signal) and put hard-split
     behind an explicit policy value; if the consolidated chunker adopts hard-split instead, rewrite that
     test deliberately and describe it as a behaviour change, never as a refactor.
  3. **byte spans** — `split_into_chunks` spans point at the cleaned text it received; `MarkdownChunker`
     spans point at its own input and chunks join on `\n\n`. If the cleaner passes the cleaned text the
     spans stay comparable, but the joined chunk text — and therefore the offsets — change. Requirement:
     the consolidated chunker's chunk text reconstructs its source from the spans (reuse
     `text_chunker::assert_spans_reconstruct` on the cleaner path), and downstream locators (chunk_index +
     char ranges, the #2429 provenance) are re-derived, not assumed intact. UNVERIFIED here: the audit that
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
feature. T8 sits outside the streak: it waits on the human schema decision named in L5 and may land at any
point after T6.

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
  other unit to start — the T8 schema decision and the human's review of this document remain open.
- L7: The human reviewed this plan against the tree (not against the document) and confirmed the forensics
  of S9, S10 and S2: the truncating `zip`, the byte budget compared against `chars().count()` in
  `MarkdownChunker`, the `panic!` default on `as_any`, and the existence of
  `cosine_similarity_fail_closed` in the domain and of `tools/intra-crate-lint`. Five defects were
  corrected in place:
  1. **S4 had no owner** — nothing in T1–T10 built the data/instruction boundary the human asked about
     (T5 removes an echo on the way out; it does not guard the way in). Added **T11** as Commit 1, holding
     the four boundary assertions ((a) no raw HTML, (b) single credential channel, (c) page text is data Kipling
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
