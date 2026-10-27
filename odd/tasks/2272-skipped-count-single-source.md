# El conteo skipped_non_utf8 sale de una sola fuente (issue #2272)

El conteo `structuredContent.skipped_non_utf8` de la capa MCP lo calcula una
segunda implementación de las semánticas de línea (`count_non_utf8_lines` en
`mcp_server/handlers/export.rs`), cuando el core ya lo calcula
(`CommitSession.skipped_non_utf8`). El export del core debe devolver el número
por su propia API y la capa MCP sólo transportarlo.

## Specs

- S1: "El conteo sale de una sola fuente (el core) y la capa MCP sólo lo transporta."
- S2: "El contrato público cambia, así que va en un slice propio con su review, no como parche dentro de otra PR."
- S3: "Un test que fije la igualdad entre el número del resumen del export y el del `structuredContent`, para que las dos cifras no puedan separarse."

## Tasks

- T1: Core: `process_results` devuelve `ExportOutcome { processed_urls, skipped_non_utf8 }`; el número viene de `CommitSession` (ya calculado por `read_jsonl_checksum_index`), sin releer.
- T2: Core: API pública `jsonl_index_report(path) -> JsonlIndexReport` (wrapper de `read_jsonl_checksum_index`) para la ruta `export_file`, que no pasa por `CommitSession`.
- T3: MCP `export_results`: transporta `outcome.skipped_non_utf8`; eliminar la pre-lectura.
- T4: MCP `export_file`: reemplazar `count_non_utf8_lines` por `jsonl_index_report`; borrar el helper y su test (`count_non_utf8_lines_uses_the_application_line_semantics`).
- T5: Adaptar los call sites de `process_results` (CLI `export_flow.rs` + ~28 tests) mecánicamente.
- T6: Test nuevo en core: el outcome reporta las líneas saltadas.
- T7: Test de igualdad (S3) en `mcp_export_skipped_count_test.rs`: `structuredContent.skipped_non_utf8` == el número del core.

## Decisiones de diseño

- El helper MCP traga errores de lectura como `0` (warn + 0); `jsonl_index_report`
  hereda la tolerancia de `build_content_hash_index`: archivo ilegible → reporte
  default + log, nunca aborta (el índice sólo PRUEBA un flush).
- El re-read en la ruta `export_file` (writer ya lee el archivo y descarta el
  reporte) queda fuera de este slice: sacarlo exige cambiar el puerto
  `Exporter`/writer, que es otro cambio de contrato. La única implementación de
  semántica de línea es la del core en ambos casos.
- `export_file` no se reroutea por `process_results`: cambiaría su semántica de
  export (CommitSession, dedup) — fuera de alcance.

## Log

- L1: "adelante pero ten cuidado en saturar los recursos del sistema cuando ejecutas comandos pesados" — implementar #2272 tras el análisis de la sesión.
- L2: Receta de build (obs #2279/#2294): seed frío (`no-seed`), builds seriales, `$(rustup which cargo)`, `env -u RUSTC_WRAPPER`, `CARGO_BUILD_JOBS=1`, cap de memoria `systemd-run --user --scope -p MemoryMax=2G`; nextest sin CPUQuota y con el bin del toolchain prepuesto al PATH.
- L3: Incidente de toolchain (nueva variante de #2294): SIN direnv, el `rustc` que cargo invoca por PATH resuelve por los shims de mise, que instalaron rustc 1.65.0 en medio del build (`-Z unstable-options`/check-cfg falla). Corrección: preponer al PATH `$(dirname "$(rustup which cargo)")` ANTES de systemd-run, no basta `$(rustup which cargo)` para el cargo externo. Target propio borrado y rebuild limpio (artefactos mezclados de dos rustc).
