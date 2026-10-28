# vector-record-provenance

**Branch:** `fix/vector-record-provenance` · **Issue:** #2429 (`status:approved`, `type:bug`)

## Specs

S# "Cada record permite verificar, sin acceso al store original, que su vector es comparable con el de cualquier otro record del mismo conjunto." (issue #2429, Expected behavior)

S# "`VectorRecord` emite identidad del modelo (`embedding_model` + `embedding_dim`, y versión o provider id si aplica)." (issue #2429, AC 1)

S# "`VectorRecord` emite posición del chunk (`chunk_index` y rango de caracteres en el documento fuente)." (issue #2429, AC 2)

S# "Decisión documentada sobre compatibilidad de schema: bump de versión del formato o campo opcional con default." (issue #2429, AC 3)

S# "Test que rechaza un record cuyo `embedding.len()` no coincide con `embedding_dim`." (issue #2429, AC 4)

S# "Fixture JSONL actualizado." (issue #2429, AC 5)

S# "necesitamos extender el trait" — decisión de diseño del usuario: la identidad del modelo llega al sink extendiendo `EmbeddingPort` (no propagando el profile por otro canal).

S# Out of scope: re-embeddear stores existentes; dedup near-duplicate (issue separado).

S# Restricción del usuario: ahorrar recursos en builds (un solo build completo, verificación agrupada al final).

## Tasks

- T1: Mapear impls de `EmbeddingPort` y callers de `StreamRepository::new` / `save_chunk`.
- T2: RED — tests de rechazo (dim mismatch) y de emisión de procedencia.
- T3: Extender `EmbeddingPort` con `model_id()`; implementar en todos los impls.
- T4: `VectorRecord`: `embedding_model` + `embedding_dim` (opcional, `#[serde(default)]`), `chunk_index` (parseado del id `"{sha}-{index}"`), decisión de schema documentada en doc comments.
- T5: Rango de caracteres (`char_start`/`char_end`): evaluar el coste real en `text_chunker`; si el chunker no rastrea offsets, documentar y decidir slice.
- T6: Actualizar fixture `crates/webfang_core/vectors.jsonl` y tests de snapshot afectados.
- T7: Verificación agrupada: `cargo check` → `nextest` focalizado → clippy/fmt; commits por work-unit.

## Log

- L1: Usuario pide revisar/analizar issue #2429 ahorrando recursos en builds. Análisis verificó las evidencias del issue; sin builds.
- L2: Usuario decide el fork de diseño: extender `EmbeddingPort`. Se crea el worktree y esta misión.
- L3: Mapeo completo: `save_chunk` ya recibe `chunk_index: i64` (el sink lo ignoraba); el wiring `with_vault_ports` → `with_elastic_ingestion` garantiza que el `EmbeddingPort` está inyectado antes de construir el sink. Char offsets (`char_start`/`char_end`) requieren cambiar el contrato de `TextChunker` (devuelve `Vec<String>`, recorta y une bloques) → **deferido a slice posterior** (T5 cerrado como decisión de scope).
- L4: RED observado (9 errores de compilación sobre la API nueva) con un único build frío (seed `cold`). GREEN: trait extendido con `model_id()`, 7 impls actualizados, `VectorRecord` con `embedding_model`/`embedding_dim`/`chunk_index` (`#[serde(default)]`), rechazo de dim mismatch en ambos boundaries (sink y `from_line`), wiring del container, fixture actualizado. Gate en verde: check, clippy workspace, fmt, doc, 5 tests nuevos + 54 del área afectada + behavioral output_vectors.

