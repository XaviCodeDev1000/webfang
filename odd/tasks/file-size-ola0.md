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
| T1 | `file_saver.rs`: extraer `rename_temp_into_place` (ambas ramas `cfg`, consts locales viajan) → `file_saver/rename.rs` `pub(super)` | S2,S7 | inline | pending | — |
| T2 | `vault_detector.rs`: extraer cluster registry (7 fns) → `vault_detector/registry.rs` `pub(super)` | S2,S7 | inline | pending | — |
| T3 | `model_profile.rs`: extraer catálogo builtin (`GRANITE_*_PROFILE_ID`, citations, `BUILTIN_IDS`, `builtin_profiles()`) → `model_profile/builtins.rs`; re-export público | S2,S7 | inline | pending | — |
| T4 | `entities/export.rs`: extraer `ExportFormat` + `StateVersion` (+ impls) → `export/format.rs`; `RawExportState`/`default_version` se quedan (frontera serde del estado); re-export | S2,S7 | inline | pending | — |
| T5 | Medir `wc -l`, cerrar ratchet en el mismo PR: `--remove` a los ≤500 (los 4, si miden), `--update` descendente solo si alguno queda >500 | S4,S6 | inline | pending | — |
| T6 | Verificación: guard + jscpd 5.4.0 + `check_windows_platform.sh` (rama `cfg(windows)`) + `ci_fast_gate.sh`; reportar skip honesto si el target no está | S4,S5 | inline | pending | — |
| T7 | Espejo Engram `odd/file-size-ola0/tasks` + readback de ambos lados | S11 | inline | pending | — |

Forecast / nota de entrega: los 4 splits + este registro suman más de ~400 líneas authored (mover ≈ borrar+crear), pero el batch de 4 es la unidad autorizada explícitamente por el usuario (decisión de entrega, no heurística).

## Log

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

L3 — 2026-11-03 · bootstrap: worktree `/home/xavi/Projects/Rust/webfang-worktrees/refactor-2597-ola0-batch1` desde `origin/main` 47349caf; `.envrc` de 5 líneas + `direnv allow`; `.env` copiado; índices de CodeGraph y CodeDB al día (codedb root = worktree, head `47349cafe2ec`); `seed_target.sh` → `seed: cold reason=no-seed` (cold build, sin problema); identity proxy OK (`refactor-2597-ola0-batch1` ↔ rama). Issue #2597 creado y `status:approved` aplicado por maintainer. Espejo Engram `odd/file-size-ola0/tasks` pendiente hasta el primer commit.
