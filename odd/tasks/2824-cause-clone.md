# La causa clasificada sobrevive al `clone()` y al recorrido `source()` (issue #2824)

El `Clone` de `DownloadError` reconstruye el error desde su `Display`, así que la
causa clasificada que `attach_cause` puso en la cadena se pierde en la copia; y
el `source()` de `DownloadError::Io` no expone el payload del `io::Error`, así
que un recorrido por la cadena no llega a la causa tipada. Las rutas de
aplicación (batch y scraper) reenvuelven el error y la causa se pierde otra vez.
Rama: `fix/2824-cause-clone` · Entrega: commits por unidad de trabajo (dominio
→ aplicación) · Runner: `cargo nextest run`.

## Specs

- S1: "`let c = attach_cause(err, cause); let d = c.clone();` conserva la causa observable por `cause()` / `source()` en los cinco variantes de transporte."
- S2: "El recorrido `source()` desde `DownloadError::Io` llega a la causa tipada."
- S3: "Una ruta de aplicación (batch y scraper) emite un `CrawlError` cuya causa es la clasificada, no `unknown`."
- S4: "El `ErrorKind` de `Io` no cambia: es la entrada de la regla de reintento y no debe moverse."
- S5: "Tests con fallos inyectados, sin red."

Las cinco variantes de transporte de S1 son `Network`, `Dns`, `Tls`, `Io` y
`Timeout` — las mismas que `attach_cause` clasifica.

## Tasks

- T1: `domain/error/cause.rs` — el carrier `ClassifiedNetworkError` pasa a ser
  `Clone` (el error original se comparte detrás de un `Arc`, así la copia
  conserva la cadena entera) y aparece `CauseCarrier`: el payload del
  `io::Error` cuyo `source()` **es** la causa clasificada, para que el recorrido
  plano por `source()` la alcance (S1, S2). · inline · W1
- T2: `domain/downloader_port.rs` — el `Clone` de `DownloadError` preserva el
  carrier en `Network`, `Dns`, `Tls`, `Io` y `Timeout` en vez de reconstruirlo
  desde el `Display`; el `ErrorKind` del `Io` no se mueve (S1, S4). · inline · W1
- T3: `domain/error/mapping.rs` — `attach_cause` envuelve el payload del `Io`
  en el `CauseCarrier`: el `ErrorKind` y el `Display` quedan idénticos (S2, S4).
  · inline · W1
- T4: Tests del dominio (RED primero): la copia conserva la causa en las cinco
  variantes; el recorrido `source()` desde `Io` llega a la causa tipada; el
  `ErrorKind` no se mueve (S1, S2, S4). · inline · W1
- T5: `application/crawler/*` — el motor recuerda la primera causa clasificada
  del run y la entrega en el `CrawlResult`; el contador por categoría no cambia
  (S3). · inline · W1
- T6: `application/batch/processor.rs` — el error agregado del batch lleva la
  causa clasificada en su `source` en vez de `None` (S3). · inline · W1
- T7: `application/scraper_service.rs` — el error de scrape lleva la causa
  clasificada hasta la frontera de aplicación (S3). · inline · W1 — la ruta ya
  conservaba la causa: se **pina** con un test, no se reescribe.
- T8: Test de aplicación con fallo inyectado, sin red: batch y scraper emiten un
  error cuya causa es la clasificada (S3, S5). · inline · W1
- T9: Verificación: `cargo check`, `cargo clippy --all-targets --all-features`,
  `cargo fmt --all -- --check`, `cargo doc`, `cargo nextest run` y
  `bash scripts/ci_fast_gate.sh`. · inline · W1

## Decisiones de diseño

- El carrier se vuelve clonable **compartiendo** el error original detrás de un
  `Arc`, no copiándolo: la copia conserva la causa *y* la cadena que el
  transporte produjo. `ClassifiedNetworkError::new` no cambia de firma — su
  representación interna es privada.
- El payload del `io::Error` es un tipo nuevo (`CauseCarrier`) y no el carrier
  directo: `std::io::Error::source()` devuelve el `source()` del **payload**, no
  el payload — con el carrier directo, el recorrido plano salteaba la causa
  tipada y sólo la alcanzaba el `get_ref()` (que es un caso especial que un
  consumidor externo no conoce). Con el `CauseCarrier` el payload's `source()`
  **es** el carrier, así que el recorrido la alcanza sin saber nada del
  `io::Error`.
- El `Display` del `Io` no cambia: `CauseCarrier` delega en el carrier, que
  renderiza `"{causa}: {detalle}"` igual que antes.
- El motor recuerda **una** causa clasificada (la primera) y no un mapa: el
  agregado del batch necesita una causa que adjuntar, y un agregado con N
  causas no tiene una sola causa honesta. El contador por categoría
  (`error_breakdown`) no se toca.
- La ruta de aplicación que ya conservaba la causa (el scraper, vía
  `HttpError::Connection { source }` → `ConnectionFailure`) se **pina** con un
  test; no se reescribe.
- El mensaje del error agregado del batch **no cambia**: el alcance de #2824 es
  la causa, no el texto. El `Display` del `ScraperError` derivado tampoco
  (mismo mensaje) — la clase tampoco (el `ConnectionReset` sintético se queda).
- El agregar del batch no conserva el error de transporte ORIGINAL (el motor
  cuenta los fallos, no los guarda): conserva la **causa clasificada**, que es
  el diagnóstico. El detalle del agregado viaja como el `source` del carrier.

## Log

- L1: "issue #2824 — clone() reconstruyendo desde el Display y source() sin
  exponer el payload del io::Error. Independiente, y el RED ya está escrito en
  el issue."
- L2: El RED de este trabajo son los criterios de aceptación del issue #2824
  (`status:approved`, `type:bug`), transcritos sin cambiar en S1-S5.
- L3: El worktree de la misión es
  `~/Projects/Rust/webfang-worktrees/fix-2824-cause-clone`, rama
  `fix/2824-cause-clone` desde `main` (c12d1ac8).
- L4: RED del dominio observado antes de implementar (S1, S2):
  `a_clone_keeps_the_class_and_the_cause` y
  `the_source_walk_from_io_reaches_the_typed_cause` fallan con
  `left: None, right: Some(ConnectRefused)`; el guardián del `ErrorKind`
  (`the_io_kind_survives_the_classification_and_the_copy`) ya estaba verde y
  tiene que seguirlo.
- L5: RED de la ruta de aplicación observado con el comportamiento previo
  restaurado a mano (S3): `the_batch_error_carries_the_classified_cause` falla
  con `left: None, right: Some(ConnectRefused)` y
  `a_classified_transport_failure_is_remembered` falla con la causa sin
  registrar. El test del scraper pasa en ambos estados: la ruta ya conservaba la
  causa y el test la **pina**.
- L6: La semántica de Rust que sostiene el diseño (payload del `io::Error`,
  `Arc<dyn Error + Send + Sync>` compartido, recorrido por `source()`) se
  verificó en un programa aislado antes de tocar el repo: con el carrier
  directo el recorrido plano salta la causa; con el `CauseCarrier` la alcanza.
- L7: Una sola unidad de trabajo (un commit), no dos: el golden del extractor
  intra-crate se refresca "en el MISMO commit que cambia el árbol"
  (`tools/intra-crate-lint/tests/real_tree.rs`), así que un commit intermedio
  sin su golden dejaría el árbol inconsistente en ese punto. La rama lleva el
  cambio de comportamiento (≈330 líneas authored) y las extracciones que el
  ratchet de tamaño de archivo EXIGE (el resto son mudanzas puras): los tests
  de `DownloadError` a `domain/downloader_port/tests.rs`, los dos módulos de
  test de la taxonomía a `domain/error/cause/tests.rs` y
  `domain/error/cause/classify_tests.rs`, los tests del bookkeeping a
  `application/crawler/crawl_task/tests_bookkeeping.rs`, y el error agregado
  del batch a `application/batch/aggregate.rs`. El baseline se BAJA (no se
  sube): processor.rs 1253 → 1250, crawl_task.rs 1460 → 1343.
- L8: `cargo nextest run --workspace --all-features`: 5581/5581 en verde;
  `cargo clippy --all-targets --all-features -D warnings` y
  `cargo fmt --all -- --check` en verde; `RUSTDOCFLAGS=-D warnings cargo doc
  --workspace --all-features` en verde; `bash scripts/ci_fast_gate.sh` en
  verde.
- L9: PR #2838 (`fix/2824-cause-clone` → `main`), un commit (`144e2484`): el
  golden del extractor se refresca en el mismo commit que cambia el árbol, así
  que un commit intermedio sin su golden dejaría el árbol inconsistente ahí.
- L10: Sin cambios de API incompatible: `ClassifiedNetworkError` mantiene su
  firma (`new`, `cause`) y gana `Clone`; `CauseCarrier` es un tipo nuevo;
  `CrawlResult` es `#[non_exhaustive]` y gana un campo + un método. El cambio de
  comportamiento del `Clone` es el que pide #2824.
