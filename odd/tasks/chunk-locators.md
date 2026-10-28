# Mission: chunk-locators

Branch: `feat/chunk-locators` · Worktree: `feat-chunk-locators` · Base: `main` @ 280f12ab

Design approved in session (2026-10-28): ADR stub "TextChunker con locators".

## Specs

- S1: "Elige la Opción 1 primero, pero prepárate para la 3. Empieza por la Opción 1 para
  no bloquear el desarrollo. Calcula los offsets en el Markdown y guarda una referencia
  de qué documento lo originó."
- S2: "Diseña pensando en la Opción 3: Cuando hagas la conversión de HTML a Markdown,
  no lo hagas a ciegas. Si usas una librería de parseo, intenta guardar un metadata
  básico (ej. número de línea o un ID de bloque HTML) en el chunk antes de que entre al
  chunker." (restricción de diseño — el build de la Opción 3 queda deferido a ADR)
- S3: "OK" — aprobación del slice propuesto: `chunk_text` devuelve `Vec<TextChunk>` con
  `Vec<ByteSpan>` (bytes, half-open `[start, end)`) contra el `&str` de entrada; cambio
  in place del trait (sin trait paralelo, sin defaults que mientan); doc-ref a cargo del
  caller; persistencia del locator `(nota_path, chunk_index, spans)` en el índice de
  vault como único cambio de comportamiento.

## Tasks

- T1: ✅ RED — compilation RED capturado (E0433/E0412/E0599 sobre `TextChunk`/`text()`/`spans()`).
- T2: ✅ GREEN — `ByteSpan` + `TextChunk` en `domain/text_chunker.rs`; firma del trait in place;
  `SemanticError::InvalidSpan { start, end }` (classify → DomainRecoverable).
- T3: ✅ GREEN — `MarkdownChunker`: `blocks()` con cursor de bytes + `push_block`,
  `split_oversized` → `push_piece` con sub-span del trim; presupuesto `max_chars` en chars.
- T4: ✅ GREEN — `IdentityChunker` (llm_extraction) y `StubChunker` (vault_search) con spans.
- T5: ✅ GREEN — locator persistido: `ChunkToPersist` en el puerto, `NoteChunkVector.spans`,
  `note_chunks.spans_json` (JSON `{start,end}`), migración v1→v2 con `ALTER TABLE` guardado.
- T6: ✅ Gate — clippy workspace `--all-features -D warnings` verde, `fmt --check` verde,
  rustdoc `-D warnings` exit 0 (ver L6), nextest full en curso al momento del log.
- T7: Commits por work-unit — commit 1 `b296fca3` (contrato + chunker + consumidores
  mecánicos); commit 2 = persistencia (este slice).

## Log

- L1 (request): "analiza este detalle o informacion: 'El chunker recorta los bloques y los
  une con \n\n: los offsets reales contra el documento fuente exigen cambiar el contrato de
  TextChunker (otro trait, otra superficie).'" — verificado correcto contra código.
- L2: Diseño interrogado (una pregunta: objetivo de los offsets). Respuesta: Opción 1 ya,
  Opción 3 como restricción de diseño.
- L3: Diseño aprobado ("OK"). Rechazados: trait paralelo (drift), span único (provenance
  falsa), default impl (miente), offsets en chars (no sliceable).
- L4: RED capturado: `cargo check -p webfang_core --all-features --tests` → E0433
  `TextChunk` undeclared, E0599 `text`/`spans` no method. Luego GREEN 37/37 en el primer
  run de nextest (chunker + llm + vault).
- L5: commit 1 `b296fca3` — 5 archivos, 298+/61−. El mapeo reveló un segundo consumidor
  productivo (`llm_extraction::build_sub_chunks`), corregido en el Risk de abajo.
- L6: rustdoc 1.91.1 ICE auto-recuperable (`scoped-tls` / `Symbol::intern` en
  `CrateInfo::read_many` del search-index): panic por crate, continúa y termina exit 0 con
  los 8 crates documentados. Anomalía del toolchain, no del diff. El gate real (lints con
  `-D warnings`) pasó tras corregir dos links de doc público a ítems privados.
- L7: `type_complexity` resolvido con `pub type ChunkToPersist<'a>` en el puerto (contrato
  nombrado, no un silenciamiento).

## Risk

- Medio: cambio breaking interno de un trait dyn (3 implementadores + 2 consumidores
  productivos — el mapeo corrigió el supuesto inicial: `llm_extraction::build_sub_chunks`
  TAMBIÉN es productivo, no solo vault-search) + migración de schema v1→v2 (gate REFUSE
  se conserva para versiones no reconocidas). Superficie tocada: `error.rs`,
  `domain/text_chunker.rs`, `infrastructure/text_chunker.rs`, `application/vault_search.rs`,
  `application/llm_extraction.rs`, `domain/note_repository.rs`,
  `infrastructure/persistence/sqlite.rs`. NO tocado: `docs/`, `CHANGELOG.md`, `.github/`,
  conversión HTML→Markdown, `VaultSearchResult` (enriquecimiento = Opción 3, deferido).

## Deferred (ADR pendiente)

- Opción 3: conversor HTML→MD anotado (`ConvertedDoc { markdown, blocks }`) — la cadena
  futura es `chunk.spans → bloque de markdown → elemento HTML`; el output del chunker no
  necesita romperse de nuevo.
- Enriquecimiento de `VaultSearchResult`/MCP con locators (read-side ya existe en
  `NoteChunkVector.spans`).
