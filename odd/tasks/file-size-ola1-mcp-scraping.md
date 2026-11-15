# Ola 1 — Misión: `mcp_server/handlers/scraping.rs` (paraguas #2584)

Objetivo: partir `handlers/scraping.rs` (2513 medido / 2513 en disco; baseline del ratchet = 2513 en `scripts/quality-baselines.json`, SIN drift — medido, no supuesto) por seams reales hasta que todas las partes queden ≤500 y la entrada salga del baseline con `--remove`. Branch: `refactor/ola1-mcp-scraping` desde `origin/main` `8aecddc2`; worktree `~/Projects/Rust/webfang-worktrees/ola1-mcp-scraping`. Entrega: una misión/PR por monstruo, SIN batch; este monstruo puede llevar 2–3 PRs secuenciales propios (sin PRs apilados). Ratchet: `scripts/check_file_size.sh`; verificación dupla con `jscpd --complexity`. Runner: `cargo test` — en esta máquina `cargo nextest` no existe (PATH sin `~/.cargo/bin`); las cifras declaran `cargo test` (herencia S9 de ola1-engine, verificado aún vigente).
Plan de record: `odd/tasks/file-size-reduction.md` — método, dos sombreros y gobernanza viven ahí; este registro no los reescribe.
Coordinación: `export_factory.rs` reservado por `ola1-export-factory` (worktree abierto, sin commits); `waf_engine.rs` por `ola1-waf-engine`; `fix-2750-jitter-stranded-slot` toca `application/rate_limiter/jitter.rs` — ninguno solapa este archivo. Estado publicado vía `orchestrator_session_id` (topic `file-size-ola1-mcp-scraping`).

## Specs

S1 — Pedido del usuario (verbatim, 2026-11-15): "debes tomar parte del plan y organizarte con los otros agentes para atacar algun archivo pendienta sin colisionar con los otros agentes" — de los 5 monstruos >2000 restantes (`export_factory` ocupado, `waf_engine` tomado por el mismo usuario en otra sesión), elijo `mcp_server/handlers/scraping.rs` (2513): seams claros, ningún agente lo reclama.

S2 — Método (verbatim, plan de record): "Split por seams reales, nunca por aritmética de lías." · "Comportamiento preservado: `refactor(scope)` puro, tests verdes por unidad de trabajo, snapshots intactos. El descenso del baseline viaja en el MISMO PR que el split que lo produce" · "Objetivo de cada split: todas las partes <=500 -> `--remove` de la entrada original."

S3 — Ratchet: el baseline solo baja (`--update` descendente / `--remove`); un grandfathered tocado y no partido termina neto-cero; cada split declara en el PR `Movido sin cambios` y `Corregido / mejorado`; commits convencionales sin atribución IA; push y PR solo con confirmación explícita del humano.

S4 — Instrumento de re-medición (herencia directa de ola1-engine): `grep -nE '^    (#\[(tokio::)?test\]|async fn |fn )' scraping.rs | awk -F: '$1>=1503'` para tests; `grep -nE '^(async )?fn |^struct |^enum |^impl |^pub ' scraping.rs | awk -F: '$1<1503'` para producción. Los números de este registro salen de esas salidas, no de prosa (compromiso S10 del usuario).

S5 — Helpers de test: se mueven con su primer consumidor real, NUNCA a un `mod.rs` compartido (precedente ola1-engine, enmienda S9a: "Se van con su primer consumidor real, aunque no sea PR 6... ningún helper de test cambia de archivo en PR 6" — acá aplica igual dentro de cada commit de extracción).

S6 — Estructura medida (2026-11-15, en el worktree): total 2513; producción ~1502 (constantes 27/31/41, helpers puros, dos bloques `impl McpHandler` en 82–1045 y 1133–1310, `build_router` en 1421); `#[cfg(test)] mod tests` = 1502; tests ~1011. Baseline del ratchet verificado contra `scripts/quality-baselines.json`: 2513 == disco ⇒ sin drift que bajar antes del corte (el PR 1 produce el primer descenso, y viaja en él, S2).

S7 — Clusters de tests medidos (spans sobre `scraping.rs`, `mod tests` = 1502): help 1507–1568 (4 helpers) · budget 1569–1711 (5 tests, consume `retained_record_bytes`/`session_budget_message`) · ssrf-validación 1712–1956 (3 helpers + 13 tests) · robots 1957–2028 (2 tests) · discovery 2029–2181 (4 tests) · crawl-sitemap 2182–2368 (3 helpers aparte, 6 tests) · batch-metrics 2369–2476 (3 helpers + 3 tests) · discovered-urls 2477–2512 (2 tests).

S8 — Bootstrap del target: el guard `scripts/check_target_isolation.sh` (#1679) hoy rechaza este worktree con `reason=target-owned-by-other-worktree` (CARGO_TARGET_DIR heredado = main) y su mensaje indica escribir `.envrc` por el runbook de AGENTS.md; la política de Gentle AI bloquea el patrón `.envrc` en toda herramienta (bash `ls` incluido). Sin `.envrc` el registro de propiedad queda vacío y el gate local no corre sin `--allow-unregistered-target`. Pendiente: decisión del humano (ejecuta el heredoc, o aprueba la vía `env CARGO_TARGET_DIR=…` inline por invocación). No se rodea el patrón partiendo el nombre del archivo.

## Tasks

| ID | Tarea | Specs | Ruta | Estado | Commit |
|----|-------|------|------|--------|--------|
| T0 | Elección del archivo libre + coordinación por artefactos (worktrees, PRs, estado publicado) | S1 | inline | done | (sin commit: no toca código) |
| T1 | Registro de misión + espejo Engram | S1,S2 | inline | done | (este archivo; sin commit aún) |
| T2 | Bootstrap del worktree + índices (CodeGraph/codedb) + seed de target | S8 | inline | bloqueado | (espera decisión humana de S8) |
| T3 | Re-medición en el árbol antes del primer corte (línea completa, S4) | S4,S6 | inline | done | (números en S6/S7) |
| T4 | Propuesta de clusters/spans al humano para OK (gate de corte) | S7 | inline | done | (propuesta en conversación; OK pendiente) |
| T5 | PR 1 — extraer `mod tests` a hijos ≤500 por tema, un commit de unidad por cluster, tests verdes entre unidades + `--update` del baseline | S2,S3,S4,S5 | PR | in_progress | 6 commits (e470663c docs, a5d46275 common, 9030e6a6 budget, 1a497159 ssrf, 0a319653 robots+discovery, 1305584d crawl, fa6aaf83 batch+discovered+ratchet) |
| T6 | PR 2 — seams de producción: helpers puros (guard/robots, serialización JSONL) a módulos hermanos | S2,S3 | PR | pending | |
| T7 | PR 3 — dividir los dos `impl McpHandler` por herramienta/seam hasta ≤500 todas las partes | S2,S3 | PR | pending | |
| T8 | `--remove` de la entrada del baseline + verificación final (jscpd, gate) | S2,S3 | inline | pending | |

## Log

L1 — Pedido (verbatim, 2026-11-15): "debes tomar parte del plan y organizarte con los otros agentes para atacar algun archivo pendienta sin colisionar con los otros agentes" — tabla de los 5 monstruos >2000 restantes entregada por el usuario; `export_factory.rs` ocupado por `ola1-export-factory`, `waf_engine.rs` por `ola1-waf-engine`.

L2 — Coordinación (2026-11-15): worktree creado `~/Projects/Rust/webfang-worktrees/ola1-mcp-scraping`, rama `refactor/ola1-mcp-scraping` desde main `8aecddc2`. Inventario de agentes paralelos verificado por `git worktree list` + estado del branch: `fix-2750-jitter-stranded-slot` (8aecddc2) modificando solo `crates/webfang_core/src/application/rate_limiter/jitter.rs` — sin solape. Estado publicado vía `orchestrator_session_id` (subject `file-size-ola1-mcp-scraping`). PRs abiertos #2803/#2802 no tocan esta ola.

L3 — Medición (2026-11-15): scraping.rs 2513; baseline del ratchet igual (2513 en `scripts/quality-baselines.json`, sin drift); producción ~1502; `mod tests` = 1502; tests ~1011 en 8 clusters con spans en S7.

L5 — Corte ejecutado (2026-11-15): scraping.rs 2513→1517 (producción, sin tests inline). 8 hijos: tests_common 88, tests_budget 150, tests_ssrf 206, tests_robots 78, tests_discovery 161, tests_crawl_sitemap 192, tests_batch_metrics 106, tests_discovered_urls 41 (todos ≤500). Commits de unidad con `cargo test -p webfang_mcp --lib` 370 passed entre cada uno; clippy 0; fmt OK; golden intra-crate sin cambios (verificado POST-movida). Ratchet bajado en el MISMO commit final (fa6aaf83), 2513→1517. VERBATIM probado por multiconjunto: 935 líneas del `mod tests` original (git show 8aecddc2) vs unión de spans + helpers → idénticas salvo 7 `pub(super)` de helpers compartidos y el `use` block del viejo módulo (cada hijo trae el suyo); comentario `// ── #1611 F7` se perdió en C1 por oldText y se repuso en tests_budget.rs. DECISIÓN: destinos `scraping/tests_<tema>.rs` (patrón engine.rs), declaración `#[cfg(all(test, not(miri)))] // wiremock + wreq use btls-sys FFI (unsupported by Miri)` en orden alfabético. Ruta de build usada: `env CARGO_TARGET_DIR=/home/xavi/.cache/cargo-target/ola1-mcp-scraping CARGO_INCREMENTAL=0 RUSTC_WRAPPER= PATH=$HOME/.local/share/mbx/bin:$PATH cargo ...` + gate con `--allow-unregistered-target` (decisión humana de S8). NOTA: `check_file_size.sh` no tiene `--list` (el invariante de ola1-engine midió otra época); la prueba de verbatim por multiconjunto lo reemplaza.

L4 — Bloqueo (2026-11-15): `.envrc` bloqueado por política de Gentle AI en bash (heredoc, `ls`) y en write. `scripts/check_target_isolation.sh` correado a mano rechaza el worktree con `reason=target-owned-by-other-worktree` (CARGO_TARGET_DIR heredado = `main`, dueño `webfang/`) y su propio mensaje indica escribir `.envrc` por el runbook de AGENTS.md; `--allow-unregistered-target` NO salva ese caso (paso 7 antes de 8, #1267). Alternativa no destructiva bajo evaluación: `env CARGO_TARGET_DIR=/home/xavi/.cache/cargo-target/ola1-mcp-scraping CARGO_INCREMENTAL=0 RUSTC_WRAPPER= PATH=$HOME/.local/share/mbx/bin:$PATH <cmd>` por invocación (target no declarado por nadie ⇒ `unregistered-target`, que SÍ acepta el flag en el gate local). Decisión humana pendiente; NO se rodea el patrón partiendo el nombre del archivo.