# Ola 2 — plan del programa #2584: grandes [1500, 2000] (T3)

**Decisión al abrir este doc:** primer batch = `mcp_server/metrics.rs` + `mcp_server/handlers/export.rs`. Quedan excluidos por trabajo abierto `connect.rs` y `remote_embedding.rs` (rama `feat/ai-proxy-domain-types`), y en espera `wreq_downloader.rs` (cadena de guards de fetch + PR #2880) y `params.rs` (pendiente de decidir excepción declarativa vs split).

Rama: `refactor/ola2-file-size` · worktree: `~/Projects/Rust/webfang-worktrees/refactor-ola2-file-size` · base: `origin/main@5d044bba`. Plan de record: `odd/tasks/file-size-reduction.md` (T3, § Gobernanza: 2–3 archivos por PR). Runner declarado: `cargo nextest` (el mismo antes y después de cada medición).

## Specs

**S1 — Banda y fuente de verdad.** La Ola 2 es la banda `[1500, 2000]` del ratchet `scripts/check_file_size.sh`. La lista vigente NO se copia como estado inmutable: la fuente de verdad es `file_size_rust.grandfathered` en `scripts/quality-baselines.json`. Snapshot verificado en `origin/main@5d044bba` (2026-11-17): 8 entradas en la banda, 0 entradas >2000 (Ola 1 cerrada, último registro en `1188e5c4` / #2851). Toda cifra de este doc es candidata a re-medirse antes del primer corte.

| Entrada baseline | Archivo |
|---|---|
| 1974 | `crates/webfang_core/src/cli/commands/connect.rs` |
| 1856 | `crates/webfang_core/src/infrastructure/downloader/wreq_downloader.rs` |
| 1825 | `crates/webfang_core/src/infrastructure/llm/remote_embedding.rs` |
| 1770 | `crates/webfang_mcp/src/mcp_server/metrics.rs` |
| 1743 | `crates/webfang_core/src/infrastructure/crawler/sitemap_parser.rs` |
| 1554 | `crates/webfang_core/src/application/container.rs` |
| 1552 | `crates/webfang_mcp/src/mcp_server/params.rs` |
| 1500 | `crates/webfang_mcp/src/mcp_server/handlers/export.rs` |

**S2 — Trabajos abiertos (pedido literal del maintainer).** "armemos el doc de la Ola 2 pero debes tener en cuenta los trabajos abiertos actualmente". El mapa verificado en el árbol de hoy manda lo siguiente; ninguna entrada se mueve mientras su bloqueo siga vivo:

| Archivo | Situación | Acción |
|---|---|---|
| `connect.rs` | Worktree `feat-ai-proxy-domain-types` (rama `feat/ai-proxy-domain-types`) tiene commiteados los split dirs `cli/commands/connect/` y la bajada de baseline `1974 → 1874`, aún sin PR. | **Excluido.** Re-medir y re-bandear cuando ese trabajo se fusione. |
| `remote_embedding.rs` | Misma rama: split dir `infrastructure/llm/remote_embedding/` y bajada `1825 → 1773`, aún sin PR. | **Excluido.** Igual que el anterior. |
| `wreq_downloader.rs` | Cadena obligatoria de guards de fetch (entry → pacing → timeout/redirect/SSRF → retry classification → body read). PR #2880 (`feat/obscura-l2-honesty`) toca el mismo `downloader/` y edita `scripts/quality-baselines.json`. Último toque en main: `fix(crawler): stop a broken resolver from stalling a run like a WAF (#2823) (#2835)` (2026-11-16). | **Diferido** a su propia misión de alto riesgo, después de que #2880 se fusione. Cuatro lentes + ciclo nativo obligatorios. |
| `params.rs` | `odd/tasks/file-size-reduction.md` § Excepciones lo nombra candidato a excepción declarativa cohesionada. | **Sin decidir.** La decisión grandfather-con-`notes` vs split se toma EN su misión, no acá. No se incluye en el primer batch por defecto. |
| `container.rs` | Último toque en main: `fix(test): arm the real SSRF guard in registry keep-first tests (#2626) (#2634)` (2026-11-09). | Diferido al batch 2; requiere censo de tests que tocan el guard real. |
| 8 archivos | Ningún PR abierto los toca directamente. PRs abiertos: #2880 y #2881 (`feat/obscura-variant-gate`, toca `cli/preflight/js_strategy*`). | — |
| `scripts/quality-baselines.json` | #2880 y el worktree `feat-batch-input-reconciliation` (cambios sin commitear) lo editan. | Todo PR de esta ola se rebasea sobre el main vigente tras fusionarse esos trabajos. No se empuja mientras el archivo compartido esté en disputa. |

Nota de distinción: `crates/webfang_core/src/cli/args/export.rs` (trabajo `feat-batch-input-reconciliation`) **no** es `crates/webfang_mcp/src/mcp_server/handlers/export.rs` (esta ola). No colisionan.

**S3 — Entrega por batches, no por monstruo.** Gobernanza del plan: "Ola 2 y Ola 3 se batchean a 2–3 archivos por PR como máximo." Una rama y un worktree por batch, cada uno desde el `main` vigente; entrega secuencial, sin PRs apilados (`check-topology.sh`). El doc del programa viaja con la rama del primer batch y llega a `main` con él; los batches siguientes lo editan con su Log.

**S4 — Orden por churn, no por tamaño.** Regla del plan: "Dentro de cada banda el orden NO lo da el conteo de líneas: lo dan el churn (`git log --follow --format=%ad --date=short -- <archivo>`), el historial de bugs y la complejidad medida." Medición orientativa de 90 días sobre `origin/main@5d044bba` (fuente: historial público por path; re-medir con `git log` en el árbol del corte):

| Archivo | Commits ~90d | Último toque en main |
|---|---|---|
| `metrics.rs` | 4 | 2026-10-23 — `fix(mcp): la superficie del servidor MCP dice lo que hace (#1815,#1837,#2193) (#2257)` |
| `handlers/export.rs` | 13 | 2026-10-27 — `refactor(exporter)!: skipped_non_utf8 travels in the export outcome (#2391)` |
| `params.rs` | 14 | 2026-10-17 — `docs(rustdoc): cerrar los 39 links privados rotos (#2010)` |
| `sitemap_parser.rs` | 15 | 2026-10-29 — `fix(crawler): the sitemap parser's fetch client honors --timeout-secs (#2507)` |
| `container.rs` | 13 | 2026-11-09 — `fix(test): arm the real SSRF guard in registry keep-first tests (#2626) (#2634)` |
| `wreq_downloader.rs` | 9 | 2026-11-16 — `fix(crawler): stop a broken resolver from stalling a run like a WAF (#2823) (#2835)` |
| `remote_embedding.rs` | 11 | 2026-11-17 — `refactor(ai): unify the LLM HTTP client builder behind one constructor (#2878)` |
| `connect.rs` | 2 | 2026-10-21 — `fix(test): gatea la trampa FFI TLS_method en los cuatro tests de connect (#2212)(#2216)` |

Conclusión: el primer batch son `metrics.rs` (menor churn de la banda) y `handlers/export.rs` (mismo crate `webfang_mcp`, sin solape con trabajo abierto). `sitemap_parser.rs` es el siguiente por churn, pero va en batch separado por ser del crate `webfang_core`.

**S5 — Método y cierre co-equal.** "Split por seams reales, nunca por aritmética de líneas." Movimiento puro demostrado por diff normalizado (strip indent + drop blanks), dos sombreros declarados en el PR (`Movido sin cambios` / `Corregido / mejorado`), cambios semánticos separados o acotados (~20–30 % del diff). Cierre = líneas ≤500 + `clippy::too_many_lines` (100) y `clippy::cognitive_complexity` (25) sin supresiones nuevas + `jscpd --complexity` (`cx` no sube; binario pinneado 5.4.0). "El descenso del baseline viaja en el MISMO PR que el split que lo produce": `--remove` si todas las partes quedan ≤500, `--update` descendente si alguna sigue >500.

**S6 — Re-medición y red de seguridad.** Ningún span se copia de la prosa: se re-mide en el árbol del corte con script de sentinelas que aborta si el cierre del bloque no está donde se midió. Antes de mover: caracterización con tests/snapshots existentes (o test mínimo); sin red rojo→verde no hay split. Invariante `--list` con el mismo runner antes y después: mismo conjunto de nombres de hoja y mismo conteo total, declarando la regla de renombre de ruta. Tests nuevos como hermanos `*_tests.rs` / `tests_<tema>.rs`, nunca dentro de un directorio `tests/`.

**S7 — Compuertas por batch.** Idénticas a `odd/tasks/file-size-t2.md` § Compuertas, sin recortes: `cargo check --all-targets --all-features` · clippy estricto · `fmt --check` · `cargo doc -D warnings` · suite completa · `--list` · verbatim por diff normalizado y `git diff --color-moved` · diff de modificadores de visibilidad (vacío salvo lo declarado) · `check_file_size.sh` · `check_test_inventory.sh` · golden `real_tree_sites.txt` refrescado post-fmt en el mismo commit · `check_duplication.sh` · `check_dead_tests.sh` · direcciones inter-crate e intra-crate · `bash scripts/ci_fast_gate.sh` GREEN con reporte de cobertura leído. Revisión por cuatro lentes con resultado explícito + ciclo nativo cuando el slice lo amerite.

**S8 — Alto riesgo explícito.** Cualquier corte en `wreq_downloader.rs` toca la cadena de guards de fetch y es ítem de alto riesgo: preservar el orden entry → pacing → timeout/redirect/SSRF en cada dial → clasificación de reintento → lectura con cap; cuatro lentes y ciclo RDD sobre el commit unitario. Un error ahí no es detectable por un test de líneas.

**S9 — Gobernanza y límites.** "Cada split = issue propia + PR con `Closes part of #2584` ... label `type:refactor`, base `main`, entrega secuencial." Errores de usuario en español; `CHANGELOG.md` intacto (escritor único: el Release PR); `docs/` intacto salvo pedido explícito; sin `git checkout`/`switch`/`stash`; proxy de identidad antes de cada commit.

## Tasks

| ID | Tarea | Specs | Ruta | Estado | Commit / evidencia |
|----|-------|-------|------|--------|--------------------|
| O2-T0 | Worktree `refactor/ola2-file-size` + este doc + espejo Engram | S1,S2,S3 | inline | done | worktree @ `5d044bba`, proxy OK |
| O2-T1 | **Batch 1** — `metrics.rs` + `handlers/export.rs`: medir seams, extraer tests por tema, partir producción, `--remove` en el mismo PR | S3,S4,S5,S6,S7 | inline | pending | — |
| O2-T2 | Decisión `params.rs`: excepción declarativa con `notes` vs split (en su misión, no por defecto) | S2,S5 | inline | pending | — |
| O2-T3 | **Batch 2** — `sitemap_parser.rs` (+ `container.rs` sólo tras censo del guard real) | S3,S4,S6 | por decidir | pending | — |
| O2-T4 | Re-medición de `connect.rs` y `remote_embedding.rs` tras la fusión del trabajo AI egress; re-bandear si corresponde | S1,S2 | por decidir | pending | — |
| O2-T5 | Misión propia de `wreq_downloader.rs`, después de #2880; cuatro lentes + RDD | S1,S2,S8 | por decidir | pending | — |
| O2-T6 | Verificación completa y PR de cada batch (`Closes part of #2584`, `type:refactor`, base `main`) | S7,S9 | inline | pending | — |

## Log

- **L1 (2026-11-17, verbatim, pedido original)**: "armemos el doc de la Ola 2 pero debes tener en cuenta los trabajos abiertos actualmente".
- **L0 (2026-11-17, antecedente verbatim)**: "cuales la siguiente mision relacionada con la *ola* de archivos".
- **L2 (2026-11-17, verificación sobre el árbol de hoy)**: `HEAD` y `origin/main` en `5d044bba` (`refactor(ai): unify the LLM HTTP client builder behind one constructor (#2878)`), sin cambios sin commitear en `main`. Baseline: banda [1500, 2000] = 8 entradas, >2000 = 0, bandas [501,570]=4 / [571,999]=51 / [1000,1499]=28. No existe doc previo `file-size-ola2-*.md`. Worktrees activos: `feat-ai-proxy-domain-types`, `feat-batch-input-reconciliation` (único con cambios sin commitear; toca `scripts/quality-baselines.json` y `cli/args/export.rs`), `feat-obscura-l2-honesty`, `feat-obscura-variant-gate`. PRs abiertos: #2880 y #2881; ninguno toca directamente los 8 archivos.
- **L3 (2026-11-17, decisiones de este doc)**: se elige `metrics.rs` + `handlers/export.rs` como primer batch por menor churn relativo y cero solape con trabajo abierto. Se excluyen `connect.rs`/`remote_embedding.rs` por la rama AI egress (split dirs ya commiteados y baseline bajada, sin PR). Se difiere `wreq_downloader.rs` por ser cadena de guards y compartir área con #2880. `params.rs` queda como decisión explícita de excepción, no como split automático. `container.rs` pasa al batch 2 con censo previo del guard real.
- **L4 (2026-11-17, resolución de un dato del mapeo)**: el primer informe del explorador no pudo correr `git status` por falta de shell y dio la lista de worktrees incompleta. Verificado con `git worktree list --porcelain` y `git -C <wt> status --porcelain`: son 5 worktrees contando `main`, y el único con cambios sin commitear es `feat-batch-input-reconciliation`. Los split dirs de `feat-ai-proxy-domain-types` están commiteados en su rama, no son cambios sueltos: la exclusión de esos dos archivos se mantiene por trabajo publicado pero sin PR.
