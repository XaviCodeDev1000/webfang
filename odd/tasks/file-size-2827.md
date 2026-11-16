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
| T1 | Worktree + bootstrap + índices + este doc + espejo Engram | S1 | inline | inicial |
| T2 | processor.rs: extraer `mod tests` (563–1253) → `processor_tests.rs` (hasta ~891) + `processor_run_tests.rs` (desde ~892) vía `#[path]`; baseline `--update` 1253→medido | S4,S5,S8,S9 | inline | TBD |
| T3 | processor.rs: corte producción 319–537 → `batch/processor_url.rs` (`mod` en `batch/mod.rs`, ítems `pub(super)`); re-point de imports; baseline `--remove` | S3,S4,S6,S7,S8 | inline | TBD |
| T4 | crawl_task.rs: extraer `mod tests` (454–1460) → 4 archivos `crawl_task_*_tests.rs` por tema vía `#[path]`; baseline `--remove` (queda ≤500) | S4,S5,S8,S9 | inline | TBD |
| T5 | crawl_task.rs: corte 31–81 → `crawler/crawl_task_errors.rs` + re-export `pub(crate) use` | S3,S6,S7 | inline | TBD |
| T6 | crawl_task.rs: corte 240–352 → `crawler/crawl_task_fetch.rs` | S4,S7,S8 | inline | TBD |
| T7 | crawl_task.rs: corte 354–453 → `crawler/crawl_task_pipeline.rs` | S4,S7,S8 | inline | TBD |
| T8 | Golden intra-crate refrescado en el mismo commit + jscpd + compuertas S10 + ci_fast_gate GREEN | S9,S10 | inline | TBD |
| T9 | Reporte: listas `Movido sin cambios` / `Corregido / mejorado` + resultado explícito por lentes | S4 | inline | — |

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
