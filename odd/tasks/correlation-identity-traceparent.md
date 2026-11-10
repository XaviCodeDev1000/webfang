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
- El span NO: `to_traceparent()` **no** aparece en el camino que arma los `span_fields`; el campo
  sale con el UUID crudo (`896f5b1a-818b-425f-811e-e3e233dca2b7`).
- Fallo observado: `assert_traceparent_embeds_run_trace` — *"correlation_id must be a W3C traceparent
  `00-<trace>-<span>-01`, got: 896f5b1a-818b-425f-811e-e3e233dca2b7"*.
- Superficie a revisar (read-only): `infrastructure/observability/file_trace_layer.rs` (captura los
  `span_fields`), `application/crawler/{engine.rs,crawl_task.rs,discovery.rs}`,
  `application/scraper_service.rs`, `cli/orchestrator.rs`, `infrastructure/observability/error_logging.rs`.

---

## Tasks

- [ ] T1 · S1, S2 — Localizar el PRODUCTOR del `correlation_id` del span y el punto exacto donde se
  decide la forma; cambiarlo para que salga el traceparent, y agregar el UUID durable en un campo
  explícito. Un solo campo nuevo, aditivo; ningún campo público cambia de nombre.
- [ ] T2 · S3, S4 — Sacar el `#[ignore]` del test y dejar el pin como está: el traceparent del span
  tiene que ser el del export. RED contra el productor viejo, GREEN con el nuevo.
- [ ] T3 · S2, S6 — Actualizar el pin de `correlation_id_fidelity.rs` y los `jq` de
  `docs/samples/debugging/` si el shape del campo cambia; el `jq` tiene que seguir encontrando la
  correlación.
- [ ] T4 · S5 — `docs/test-inventory.md`: filas 12 → 11 (7 atributos + 4 menciones), con la fila que
  sale y el motivo real que queda. `docs/test-inventory.json` regenerado.
- [ ] T5 · S7 — Verificación completa: `cargo nextest run` del módulo afectado, clippy
  `--all-targets --all-features -D warnings`, `cargo fmt --all -- --check`, `cargo doc` con
  `RUSTDOCFLAGS=-D warnings`, y el gate `scripts/check_ignored_guard.sh` en verde.
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

L3 — Pendiente de decisión de implementación (no bloquea): el NOMBRE del campo nuevo del UUID
durable. Propuesta: `correlation_uuid` (par de `correlation_id`, explícito sobre qué es). El owner
puede cambiarlo; el default es ese.
