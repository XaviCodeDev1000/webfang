# feat(cli): --jitter-ms — aleatorización asimétrica del delay (issue #2610)

**Objective**: break the metronomic request cadence produced by `--delay-ms` so a
rate limiter (Cloudflare) cannot align its counting window with our request pattern.
**Branch**: `feat/jitter-ms-pacing` (worktree `~/Projects/Rust/webfang-worktrees/feat-jitter-ms-pacing`).
**Delivery strategy**: single PR, two work-unit commits (core → plumbing).
**Test runner**: `cargo nextest run` (focused per task) + `scripts/ci_fast_gate.sh` at the end.

## Specs

- **S1** — "`--jitter-ms <MS>` (o `--jitter <PERCENT>`, a decidir en diseño) que aleatoriza el delay efectivo de cada request: **Base**: el `--delay-ms` configurado. **Rango asimétrico con sesgo hacia arriba** … Default: 0 (comportamiento actual) para no romper tests deterministas de pacing."
  Accepted form (design decision, 2026-11-12): absolute `--jitter-ms <MS>`; effective
  interval is uniform over `[delay_ms, delay_ms + jitter_ms]` — upward-only, never
  below the configured delay. Mean interval becomes `delay_ms + jitter_ms / 2`, so
  throughput drops against the configured `--delay-ms`; operators who need the same
  rate must lower `--delay-ms` accordingly.
  Acceptance: the flag parses from CLI, `WEBFANG_JITTER_MS`, and TOML `jitter_ms`;
  default 0 keeps today's behavior byte-for-byte.

- **S2** — "**Precedencia intacta**: el backoff con `Retry-After` del fetch guard chain (stage 4) manda sobre cualquier jitter — el jitter solo aplica al pacing normal, nunca a una espera dictada por el servidor."
  Acceptance: with jitter active, a `429` carrying `Retry-After: N` still waits ≥ N
  seconds before the retry (wiremock regression).

- **S3** — "**Secuencia por corrida/proceso**, no un patrón repetido entre invocaciones del batch (si el orquestador lanza chunks, cada proceso debe tener su propia secuencia)."
  Acceptance: the RNG is seeded from OS entropy per limiter instance (one per run),
  so two processes never replay the same sequence.

- **S4** — "Con `--jitter-ms` activo, los intervalos observados caen dentro del rango configurado (test con `MockClock`/inyección de azar, determinista vía seed)."
  Acceptance: every observed inter-dispatch gap is in `[delay_ms, delay_ms + jitter_ms]`
  (real-time tolerance), and the scheduling core is unit-tested deterministically with a
  seeded RNG.

- **S5** — "Default 0: los tests existentes de pacing no cambian."
  Acceptance: `jitter_ms == 0` builds NO jitter gate — the governor token bucket stays
  the only pacing mechanism, burst included, and every existing pacing test stays green
  untouched.

- **S6** — Burst decision (2026-11-12, maintainer): the burst is the other bot signature.
  With `jitter_ms > 0` the gate covers EVERY request from the first one — the first
  `concurrency` permits are NOT exempt. With `jitter_ms == 0` the burst is untouched
  (S5). `budget.burst()` on this machine is 8 (16 cores → `auto_crawl_table` → 8), so
  today the first 8 requests of a run leave simultaneously; jitter removes that.

- **S7** — Cancellation (2026-11-12, maintainer): a cancelled jitter wait must not leave
  a stale future slot that delays another task if the limiter is reused. The reserved
  slot is returned to the schedule when the cancelled reservation is still the latest one.

- **S8** — Scope check (2026-11-12): the `http_client` RPM limiter is NOT on the target
  fetch path. `HttpClientConfig::rate_limit_rpm` defaults to `None` and no production
  builder sets it, so `HttpClient::new` builds no limiter at all; and the
  `HttpClientPort::get` implementation never consults it. The only pacing on the target
  path is `SharedRateLimiter`. The `http_client` RPM limiter stays a non-goal.

## Tasks

- **T1** — limiter core: `jitter_ms` on `RateLimiterConfig` (+ `with_jitter`), the cumulative
  schedule gate in every `SharedRateLimiter` wait method, slot rollback on cancellation,
  unit tests. → S1, S3, S4, S5, S6, S7. Route: inline. Commit: pending.
- **T2** — CLI plumbing: `OptionSpec` entry + GROUP, `spec_command` binding + slot,
  `CrawlerArgs`, `NetworkOptions`, preflight stages, `ConfigDefaults`, `CrawlerConfig`,
  the three limiter builders. → S1, S5. Route: inline. Commit: pending.
- **T3** — `Retry-After` regression with jitter active (wiremock). → S2. Route: inline.
- **T4** — verification: `cargo check` / `clippy --all-targets --all-features` / `fmt --check` /
  `cargo doc`, focused `nextest` suites, independent verifier (Risk: item 4), RDD assess.
  Route: inline.

## Log

- **L1** (2026-11-12, verbatim) — issue #2610 "feat(cli): --jitter-ms — aleatorización asimétrica
  del delay para eliminar la firma metronómica":

  > ## Problema
  >
  > `--delay-ms` aplica una separación **fija** entre requests. Un ritmo perfectamente metronómico es por sí mismo una firma de bot: los intervalos son predecibles y pueden alinearse con la ventana de conteo de un rate limiter (Cloudflare evalúa ventanas de segundos a minutos por IP).
  >
  > Evidencia de campo (2026-11-05, corrida real contra un sitio detrás de Cloudflare con rate limiting agresivo): con delay fijo de 1,5 s y 4 s, el sitio tropezó repetidamente por tasa mientras que las peticiones aisladas (fingerprint TLS de wreq) siempre pasaron con 200. El fingerprint ya no es el cuello de botella; el patrón temporal sí aporta señal.
  >
  > ## Propuesta
  >
  > Flag `--jitter-ms <MS>` (o `--jitter <PERCENT>`, a decidir en diseño) que aleatoriza el delay efectivo de cada request:
  >
  > - **Base**: el `--delay-ms` configurado.
  > - **Rango asimétrico con sesgo hacia arriba**, p. ej. base 5.000 ms → efectivo uniforme entre 4.000 y 7.500 ms. Simétrico ±25% está mal: los picos hacia abajo acercan al umbral justo cuando el sitio está más sensible.
  > - **Precedencia intacta**: el backoff con `Retry-After` del fetch guard chain (stage 4) manda sobre cualquier jitter — el jitter solo aplica al pacing normal, nunca a una espera dictada por el servidor.
  > - **Secuencia por corrida/proceso**, no un patrón repetido entre invocaciones del batch (si el orquestador lanza chunks, cada proceso debe tener su propia secuencia).
  > - Default: 0 (comportamiento actual) para no romper tests deterministas de pacing.
  >
  > ## Criterios de aceptación
  >
  > - Con `--jitter-ms` activo, los intervalos observados caen dentro del rango configurado (test con `MockClock`/inyección de azar, determinista vía seed).
  > - Con jitter activo, `Retry-After` sigue respetándose textualmente.
  > - Default 0: los tests existentes de pacing no cambian.

- **L2** (2026-11-12) — Design review with the maintainer. Decision (a): upward-only
  `[delay, delay + jitter]` in ms; the issue's own example (`[4.000, 7.500]` for a 5.000 ms
  base) contradicts its own rationale and was retired in favour of a hard floor at the
  configured delay. Mechanism: cumulative schedule (`slot = max(next_slot, now)`,
  `next_slot = slot + delay + jitter`), never a post-grant sleep (which accumulates into
  sub-base dips) nor a pre-grant sleep (which the token bucket absorbs).
- **L3** (2026-11-12) — Maintainer questions resolved: burst is NOT exempt when jitter is on
  (S6); cancellation must return the reserved slot (S7); the `http_client` RPM limiter is
  off the target path (S8); AC2 gets a real wiremock test, not a structural argument.
- **L4** (2026-11-12) — Environment: worktree `feat-jitter-ms-pacing`; seeds are rustc-1.88
  artifacts and this tree needs 1.91, so the target dir is a cold build.
