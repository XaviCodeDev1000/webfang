# Mission: jsonl-writer-index-read (issue #2393)

**Branch:** `chore/jsonl-writer-index-read` · **Type:** `chore(exporter)` · **Issue:** #2393 (status:approved)

## Specs

- S1: "Un export batch lee el archivo de salida **una sola vez**, o queda documentada la decisión de por qué dos lecturas son necesarias (con la evidencia de qué usa el writer del índice)."
- S2: "El conteo `skipped_non_utf8` sigue viajando por `ExportOutcome` / `jsonl_index_report` sin que aparezca una segunda implementación de la semántica de línea."
- S3: "Los tests de resume gate y non-UTF-8 policy pasan sin cambios semánticos (los bytes en disco y las decisiones del gate son idénticos)."
- S4: "Hay que decidir y aplicar una de dos: (a) el writer no necesita el índice … y la lectura entera se elimina de esa capa; o (b) el índice viaja: el que ya lo tiene (`CommitSession`) se lo inyecta al exporter."

## Decision

Variante **(a)**: eliminar la lectura del writer. Evidencia:

- `JsonlSession::open` no usa el índice internamente: `recover_torn_tail` lee solo una
  ventana del tail, `gc_stray_tmp_siblings` no lee el archivo. El índice devuelto tiene un
  único caller de producción (`JsonlExporter::session()`) que lo descarta.
- El WARN de `without_checksum` del writer ya lo emite idéntico la capa de aplicación
  (`build_content_hash_index`, `application/export_factory.rs`).
- Elimina la única arista `infrastructure → application` del árbol
  (`build_hash_index` → `read_jsonl_checksum_index`).
- Rutas resultantes: batch lee 1 vez (`CommitSession::open`); `export_file` lee 1 vez
  (`count_non_utf8_lines` del handler MCP).

## Tasks

- [ ] T1: `JsonlSession::open` → `io::Result<Self>`; borrar `build_hash_index`; actualizar docs.
- [ ] T2: `JsonlExporter::session()` adapta el destructuring; actualizar docs del campo `session`.
- [ ] T3: mover el contrato `without_checksum` (`renamed_checksum_field…`) a un test de
      `read_jsonl_checksum_index` en `application/export_factory.rs`.
- [ ] T4: actualizar tests unitarios (`tail_window.rs`) y de integración
      (`export_torn_tail_recovery_test.rs`, `jsonl_concurrency_test.rs`,
      `adversarial_stress_matrix.rs`).
- [ ] T5: refrescar el golden set de `tools/intra-crate-lint` en el mismo commit.
- [ ] T6: verificar (check/clippy/fmt/doc + nextest objetivo) y PR.

## Log

- L1: "Issue #2393 analizar y revisar luego implementar, pero ten cuidado en saturar los
  recursos del sistema cuando ejecutas comandos pesados" — sesión main, 2026-10-27.
- L2: main avanzó a `4f525822` (#2391 merged) durante el análisis; worktree creado sobre
  `21ea545a`. Merge de main sin conflictos antes de seguir.
- L3: Golden set de intra-crate-lint regenerado en commit propio sobre el árbol combinado
  (solo shifts esperados).
