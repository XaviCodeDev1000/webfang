# Ola 0 — batch 2 del programa de reducción de file-size (paraguas #2584)

Objetivo: partir los 4 archivos elegidos por seams reales hasta dejarlos ≤500 y sacarlos del baseline con `--remove` en el mismo PR. Branch: `refactor/2597-ola0-batch2` (desde `origin/main` 055d849a). Entrega: un PR único contra `main` (el batch es la unidad autorizada por el usuario). Ratchet: `scripts/check_file_size.sh`; verificación dupla con `jscpd --complexity` (binario pineado 5.4.0). Test runner: `cargo nextest run`.

## Specs

S1 (batch): "Opción A con los 4 archivos de tests-inline" — corregida: 3 de tests-inline + 1 seam de producción (`author_extractor.rs` no es tests-mayoría: 279 prod / 227 test = 44%; forzarlo a mover tests viola "split por seams reales, nunca por aritmética de líneas").
S2 (método): "Split por seams, un commit de unidad por extracción, tests verdes entre unidades, movimiento mecánico primero."
S3 (revisión): "Los cuatro lentes con resultado explícito; el PR declara \"Movido sin cambios\" y \"Corregido / mejorado\"."
S4 (cierre): "Cierre en el ratchet en el MISMO PR: --remove a lo que quede <=500, --update descendente a lo que quede >500; verificá check_file_size.sh y jscpd --complexity (binario pinneado 5.4.0) sobre los tocados."
S5 (gate): "Gate local con ci_fast_gate.sh — y sabé que el guard real corre en CI repo-guards; si tu lane local no lo ejercita, corrélo a mano."
S6 (baseline): "el baseline solo baja (--update descendente / --remove; subir es edición manual + notes, excepcional); un grandfathered que toques y no partas termina neto-cero"
S7 (scope y flujo): "sin PRs apilados (el batch siguiente recién cuando el anterior mergeó); commits convencionales sin atribución IA; push y PR solo con mi confirmación explícita"
S8 (aprobación del batch): OK del usuario al seam plan por archivo tras verificar callers, blast radius y seams contra el código real.
S9 (gobernanza): issue del batch con `Closes part of #2584`, label `type:refactor`.
S10 (registro): "registro de misión en odd/tasks/file-size-ola0-batch2.md con mis specs textuales (viaja en el PR)"
S11 (scope de tests, decisión de usuario): "Híbrido por archivo" — "Extraer tests inline a hermanos SOLO donde el archivo sea casi todo tests y el movimiento sea verbatim; preferir seam de producción cuando exista y sea limpio." Resultado: 3 archivos tienen tests como mayoría (embedding_semantic_inspector 62%, llm/client 58%, dom_inspector 58%) → split de tests verbatim a hermanos; `author_extractor.rs` (44% tests) → seam de producción real (cluster JSON-LD → `author_extractor/jsonld.rs`).
S12 (golden): "todo commit que toque `crates/**/src` refresca el golden del real tree en el MISMO commit con `bash scripts/refresh_real_tree_golden.sh`" (regla L6 del batch 1).
S13 (plan): el método completo (dos sombreros, batching, gobernanza, excepciones) vive en `odd/tasks/file-size-reduction.md` — este registro no lo reescribe.

## Tasks

| ID | Tarea | Specs | Ruta | Estado | Commit |
|----|-------|-------|------|--------|--------|
| T1 | extraer `#[cfg(test)] mod tests` → `embedding_semantic_inspector_tests.rs` | S2,S7,S11 | inline | done | 49cb5f1c |
| T2 | extraer `#[cfg(test)] mod tests` → `client_tests.rs` | S2,S7,S11 | inline | done | 51fc36fb |
| T3 | extraer `#[cfg(test)] mod tests` + `mod default_dom_inspector_tests` → `dom_inspector_tests.rs` | S2,S7,S11 | inline | done | b0cd4004 |
| T4 | extraer cluster JSON-LD (4 fns + `BYLINE_PREFIXES`) → `author_extractor/jsonld.rs` | S2,S7,S11 | inline | done | ae29d4b3 |
| T5 | `wc -l` + `--remove` x4; golden refresh; guard verde | S4,S6,S12 | inline | done | a0cbbb26 |
| T6 | Verificación: guard + jscpd + windows-check + gate local GREEN + suite workspace completa | S4,S5 | inline | done | — |
| T7 | Espejo Engram `odd/file-size-ola0-batch2/tasks` actualizado con evidencia de ejecución | S10 | inline | done | — |

Forecast / nota de entrega: los 4 splits + este registro suman más de ~400 líneas authored (mover ≈ borrar+crear), pero el batch de 4 es la unidad autorizada explícitamente por el usuario (decisión de entrega, no heurística).

## Log

L1 — Pedido original (verbatim, 2026-11-04):

"Empezar por la Opción A con los 4 archivos de tests-inline"

Corrección (2026-11-04): `author_extractor.rs` no es tests-mayoría (279 prod / 227 test = 44%). El usuario decidió: "Seam de producción real" — extraer cluster JSON-LD a `author_extractor/jsonld.rs` en lugar de mover tests.

L2 — Aprobación del seam plan (verbatim, 2026-11-04): el usuario verificó el seam plan por archivo (3 splits de tests verbatim + 1 seam de producción) y dictaminó: "Aprobar el plan y arrancar el seam."

L3 — Ejecución y cierre (2026-11-05):
- **Reubicación a worktree**: la sesión anterior dejó la misión en el checkout principal; a pedido explícito del usuario ("todo debe estar en un worktree") se reubicó la rama `refactor/2597-ola0-batch2` a `~/Projects/Rust/webfang-worktrees/refactor-2597-ola0-batch2` (copia verificada byte a byte del estado sin commitear, sin `stash` ni `--force`; repo principal restaurado a `main` limpio). Bootstrap completo: `.envrc` propio, índices CodeDB/CodeGraph del worktree.
- **T4 completado**: la extracción estaba a medio camino — el cluster ya estaba copiado a `jsonld.rs` pero los 4 fns duplicados seguían en el archivo original (E0255 latente). El cierre fue borrar el duplicado: movimiento mecánico puro, `mod.rs` 503 → 458, `jsonld.rs` 61. Los puntos de entrada quedan `pub(super)`; la superficie pública (`extract_author`, `extract_author_from_document`, `AuthorExtractor`) no cambia (callers verificados: solo `application/extraction.rs`).
- **Evidencia T4**: `cargo check` exit 0 · golden refrescado en el mismo commit (+9 sitios, S12) · `cargo fmt --check` OK · nextest lib webfang_core 2959/2959.
- **T5**: wc: client.rs 213 · embedding_semantic_inspector.rs 204 · dom_inspector.rs 219 · author_extractor 458+61 — todo ≤500. `--remove` x4 en el ratchet: grandfathered 109 → 105, guard verde.
- **Evidencia T6 parcial**: clippy `--all-targets --all-features -D warnings` exit 0 · gate de duplicación 8317 ≤ 8325 (baja) · jscpd dirigido sobre `author_extractor/`: 0 clones / 0 líneas duplicadas · windows-check N/A (el batch no toca `infrastructure/platform/`).
- **Evidencia T6 completa**: `cargo doc --workspace --all-features` con `RUSTDOCFLAGS=-D warnings` exit 0 · suite workspace completa `cargo nextest run`: **5048/5048 passed, 9 skipped** · `ci_fast_gate.sh` lane `code`: **PASS=40 FAIL=0 SKIP=2** (skips declarados: lint pineado y windows cross-target, que no aplica al batch) · reporte de cobertura leído: `test-full` cubierto a mano con la suite completa, `repo-guards` parcial cubierto con el ratchet de duplicación a mano, resto NOT covered = N/A (el batch no toca deps, toolchain ni productores de artefactos).
- **Aviso advisory (no bloqueante)**: 17 NOTICE de regression-naming sobre tests MOVIDOS en T1–T3 (los detecta como "agregados" por vivir en archivos nuevos). El PR lo declara bajo revisión; renombrarlos sería cambio semántico fuera del movimiento mecánico.
