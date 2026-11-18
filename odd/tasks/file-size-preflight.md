# Misión: file-size-preflight (F1a)

Objetivo: entregar la fase F1a de la política file-size v3 (paraguas #2584):
`scripts/file_size_preflight.sh` con `--simulate`, su harness semántico y el alta en el gate.
Branch: `feat/file-size-preflight` (worktree `feat-file-size-preflight`; base ff en `0d2c16c2`).
Delivery: UN PR, commits work-unit; runner: `bash scripts/test_file_size_preflight.sh` (+ `--negation`),
`bash scripts/test_ci_gate_structure.sh`, `bash scripts/ci_fast_gate.sh` (lane ci).
El cuerpo del PR es la política v3 (texto pegado por el owner el 2026-11-18; re-pedirlo si la sesión no lo conserva).

## Specs

S1 (entregables — §11 F1a, verbatim): "Cuatro archivos nuevos: scripts/file_size_preflight.sh (lee
hard_limit, importa el corpus del guard, emite el bloque con unidad y comando por línea, clasifica por
predicado, constante P); --simulate (destino ← rangos, sin parser de Rust);
scripts/test_file_size_preflight.sh; filas en ci_fast_gate_coverage.tsv para el guard y para el
preflight." Y el cierre de §11: "Todo script nuevo se da de alta en el gate en el mismo PR."
Superficie concreta derivada: `scripts/file_size_preflight.sh` (nuevo) · `scripts/test_file_size_preflight.sh`
(nuevo, ya en RED) · `.github/workflows/ci.yml` (2 steps en repo-guards) · `scripts/ci_fast_gate.sh`
(run_guard + lane_ci disparado) · `scripts/ci_fast_gate_coverage.tsv` (nota de la fila repo-guards).

S2 (el bloque — §4.1 verbatim): el preflight "usa el corpus del guard (no lo re-implementa) y emite:
total, producción, tests, churn, funciones largas, allow, blast radius, solape con PRs abiertos
(ci_pr_overlap.sh) y una clasificación sugerida de §5". Regla de citas (§6 verbatim): "todo número
lleva su unidad y el comando que lo produce en la misma línea, no en una nota al pie". Clave
`funciones` reservada y vacía (§11: "Clave de funciones reservada y vacía si el parser la tolera").

S3 (corpus): `--corpus-literal` imprime la expresión `find` del guard byte a byte (harness 2.1:
"--corpus-literal == el find del guard, byte a byte"); la expresión se EXTRAYE del texto de
`scripts/check_file_size.sh` en runtime y la extracción falla con causa nombrada si el guard cambia
de forma (fail-closed). Un corpus, un dueño: el guard.

S4 (--simulate): §4.3 verbatim: "--simulate toma el plan (destino ← rangos) y calcula el tamaño
resultante de cada archivo con wc -l total, más cabeceras. Rechaza si algún destino no grandfathered
supera 500." Plan `<dest>:<FROM>-<TO>[:header=<N>]`; claves del plan relativas al directorio padre
del directorio del archivo fuente (para archivos del corpus: el directorio del crate). rc: 0 legal ·
3 violación de ratchet (destino no grandfathered > hard_limit; destino grandfathered > su baseline;
fuente ≤ hard_limit no grandfathered → el mensaje apunta a `--remove`/consolidación) · 4 defecto
nombrado (rango invertido, rango pasado EOF, rangos solapados, destino == fuente) · 2 uso · 1 fallo
de herramienta. El bloque y `prs` se EMITEN antes de reclamar: `prs` usa moved recortado a EOF
(harness 13.6: rango 1-3000 sobre archivo de 900 → prs=1).

S5 (constante P — §6 verbatim): "Aviso en 3000 líneas (add+del); no bloquea. Es una elección de
política, no un valor derivado... Constante en el preflight [F1a] con esta muestra en un comentario,
comparador > estricto, y solo cambia a la baja". Muestra (los cuatro diffs medidos por la revisión
del 2026-11-17): 2246 (#2901), 3018 (#2896), 3933 (#2856), 5101 (#2893) — cada una con su comando
`gh pr view <pr> --json additions,deletions` en la salida de `--budget`. Fórmula (§6): "≈ ceil(2 ×
líneas movidas ÷ P)".

S6 (lexer): el span de tests inline se lexea, NO se cuenta por llaves: llaves dentro de strings,
chars, comentarios y lifetimes no lo mueven (harness 7.x, fixture adversario); archivo sin balancear
→ span `indeterminado` + `inline_tests_note` con la causa, nunca un span inventado (8.x); el span
incluye la línea `#[cfg(test)]` anterior al `mod` y cierra en la línea de la llave de cierre.

S7 (honestidad / hermeticidad): nada se escribe en el árbol medido (14.1: árbol byte-idéntico tras
todos los modos); churn fuera de git → `indeterminado`, nunca 0 (9.1: "0 commits" y "cannot know"
son hechos distintos); path inexistente → rc=1 con causa nombrada (3.x); path fuera del corpus →
`in_corpus false` con rc=0, reportado no medido por el ratchet (4.x); harness hermético con la
herramienta ejercida como COPIA en sandboxes mktemp; `--negation` falla sobre un árbol correcto
("A harness that cannot fail proves nothing (#2056)").

S8 (clasificación por predicado): línea `clasificacion` sugerida — `exception` si el archivo está en
el mapa `exceptions` del baseline (señal documental; NO exonera del límite); `tests-mayoria` si los
tests inline superan el 50% del total; `funcion-gigante` si el archivo contiene
`allow(clippy::too_many_lines)`; si no, `revisar` con nota que nombra las señales §5 no mecánicas
(módulos pegados, declarativo cohesivo, alto riesgo, arquitectónico: decisión de maintainer según la
columna Señal de §5). [Supuesto reconciliado — ver L4.]

S9 (cableado del gate): ci.yml nombra AMBOS scripts en repo-guards (harness 15.1 corre el harness,
15.2 corre el preflight mismo: línea terminando en `file_size_preflight.sh`); el .tsv NO gana fila
por script (15.3: "The .tsv is a table of JOBS (#2559)") — la declaración va en la nota de la fila
repo-guards EXISTENTE; `test_ci_gate_structure.sh` verde; en `ci_fast_gate.sh`: preflight como
`run_guard` en `lane_fmt_and_guards` (junto al ratchet) y harness en `lane_ci` disparado por diff
sobre el par {preflight, harness} (mismo patrón que `test_file_size.sh`); lane full lo corre siempre
(CI_HARNESS_MODE=always).

S10 (lectura del baseline): hard_limit, grandfathered y exceptions leídos con UN lector python3
embebido (el patrón de un solo lector del guard), estados nombrados fail-closed (MISSING_FILE,
BAD_JSON, MISSING_HARD_LIMIT, GRANDFATHERED_NOT_OBJECT, ...). El parser no exonera: `exceptions` es
señal de clasificación, nunca exención del límite ni del net-zero. [Decisión — ver L5.]

S11 (aceptación — §11 F1a verbatim): "Reproduce server.rs 2012→360; --simulate rechaza un plan con
un destino no grandfathered sobre hard_limit; el harness clasifica un fixture de cada tipo de §5;
test_ci_gate_structure.sh verde. Los archivos nuevos fuera de crates/ requieren la aprobación de
AGENTS.md (Ask first)." Reconciliación: "Reproduce server.rs 2012→360" se ejercita como 10.x
(--simulate == corte real línea a línea sobre fixtures) + spot check en el repo real (total ==
wc -l, churn reproducible, clasificación de `crates/webfang_mcp/src/mcp_server/params.rs` =
exception). Ask-first: APROBADO por el owner (paste 2026-11-18: "Mi recomendación es sí, con
--simulate en el mismo PR" + "esto es el plan de esta mision").

## Tasks

T0 ✅ (S11): sync branch — ff-only origin/main → 0d2c16c2 (housekeeping, sin commit).
T1 (S7,S8,S10): refinamientos RED del harness: fix 1.2 (`--` suelto que degradaba la aserción),
`sim_age` muerto fuera, tests 16.x de clasificación (fixtures tests-mayoria / funcion-gigante /
revisar / exception) + `write_baseline` con mapa exceptions. Commit `test(ci)`.
T2 (S2-S8,S10): implementar `scripts/file_size_preflight.sh`; harness 1-14 GREEN. Commit `feat(ci)`.
T3 (S1,S9,S3): cableado: ci.yml (2 steps en repo-guards), ci_fast_gate.sh (run_guard + lane_ci),
nota repo-guards del .tsv; harness 15.x GREEN + `test_ci_gate_structure.sh` verde. Commit `feat(ci)`.
T4 (S11,S7): verificación: `--negation` (rojo sobre árbol correcto), spot checks reales
(params.rs exception, total == wc -l, churn reproducible), lane ci del gate local sobre el diff,
cobertura del gate leída antes de pushear.
T5: Log final + espejo Engram + resumen. Push/PR = decisión del owner (cuerpo del PR = política v3).

## Log

L1 (2026-11-18): pedido verbatim: "vamos a terminar la mision que esta en
/home/xavi/Projects/Rust/webfang-worktrees/feat-file-size-preflight". Plan pegado por el owner:
síntesis v3 ("Actualicé el documento a v3 con todo lo que reportó M0, salvo una cosa que aún tiene
que cerrar el maintainer. Mi recomendación es abrir F1a ahora, con --simulate en el mismo PR.") +
política v3 completa ("esto es el plan de esta mision").

L2 (2026-11-18): estado al abrir: branch en aba0e9cf SIN commits propios; harness
`scripts/test_file_size_preflight.sh` sin seguimiento (RED confirmado: rc=1, "falta
...file_size_preflight.sh -- F1a todavia no esta escrito"); sin doc de misión. FF-merge a 0d2c16c2.
M0 re-medido al abrir F1a (como ordena §11): jq sobre el baseline → hard_limit=500, grandfathered=86,
exceptions=4 (`crates/webfang_mcp/src/mcp_server/params.rs`, `crates/webfang_mcp/src/mcp_server/provenance.rs`,
`crates/webfang_core/src/infrastructure/platform/windows_console.rs`, `crates/webfang_mcp/src/mcp_server/handlers/ai.rs`)
— coincide con la revisión del owner. `.tsv`: 0 menciones de file[_-]size en sus 39 líneas
confirmado por lectura completa (fila repo-guards sin nombrar el guard).

L3: reconciliación del .tsv: el esquema (test_ci_gate_structure.sh, #2559) es UNA FILA POR JOB de
gate.needs — "filas en el .tsv para el guard y para el preflight" (§11) se cumple declarando el par
en la nota de la fila repo-guards existente, y el harness 15.3 fija que los scripts nunca son filas
propias (una fila por script rompería test_ci_gate_structure.sh).

L4: reconciliación de clasificación: "el harness clasifica un fixture de cada tipo de §5" (§11) —
las señales de módulos pegados, declarativo cohesivo, alto riesgo y arquitectónico son semánticas
(columna Señal de §5), no medibles por un script offline; el harness fija un fixture por tipo
MECÁNICO (exception, tests-mayoria, funcion-gigante) + el fallback `revisar`. La clasificación es
"sugerida" (§4.1), nunca un veredicto.

L5: lector del baseline: python3 embebido (patrón del guard: un solo lector, estados nombrados,
fail-closed) en lugar de jq (§6 lo cita como consulta humana del límite); python3 ya es dependencia
garantizada por el guard y jq añadiría una dependencia de entorno nueva al preflight.

L6: modo default del preflight = `--budget`: la invocación desnuda de ci.yml (15.2 exige la línea
terminando en `file_size_preflight.sh`) corre el bloque de parámetros — smoke real contra el repo
sin red ni cargo; el baseline corrupto lo levanta el guard, que CI también corre.
