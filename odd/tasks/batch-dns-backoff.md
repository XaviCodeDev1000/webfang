# Feature: #2823 — backoff de batch por causa (resolución ≠ bloqueo)

**Issue:** #2823 (`status:approved`, `type:bug`) — un fallo de resolución no debe gastar la
escalera de trips
**Labels:** `type:bug`
**Branch:** `fix/batch-dns-backoff`
**Worktree:** `~/Projects/Rust/webfang-worktrees/fix-batch-dns-backoff`
**Delivery:** un PR. Cambia comportamiento (reintentos y abort), no solo reporte.
**Runner:** `cargo nextest run -p webfang_core`, `cargo clippy --all-targets --all-features
-- -D warnings`, `cargo fmt --all -- --check`

## Specs

**S1 — Un fallo `Dns` no incrementa el contador de intentos del backoff.**
Cita textual del issue: *"Un `Dns` / `DnsTimeout` no consume la escalera de reintentos: se
reintenta con un backoff fijo y corto, y **no** incrementa el contador de 'el sitio nos
rechaza'."*
Criterio: *"Un fallo `Dns` no incrementa el contador de intentos del backoff."*

Alcance: la escalera exponencial vive en `WreqDownloader::fetch_inner`
(`attempt`/`backoff_delay_ms`). Un fallo de resolución (causa `dns_resolution`, que incluye el
resolver que nunca responde porque `refine_connect_cause` lo convierte) se reintenta con un
presupuesto propio y un retardo fijo; nunca consume `attempt`.

**S2 — Umbral distinto para abortar.**
Cita: *"Si el 100% de los fallos de los últimos N son de resolución, abortar la corrida con un
mensaje que lo diga ("el resolver no responde"), en vez de seguir hasta el final como si fueran
bloqueos."*
Criterio: *"Con un resolver que no responde, la corrida aborta pronto con un mensaje que nombra
la resolución (no 'demasiados bloqueos')."*

Alcance: `application/batch/processor.rs`. Se dejan de despachar URLs cuando la racha de fallos
de resolución llega al umbral, y el resultado/reports nombran la resolución.

**S3 — `BlockedByWebsite` sigue igual.**
Cita: *"Un fallo `BlockedByWebsite` sigue exactamente igual que hoy (no cambia su comportamiento
ni su umbral)."*
Criterio: idem.

Nota: en este árbol no existe la variante `BlockedByWebsite`; la familia que el issue describe
como "el sitio nos rechaza" es `CrawlError::WafChallenge` / `CrawlError::RateLimited` (y el
`Http` 401/403/429). El cambio ramifica SOLO por causa de resolución: ninguna otra rama cambia.

**S4 — Test con un resolver inyectado.**
Cita: *"Test con un resolver inyectado (no un dominio real): el fallo de resolución es
determinista y el test no depende de la red."*
Criterio: idem.

Alcance: el seam `NameSource` ya existe (`infrastructure/ssrf/resolution_pair.rs`). Se usa para
construir un cliente cuyo resolver no responde y se comprueba el comportamiento del downloader.

**S5 — Contadores separados.**
Cita: *"La métrica de 'bloqueos' no debe incluir fallos de resolución: hoy
`CrawlErrorCategory::Network` mezcla los dos mundos y produce métricas contradictorias (esto se
unifica en el issue hermano de taxonomías)."*
Criterio: *"Los contadores de 'bloqueo' y de 'resolución' se reportan por separado."*

Alcance: `BatchResult` expone `resolution_failures` y `block_failures` por separado; el resumen
de `log_batch_summary` los reporta. `CrawlErrorCategory` NO se toca (hermano de taxonomías).

## Decisiones

- Presupuesto de reintentos de resolución: 2 reintentos extra con retardo fijo de 250 ms.
  Justificación: un resolver roto no se arregla esperando; dos intentos cubren un fallo
  transitorio sin inflar la corrida.
- Umbral de abort: 5 fallos de resolución consecutivos. Justificación: por debajo de eso el
  despacho ya está amortizado y abortar antes castiga un fallo aislado.
- La detección es CAUSA, no texto: `cause_in_chain(err) == Some(NetworkCause::DnsResolution)`.
  Un `Dns` sin causa clasificada no entra en la rama (falla-open hacia el comportamiento
  histórico), nunca al revés.

## Tasks

- [x] T1 — helper de dominio `is_resolution_failure` (S1) — `domain/error/mapping.rs`
- [x] T2 — `WreqDownloader::fetch_inner`: presupuesto propio y retardo fijo (S1)
- [x] T3 — batch: racha + abort + contadores separados (S2, S5)
- [x] T4 — tests con resolver inyectado (S4)
- [x] T5 — verificación (sin cambios de help/docs: no hay flags nuevos)

## Log

L1 — Petición original del usuario:
"issue #2823 — el de más valor para corridas reales. Bloquea la escalera de trips cuando el
resolver está roto, que hoy frena 50 URLs como si fuera un WAF."
## Evidencia

- `cargo nextest run -p webfang_core`: **4707 passed, 8 skipped**.
- `cargo clippy -p webfang_core --all-targets --all-features -- -D warnings
  -W clippy::cognitive_complexity -W clippy::too_many_lines`: limpio.
- `cargo fmt --all -- --check`: limpio.
- `cargo check --workspace --all-targets`: limpio (un warning preexistente en
  `webfang_mcp/tests/scraping_coverage_test.rs`).
- `cargo doc` con `RUSTDOCFLAGS=-D warnings`: **no disponible** — ICE de rustdoc
  (`scoped-tls: cannot access a scoped thread local variable`) reproducible
  también en `fix-2824-cause-clone` (base sin estos cambios). `cargo doc` sin
  `RUSTDOCFLAGS` sí genera los docs.
- Snapshot actualizado (revisado): `dns_tls_exit_code_test__dns_failure_redacted_stderr`
  ahora muestra los dos WARN del presupuesto de resolución y sigue saliendo 69.

## Decisiones de implementación

- El "contador de intentos" del sitio vive en `WreqDownloader::fetch_inner`
  (`attempt` + `backoff_delay_ms`). El fallo de resolución se reconoce por la
  CAUSA (`dns_resolution`) y se reintenta con presupuesto propio
  (`DNS_RETRY_BUDGET = 2`, retardo fijo `DNS_RETRY_DELAY_MS = 250`); nunca
  incrementa `attempt`.
- El batch NO tiene backoff propio (usa el `fetch_url` estático, sin reintentos).
  Su problema real es que ninguna corrida aborta: 50 URLs × connect timeout.
  Por eso el abort se implementa en `application/batch/processor.rs` con una
  racha de fallos de resolución (`RESOLUTION_ABORT_THRESHOLD = 5`).
- Para que el batch pueda clasificar por causa, el motor cuenta los fallos de
  resolución en `CrawlResult.resolution_failures` (campo nuevo, fuera de
  `error_breakdown`, que el issue hermano de taxonomías no toca).
- `BatchProcessor::with_downloader_factory` es el seam que permite inyectar el
  resolver en el test; en producción queda `None` (camino estático de siempre).
