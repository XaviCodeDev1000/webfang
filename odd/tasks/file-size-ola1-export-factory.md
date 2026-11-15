# Ola 1 — export_factory.rs (monstruo del programa de file-size, paraguas #2584)

Misión ODD del programa `odd/tasks/file-size-reduction.md` (T2 — Ola 1, monstruos >2000,
una misión/PR-sequence por monstruo). Es el **segundo** monstruo de la Ola 1: `engine.rs`
salió del baseline en PR 14 de su misión (478 líneas, `c0fb547d`); tras esa salida,
`export_factory.rs` quedó como el peor del baseline.

Ratchet: `scripts/check_file_size.sh` + harness `scripts/test_file_size.sh`.
Fuente de verdad del conteo: `file_size_rust.grandfathered` en `scripts/quality-baselines.json`.

## Specs

S1 (pedido, verbatim): "adelante" (2026-11-14, tras presentar que la siguiente fase es el
siguiente monstruo de la Ola 1: `export_factory.rs`).
S2 (método): el método completo, el protocolo de dos sombreros y las reglas de evidencia
viven en `odd/tasks/file-size-reduction.md` — este registro no los reescribe.
S3 (entrega): push y PR solo con confirmación explícita del usuario; PR con
`Closes part of #2584`, label `type:refactor`, base `main`, entrega secuencial sin PRs
apiladas (`check-topology.sh`).
S4 (re-medición, regla S10 de la misión engine): los spans y conteos se re-miden sobre el
árbol del corte antes de cada movimiento; la prosa de este registro puede quedar stale.
S5 (destino de tests, regla L12 de la misión engine): TODO módulo de tests extraído va a un
archivo hijo propio ≤500 (`#[cfg(test)] mod tests;` declarado en el hijo, posición
ALFABÉTICA de `mod` — `reorder_modules` de rustfmt lo exige) o subcarpeta; un destino inline
que lleve a un archivo no-grandfathered por encima de 500 = ROJO en `check_file_size.sh`.

## Estructura medida (2026-11-14, sobre `main` @ `8aecddc2`)

- Total: **2643 líneas** (grandfathered, banda Ola 1).
- Producción: L1–L1293 (**1293 líneas**), tests inline `#[cfg(test)] mod tests`: L1294–L2643
  (**1350 líneas**, 27 tests + helpers).
- Drivers reales identificados (candidatos a seam, a validar por blast radius antes de cada corte):
  1. **`CommitSession`** (L99–L486, ~390 líneas): ciclo de vida de commit/resume/persist
     (`open`, `decide`, `claim_append`, `commit_item`, `merge_persist`, `final_persist`,
     `save_*`), con `ItemDecision`, `ResumeContext` (L49) y helpers libres `now_millis` /
     `drive_to_processed` (L488–L575) pegados al mismo seam de estado de records.
  2. **Índice JSONL de resume** (L577–L797, ~220 líneas): `JsonlIndexReport`,
     `JsonlChecksumIndex`, `read_jsonl_checksum_index`, `build_content_hash_index`,
     `jsonl_index_report`, `log_io_error_with_kind`.
  3. **Fábrica de exporters** (L799–L874, ~75 líneas): `create_exporter` + variantes
     jsonl/vector/auto/detected/default + `classify_export_failure`.
  4. **Pipeline de export** (L876–L1085, ~210 líneas): `SingleItemOutcome`,
     `process_single_item`, `process_results`, `ExportOutcome`, `domain_from_url`.
  5. **Cluster AI chunks** (L1087–L1292, ~205 líneas): `AiChunkOutcome`, `plan_ai_chunks`,
     `commit_ai_outcomes`, `process_results_with_chunks`.

## Estrategia prevista (secuencia de PRs, cada uno re-mide S4)

- **PR 1**: extraer `mod tests` inline (1350 líneas) a archivos hijos ≤500 por temas
  (precedente engine.rs PRs 6–14): p. ej. `export_factory/tests_process_results.rs`,
  `tests_auto_format.rs`, `tests_resume.rs`, `tests_ai_chunks.rs` + `tests_common.rs` si hay
  base compartida (`make_scraped_content`, `export_urls`, helpers de tempdir/wiremock).
  Produciría export_factory.rs ≈1300 → varias partes <500 ⇒ probablemente 1–2 entradas
  nuevas <=500 y el hub bajo el límite o cerca.
- **PR 2..N**: splits de producción por los seams 1–5 (extract module con `pub use` /
  re-exports, visibilidad mínima, cero widenings no justificados por el compilador),
  hasta `--remove` de la entrada.

## Tasks

| ID | Tarea | Specs | Ruta | Estado | Commit |
|----|-------|-------|------|--------|--------|
| T1 | CodeGraph explore + outline + estructura medida | S4 | inline | done | (sin commit: no toca código) |
| T2 | Registro de misión + espejo Engram | S2 | inline | done | (este archivo) |
| T3 | Propuesta de slice PR 1 al maintainer | S1,S3 | inline | pending | — |
| T4 | PR 1: `mod tests` inline → archivos hijos ≤500 | S5 | worktree propio | pending | — |
| T5 | PRs siguientes: seams de producción 1–5 | S2,S4 | worktree propio | pending | — |

## Log

L1 — Pedido (verbatim, 2026-11-14): "adelante", tras el informe de fase: Ola 0 cerrada
(PR #2619 mergeado, 3 excepciones declaradas), engine.rs salió del baseline (478 líneas),
y el siguiente monstruo de la Ola 1 es `export_factory.rs` (2643).
L2 — Medición inicial (S4, árbol `8aecddc2`): 1293 de producción + 1350 de tests inline;
seams 1–5 identificados en la sección de estructura. Propuesta de PR 1: partir el `mod
tests` primero (mueve ~1350 líneas de golpe, bajo riesgo, precedentes en engine.rs) antes
de los seams de producción.
