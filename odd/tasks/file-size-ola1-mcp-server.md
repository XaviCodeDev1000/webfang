# Ola 1 — Misión: `mcp_server/server.rs` (paraguas #2584)

Objetivo: partir `server.rs` (2012 medido / 2012 en disco sobre `main` @ `acd48b04`;
baseline del ratchet = 2012 en `scripts/quality-baselines.json`, SIN drift) por seams
reales hasta que todas las partes queden ≤500 y la entrada salga del baseline con
`--remove`. Branch: `refactor/ola1-mcp-server` desde `origin/main` `acd48b04`; worktree
`~/Projects/Rust/webfang-worktrees/ola1-mcp-server`. Entrega: una misión por monstruo,
SIN batch; este monstruo puede llevar 2–4 PRs secuenciales propios (sin PRs apilados).
Ratchet: `scripts/check_file_size.sh`; verificación dupla con jscpd 5.4.0
(`--min-tokens 50`, rust, mild). Runner: `cargo test` (herencia de las misiones
anteriores: en esta máquina `cargo nextest` no está en PATH; declarar el runner usado).
Plan de record: `odd/tasks/file-size-reduction.md` — método, dos sombreros y gobernanza
viven ahí; este registro no los reescribe.

Coordinación (verificado por worktrees, 2026-11-15): ninguna otra misión activa toca
`crates/webfang_mcp/src/mcp_server/`; `ola1-waf-engine` está en `infrastructure/http/`
y `refactor/ola1-export-factory-tests` está cerrada (su monstruo salió del baseline).
Estado publicado vía `orchestrator_session_id` (topic `file-size-ola1-mcp-server`).

## Estructura medida (2026-11-15, sobre `main` @ `acd48b04`, S8)

- Total: **2012 líneas**; `#[cfg(test)] mod tests` en **1069** ⇒ producción **1068**,
  tests **944** (47 % tests). 42 tests `#[test]`/`#[tokio::test]` inline.
- Producción por temas (spans candidatos, re-medir antes de cada corte):

| Tema | Span aprox | Contenido |
|---|---|---|
| `ServerOptions` + auth/bind | 57–262 | struct, Debug redactado (#1615 F9), Default, `require_auth_for_external_bind`, `require_auth_or_explicit_anonymous`, `PLAINTEXT_BIND_WARNING`, `plaintext_bind_warning` |
| Router / composición | 263–461 | `build_mcp_router`, `build_mcp_router_with_service` (stack completo documentado), `into_axum_response`, `jsonrpc_panic_response` |
| Rate limiting | 462–677 | `build_rate_limiter`, `KeyedRateLimiter`, `RateKey`/`rate_key`, `rate_limit_middleware`, `retry_after_secs`, `RATE_LIMIT_OPERATION` |
| Session cap | 678–993 | `MCP_ENDPOINT_PATH`, defaults (`DEFAULT_MAX_SESSIONS` 64, ventana 300, `MAX_ALLOWED_SESSIONS_CAP` 100k), `SessionCap`, `session_cap_middleware`, `shed_response`, `is_mcp_endpoint`, `creates_session` |
| Arranque/shutdown | 994–1068 | `start_mcp_server`, `shutdown_signal` (drain del writer #1121, token #2193) |

- Tests inline 1069–2012 (~944): panic mapping (1087–1137), harness `test_handler`
  (1138–1148), sanity de lógica por handler (1149–1331), `ServerOptions::default`
  (1332–1363), `SessionCap` unitarios (1364–1631), rate limiter (1632–1770),
  auth/bind (1784–1902), panic payload render (1903–2012).

## Blast radius (CodeGraph, 2026-11-15)

- `start_mcp_server`: 1 llamador — `bin/mcp_server_http.rs`.
- `ServerOptions`: consumido por `bin/mcp_server_http.rs` (construcción + tests de argv)
  y suites de integración: `mcp_auth_fail_closed_test.rs`, `mcp_session_cap_test.rs`,
  `scraping_coverage_test.rs`, `tests/common/mod.rs`.
- `build_mcp_router`: `tests/common/mod.rs` (`webfang_binary` harness), suites
  behavioral/session-cap/scraping-coverage; `build_mcp_router_with_service` además por
  `mcp_panic_containment_test.rs`.
- Re-exports a preservar verbatim en el hub (`mod.rs` y tests externos importan desde
  `mcp_server::server::`): `build_mcp_router`, `build_mcp_router_with_service`,
  `start_mcp_server`, `ServerOptions`, `MCP_ENDPOINT_PATH`, `DEFAULT_MCP_ADDR`,
  `DEFAULT_MAX_SESSIONS`, `DEFAULT_SESSION_CAP_WINDOW_SECS`, `MAX_ALLOWED_SESSIONS_CAP`,
  `REDACTED_CREDENTIAL`, `PLAINTEXT_BIND_WARNING`, `require_auth_for_external_bind`,
  `require_auth_or_explicit_anonymous`, `plaintext_bind_warning`.
  ⇒ `pub use` en `server.rs` (o `server/mod.rs`) hacia los hijos; rutas externas NO cambian.

## Estrategia prevista

- **PR 1**: extraer `mod tests` inline (944) a hijos ≤500 por temas:
  `tests_panic.rs` (panic mapping + render payload, ~150),
  `tests_options.rs` (defaults + auth/bind, ~120),
  `tests_session_cap.rs` (SessionCap unitarios + creates_session, ~270),
  `tests_rate_limit.rs` (~140), sanity de handlers + harness
  (`tests_handler_logic.rs` + `tests_common.rs` si hay base compartida, regla L31
  de engine: medir consumidores antes de elegir destino). Resultado: server.rs ≈ 1075
  ⇒ `--update` descendente 2012→~1075 (sigue grandfathered, banda Ola 3).
- **PR 2..N**: seams de producción, un módulo hijo por tema con `pub use` en el hub:
  (a) `options.rs` (ServerOptions + Debug redactado + Default, ~200),
  (b) `rate_limit.rs` (KeyedRateLimiter + middleware + rate_key, ~215),
  (c) `session_cap.rs` (SessionCap + middleware + predicates, ~315),
  (d) posiblemente `router.rs` (composición del stack) o `startup.rs`
  (start_mcp_server + shutdown_signal). El hub queda con los re-exports + composición.
  Meta: todas las partes ≤500 ⇒ `--remove`.

## Specs

Heredan íntegras del método del programa (`odd/tasks/file-size-reduction.md`) y de los
precedentes de la Ola 1 (`file-size-ola1-engine.md` S1–S10, `file-size-ola1-mcp-scraping.md`):
S4 re-medición antes de cada corte (la prosa puede quedar stale) · S5 todo módulo de
tests extraído va a hijo propio ≤500 (`#[cfg(test)] mod tests;` con `#[path]`, posición
alfabética) · S6 movimiento puro verbatim probado (diff normalizado línea a línea) ·
cero ensanchamientos de visibilidad (diff de modificadores vacío; el `#[path]` de los
hijos de test se re-apunta, no se ensancha nada) · S7 baseline sólo baja · push y PR
solo con confirmación explícita del usuario · PR con `Closes part of #2584`,
label `type:refactor`, base `main`, sin PRs apiladas.

**S-ms1 — El orden del stack NO se toca.** `build_mcp_router_with_service` monta 8
capas con un orden que ES la corrección de seguridad #1611 (F5: auth fuera del rate
limiter; panic-containment dentro, etc.). Cualquier PR que toque la composición es
movimiento verbatim del bloque `Router::new()` completo — ni reordenar ni "limpiar".

**S-ms2 — Contratos de middleware documentados viajan con su código.** Los doc-comments
de `creates_session`, `session_cap_middleware` y `SessionCap` son especificación
(seguridad #1611 F6); se mueven verbatim con sus ítems.

**S-ms3 — Invariantes de tests.** Invariante `--list` (conjunto de hojas + conteo,
renombres declarados); los tests de integración externos (`tests/mcp_*.rs`,
`tests/common/mod.rs`) NO se tocan: compilan contra `mcp_server::server::` vía
re-exports y deben seguir haciéndolo sin edición.

## Tasks

| ID | Tarea | Specs | Ruta | Estado | Commit |
|----|-------|-------|------|--------|--------|
| T1 | Medición + estructura + blast radius (CodeGraph + wc/grep) | S4 | inline | done | (sin commit) |
| T2 | Registro de misión + espejo Engram | S2 | inline | done | (este archivo) |
| T3 | Propuesta de slice PR 1 al maintainer | S1,S3 | inline | pending | — |
| T4 | PR 1: `mod tests` inline → hijos ≤500 + `--update` 2012→~1075 | S5,S6 | worktree | pending | — |
| T5 | PRs siguientes: seams de producción hasta `--remove` | S2,S4,S-ms1,S-ms2 | worktree | pending | — |
| T6 | Verificación completa (compuertas), RDD, push + PR | S3 | inline | pending | — |

## Log

L2 — PR 1 ejecutado (2026-11-15, trabajo en curso en el worktree, sin commit todavía):
`mod tests` inline (1069–2012, 944 líneas) → 5 hijos + base común, TODOS ≤500:
`server/tests_panic.rs` 188 · `tests_options.rs` 166 · `tests_session_cap.rs` 278 ·
`tests_rate_limit.rs` 143 · `tests_handler_logic.rs` 186 · `tests_common.rs` 16
(harness `test_handler`, compartido por panic/options/handler_logic).
**server.rs 2012 → 1086** ⇒ `--update` descendente en el baseline (ratchet verde,
95 entradas grandfathered intactas). Tileo probado por el script de extracción
(1077–2011 sin huecos ni solapes; header del mod 1069–1076 se reparte). Prueba de
movimiento: 44/45 fns de test idénticas verbatim línea a línea; 2 reformateadas por
`cargo fmt` (`limiter_with_auth`, `test_convert_wiki_links_logic` — diff unificado
confirma solo wrapping de firmas/llamadas, cero cambio de contenido); `test_handler`
viaja a `tests_common.rs` como `pub(super)` (S9: se mueve con su primer consumidor
real). Gates A: check 0 · clippy estricto 0 · fmt --check 0 · doc -D warnings 0.
Gates B: suite lib 369=369 (antes=post stash A/B, mismo runner `cargo test -p
webfang_mcp --lib --all-features`) · invariante `--list` 369=369 con renombres de
ruta declarados `server::tests::X → server::tests_<tema>::X` (mismo conjunto de
nombres de hoja, 0 renombres de nombre) · suites de integración que consumen el
hub: mcp_session_cap 4/4 · mcp_panic_containment 8/8 · mcp_auth_fail_closed 9/9.
Gates D: ratchet verde con `--update 2012→1086` · inventario OK · dead-tests OK ·
dependencia inter-crate OK · dirección intra-crate OK (binario reconstruido con
`env -u CARGO_TARGET_DIR`) · golden intra-crate refrescado en el mismo cambio
(sin drift: `git diff real_tree_sites.txt` vacío) · jscpd: 9081→8158 duplicadas
(795→785 clones) — el movimiento BAJA la métrica (los tests inline del hub eran
clones). Falta: commit unidad + Log en el registro + push/PR con confirmación.

L1 — Pedido (2026-11-15): el usuario arranca la misión del último monstruo >2000 de la
Ola 1 (`server.rs` 2012). Worktree creado (`ola1-mcp-server`, rama
`refactor/ola1-mcp-server` desde `origin/main` acd48b04). Medición: 2012 = 1068
producción + 944 tests (47 %); mod tests en 1069; 42 tests inline. Blast radius medido
(CodeGraph): los consumidores externos viven en `bin/mcp_server_http.rs` y en
`tests/common/mod.rs` + 4 suites `mcp_*.rs` — todos por ruta `mcp_server::server::`,
por lo que el plan de re-exports del hub los cubre sin tocarlos.
