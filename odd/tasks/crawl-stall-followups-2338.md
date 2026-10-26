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
