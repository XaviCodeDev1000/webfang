# Ola 1 — Misión 1: `application/crawler/engine.rs` (paraguas #2584)

Objetivo: partir `engine.rs` (4063 en el baseline / 4064 en disco, el peor y el hot path) por seams reales hasta que todas las partes queden ≤500 y la entrada salga del baseline con `--remove`. Branch: `refactor/2584-ola1-engine` desde `origin/main` 840cc94d; worktree `~/Projects/Rust/webfang-worktrees/refactor-2584-ola1-engine`. Entrega: una misión/PR por monstruo, SIN batch; este monstruo puede llevar 2–3 PRs secuenciales propios (sin PRs apilados). Ratchet: `scripts/check_file_size.sh`; verificación dupla con `jscpd --complexity` (5.4.0 pinneado). Test runner: `cargo nextest run`.
Plan de record: `odd/tasks/file-size-reduction.md` — método, dos sombreros y gobernanza viven ahí; este registro no los reescribe.
Estado: **PR 1 implementado y verificado localmente** — `EngineOptions` → `engine/options.rs` (171 líneas) y las primitivas de pausa (`WaitOutcome`/`PauseVerdict`/`PauseWatchdog`/`PauseTracker`) → `engine/pause.rs` (110 líneas). `engine.rs` 4063→3814 líneas (sigue grandfathered, >2000). `cargo check -p webfang_core` limpio; tests unitarios del módulo `engine` verdes 40/40; gates OK (duplicación 8316≤8325, golden refrescado). Falta: tests de integración EngineOptions (corriendo) + commit de unidad + verificación por 4 lentes (T5, pre-PR). Push/PR con confirmación explícita (S8).

## Specs

S1 — Pedido original del programa (verbatim, 2026-11-06): "queda del programa (cuando quieras arrancarlo): Ola 1 — los 9 monstruos >2000, empezando por application/crawler/engine.rs (el peor y el hot path; puede llevar 2-3 PRs él solo). Una misión/PR por monstruo, sin batch."

S2 — Autorización (verbatim, 2026-11-06): "adelante arrancamos"

S3 — Plan de record, T2 (verbatim): "**T2 — Ola 1, monstruos >2000:** uno por misión/PR. `application/crawler/engine.rs` primero — el peor y el hot path; puede requerir 2–3 PRs secuenciales solo él."

S4 — Plan de record, gobernanza (verbatim): "Batching por ola: Ola 0 se batchea (misma base, archivos disjuntos — `scripts/ci_pr_overlap.sh` verifica el no-solape; ~4 archivos por batch); Ola 1 es una misión por monstruo, sin batch (engine.rs puede llevar 2–3 PRs propios)".

S5 — Método, paso 1 (verbatim): "**CodeGraph explore** (callers + blast radius) antes de tocar — nunca editar a ciegas."

S6 — Principios (verbatim): "Split por seams reales, nunca por aritmética de líneas. Un split que rompe cohesión es peor que el archivo gordo, y el reviewer lo ve." · "Comportamiento preservado: `refactor(scope)` puro, tests verdes por unidad de trabajo, snapshots intactos. El descenso del baseline viaja en el MISMO PR que el split que lo produce". · "Objetivo de cada split: todas las partes <=500 -> `--remove` de la entrada original."

S7 — Herencia de la Misión Ola 0, marcada como SUPUESTO: su L1 pedía "Antes de editar una línea: presentame el batch y el seam plan por archivo (CodeGraph explore: callers + blast radius + driver real) para mi OK." Se aplica el mismo gate acá por tratarse del hot path; si el usuario lo descarta, se elimina este S# y se ejecuta sin gate.

S8 — Restricciones ya vigentes en el programa: baseline solo baja (`--update` descendente / `--remove`); un grandfathered tocado y no partido termina neto-cero; un commit que toque `crates/**/src` refresca el golden `tools/intra-crate-lint/tests/fixtures/real_tree_sites.txt` en el MISMO commit (`scripts/refresh_real_tree_golden.sh`); cada split declara en el PR `Movido sin cambios` y `Corregido / mejorado`; commits convencionales sin atribución IA; push y PR solo con confirmación explícita.

## Tasks

Propuesta de slice para el PR 1 (pendiente de OK, S7). Un commit de unidad por extracción, tests verdes entre unidades, movimiento mecánico primero.

| ID | Tarea | Specs | Ruta | Estado | Commit |
|----|-------|-------|------|--------|--------|
| T0 | CodeGraph explore + codedb outline: callers, blast radius, driver real | S5 | inline | done | (evidencia en Log L3; sin commit: no toca código) |
| T1 | Registro de misión + espejo Engram | S1,S2 | inline | done | (este archivo; sin commit aún) |
| T2 | `EngineOptions` + `Debug` + `Default` → `engine/options.rs` (171 líneas), con `pub use engine::EngineOptions` | S3,S6,S8 | delegada al padre (inline) | done | 834af8bd |
| T3 | Primitivas de pausa: `WaitOutcome`, `PauseVerdict`, `PauseWatchdog`, `PauseTracker` → `engine/pause.rs` (110 líneas), con `pub use` desde `engine.rs` | S3,S6,S8 | delegada al padre (inline) | done | 834af8bd |
| T4 | Ratchet: `engine.rs` 4063→3814 (`--update`, sigue grandfathered); duplicación `jscpd --complexity` 8316≤8325 OK; golden intra-crate refrescado; suite verde | S6,S8 | delegada al padre (inline) | done | 834af8bd |
| T5 | Verificación por cuatro lentes sobre lo movido; resultado explícito en el PR | S6 | — | pendiente (pre-PR, requiere OK) | — |
| T6 | PR 2: cluster de checkpoint (12 métodos en 4 bloques, ~196 líneas) → `engine/checkpoint.rs` (hijo), `impl Engine` en el hijo; visibilidad mínima que exija el límite de módulo | S3,S6,S8 | delegada al padre (inline) | en ejecución | — |
| T7 | PR 3+ (sin planificar): lifecycle/run/shutdown (~400), crawl-loop/wait (~300), entry points (~310), `mod tests` inline (~1843) | S3,S4 | propuesta | sin planificar | — |

Forecast: PR 1 mueve ~220 líneas ⇒ ~440 authored (borrar+crear), en la banda de la heurística de ~400. El `mod tests` (1843) queda fuera del PR 1 a propósito: moverlo entero haría el diff no revisable, y el criterio de Ola 0 S12 prefiere seam de producción cuando existe y es limpio.

## Log

L1 — Pedido del programa (verbatim): "queda del programa (cuando quieras arrancarlo): Ola 1 — los 9 monstruos >2000, empezando por application/crawler/engine.rs (el peor y el hot path; puede llevar 2-3 PRs él solo). Una misión/PR por monstruo, sin batch."

L2 — Autorización (verbatim): "adelante arrancamos".

L3 — Evidencia de exploración sobre `840cc94d` (CodeGraph explore + codedb outline, 2026-11-06). Estructura real de `crates/webfang_core/src/application/crawler/engine.rs` (4064 líneas):

- L1–L63 imports (31) y `ShutdownSignal` (L64).
- L73–L122 `TerminationOutcome` (+`from_wait`/`stops_run`) y `TERMINATION_REGISTRATION_REJECTED` (L121).
- L130–L157 constantes `SHUTDOWN_GRACE_SECS`, `FAILED_RUN_DRAIN_GRACE`, `FETCH_TIMEOUT_PHASES`, `IDLE_POLL_INTERVAL`.
- L165–L258 `WaitOutcome`, `PauseVerdict`, `PauseWatchdog` (+`from_config`/`verdict`), `PauseTracker` (+`elapsed`/`reset`).
- L265–L361 `Engine` (87 líneas) y `rate_limiter_config`.
- L362–L1695 `impl Engine` (1334 líneas, 43 métodos), con cuatro clusters: builders/construcción (`build_machinery` 373, `from_session` 473, `with_*` 557–819), checkpoint (`with_persistence` 557, `with_checkpoint` 574, `load_checkpoint_state` 596, `save_checkpoint` 820, `build_checkpoint_state` 828, `persist_checkpoint` 856, `restore_*` 1166–1237, `maybe_save_periodic_checkpoint` 1455), lifecycle/run/shutdown (`spawn_signal_handler` 885, `cancel_run` 922, `cancel_handle` 932, `set_shared_limiter` 942, `run` 954, `collect_result_summary` 1131, `fail_run_with_partials` 1151, `worker_wait_bound` 1238, `record_run_outcome` 1570, `collect_error_breakdown` 1584, `log_crawl_summary` 1596, `drain_tasks` 1644, `shutdown` 1679), crawl-loop/wait (`crawl_loop` 1267, `is_drained` 1355, `wait_or_stop` 1366, `wait_out_pause` 1400, `stalled` 1438, `process_completed_tasks` 1448, `remaining_budget` 1487, `spawn_available_tasks` 1498, `wait_for_task` 1532).
- L1721–L1847 `EngineOptions` (73) + `Debug` (52) + `Default`.
- L1911–L2220 seis entry points: `crawl_site`, `crawl_site_capturing`, `build_crawl_session`, `crawl_site_inner`, `crawl_site_with_options`, `crawl_site_with_options_inner`.
- L2221–L4064 `mod tests` inline (~1843 líneas, ~45% del archivo).

**Driver real:** "varios módulos pegados", no una función gigante ni un `match` enorme ⇒ estrategia `extract module`. El cluster de checkpoint tiene destino natural: ya existe el módulo hermano `crawler::checkpoint` con los tipos de store (`BincodeCheckpoint`, `CheckpointPath`, `CheckpointStore`, `CrawlCheckpoint`, `BannedDomain`, `CURRENT_CHECKPOINT_VERSION`) que `engine.rs` importa en L33–L36.

**Blast radius verificado:**
- `crates/webfang_core/src/application/crawler/mod.rs` L14 `pub mod engine;`; L28 `pub use engine::{crawl_site_with_options, EngineOptions};`; L34 `pub use engine::{crawl_site, crawl_site_capturing};`. Todo el consumo externo pasa por esos re-exports ⇒ cualquier extracción debe preservarlos (`pub use` desde `engine.rs`, sin tocar `mod.rs`).
- `crates/webfang_core/src/cli/shutdown.rs` L16 importa `TerminationOutcome` y `TERMINATION_REGISTRATION_REJECTED` desde `crate::application::crawler::engine::` ⇒ mover esos dos símbolos exige re-export o editar ese archivo. Quedan FUERA del PR 1 por eso.
- CodeGraph: `spawn_signal_handler` 4 callers, `with_autoscale` 3, `crawl_loop` 5, `TERMINATION_REGISTRATION_REJECTED` 1 — todos dentro de `engine.rs`. `crawl_site` tiene 1 call site (L2836, dentro de su propio test `deprecated_crawl_site_shim_matches_explicit_options_entry`), así que la API pública se consume vía re-exports.
- Restricción de visibilidad: los `impl Engine` leen campos privados de `Engine`, así que los módulos extraídos deben ser **hijos** de `engine` (`engine/*.rs`), no hermanos en `crawler/`. Un módulo hijo sí accede a los privados del padre.

L4 — Decisiones de encuadre: (a) SIN batch, confirmado contra S4 y contra el cierre de Ola 0 (`main` en `840cc94d` = PR #2619); (b) `error.rs` queda para SU misión, no entra acá; (c) sin Agent tool disponible en esta sesión (ToolSearch devuelve sólo `Workflow`, que exige opt-in explícito), la exploración se hizo inline con CodeGraph/CodeDB en llamadas acotadas — degradación documentada, sin worker delegado.

L5 — PR 1 ejecutado y commiteado (834af8bd, 2026-11-06). Movimiento mecánico: `EngineOptions`+`Debug`+`Default` → `engine/options.rs` (171) y las primitivas de pausa (`WaitOutcome`/`PauseVerdict`/`PauseWatchdog`/`PauseTracker`) → `engine/pause.rs` (110); `engine.rs` 4063→3814. `pub use engine::EngineOptions` y `pub use engine::pause::{...}` preservan el path público y el re-export de `crawler/mod.rs`. Visibilidad forzada a `pub` en los ítems movidos (campos y métodos de `PauseTracker`/`PauseWatchdog`, y los tipos) porque el loop y `engine::tests` los referencian a través del límite de módulo — único cambio fuera de "mover sin tocar". Verificación: `cargo check -p webfang_core` limpio; `cargo nextest run -p webfang_core --lib crawler::engine` 40/40 ok; tests de integración `engine_options_1369` + `engine_js_strategy_timeout_test` + `integration_engine_tests` ok; duplicación `jscpd 5.4.0` 8316≤8325; golden intra-crate refrescado en el mismo commit (S8). Ratchet `--update` engine.rs 4063→3814 (sigue grandfathered, >2000). Falta T5 (4 lentes) y push/PR con OK explícito.

L6 — OK del usuario al plan del PR 2 (verbatim: "OK", 2026-11-06). Alcance: cluster de checkpoint → `engine/checkpoint.rs` (hijo). Exploración post-PR 1 (codedb context + explain): 12 métodos en 4 bloques no contiguos de `impl Engine` — builders L454–518 (`with_persistence`/`with_checkpoint`/`load_checkpoint_state`), save-path L723–768 (`save_checkpoint`/`build_checkpoint_state`/`persist_checkpoint`), restore L1068–1125 (`restore_checkpoint_state` + 4 helpers asociados), periódico L1358–1384 (`maybe_save_periodic_checkpoint`). Callers verificados: `with_persistence` solo tests internos; `maybe_save_periodic_checkpoint` ← `crawl_loop`; `restore_checkpoint_state` y `save_checkpoint` ← flujo run/shutdown del padre. `persist_checkpoint` delega en `super::checkpoint::persist_checkpoint_state` — la ruta `super::checkpoint` resuelve igual desde el módulo hijo, sin tocar esos call sites. Visibilidad: los dos builders quedan `pub` (sin cambio); los privados que el padre llama se ensanchan al mínimo que exija el compilador (`pub(crate)` donde alcance — criterio de mínimo, se declara en "Corregido/mejorado" del PR); los 4 helpers `restore_*` asociados quedan privados (sin callers fuera del cluster). Nota: `codegraph` CLI falla (exit 1) en esta sesión; la exploración se hizo con codedb MCP (context/explain) + lecturas acotadas para el texto verbatim de los bloques a cortar.
