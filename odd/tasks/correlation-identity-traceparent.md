# La identidad de correlación del trace habla traceparent, no UUID interno

Objetivo: el `span_fields.correlation_id` de un span de página debe llevar el **traceparent W3C**
(`00-<32hex>-<16hex>-01`), igual que el `correlation_id` del documento exportado, para que el join
trace↔export cierre sobre el estándar de observabilidad. El UUID durable de `CorrelationId` queda en
un campo explícito de los `span_fields` para quien necesite la identidad cross-run.
Rama: `fix/correlation-traceparent`. Worktree:
`/home/xavi/Projects/Rust/webfang-worktrees/fix-correlation-traceparent`.
Cierra: #2672 (el bug de producto que la lane de Bug Discovery destapó al destrabar el test de
`trace_correlation_test.rs`).

---

## Specs

**S1.** El `span_fields.correlation_id` de un span de página es el traceparent W3C (del owner):

> El UUID durable de `CorrelationId` queda como campo explícito en los span_fields para quien lo
> necesite, pero el join trace↔export tiene que operar sobre el estándar de observabilidad, no
> sobre una identidad interna.

**S2.** El UUID durable queda en un campo explícito (del owner):

> El UUID durable de `CorrelationId` queda como campo explícito en los span_fields para quien lo
> necesite

**S3.** El pin del test se cumple y el test sale de `#[ignore]` (del owner):

> Cuando abras el PR de fix, el test sale de `#[ignore]` y la lane lo valida de una.

> Eso mantiene la coherencia con la doc de tracing y deja el pin del test alineado con la semántica
> de página.

**S4.** El join trace↔export es la prueba de que el cambio es real (de #2672):

> el `correlation_id` de los `span_fields` lleva el **UUID durable** de `CorrelationId` donde el
> `correlation_id` del documento exportado lleva el **traceparent W3C**.

> Consecuencia: **el join trace ↔ export no se puede hacer**.

**S5.** El catálogo oficial se mueve en el MISMO PR (regla del repo, la que `check_ignored_guard.sh`
enforcea):

> Update this file in the same PR when adding/removing an ignored test or a doc/comment mention.

**S6.** Los consumidores del campo siguen siendo coherentes: los `jq` de `docs/samples/debugging/`
(`12-page-pacing-by-correlation.sh`, `17-correlate-wait-with-page.sh`) y el pin de
`crates/webfang_core/tests/correlation_id_fidelity.rs` (`correlation_id_json_contains_traceparent`)
deben seguir encontrando la correlación que prometen.

**S7.** La lane lo valida de una: `ignored-tests` corre el test destrabado y el reporter nombra el
fallo si algo queda roto (el reporter ya no dice "no findings" sobre una corrida roja).

### Hechos verificados (previa, 2026-11-10)

- `CorrelationId` (`domain/value_objects.rs`): `to_traceparent()` produce `00-<trace>-<span>-01`;
  su `Display` también imprime el traceparent (`write!(f, "{}", self.to_traceparent())`).
- El export YA habla traceparent: `DocumentChunk::from` / `from_scraped_content_with_correlation` /
  `enrich_from_scraped_content` copian `scraped.correlation_id.map(|c| c.to_traceparent())`
  (`domain/entities/content.rs`).
- El MCP ya habla traceparent: `ScrapeEvent.correlation_id = Some(correlation.to_traceparent())`
  (`webfang_mcp/src/mcp_server/metrics.rs`).
- Superficie a revisar (read-only): `infrastructure/observability/file_trace_layer.rs` (captura los
  `span_fields`), `application/crawler/{engine.rs,crawl_task.rs,discovery.rs}`,
  `application/scraper_service.rs`, `cli/orchestrator.rs`, `infrastructure/observability/error_logging.rs`.

### Hechos verificados (T1, 2026-11-11 — la previa era incorrecta)

La previa suponía que el span de página escribía el UUID crudo. **Falso**: el span de página YA
habla traceparent. El productor real del UUID crudo es OTRO subsistema.

- **El `%` de `tracing` usa `Display`, y `Display for CorrelationId` imprime el traceparent.**
  Todo productor que declara `correlation_id = %<CorrelationId>` escribe el traceparent:
  `crawler::engine::entry` (`crawl_site`, `crawl_site_with_options`), `crawler::crawl_task`
  (`crawl_page`), `crawler::session` (`run identity`), `crawler::sitemap_session`,
  `crawler::discovery` (`scrape_single`), `scraper_service` (`scrape_with_config`),
  `cli::orchestrator` (`run`), `rate_limiter`, `crawler::sitemap_parser`,
  `webfang_benchmark::runner`.
- **El único infractor es `infrastructure/llm/remote_embedding.rs`**: los spans `embed` y
  `embed_batch` minan `uuid::Uuid::new_v4()` y la declaran como `correlation_id = %correlation_id`.
  No es un `CorrelationId`: es un **UUID v4 por request**, y no tiene relación con la corrida.
- **Por qué el span del embeddings pisa al del padre**: `FileTraceLayer` fusiona los campos del
  scope (padre → hijo) y el hijo gana (`span_fields.entry(k).or_insert(v)`,
  `infrastructure/observability/file_trace_layer.rs`). El span `embed_batch` heredaría el
  traceparent de la corrida; su campo propio lo **sobrescribe** con el UUID v4.
- **Evidencia en vivo** (run manual con `--trace-file`, servidor de embeddings propio):
  `embed_batch` → `span_fields.correlation_id = "c8af6991-b34d-4208-a6c2-3d8714681093"` (v4)
  mientras `run` / `scrape_single` / `crawl_with_sitemap_resolved` →
  `00-01a1c8506ecc76b697874bd36076b6e7-<span>-01`.
- **El export ya es correcto**: los documentos exportados llevan
  `00-01a1c8506ecc76b697874bd36076b6e7-632f1987b67959b2-01` y
  `00-01a1c8506ecc76b697874bd36076b6e7-1922939e06390423-01`, que están en el trace. El join
  trace↔export cierra en cuanto `span_fields.correlation_id` sea uniforme.
- **El contrato ya está escrito y pineado**: `docs/src/debugging.md` dice que
  `span_fields.correlation_id` es el traceparent W3C único por página y que
  `span_fields.trace_id` es el UUID durable del run-root; el pin del layer
  `contract_captures_correlation_identity_declared_at_span_creation` exige el traceparent.
  El adaptador de embeddings es el único que rompe ese contrato.
- **El UUID durable ya está expuesto**: `span_fields.trace_id` (UUID v7 del run-root, p.ej.
  `01a1c850-6ecc-76b6-9787-4bd36076b6e7`). La premisa de S2 («el UUID durable vive hoy en
  `correlation_id`») no se sostiene: un campo `correlation_uuid` con esos mismos bytes
  duplicaría `trace_id`.

---

## Tasks

- [x] T1 · S1, S2 — **PRODUCTOR LOCALIZADO Y CORREGIDO** (ver «Hechos verificados (T1, 2026-11-11)»): el
  infractor era `infrastructure/llm/remote_embedding.rs` (spans `embed` / `embed_batch`), que publicaba
  un UUID v4 por request bajo la clave `correlation_id` y pisaba el traceparent heredado del scope. El
  span de página YA hablaba traceparent, así que el fix no estaba donde decía la previa. Shape
  decidido por el owner (L5, opción A): el UUID por request pasa a `request_id` — el nombre honesto,
  es un id de request y no una correlación — y NO se agrega `correlation_uuid`: el UUID durable del
  `CorrelationId` ya está expuesto como `span_fields.trace_id`, donde ya estaba y ya está
  documentado. Commit propio `9e95f974`.
- [x] T2 · S3, S4 — Destrabado: `scrape_trace_and_vector_export_share_per_page_correlation` sale de
  `#[ignore]` y corre VERDE. RED observado contra el productor viejo (v4 crudo por corrida),
  GREEN con el productor nuevo. Commit `1bed39d2`.
- [x] T3 · S2, S6 — **Sin cambios necesarios, verificado**: los `jq` de `docs/samples/debugging/`
  (`12-page-pacing-by-correlation.sh`, `17-correlate-wait-with-page.sh`) ya filtran por
  `.fields.correlation_id` con un valor TRACEPARENT (`CID=00-01949e0e…-01`) — el shape del evento no
  lo toca este cambio. El pin `correlation_id_json_contains_traceparent` de
  `correlation_id_fidelity.rs` asserta las keys del `CorrelationId` serializado (`trace_id`,
  `span_id`), no el shape del span: tampoco depende de este cambio. Verificado en vivo: el evento
  `rate limit wait` sigue llevando `fields.correlation_id = "00-01a1c8a1…-3ee3331358f5655b-01"`.
- [x] T4 · S5 — `docs/test-inventory.md`: filas 12 → 11 (7 atributos + 4 menciones), con la fila que
  sale y el motivo real que queda (Unit A6). `docs/test-inventory.json` regenerado (181 archivos;
  el `ignored_count` del área afectados pasa 1 → 0). `scripts/check_ignored_guard.sh` en verde.
- [x] T5 · S7 — Verificación completa (ver L6).
- [ ] T6 · S7 — La lane valida: confirmar en la corrida programada que `ignored-tests` reporta el
  test destrabado si algo falla, y que no reporta nada cuando pasa.

### Riesgo

Ítem 3 (contratos que otros consumen): el shape del `span_fields.correlation_id` del trace JSONL lo
consumen scripts del repo, exports y pins. El cambio es ADITIVO en el JSON (un campo nuevo) pero
cambia el VALOR de un campo existente ⇒ es un cambio de contrato observable: va en su propio commit
para que la revisión lo tome solo.

---

## Log

L1 — Decisión del owner (verbatim):

> vamos con la **opción (1)** — unificar en el `traceparent` W3C. El UUID durable de `CorrelationId`
> queda como campo explícito en los span_fields para quien lo necesite, pero el join trace↔export
> tiene que operar sobre el estándar de observabilidad, no sobre una identidad interna. Eso mantiene
> la coherencia con la doc de tracing y deja el pin del test alineado con la semántica de página.
> Cuando abras el PR de fix, el test sale de `#[ignore]` y la lane lo valida de una.

L2 — Previa (2026-11-10): el hallazgo no lo hunted, lo destapó la lane de Bug Discovery al destrabar
el test de `trace_correlation_test.rs` con el seam de provider hermético (#2666). El test corre y
falla por la identidad, no por el provider.

L3 — Decisión del owner, CONFIRMADA (verbatim):

> Vamos con la opción **traceparent W3C**.
>
> Reglas:
>
> - `traceparent` sigue siendo la identidad pública de correlación para join trace↔export.
> - El UUID durable no se borra: se expone como campo adicional.
> - El nombre propuesto `correlation_uuid` está aprobado como cambio aditivo.
> - El cambio de shape del trace JSONL merece commit propio, porque puede tocar consumidores, exports y pins.
> - Al cerrar #2672:
>   - `scrape_trace_and_vector_export_share_per_page_correlation` sale de `#[ignore]`.
>   - `docs/test-inventory.md` baja a 11 filas en el mismo PR.
>   - La lane de `ignored-tests` debe validarlo verde.

### Consecuencia para las tareas

- **T2** cierra con el test FUERA de `#[ignore]` (no "cuando se arregle"): el fix del productor y el destrabe van en el mismo PR, y las gates corren el test en verde.
- **T1** lleva el cambio de shape del trace JSONL en un **commit propio** (el owner lo pidió explícitamente): el commit del productor + el campo `correlation_uuid` no se mezclan con el destrabe ni con el catálogo.
- **T4** actualiza `docs/test-inventory.md` a **11 filas** (7 atributos + 4 menciones) y regenera `docs/test-inventory.json`.
- **T6** verifica con la lane: `ignored-tests` corre el test destrabado y tiene que salir VERDE.

L4 — T1 cerrado (2026-11-11): **la premisa de la previa y de S4 no se sostiene.** El `span_fields`
de página ya habla traceparent; el UUID crudo lo publica el adaptador de embeddings
(`infrastructure/llm/remote_embedding.rs`), que reutiliza la clave `correlation_id` para una
identidad distinta (un UUID v4 por request). Evidencia: run manual con `--trace-file` — el span
`embed_batch` escribe `span_fields.correlation_id = "c8af6991-b34d-4208-a6c2-3d8714681093"` (v4)
mientras el resto del trace escribe traceparents; el RED del test reproduce con un v4 distinto en
cada corrida (`a4ecf457-…`, `369495bd-…`, `6797516b-…`), o sea un UUID por request y no la
identidad de la corrida. El UUID durable del `CorrelationId` ya está expuesto como
`span_fields.trace_id` (v7 del run-root) y está documentado en `docs/src/debugging.md`; por eso
S2, tal como está redactada, no tiene un hueco real que tapar.

L5 — DECISIÓN DEL OWNER (2026-11-11), opción **(A)**: el UUID por request del adaptador de embeddings
pasa a `request_id` (el nombre honesto: es un id de request, no una correlación) y NO se agrega
`correlation_uuid` — el UUID durable del `CorrelationId` sigue en `span_fields.trace_id`, donde ya
estaba y ya está documentado (`docs/src/debugging.md`). Se descartaron (B) y (C) porque
`correlation_uuid` habría pasado a significar dos cosas distintas (id del request de embeddings vs.
UUID durable) o habría duplicado byte a byte `span_fields.trace_id`. Con (A) el shape del trace
JSONL queda: `span_fields.correlation_id` = traceparent W3C (uniforme en todo el trace),
`span_fields.trace_id` = UUID durable del run-root, `span_fields.request_id` = el id por request
del adaptador de embeddings (único en su columna).

L6 — Cierre de T1-T5 (2026-11-11):

- **Commits**: `9e95f974` (el productor: el rename del campo + el doc del porqué, commit propio como
  pidió el owner) · `1bed39d2` (el destrabe del pin + el catálogo a 11 filas).
- **RED → GREEN**: el pin destrabado falló con un UUID v4 DISTINTO en cada corrida
  (`a4ecf457-…`, `369495bd-…`, `6797516b-…`) — la prueba de que era un id por request y no la
  identidad de la corrida; con el productor corregido corre VERDE.
- **Verificación**: `cargo nextest run --workspace` → 5135/5135 verde (7 skipped); el target
  `behavioral` completo → 176/176 verde (1 skipped: la fila `#[ignore]` que sigue viva por diseño);
  `cargo clippy --all-targets --all-features -- -D warnings -W clippy::cognitive_complexity -W
  clippy::too_many_lines` verde; `cargo fmt --all -- --check` verde; `cargo doc --workspace
  --all-features --no-deps` con `RUSTDOCFLAGS=-D warnings` exit 0; `scripts/check_ignored_guard.sh`
  verde (11 menciones: 7 atributos + 4 doc/comment); `scripts/check_test_inventory.sh` verde.
- **Evidencia en vivo (post-fix)**: en un `--trace-file` real, TODO `span_fields.correlation_id` del
  trace es un traceparent (el filtro `grep -vE '^00-[0-9a-f]{32}-[0-9a-f]{16}-01$'` sobre el campo no
  devuelve nada), el span `embed_batch` hereda el traceparent de la corrida
  (`00-01a1c8a1…-19ab785b87823681-01`) y el id por request aparece como `request_id`
  (`95b6215d-…`, `f23bf1f8-…`).
- **Ratchets del repo (los dos que el cambio tocaba)**: `scripts/check_file_size.sh` — el archivo
  `remote_embedding.rs` está en el TECHO de su baseline (1825/1825) y no puede crecer ni una línea:
  por eso el cambio es un rename neto-cero y el doc del porqué se compensó recortando el doc
  verboso de `as_any`, en vez de growing el archivo. `scripts/check_duplication.sh` — 8306 líneas
  duplicadas, 19 POR DEBAJO del baseline de 8325.

L7 — Lo que este ciclo deja anotado (2026-11-11):

- **El pin unitario que se escribió primero se Sacó, y el E2E quedó como pin**: el pin unitario
  (el `embed_batch` span no debe declarar `correlation_id`) iba a vivir en `remote_embedding.rs`, un
  archivo grandfathered SIN HEADROOM: el ratchet de tamaño falla si crece una sola línea, y el
  ratchet de duplicación sube +2 por el bloque `FileTraceLayer` copiado del pin de `orchestrator.rs`.
  Sacarlo dejó el archivo en su techo exacto (1825) y la duplicación 19 líneas por debajo del
  baseline. Y el pin unitario era REDUNDANTE: el test conductual destrabado asserta la regla
  COMPLETA (todo `span_fields.correlation_id` es un traceparent que embebe el trace del run-root, y
  los `correlation_id` del export están en ese conjunto) y corre en cada lane. La extracción del
  harness de captura (`FileTraceLayer` → JSONL) que las dos cosas exigirían es un trabajo de otro
  PR: toca `rate_limiter.rs` y `orchestrator.rs`, que no tienen nada que ver con #2672.
- **`Display for CorrelationId` ES el traceparent**: el `%` de `tracing` usa `Display`, y
  `Display for CorrelationId` imprime `to_traceparent()`. Cualquier productor que declare
  `correlation_id = %<CorrelationId>` escribe el traceparent sin tocar nada. El infractor era el
  único que NO usaba un `CorrelationId` — usaba un `Uuid::new_v4()` — y por eso su campo era la
  ÚNICA columna del trace con dos identidades mezcladas.
- **El merge del scope es la causa raíz**: `FileTraceLayer` fusiona los campos del scope
  padre → hijo y el hijo gana. Un span anidado que declare su propio `correlation_id` pisa el
  traceparent heredado. La lección para cualquier span anidado futuro: la clave `correlation_id`
  es de la RUN, no del span.

