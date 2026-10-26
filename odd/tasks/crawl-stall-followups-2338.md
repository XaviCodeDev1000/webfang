# Mission: crawl-stall-followups-2338

**Issue:** #2338 — chore(engine): follow-ups de la review de #2313
**Branch:** `chore/crawl-stall-followups-2338` (worktree `chore-2338-stall-followups`)
**Base:** main @ e1af93f9

## Specs

- **S1** — "Avanzamos con la implementación dentro de #2338" (todos los follow-ups abiertos de la issue).
- **S2** — Decisión de diseño (usuaria, esta sesión): *"Err que cargue los resultados"* — los resultados parciales viajan en una variante de `CrawlError`; se mantiene la semántica de #2313 (run fallido, checkpoint para resume, exit code de error); el CLI escribe los parciales antes de reportar el fallo.
- **S3** — Issue #2338, criterios de aceptación: mensaje de stall con aserción (ya resuelto); decisión escrita e implementada sobre parciales; test e2e de `run()`; slack del bound cubre la vida completa del worker; `give_up_after` configurable o 30 min justificado; latencia del stall es un bound, no dos.

## Tasks

- T1: `CrawlError` variante con resultados parciales; `Engine::run()` los colecta en Stalled/GiveUp antes de retornar; CLI los escribe; exit code de error intacto. Test RED primero.
- T2: Test e2e que fija que `run()` propaga el stall (hoy solo se testea `crawl_loop` directo).
- T3: Latencia del stall con un solo bound (hoy: espera del loop + drain con bound completo = 2×).
- T4: `worker_wait_bound` cubre o documenta el tramo pipeline post-fetch.
- T5: `give_up_after` configurable en `CrawlerConfig` (o justificar el 30 min).
- T6: Gate completo (ci_fast_gate) + PR hacia main con `Closes #2338`.

## Log

- L1: Issue #2338 abierta con los 6 follow-ups de la review de #2313 (2026-10-25).
- L2: Análisis verificado contra main post-#2339: #2/#3/#5 pendientes confirmados, #4/#6 parcialmente resueltos (obs #2507 Engram).
- L3: Decisión de diseño S2 tomada (Err que cargue los resultados). Worktree creado e indexado.
- L4: Commit `835cfe0e` — variante `CrawlError::PartialRun` + classify + categoría + From + fila 34 de la matriz (código y doc de la matriz: contrato obligatorio del enum, no scope creep).
- L5: Commit `9ac75cfb` — `Engine::run` envuelve el error del loop con parciales; CLI intercepta `PartialRun` en `discover_urls_unified`, enhebra `truncated_by` por `DiscoveryOutput`/`PrepareResult` y el orquestador reporta el fallo (69) DESPUÉS del export parcial. Dry-run igual. Tests RED→GREEN: `stalled_run_returns_partial_results_in_the_error` (wedge async por `PipelineStage` pendiente — un bloqueo sync no es abortable y colgaría el drain) y `fail_run_with_partials_carries_collected_urls`.
- L6: Commit `0fa6200c` — slice 3: drain post-fallo con gracia de 1s (`FAILED_RUN_DRAIN_GRACE`) en vez del bound completo; latencia de fallo medida 24s → 13s; aserción de elapsed pina el contrato.
- L7: Commit `7b1fa2b4` — slice 5: `pause_alert_after_secs`/`pause_give_up_after_secs` en `CrawlerConfig` (defaults 300/1800; `build` clampea give-up ≥ alert); `PauseWatchdog::from_config` en `run()`. Superficie CLI flag: decisión deliberada de NO incluirla en este slice (superficie programática batch/MCP basta; flag = plumbing separado).
- L8: Commit `4f055b9f` — slice 4: criterio "cubre O documenta" cerrado por la rama documental: el tramo pipeline queda documentado como riesgo residual en `worker_wait_bound` y en el call site (un constante sin medición sería fabricación).
- L9: Pendiente: gate completo + PR. El camino sitemap del CLI no corre el Engine (descubrimiento por XML + scrape fase), así que `PartialRun` solo alcanza el flujo DOM/batch — sin cambio de comportamiento allí.
