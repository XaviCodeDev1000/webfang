# Ratchet de tamaño de archivo (~500 líneas) — `scripts/check_file_size.sh`

AGENTS.md documenta la regla "ningún archivo nuevo debe superar ~500 líneas" pero
nada la fuerza: el estado medido en `main@3fcdd431` es 113 de 332 archivos no-test
por encima de 500 (máximo `engine.rs` 4063). Un gate duro amanecería rojo con 113
ofensores, así que el enforcement es un **ratchet descendente con grandfathered
map**, siguiendo el patrón de `quality-baselines.json` + `check_duplication.sh`
(#516): los 113 quedan congelados en su conteo actual, todo archivo no listado
queda bajo el límite duro de 500, y la lista solo baja.

## Specs

- S1: "arrancar solo con no-test (`crates/**/src/**`), igual que el comando de
  AGENTS.md". El script usa la expresión `find` de AGENTS.md **textual**
  (`find crates -name '*.rs' -not -path '*/tests/*'`), que además incluye
  `crates/webfang_core/build.rs` (hoy bajo 500: 0 ofensores extra).
- S2: "`wc -l` mayor que `hard_limit` falla". Comparación estricta: 500 exacto
  pasa, 501 falla.
- S3: "el baseline debe generarse con el script (flag `--generate`, de un solo
  uso), no con la medición ad hoc. Así el JSON y el script no pueden divergir".
  `--generate` rechaza correr si `file_size_rust` ya existe.
- S4: "usá `find -print0 | xargs -0`" y filtrar las líneas `total` de `wc`.
  Implementado estructuralmente: recorrido NUL-safe + `wc -l < archivo` por
  archivo (no existen líneas `total` ni parsing por lotes).
- S5: "una entrada del baseline sin archivo en disco **falla** con un mensaje
  claro ('quitá esta entrada'), en vez de pasar en silencio". Implementado como
  "entrada no presente en el corpus medido" (cubre borrado Y mudanza fuera de
  `crates/**/src`, p.ej. a `tests/`, que de otro modo sería un escape del ratchet).
- S6: "Archivo grandfathered que baja de 500: también debe fallar (o al menos
  avisar fuerte) hasta que se quite del JSON... Yo haría que falle: forzar a
  sacarlo de la lista es lo que consolida la victoria."
- S7: "--update... solo baja valores o elimina entradas, nunca sube ni agrega.
  Subir o agregar debe ser edición manual del JSON, para que quede visible en la
  review como decisión de maintainer". `--update` además rechaza contar ≤500
  (ese camino termina en `--remove`, no en una entrada piso-500).
- S8: "generá el JSON con claves ordenadas (`jq -S`) para que los diffs de PR
  sean mínimos": claves de `grandfathered` ordenadas; el round-trip preserva el
  formato del archivo (indent 2, sin newline final, orden de claves top-level).
- S9: casos mínimos del harness: archivo nuevo de 501 → falla; de 500 → pasa;
  grandfathered que crece 1 → falla; igual → pasa; que baja pero sigue >500 →
  pasa con aviso; que baja a ≤500 → falla pidiendo quitar la entrada; entrada
  sin archivo → falla.
- S10: "Sobre el ítem de `.github/`: dejalo para un PR aparte. El gate local en
  `ci_fast_gate.sh` ya aporta el valor."

## Decisiones de diseño

- `check_duplication.sh` NO corre en `ci_fast_gate.sh` local (jscpd es binario
  externo; vive en el job CI `code-quality`). El guard nuevo no requiere
  binarios externos (bash + python3 + find + wc), así que SÍ se engancha local
  en `lane_fmt_and_guards` vía `run_guard` (guard + harness), junto a los otros
  guards baratos. El wiring CI (job `code-quality`/`repo-guards` de `ci.yml`)
  queda para el PR aparte de `.github/` (S10).
- Lectura del baseline fail-closed con causas nombradas, mismo protocolo
  `status\tvalue` de `check_duplication.sh`.
- Corpus vacío o `crates/` inexistente falla (fail-closed, nunca "0 archivos
  = todo verde").
- Los mensajes de usuario van en español con `::error::`, comentarios en
  inglés (convención de `scripts/`).

## Tasks

- T1: `scripts/check_file_size.sh` — modos: check (default), `--generate`
  (one-shot), `--update path=count...` (solo descendente), `--remove path...`.
- T2: `scripts/test_file_size.sh` — harness hermético (sandbox mktemp, guard
  ejercitado como copia, 13 casos).
- T3: `--generate` en este worktree; verificar que las entradas == 113 (la
  medición ad hoc); `git diff` del JSON mínimo (solo la entrada nueva).
- T4: wiring `run_guard` en `ci_fast_gate.sh::lane_fmt_and_guards`.
- T5: commits por unidad de trabajo; `bash -n` + shellcheck (lista compartida
  de ignores).

## Log

- L1: "tengo alguna regla o script relacionado con el tamano de archivos?" —
  pregunta original de la sesión; derivó en el diseño del ratchet.
- L2: medición en main@3fcdd431: 113 ofensores (core 94, mcp 17, cli 1,
  test_utils 1). Única diferencia entre las dos expresiones `find`:
  `crates/webfang_core/build.rs` (bajo 500). Cola de borde 501–570: 8 archivos
  (las victorias baratas del ratchet).
- L3: decisión de usuario: `tests/` fuera de scope (extensible después sin
  reescribir); `.github/` en PR aparte.
- L4: ENMIENDA de la segunda parte de L3, forzada por la política del repo:
  el meta-guard de cobertura de guards (#2056) falla el PR con
  `unwired-guard: check_file_size.sh is reachable from no workflow` y su
  baseline "solo encoge" prohibe registrarlo como conocidamente sin cablear.
  El guard + su harness se cablean en el job `repo-guards` de `ci.yml`
  DENTRO de este PR (patrón dependency-direction, pasos vecinos), consultado
  con el usuario sin respuesta explícita; decisión tomada sobre la única
  ruta conforme hacia un PR mergeable.
