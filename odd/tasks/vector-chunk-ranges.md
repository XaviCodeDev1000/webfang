# vector-chunk-ranges

**Branch:** `feat/vector-chunk-ranges` · **Issue:** #2429 (AC 2, mitad de rangos; `status:approved`) · **Base:** `main` @ 0bbc8458 (post #2451)

## Specs

S# "`VectorRecord` emite posición del chunk (`chunk_index` y rango de caracteres en el documento fuente)." (issue #2429, AC 2 — este slice cierra los rangos; `chunk_index` ya vive en main)

S# "Cada record permite verificar, sin acceso al store original, que su vector es comparable con el de cualquier otro record del mismo conjunto." (issue #2429, Expected behavior)

S# Out of scope del issue: re-embeddear stores existentes; dedup near-duplicate.

S# Decisión de secuencia del usuario (esta sesión): asumir y mergear primero `feat/chunk-locators` (PR #2451, contrato `TextChunk`/`ByteSpan`) y construir este slice desde el nuevo main reutilizando ese contrato. Sin PRs apilados.

S# Restricción: ahorrar recursos en builds (verificación agrupada; seed `cold` aceptado).

## Tasks

- T1: Mapear el flujo de chunks: `SemanticCleaner` → `ElasticIngestion::cleaner_chunks` → `save_chunk`; decidir el transporte de spans (firma de `save_chunk` vs nuevo campo).
- T2: RED — tests que fallan sobre la API nueva.
- T3: GREEN — spans llegan al sink; `VectorRecord` emite `char_start`/`char_end` (o `spans`); SQLite impl y demás impls del port actualizados; fixture actualizado; decisión de schema documentada.
- T4: Verificación agrupada + clippy/fmt/doc + commits por work-unit + PR B (`Closes #2429`).

## Log

- L1: Slice 2 definido en la misión #2429 (char ranges deferidos porque exigían el cambio de contrato de `TextChunker`). PR #2451 (merge 0bbc8458) introdujo `TextChunk { text, spans }` con `ByteSpan` half-open en bytes y el principio "no defaults que mientan".
- L2: Usuario aprueba asumir `feat/chunk-locators` (otra sesión) y la entrega secuencial. PR A mergeado; este slice abre sobre 0bbc8458.
- L3: Diseño decidido (T1): los spans nacen en `split_into_chunks` del cleaner (técnica cursor de #2451: párrafos con trim, spans apuntando a la región recortada); viajan `DocumentChunk.chunk_spans: Option<Vec<ByteSpan>>` → `ProcessedChunk.spans` → firma de `save_chunk(..., spans: Option<&[ByteSpan]>)`. El `VectorRecord` emite `byte_start`/`byte_end` = rango envolvente del chunk (primer span start, último span end) — bytes, no chars, consistente con `ByteSpan` (#2451 rechazó offsets en chars). Semántica del doc-ref: offsets contra el texto extraído que el cleaner chunka (no el HTML crudo — eso es la Opción 3 deferida del ADR de chunk-locators). SQLite ignora los spans en `save_chunk` (documentado): el store de vectores no los persiste en este slice.
- L4: RED observado (compilación sobre `text()`/`spans()`/`byte_start` y la firma de 6 args de `save_chunk`). Durante el GREEN el propio nextest cazó un bug real del cursor: el start del span se calculaba con el offset ya avanzado (post-'\n') — todos los spans quedaban corridos; fix capturando `line_start` antes de avanzar. Los tests de spans del cleaner lo pinaron en el primer run: la disciplina RED-GREEN pagó.
- L5: Gate en verde: check workspace, clippy `-D warnings`, fmt, doc `-D warnings`, 64 tests focalizados + golden de intra-crate-lint refrescado.
