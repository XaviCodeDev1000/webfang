# Fila restantes del issue #1828: writer death, partición de red y `PermitGuard` panic safety

Objetivo: adjudicar las tres filas del issue #1828 que aún no tienen test ni decisión
registrada, con un test por fila que pina el contrato existente **sin cambiar producción**
(sin punto de arming nuevo, sin `catch_unwind` en producción, sin dependencias nuevas),
según el alcance aprobado por el owner en el issue
(issuecomment-6483111481, aprobado: "abre el worktree").

Rama: `test/crash-matrix-remaining-rows`. Worktree:
`/home/xavi/Projects/Rust/webfang-worktrees/test-crash-matrix-remaining-rows`.

Cierra (parcialmente): #1828 — las tres filas restantes de la AC 1. El issue queda
abierto hasta que las tres aterricen y el owner cierre.

---

## Specs

### S1 · Test 1 — writer death (HIGH): pinar el contrato de fallo existente

Del alcance aprobado, verbatim:

> **1. Writer death (HIGH) — pin the existing failure contract.**
> `application/crawler/bounded_sink.rs` has 12 tests for cancellation and backpressure,
> none that kills the writer. The test aborts (or panics) the spool writer task and
> asserts that the producer observes the failure through the closed channel and that
> `finish` reports it, instead of hanging or silently dropping accepted pages.

Prueba nombrada: `a_dead_writer_is_reported_through_the_channel_and_by_finish`
(in-module, convención sin prefijo).

### S2 · Test 2 — partición de red: mock held-open, no un kill temporizado

Del alcance aprobado, verbatim:

> **2. Network partition — a held-open mock, not a timed kill.**
> The issue correctly flags that a wall-clock kill breaks harness determinism. The
> deterministic fixture parks a wiremock handler without responding: the connection
> stays open, the configured timeout fires deterministically, and the row runs the same
> crash → resume → four-invariants machine as the rest of the matrix. This covers the
> sitemap-discovery partition case as a distinct failure class (stalled peer vs. killed
> process).

Prueba nombrada: en el archivo de matriz o sitemap que corresponda tras estudiar los
fixtures existentes (el nombre lo fija el estudio, siguiendo la convención local).

### S3 · Test 3 — `PermitGuard` panic safety: el unwind libera el permiso

Del alcance aprobado, verbatim:

> **3. `PermitGuard` panic safety — pin the unwinding contract, no `catch_unwind` in
> production.**
> The audit asked for `catch_unwind` in `resource_downloader.rs`, but in Rust the
> unwinder already runs `Drop`, so the RAII permit is released on panic. Adding
> `catch_unwind` would be new production behavior serving a test. Instead, a unit test
> panics inside the guarded scope and asserts the semaphore returns to full capacity,
> pinning exactly the property the audit was worried about.

Prueba nombrada: `a_panicking_scope_releases_its_permit` (in-module).

### Restricciones transversales (del issue y del alcance aprobado)

- Test-only: ningún cambio en `src/` de producción.
- Sin punto de arming nuevo, sin `catch_unwind` en producción, sin dependencias nuevas.
- Determinismo absoluto: nada de wall-clock ni sleeps; fixtures acotados.
- Ahorro de recursos (pedido del owner): builds estrechos
  (`cargo nextest run --test <target>`), sin `--all-targets` salvo en la verificación final.

---

## Tasks

- [ ] T1 · S1 — test de writer death en `application/crawler/bounded_sink.rs` (módulo tests).
- [ ] T2 · S2 — test de partición (sitemap discovery) con wiremock held-open.
- [ ] T3 · S3 — test de panic safety de `PermitGuard` en
      `infrastructure/crawler/resource_downloader.rs` (módulo tests).
- [ ] T4 — verificación: `cargo nextest run` de los targets tocados; `cargo check` +
      `clippy --all-targets --all-features` + `fmt --check` al cierre; commits por work-unit.

---

## Log

L1 — Petición del owner (verbatim, de esta sesión):

> Issue #1828 ·revisa y analiza en profundidad su contenido pero debes ahorrar recursos
> cuando haces builds para no saturar los recursos del sistemacontinua

> primero postee en el issue las decisiones de alcance para que apruebes el enfoque

> abre el worktree

El "abre el worktree" es la aprobación del alcance posteado en
[issuecomment-6483111481](https://github.com/XaviCodeDev1000/webfang/issues/1828#issuecomment-6483111481).

L2 — Estado verificado contra `main@0bbc8458` (análisis previo, en Engram
`issues/crash-matrix-coverage-1828`): las tres filas sin test ni decisión son writer death,
partición de red y `PermitGuard` panic safety. Los paths citados por el issue (2026-10-04)
derivaron: `bounded_sink.rs` está hoy en `application/crawler/`, y `resource_downloader.rs`
en `infrastructure/crawler/`.

L3 — **T1 (writer death): GREEN al primer corrimiento.** El contrato ya existe:
`BoundedFileSink::finish` hace join del `JoinHandle` del writer y mapea el `JoinError` a
`BoundedSinkError::Writer` — la rama estaba marcada `LCOV_EXCL defensive`, así que el test
la cubre por primera vez y los marcadores se quitaron (el comentario ahora nombra el test
que la pina). `abort()` sobre el handle vivo es determinista: el writer está idle en
`rx.recv()` con el canal abierto y sin cancel, así que el join produce el JoinError
cancelado. Se pinan además: capture tardío descartado sin acreditar, y segundo finish ⇒
`AlreadyFinished`. Commit `89c787bd`.

L4 — **T3 (PermitGuard): GREEN al primer corrimiento.** `std::panic::catch_unwind` +
`AssertUnwindSafe` con hook de pánico silenciado y restituido; el semáforo arranca en 0
disponibles y el guard sostiene 50 unidades: tras el unwind, `Drop` restaura las 50 y
`try_acquire_many(50)` tiene que prosperar. Sin `catch_unwind` en producción. Commit
`0cb29383`.

L5 — **T2 (partición): dos rediseños, el segundo es el que quedó.** Primera versión:
sitemap explícito estancado con `set_delay(600s)` + `--timeout-secs 1`. Pasó pero en
**113s**: el fetch del sitemap corre por `SitemapParser`, cuyo cliente NO hereda
`config.timeout_secs` (usa su default de 30s × reintentos) — el knob del operador solo
llega a los probes de discovery (`build_discovery_client`), no al parser. Segunda versión
(la commiteada): el peer estancado ES el seed del intento 1, con auto-descubrimiento
(`--use-sitemap` sin `--sitemap-url`); los probes de discovery son wreq crudo sin wrapper
de reintento y cada fallo de stage degrada al siguiente (`fetch_robots_sitemap` traga el
error de red), así que la partición se observa honestamente como discovery que no encuentra
nada: `SitemapNotFound`, run fallido, sin JSONL — en **7.2s**. Determinismo: un deadline de
cliente de 1s no puede perder contra un hold de servidor de 600s. Intento 2 contra el
server sano + resume sobre el mismo state dir + cuatro invariantes. Commit `3bfe3c01`.

L6 — **El hallazgo incidental de L5: el parser de sitemap ignora `--timeout-secs`.** Es un
hueco real de configuración (el operador achica el timeout y el fetch del sitemap no se
entera), pero tocarlo es un cambio de PRODUCCIÓN — fuera del alcance aprobado (test-only).
Queda anotado como candidato a issue; no se arregla aquí.

L7 — **Defecto propio, commiteado y reparado: el edit que insertó la fila de partición
consumió el bloque final de la fila `clean-ai` existente** (`wait_exit`,
`assert_killed_by_sigkill` y el `resume_run_with` completo): la fila quedó SIGNAL → store,
sin resume, y fallaba en 0.4s con `left: 0` — exactamente el síntoma que primero atribuí a
un fallo ambiental preexistente (CI estaba verde en el mismo base). La atribución fue
incorrecta: `git show origin/main:...` contra el commit mostró la fila mangueada DENTRO de
`3bfe3c01`. La lección operativa: un edit cuyo `old_string` termina en la cola de una
función existente puede consumirla; releer el diff completo (`git diff origin/main`) antes
de commitear, no después. Reparación verbatim desde origin/main, commit `4eab5c83`, y las
dos filas drain en verde (1.03s) tras reparar.

L8 — **Verificación del lote** (targets tocados): módulos lib `bounded_sink` +
`resource_downloader` 26/26; matriz de crash completa + `--retries 0` (resultado en el
reporte final); gate completo `fmt --check` + `clippy --all-targets --all-features -D
warnings -W cognitive_complexity -W too_many_lines` + `RUSTDOCFLAGS=-D warnings cargo doc`
+ `cargo check --workspace --all-features` (resultado en el reporte final). Ahorro de
recursos: `cargo build --tests -p webfang_core` de warm-up una sola vez, corridas
incrementales después; seed frío (`no-seed`), un solo target dir propio del worktree.
