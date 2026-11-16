# Misión #2827 (Ola 3 del programa #2584): partir `processor.rs` y `crawl_task.rs` por debajo de su baseline

Split puro (movimiento verbatim por seams) de los dos archivos grandfathered que el
ratchet congela en su conteo exacto, para liberar headroom y que el desglose por
causa (#2824 / paso 2 de #2827) pueda enchufarse después. Base: `main@c12d1ac8`.
Worktree: `~/Projects/Rust/webfang-worktrees/refactor-2827-split-processor-crawl-task`
(rama `refactor/2827-split-processor-crawl-task`).

## Specs

**S1 — Alcance autorizado (verbatim del pedido):** "issue #2827 primero si querés que
#2824 llegue hasta el resumen de batch. Es el que parte processor.rs y crawl_task.rs
(los dos grandfathered que no pueden crecer ni una línea), y sin eso el desglose por
causa no entra." ⇒ esta misión ejecuta el paso 1 del orden sugerido del issue #2827:
"Partir `processor.rs` y `crawl_task.rs` (los dos grandfathered que no pueden crecer)."

**S2 — Criterio de aceptación del split (verbatim del issue #2827):** "`processor.rs` y
`crawl_task.rs` por debajo de su baseline, sin subir ninguno." + política del programa:
"La política del repo es **bajar** baselines, no subirlas." ⇒ objetivo: ambas entradas
salen del baseline con `--remove` (cada archivo queda ≤500).

**S3 — El desglose por causa queda para el PR siguiente** (verbatim del issue: "En el
mismo PR, o en el siguiente, agregar el desglose por causa y usarlo en el error
sintético del batch."). Marca pendiente de maintainer (verbatim): "boxear `CrawlResult`
en `PartialRun` es la salida si el desglose se queda (rompe la forma de un variant
público: decisión de maintainer)". ⇒ NO si se toca `CrawlError`/`CrawlResult` en esta
misión; el split ubica `handle_task_error` en `crawl_task_errors.rs` y
`process_single_url` + builders en `processor_url.rs` (módulos nuevos con headroom).

**S4 — Método (plan of record `odd/tasks/file-size-reduction.md`):** split por seams
reales, nunca por aritmética de líneas; movimiento verbatim demostrado (diff
normalizado: strip indent + drop blanks, igualdad línea a línea por bloque); dos
sombreros (cero cambio semántico en estos commits); partes nuevas ≤500; el descenso
del baseline viaja en el MISMO commit que el corte; verificación doble guard (líneas)
+ `jscpd --complexity` (binario pinneado 5.4.0) sobre los tocados.

**S5 — Tests:** los `mod tests` inline se extraen a archivos hermanos nombrados
`*_tests.rs`, declarados desde el módulo dueño con `#[cfg(test)] #[path = "..."]`,
cada uno ≤500 y partido por tema. Invariante `--list`: mismo conjunto de nombres de
hoja + mismo conteo total, con la regla de renombre declarada en el reporte. Ningún
archivo de test nuevo dentro de un directorio `tests/` (prohibido: esconde la deuda).

**S6 — Paths públicos estables:** `BatchProcessor`/`BatchResult`/`BatchError` quedan en
`processor.rs` (`batch/mod.rs` re-exporta `pub use processor::{BatchProcessor,
BatchResult}` y `manager.rs` importa `super::processor::{BatchError, ...}` sin cambio);
`crawler::crawl_task::{handle_crawl_result, run_crawl_task}` se preserva con
`pub(crate) use crawl_task_errors::handle_crawl_result;` dentro de `crawl_task.rs`.

**S7 — Visibilidad:** sólo los ítems movidos cambian a `pub(super)`; el diff de
modificadores de visibilidad se reporta y debe ser exactamente esa lista (precedente:
`odd/tasks/file-size-t2-ssrf-guard.md` L5-1). Cero ensanches más allá.

**S8 — Re-medición antes de cada corte:** ningún span se copia de la prosa: se re-mide
en el árbol del corte con script de sentinelas que aborta si el cierre del bloque no
está donde se midió.

**S9 — Superficies de drift:** `scripts/quality-baselines.json` (entradas
`crates/webfang_core/src/application/batch/processor.rs`=1253 y
`crates/webfang_core/src/application/crawler/crawl_task.rs`=1460; sólo bajan);
`tools/intra-crate-lint/tests/fixtures/real_tree_sites.txt` se refresca con
`bash scripts/refresh_real_tree_golden.sh` en el MISMO commit del corte que mueve
líneas; `docs/test-inventory.json` no driftea (el generador sólo camina
`crates/*/tests/**`); `scripts/concurrency_lint_allowlist.txt` (clave
`processor.rs|max_concurrent`) sigue válido porque `max_concurrent` se queda en
`processor.rs`.

**S10 — Compuertas (idénticas a CI, por commit/PR):** `cargo check --workspace
--all-targets --all-features` 0 warnings · `cargo clippy --all-targets --all-features
-- -D warnings -W clippy::cognitive_complexity -W clippy::too_many_lines` · `cargo fmt
--all -- --check` · `env "RUSTDOCFLAGS=-D warnings" cargo doc --workspace --all-features
--no-deps` · suite nextest completa verde · invariante `--list` · verbatim por bloque ·
`scripts/check_file_size.sh` verde · `scripts/check_duplication.sh` (cx no sube) ·
`scripts/check_test_inventory.sh` · `scripts/check_dead_tests.sh` ·
`scripts/check_dependency_direction.sh` + `scripts/check_intra_crate_direction.sh` ·
`bash scripts/ci_fast_gate.sh` GREEN con el reporte de cobertura leído.

## Tasks

| ID | Tarea | Specs | Ruta | Commit |
|----|-------|-------|------|--------|
| T1 | Worktree + bootstrap + índices + este doc + espejo Engram | S1 | inline | (inicial, en T2) |
| T2 | processor.rs: tests inline → `processor_tests.rs` + `processor_run_tests.rs` vía `#[path]`; baseline `--update` 1253→570 | S4,S5,S8,S9 | inline | b366ea5a |
| T3 | processor.rs: corte producción 319–537 → `batch/processor_url.rs`; baseline `--remove` | S3,S4,S6,S7,S8 | worker mwinyavg-2-3di8 | 99237d06 |
| T4 | crawl_task.rs: tests inline → 4 `crawl_task_*_tests.rs`; baseline `--remove` | S4,S5,S8,S9 | worker mwiouwfi-3-n42s (timeout en verificación; parent la terminó) | e0ac0547 |
| T5 | crawl_task.rs: corte 31–81 → `crawl_task_errors.rs` + `pub(crate) use` | S3,S6,S7 | worker mwjb7tlk-4-7899 | 0428c782 |
| T6 | crawl_task.rs: corte 185–300 → `crawl_task_fetch.rs` | S4,S7,S8 | worker mwjdb3df-5-t97c | 12fcc7fb |
| T7 | crawl_task.rs: corte 182–281 → `crawl_task_pipeline.rs` | S4,S7,S8 | worker mwjgaas9-6-juog | f613bbf9 |
| T8 | fix intra-crate (UrlQueue a cfg(test) del padre) + golden en el mismo commit; compuertas S10; fast gate GREEN | S9,S10 | inline | df2aec10 |
| T9 | skill symbol map + reporte + push/PR | S4 | inline | 0000e4d6 + (doc/PR) |

## Log

- **L1 (verbatim del pedido del usuario)**: "issue #2827 primero si querés que #2824
  llegue hasta el resumen de batch. Es el que parte processor.rs y crawl_task.rs (los
  dos grandfathered que no pueden crecer ni una línea), y sin eso el desglose por
  causa no entra. Es el trabajo más grande de los cuatro."
- **L2**: Relevamiento delegado a `gentle-ai-explore` (task `mwimlcry-1-2qtr`): mapa de
  estructura + seams + callers/blast radius vía CodeGraph. Hallazgo: ya existe el
  worktree `fix-2824-cause-clone` (sesión paralela sobre #2824) — esta misión es el
  prerrequisito y no lo toca. Spans re-medidos en el worktree antes de cada corte (S8).
- **L3**: El guard del harness bloquea `.env`/`.envrc` por patrón de path ⇒ no si crea
  `.envrc`; el aislamiento del target se hace con `CARGO_TARGET_DIR` inline
  (`/home/xavi/.cache/cargo-target/refactor-2827-split-processor-crawl-task`), que es lo
  que el guard verifica. Seed: `cold reason=no-seed` (resultado normal). Build warm:
  `cargo check --workspace --all-targets --all-features` OK (~60s con mbx).
- **L4 — T2 ejecutado (corte de tests de `processor.rs`)**: script de sentinelas OK;
  `processor.rs` 1253→570 (post-fmt), `mod tests` inline reemplazado por
  `#[cfg(test)] #[path = "processor_tests.rs"] mod tests;` (cuerpo 579–890 + uses) y
  `#[cfg(test)] #[path = "processor_run_tests.rs"] mod tests_run;` (entry_off 566–572 +
  uses recortados a lo que usa + cuerpo 892–1252). Archivos nuevos: 317 y 375 líneas
  (≤500 ✓). Verbatim: diff normalizado (strip todo indent + drop blanks) vacío en A y
  en el header de B; en el cuerpo de B sólo 1 línea que `rustfmt` re-unió al dedentar
  (`ResponseTemplate::new(200).set_body_string(...)`) — colapso de formato, sin cambio
  de lógica. Conteo de atributos de test 29 = 29. Renombre declarado: los tests de
  `tests_run` pasan de `processor::tests::*` a `processor::tests_run::*` (los de
  `processor_tests.rs` conservan el path `processor::tests::*`). Baseline:
  `--update` 1253→570 descendente en el mismo cambio; `check_file_size.sh` GREEN.
  Única línea no verbatim además de los `mod`: el bloque `use` recortado de
  `processor_run_tests.rs` (`BatchJob` solo; el import atomic queda sólo ahí).
- **L5 — T2 commiteado como `b366ea5a`** (5 archivos, slice de 1505 líneas
  principalmente movimiento verbatim de tests — boundary de slice registrado acá;
  los slices siguientes son cortes de producción más chicos). Gates del commit:
  check/clippy estricto/fmt limpios, nextest `batch::processor` 30/30 (20 en
  `tests::` + 10 en `tests_run::`), ratchet GREEN con baseline 570. Context backstop
  del parent disparado (152k tokens) ⇒ T3..T7 se delegan a `gentle-ai-worker` con
  surfaces acotados; el parent commitea. Patrón: los workers NUNCA tocan
  `quality-baselines.json` ni el golden — el parent hace `--remove`/`--update` y el
  refresh en el mismo commit (evita que dos workers toquen el JSON en paralelo).
- **L6 — T3 ejecutado por `gentle-ai-worker` (task `mwinyavg-2-3di8`), verificado por el
  parent**: bloque 319–537 movido verbatim a `batch/processor_url.rs` (240 líneas; los 4
  fns a `pub(super)` = único diff de visibilidad); `processor.rs` 570→348 (≤500) ⇒
  entrada fuera del baseline con `--remove` en el mismo cambio; golden refrescado (15
  sitios nuevos a processor_url). Dos líneas de doc-link declaradas (rustc marca como
  unused los imports usados sólo por intra-doc links bajo -D warnings):
  [`EngineOptions`] → path completo en el module doc de processor.rs;
  [`BatchProcessor::with_content_sink`] → `super::processor::...` en processor_url.rs.
  +1 línea de import en `processor_run_tests.rs` (probada por cargo check).
  Verbatim: diff normalizado vacío (209/209 líneas). Gates: check / clippy estricto /
  fmt / doc limpios; nextest `batch::processor` 30/30; ratchet GREEN.
  `processor_url.rs` es el módulo con headroom donde aterriza después el adjunto de
  causa dominante (#2824 / paso 2 de #2827).
- **L7 — T4 ejecutado por `gentle-ai-worker` (task `mwiouwfi-3-n42s`, timeouteó en la
  verificación pero el corte quedó completo; el parent terminó la verificación y
  commiteó)**: `mod tests` inline de `crawl_task.rs` (454–1460) → 4 archivos vía
  `#[cfg(all(test, not(miri)))] #[path]`: `crawl_task_support_tests.rs` (358),
  `crawl_task_fetch_tests.rs` (201), `crawl_task_pipeline_tests.rs` (292),
  `crawl_task_errors_tests.rs` (171) — todos ≤500. `crawl_task.rs` 1460→468 (≤500) ⇒
  entrada fuera del baseline con `--remove` en el mismo commit (hecho por el parent).
  Verbatim post-fmt: sólo líneas de continuación de bloques `use` reubicadas (cada
  archivo tiene su propio header) + joins de rustfmt — sin cambio de lógica. Tests
  33 = 33; nextest `crawl_task` 33/33. Gates: fmt/check/clippy estricto/doc limpios.
  Renombre declarado: `crawl_task::tests::*` → `crawl_task::tests_{support,fetch,
  pipeline,errors}::*`. Lección: el worker stalleó >4 min en un `cargo` largo sin
  salida (mbx reconstruyendo) — los gates del parent con salida acotada salieron en 6s
  con la cache tibia.
- **L8 — T5 ejecutado por `gentle-ai-worker` (task `mwjb7tlk-4-7899`), verificado por el
  parent**: bloque 31–81 (`handle_crawl_result` pub(crate) + `handle_task_error` +
  `handle_join_error`) movido byte-idéntico a `crawl_task_errors.rs` (65 líneas);
  `crawl_task.rs` 468→417; re-export `pub(crate) use super::crawl_task_errors::
  handle_crawl_result;` preserva el path externo (crawl_loop.rs / lifecycle.rs sin
  tocar). Diff de visibilidad VACÍO (S7). Verbatim: cmp byte-idéntico + diff
  normalizado vacío. Gates: check/clippy estricto/fmt/doc limpios; nextest
  `crawl_task` 33/33. Golden refrescado en el mismo commit (sitios 175/193/220
  driftean a 124/142/169). `crawl_task_errors.rs` es el módulo con headroom para los
  contadores por `NetworkCause` (#2824 / paso 2 de #2827). Nota: `mod crawl_task_errors;`
  quedó tras `pub mod crawl_task_ctx;` porque rustfmt ordena los mods alfabéticamente.
- **L9 — T6 ejecutado por `gentle-ai-worker` (task `mwjdb3df-5-t97c`), verificado por el
  parent**: bloque re-medido 185–300 (los 6 helpers fetch/session/WAF/cookies) movido
  verbatim a `crawl_task_fetch.rs` (142 líneas); `crawl_task.rs` 417→297 post-fmt.
  Diff de visibilidad = exactamente los 6 fns a `pub(super)` (S7). Verbatim a nivel de
  tokens: único delta no normalizado = una coma final que rustfmt re-wrappeó en la firma
  de `report_session_success` (misma clase que L4). `crawl_task_fetch_tests.rs` sin
  tocar (cero llamadas directas a los helpers desde tests, confirmado con
  codegraph/codedb). Gates: check/clippy estricto/fmt/doc limpios; nextest `crawl_task`
  33/33. Golden refrescado en el mismo commit. Seguridad: WAF ban + cookie bridge
  movidos verbatim (guard chain).
- **L10 — T7 ejecutado por `gentle-ai-worker` (task `mwjgaas9-6-juog`), verificado por el
  parent**: bloque 182–281 (`run_pipeline` + `extract_and_queue_links`) movido verbatim
  a `crawl_task_pipeline.rs` (125 líneas); `crawl_task.rs` 297→**201**. Diff de
  visibilidad = exactamente los 2 fns a `pub(super)`. Verbatim a nivel de tokens: único
  delta = la coma final del re-wrap de firma de `run_pipeline` (fmt). El worker añadió 3
  imports `#[cfg(all(test, not(miri)))]` en crawl_task.rs porque los archivos de test
  hermanos resuelven esos nombres vía `use super::*` (probado con cargo check: 17
  E0412/E0433 antes del fix) — desviación declarada, sólo en cfg(test). Tests de
  pipeline sin tocar. Gates: check/clippy estricto/fmt/doc limpios; nextest `crawl_task`
  33/33. Golden en el mismo commit. Robots/is_internal_link/is_allowed movidos
  byte-for-byte. **Ambas entradas fuera del baseline**: processor.rs y crawl_task.rs ya
  no están en `quality-baselines.json`.
- **L11 — T8 (compuertas finales) + fix intra-crate**: suite completa
  `cargo nextest run --workspace --all-features` = **5572 passed, 8 skipped, 0 failed**
  (`--list` 5572); check/clippy estricto/fmt/doc limpios; `check_file_size.sh` GREEN;
  `check_test_inventory.sh`, `check_dead_tests.sh`, `check_dependency_direction.sh` OK;
  `check_duplication.sh` 8157 ≤ 8219 (jscpd 5.4.0); golden harness 8/8. Hallazgo y fix:
  el extractor AST intra-crate barre cada `.rs` por separado y sólo excluye regiones
  cfg(test) del MISMO archivo (B36), así que `use crate::infrastructure::crawler::UrlQueue`
  en `crawl_task_support_tests.rs` era 1 violación ADR-0010 ⇒ se movió a `crawl_task.rs`
  bajo `#[cfg(all(test, not(miri)))]` (llega a los hijos vía `use super::*`); gate en 0
  violaciones SIN tocar el allowlist (commit `df2aec10`, golden refrescado en el mismo
  commit tras el amend). 2 líneas de doc-link a paths completos declaradas (rustc marca
  unused los imports sólo-de-doc-link bajo -D warnings). `ci_fast_gate.sh` (con
  `--allow-unregistered-target` porque el guard del harness bloquea `.envrc`):
  **PASS=44 FAIL=0 SKIP=2** (skips declarados: pinned-lint por scope, windows
  cross-target); reporte de cobertura leído — `test-full` corrido a mano (5572/5572) y
  doc gate corrido a mano.
- **L12 — Cierre**: push de `refactor/2827-split-processor-crawl-task` y PR abierto con
  `Closes part of #2584` + `Closes part of #2827`, label `type:refactor`, base `main`
  (autorizado por el usuario: "push y pr cuando termines todo"). Merge queda en decisión
  del maintainer. El paso 2 de #2827 (desglose por causa: contadores en
  `crawl_task_errors.rs::handle_task_error`, causa dominante en
  `processor_url.rs::process_single_url`) es el PR siguiente; boxear `CrawlResult` en
  `PartialRun` sigue siendo decisión de maintainer. Estado final: processor.rs 348,
  crawl_task.rs 201, ambos ≤500 y fuera del baseline; archivos nuevos: processor_url.rs
  (240), processor_tests.rs (317), processor_run_tests.rs (375), crawl_task_errors.rs
  (65), crawl_task_fetch.rs (142), crawl_task_pipeline.rs (125),
  crawl_task_{support,fetch,pipeline,errors}_tests.rs (358/201/292/171).
