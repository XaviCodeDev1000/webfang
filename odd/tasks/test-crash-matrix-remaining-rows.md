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
