# Ola 0 — batch 3 del programa de reducción de file-size (paraguas #2584)

Objetivo: partir 4 archivos elegidos por seams reales hasta dejarlos ≤500 y sacarlos del baseline con `--remove` en el mismo PR, más los 3 follow-ups de documentación del batch 2. Branch: `refactor/ola0-batch3` (desde `origin/main` 5d631d12). Entrega: un PR único contra `main` (el batch es la unidad autorizada por el usuario, precedente batch 2). Ratchet: `scripts/check_file_size.sh`; verificación dupla con `jscpd --complexity` (binario pineado 5.4.0). Test runner: `cargo nextest run`.

## Specs

S1 (batch): "arrancar el batch 3" — 4 archivos del batch 3 (veredictos de la exploración) + los 3 follow-ups del review del batch 2 (#2613, #2614, #2615, con `status:approved` ya aplicado por el usuario).
S2 (método): "Split por seams, un commit de unidad por extracción, tests verdes entre unidades, movimiento mecánico primero." (spec S2 del batch 2, vigente).
S3 (revisión): "Los cuatro lentes con resultado explícito; el PR declara 'Movido sin cambios' y 'Corregido / mejorado'." (S3 del batch 2).
S4 (cierre): "Cierre en el ratchet en el MISMO PR: --remove a lo que quede <=500; verificá check_file_size.sh y jscpd --complexity (binario pinneado 5.4.0) sobre los tocados." (S4 del batch 2).
S5 (gate): "Gate local con ci_fast_gate.sh — el guard real corre en CI repo-guards; si tu lane local no lo ejercita, corrélo a mano." (S5 del batch 2). windows-check N/A: el batch no toca `infrastructure/platform/` (windows_console.rs queda diferido).
S6 (baseline): "el baseline solo baja (--remove; subir es edición manual + notes, excepcional); un grandfathered que toques y no partas termina neto-cero" (S6 del batch 2). Los 3 follow-ups tocan archivos NO grandfathered (creados en batch 2 o bajo el límite) → sin interacción con el baseline.
S7 (scope y flujo): "sin PRs apilados; commits convencionales sin atribución IA; push y PR solo con mi confirmación explícita" (S7 del batch 2).
S8 (aprobación del batch): OK del usuario al seam plan por archivo tras verificar callers, blast radius y seams contra el código real (exploración: verdictos por archivo con evidencia codedb).
S9 (gobernanza): PR con `Closes #2613`, `Closes #2614`, `Closes #2615`, `Closes part of #2584`, label `type:refactor` — los tres issues ya llevan `status:approved`.
S10 (registro): "registro de misión en odd/tasks/file-size-ola0-batch3.md que viaja en el PR" (S10 del batch 2).
S11 (mcp): los archivos de `webfang_mcp` conservan el registro de herramientas byte-estable (36 tools pineado por `stdio_handshake.rs`; provenance gate con read_dir plano de `handlers/`) — la extracción de url_utils va en forma submódulo (`url_utils.rs` + `url_utils/tests.rs`), nunca hermano `.rs` plano ni conversión a `mod.rs`.
S12 (golden): "todo commit que toque `crates/**/src` refresca el golden del real tree en el MISMO commit con `bash scripts/refresh_real_tree_golden.sh`" (S12 del batch 2).
S13 (plan): el método completo vive en `odd/tasks/file-size-reduction.md` — este registro no lo reescribe.

## Seam plan (de la exploración, pendiente de S8)

| # | Archivo | Driver | Seam | Resultado |
|---|---------|--------|------|-----------|
| T1 | `application/url_filter.rs` (527) | tests 59% | verbatim `mod tests` → `url_filter/tests.rs` | ~215 + ~312 |
| T2 | `mcp_server/handlers/url_utils.rs` (567) | tests 59% (2 módulos) | verbatim ambos → `url_utils/tests.rs` (submódulo, S11) | ~229 + ~336 |
| T3 | `infrastructure/downloader/resource_governor.rs` (524) | tests 52% (mayoría acotada) | verbatim `mod tests` (cfg + cfg-not-miri viajan) → `resource_governor/tests.rs` | ~253 + ~272 |
| T4 | `cli/args/ai.rs` (557) | cluster ADR-0004 + tests (40%) | cluster de anuncios → `args/ai_notices.rs` + `pub use` en `args::ai` (3 consumidores por path completo: main.rs ×2, orphan_threshold_warning_test ×3) | ~400 + ~160 |
| T5 | follow-ups | — | #2613: corregir header de `dom_inspector_tests.rs`; #2615: header de provenance en `client_tests.rs`; #2614: opción A — documentar la excepción `#[path]` con doc comment en `client.rs` | 3 mini-fixes de comentarios |

Diferidos con veredicto: `provenance.rs` (excepción candidata — envolvente de seguridad cohesiva, 16 líneas sobre), `windows_console.rs` (batch 4/excepción — riesgo cfg(windows) cross-target), `handlers/ai.rs` (excepción — pin de registro de 9 categorías, 8 líneas sobre). El PR declara estos veredictos para la decisión del maintainer en el ratchet (notes del JSON).

## Tasks

| ID | Tarea | Specs | Ruta | Estado | Commit |
|----|-------|-------|------|--------|--------|
| T1 | extraer `mod tests` → `url_filter/tests.rs` | S1,S2,S11-S12 | inline | done | bea9b245 |
| T2 | extraer 2 módulos de tests → `url_utils/tests.rs` (submódulo) | S1,S2,S11-S12 | inline | done | 0a1dc750 |
| T3 | extraer `mod tests` → `resource_governor/tests.rs` | S1,S2,S12 | inline | done | 10689f80 |
| T4 | extraer cluster ADR-0004 → `args/ai_notices.rs` + re-exports | S1,S2,S12 | inline | done | 99496790 |
| T5 | follow-ups #2613/#2614/#2615 (comentarios) | S1,S9 | inline | done | 3aa0fa44 |
| T6 | `wc -l` + `--remove` x4; golden en cada commit; guard verde | S4,S6,S12 | inline | done | 5ccccfee |
| T7 | Verificación: guard + jscpd + gate local GREEN + suite workspace completa | S4,S5 | inline | done | — |
| T8 | Espejo Engram + PR (push solo con confirmación) | S7,S9,S10 | inline | done | — |

## Log

L1 — Pedido original (verbatim, 2026-11-05): "ok, aplica status:approved aplicado arrancar el batch 3" — labels verificados en #2613/#2614/#2615 (`status:approved,type:refactor`), worktree `refactor-ola0-batch3` creado sobre `main` 5d631d12, exploración de 7 candidatos delegada a explorador read-only (veredictos con evidencia codedb en este documento).

L2 — Aprobación del seam plan (2026-11-05): el usuario eligió "Aprobar el plan" (opción A para #2614: documentar la excepción `#[path]`, no reestructurar `client.rs`).

L3 — Ejecución y cierre (2026-11-05):
- **T1** (`bea9b245`): tests de `url_filter` → `url_filter/tests.rs` (movimiento verbatim con dedent determinista; ruta `url_filter::tests` preservada). 20/20 tests del módulo en verde; golden sin cambios (los sites de tests inline no se registran).
- **T2** (`0a1dc750`): los dos módulos de tests de `url_utils` → `url_utils/{tests.rs,handler_tests.rs}` en forma submódulo (S11). Suite completa de `webfang_mcp`: 648 tests en verde, incluyendo `stdio_handshake` (pin de 36 tools) y el `provenance_gate` (read_dir plano de `handlers/` — solo ve `url_utils.rs`).
- **T3** (`10689f80`): tests de `resource_governor` → `resource_governor/tests.rs`; ambas gates (`cfg(test)` + `cfg(not(miri))` con su comentario Miri) viajan en la declaración. Golden refrescado EN el commit (+sitios: los `use crate::…` del cuerpo quedaron a nivel de archivo).
- **T4** (`99496790`): seam de producción — cluster de anuncios ADR-0004 → `args/ai/ai_notices.rs` con `pub use` re-exports en `args::ai` (consumidores externos verificados con codedb: `webfang_cli/src/main.rs` y `tests/orphan_threshold_warning_test.rs` sin cambios, 22/22 del test de integración en verde); `pending_notices` pasa a `pub(super)` (única visibilidad cambiada, declarada). Callers verificados: `touches_ai_surface` intra-cluster, marker ×3 externos, notice ×1 en main.rs.
- **T5** (`3aa0fa44`): los 3 follow-ups — #2613 header corregido (paths crate-absolutos), #2615 header de provenance en `client_tests.rs` (tipo verificado: `OpenAiLlmClient`), #2614 opción A (doc comment en la declaración `#[path]`). Solo comentarios; golden refrescado (shifts de línea).
- **T6** (`5ccccfee`): conteos finales — url_filter 216+316, url_utils 235+89+254, resource_governor 255+275, args/ai 402+169: todo ≤500. `--remove` ×4: grandfathered 105 → **101**, guard verde.
- **T7**: verificación completa — clippy `--all-targets --all-features -D warnings` exit 0 · `cargo doc --workspace --all-features` con `RUSTDOCFLAGS=-D warnings` exit 0 · suite workspace completa `cargo nextest run`: **5048/5048 passed, 9 skipped** · `ci_fast_gate.sh` lane `code`: **PASS=40 FAIL=0 SKIP=2** (skips declarados: lint pineado y windows cross-target — N/A al batch) · reporte de cobertura leído: `test-full` cubierto con la suite completa, `repo-guards` parcial cubierto con el ratchet de duplicación a mano, resto NOT covered = N/A (sin cambios de deps, toolchain ni artefactos) · windows-check N/A (el batch no toca `infrastructure/platform/`).
- **Duplicación**: gate 8316 ≤ 8325 (desciende de nuevo) · jscpd dirigido sobre los 4 módulos tocados: **0 clones / 0 líneas duplicadas** en 1103 líneas.
- **Aviso advisory (no bloqueante)**: 26 NOTICE de regression-naming sobre tests MOVIDOS (los detecta como "agregados" por vivir en archivos nuevos) — mismo patrón del batch 2, declarado en el PR; renombrar sería cambio semántico fuera del movimiento mecánico.
- **Veredictos diferidos declarados en el PR**: `provenance.rs` (excepción candidata — envolvente de seguridad cohesiva), `windows_console.rs` (batch 4/excepción — riesgo cross-target), `handlers/ai.rs` (excepción — pin del registro de 9 categorías).
