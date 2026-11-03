# Ola 0 — batch 1 del programa de reducción de file-size (paraguas #2584)

Objetivo: partir los 4 archivos elegidos por seams reales hasta dejarlos ≤500 y sacarlos del baseline con `--remove` en el mismo PR. Branch: `refactor/2597-ola0-batch1` (desde `origin/main` 47349caf). Entrega: un PR único contra `main` (el batch es la unidad autorizada por el usuario). Ratchet: `scripts/check_file_size.sh`; verificación dupla con `jscpd --complexity` (binario pineado 5.4.0). Test runner: `cargo nextest run`.

## Specs

S1 (batch): "Elegí el primer batch: 3–4 archivos DISJUNTOS, los más simples e independientes — esta ola calibra el flujo, no persigue líneas."
S2 (método): "Split por seams, un commit de unidad por extracción, tests verdes entre unidades, movimiento mecánico primero."
S3 (revisión): "Los cuatro lentes con resultado explícito; el PR declara \"Movido sin cambios\" y \"Corregido / mejorado\"."
S4 (cierre): "Cierre en el ratchet en el MISMO PR: --remove a lo que quede <=500, --update descendente a lo que quede >500; verificá check_file_size.sh y jscpd --complexity (binario pinneado 5.4.0) sobre los tocados."
S5 (gate): "Gate local con ci_fast_gate.sh — y sabé que el guard real corre en CI repo-guards; si tu lane local no lo ejercita, corrélo a mano."
S6 (baseline): "el baseline solo baja (--update descendente / --remove; subir es edición manual + notes, excepcional); un grandfathered que toques y no partas termina neto-cero"
S7 (scope y flujo): "sin test files en scope; sin PRs apilados (el batch siguiente recién cuando el anterior mergeó); commits convencionales sin atribución IA; push y PR solo con mi confirmación explícita"
S8 (aprobación del batch): "Veredicto: dale con los 4, sin cambios." (OK del usuario tras verificar los claims de seams contra el código: callers únicos, autocontención y re-exports — ver Log L2)
S9 ( gobernanza ): issue del batch con `Closes part of #2584`, label `type:refactor`; `status:approved` de maintainer en #2597 (aplicado 2026-11-03).
S10 (worktree): "worktree propio (branch refactor/<N>-ola0-batch1 desde origin/main, bootstrap completo con índices)"
S11 (registro): "registro de misión en odd/tasks/file-size-ola0.md con mis specs textuales (viaja en el PR)"
S12 (scope de tests, decisión de usuario): "Híbrido por archivo (Recomendado)" — "Extraer tests inline a hermanos SOLO donde el archivo sea casi todo tests y el movimiento sea verbatim; preferir seam de producción cuando exista y sea limpio." Resultado: los 4 archivos tienen seam de producción limpio → **ninguno mueve tests ni crea archivos de test**.
S13 (plan): el método completo (dos sombreros, batching, gobernanza, excepciones) vive en `odd/tasks/file-size-reduction.md` — este registro no lo reescribe.

## Tasks

| ID | Tarea | Specs | Ruta | Estado | Commit |
|----|-------|-------|------|--------|--------|
| T1 | extraer `rename_temp_into_place` → `file_saver/rename.rs` | S2,S7 | inline | done | 03d9f3f5 (parent 489; check_windows_platform.sh OK para x86_64-pc-windows-msvc) |
| T2 | extraer cluster registry (7 fns) → `vault_detector/registry.rs` | S2,S7 | inline | done | ec6a08c1 (parent 411; tests verdes, 1 línea de import de test declarada) |
| T3 | extraer catálogo builtin → `model_profile/builtins.rs` | S2,S7 | inline | done | 8f9c8724 (parent 484; 24/24 model_profile_test verdes, sin tocar tests; re-export verificado) |
| T4 | extraer `ExportFormat` + `StateVersion` → `export/format.rs` | S2,S7 | inline | done | 02ba0a16 (parent 428; 12/12 resume_mode_tests verdes; 1 doc-link path-qualified) |
| T5 | `wc -l` + `--remove` x4; guard verde; descartado cx como métrica de mejora (split mecánico preserva CX al carácter; ver L4) | S4,S6 | inline | done | 094efe6d (109 en el baseline; el diff son exactamente las 4 entradas) |
| T6 | Verificación: guard + jscpd + windows-check + gate local GREEN + suite workspace completa | S4,S5 | inline | done | (gate PASS=39 FAIL=0 SKIP=3; nextest workspace --all-features: 5151 passed / 9 skipped / 0 failed; check_windows_platform.sh OK; jscpd 5.4.0 CX preservado carácter a carácter; cargo doc: ICE de rustdoc en este workstation — ver L5) |
| T7 | Espejo Engram `odd/file-size-ola0/tasks` actualizado con evidencia de ejecución | S11 | inline | done | (obs 2724 + este registro) |

Forecast / nota de entrega: los 4 splits + este registro suman más de ~400 líneas authored (mover ≈ borrar+crear), pero el batch de 4 es la unidad autorizada explícitamente por el usuario (decisión de entrega, no heurística).

## Log

L6 — 2026-11-03 · corrección CI (commit de seguimiento, rama ya pusheada): la primera corrida de CI puso rojo `Repo guards` → "Intra-crate lint tool tests": el golden set `tools/intra-crate-lint/tests/fixtures/real_tree_sites.txt` no se refrescó en el mismo commit que cambió `crates/webfang_core/src` (contrato de `real_tree.rs`; writer: `scripts/refresh_real_tree_golden.sh`, fail-closed e idempotente). Diagnosticado reproduciendo el test en local: delta 9/9 líneas — sitios de `export/format.rs` y `model_profile/builtins.rs` agregados, números de `export.rs` corridos; `rename.rs`/`registry.rs` no referencian rutas `crate::` y por eso no figuran. **REGLA PARA LOS PRÓXIMOS BATCHES: todo commit que toque `crates/**/src` refresca el golden en el MISMO commit con `bash scripts/refresh_real_tree_golden.sh`.** El remedio ya estaba documentado en Engram (obs #2638).

L1 — Pedido original (verbatim, 2026-11-03):

"Misión: Ola 0 del programa de reducción de file-size (paraguas #2584).
El plan de record es odd/tasks/file-size-reduction.md en origin/main — leélo
entero antes de tocar nada (método, dos sombreros, batching, gobernanza).
Contexto de las rondas anteriores en Engram (odd/file-size-reduction).

1. Listado: banda [501, 570] vía la query jq del plan sobre
   scripts/quality-baselines.json. Los conteos nunca van a prosa.
2. Elegí el primer batch: 3–4 archivos DISJUNTOS, los más simples e
   independientes — esta ola calibra el flujo, no persigue líneas. Antes de
   editar una línea: presentame el batch y el seam plan por archivo
   (CodeGraph explore: callers + blast radius + driver real) para mi OK.
3. Con mi OK: issue del batch con `Closes part of #2584` → pedime
   status:approved → worktree propio (branch refactor/<N>-ola0-batch1 desde
   origin/main, bootstrap completo con índices) → registro de misión en
   odd/tasks/file-size-ola0.md con mis specs textuales (viaja en el PR).
4. Ejecución: split por seams, un commit de unidad por extracción, tests
   verdes entre unidades, movimiento mecánico primero. Los cuatro lentes con
   resultado explícito; el PR declara "Movido sin cambios" y
   "Corregido / mejorado".
5. Cierre en el ratchet en el MISMO PR: --remove a lo que quede <=500,
   --update descendente a lo que quede >500; verificá check_file_size.sh y
   jscpd --complexity (binario pinneado 5.4.0) sobre los tocados.
6. Gate local con ci_fast_gate.sh — y sabé que el guard real corre en CI
   repo-guards; si tu lane local no lo ejercita, corrélo a mano.

Reglas duras: el baseline solo baja (--update descendente / --remove; subir
es edición manual + notes, excepcional); un grandfathered que toques y no
partas termina neto-cero; sin test files en scope; sin PRs apilados (el
batch siguiente recién cuando el anterior mergeó); commits convencionales
sin atribución IA; push y PR solo con mi confirmación explícita; Ask first
sin respuesta mía = te detenés y esperás."

L2 — Aprobación del batch (verbatim, 2026-11-03): el usuario verificó contra el código real los tres claims de mayor riesgo (file_saver: 2 defs cfg, 1 call site; vault_detector: hub llama exactamente a los miembros del cluster, único caller externo `detect_from_registry`; model_profile: re-export obligatorio, 4/7 call sites en `tests/model_profile_test.rs`) y dictaminó: "**Veredicto: dale con los 4, sin cambios.**" El #4 (export.rs) quedó como el de riesgo más bajo, cubierto por compilación + suite.

L5 — 2026-11-03 · cierre: gate local `ci_fast_gate.sh` GREEN (PASS=39 FAIL=0 SKIP=3; coverage report leído: `repo-guards` parcial con test-inventory en este lane, `code-quality` con la duplication ratchet, `test-core` parcial — nextest de los crates seleccionados por el clasificador; el SKIP de `nextest integration` y de `check_windows_platform` queda cubierto acá por la suite completa (5151/5151) y por la corrida manual de `scripts/check_windows_platform.sh`). `check_file_size.sh` corrido a mano: verde (109 entradas en el baseline). `cargo doc` local: rustdoc 1.91.1 ICEs ("scoped-tls ... set first", search-index merge) en CUALQUIER render fresco acá — reproducido con el fuente pristine de origin/main, o sea no lo introduce este diff; en `main` todo sale cacheado y por eso ahí nunca crashea; CI corre el mismo comando con el mismo toolchain y está verde en main. Docs se generaron igualmente (303 index.html). Los dumps rustc-ice (gitignored) se limpiaron. Veredicto de lentes: sin hallazgos de comportamiento; 2 adaptaciones mecánicas declaradas (1 doc-link path-qualified en format.rs; 1 línea de import en tests de vault_detector).

L4 — 2026-11-03 · ejecución: T1–T4 commit por unidad, tests verdes entre unidades, `cargo fmt --check` limpio, `cargo check -p webfang_core` sin errores ni warnings, `check_windows_platform.sh` OK (cfg(windows)). Guard de tamaño: los 4 padres miden ≤500 y sus partes también; `--remove` x4 vía `scripts/check_file_size.sh --remove` → baseline en 109. jscpd 5.4.0 `--complexity` antes/después: CX preservado al carácter en los 4 splits (vault_detector 72→48+24; export 59→42+17; file_saver 57→50+7; model_profile 41→40+1) — un split mecánico ni añade ni quita complejidad; la ganancia es estructural. Sin hallazgos funcionales en el movimiento (2 sombreros): el único cambio no literal del batch es el doc-link path-qualified en `format.rs` (rustdoc) y el `use super::registry::get_registry_path` en los tests de vault_detector (1 línea, requerido por `pub(super)`).

L3 — 2026-11-03 · bootstrap: worktree `/home/xavi/Projects/Rust/webfang-worktrees/refactor-2597-ola0-batch1` desde `origin/main` 47349caf; `.envrc` de 5 líneas + `direnv allow`; `.env` copiado; índices de CodeGraph y CodeDB al día (codedb root = worktree, head `47349cafe2ec`); `seed_target.sh` → `seed: cold reason=no-seed` (cold build, sin problema); identity proxy OK (`refactor-2597-ola0-batch1` ↔ rama). Issue #2597 creado y `status:approved` aplicado por maintainer. Espejo Engram `odd/file-size-ola0/tasks` creado (obs 2724) y conflictos juzgados.
