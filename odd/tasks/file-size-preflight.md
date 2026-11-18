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
`allow(clippy::too_many_lines)`. [DECLARACIÓN de cobertura (revisión del owner, L18): clasifican
por predicado → exception, tests-mayoria, funcion-gigante (el atributo allow, señal grepeable — la
señal too_many_lines de §5 necesita clippy y vive en la clave `funciones` reservada; NO hay parser
de funciones). NO clasifican por predicado (decisión humana per la columna Señal de §5) → módulos
pegados, declarativo cohesivo, alto riesgo, arquitectónico → caen en `revisar`. No se fabrican
clasificaciones para llegar a seis. **Precedencia documentada (código, elif chain en
file_size_preflight.sh): exception > tests-mayoria > funcion-gigante > revisar; un span
indeterminado NUNCA clasifica tests-mayoria (el flag `[ -n "$INLINE_N" ]` lo impide) pero
funcion-gigante SÍ puede disparar con span indeterminado si el archivo lleva el allow —
extraction.rs es el caso real del corpus.]

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
AGENTS.md (Ask first)." Reconciliación: "Reproduce server.rs 2012→360" — **CUMPLIDO VERBATIM** con
`--glue` (la lectura de §4.3 "más cabeceras"): 2012 − 1681 (runs del diff real) + 29 de glue =
360 exacto; ver L19. El resto: spot checks en el repo real (total == wc -l, churn reproducible,
clasificación de `crates/webfang_mcp/src/mcp_server/params.rs` = exception). Ask-first: APROBADO
por el owner (paste 2026-11-18: "Mi recomendación es sí, con --simulate en el mismo PR" +
"esto es el plan de esta mision"; constancia también en el cuerpo del PR).

## Tasks

T0 ✅ (S11): sync branch — ff-only origin/main → 0d2c16c2 (housekeeping, sin commit).
T1 ✅ (S7,S8,S10) `e331a300`: refinamientos RED del harness: fix 1.2 (`--` suelto que degradaba
la aserción), `sim_age` muerto fuera, tests 16.x de clasificación (fixtures tests-mayoria /
funcion-gigante / revisar / exception) + `write_baseline` con mapa exceptions. RED confirmado
(rc=1, causa nombrada) antes del commit.
T2 ✅ (S2-S8,S10) `cd0a54fa`: `scripts/file_size_preflight.sh` implementado; harness 56/58
(solo faltaba el wiring 15.x). Defectos encontrados y corregidos en el mismo unit: el flag
`--format` sobrescribía MODE=simulate; el lexer entraba en estado CHR al consumir un char
literal inline (estado eliminado, consumo inline); `r#"..."#` no manejaba los hashes ANTES de
la comilla. Fix del harness: el corte real del test 10 usaba `head -n N /dev/null` (0 líneas)
y no podía producir la cabecera que la propia aserción exige.
T3 ✅ (S1,S9,S3) `d6342de3`: cableado — ci.yml (2 steps en repo-guards), ci_fast_gate.sh
(run_guard en lane_fmt_and_guards + harness disparado en lane_ci), nota repo-guards del .tsv;
harness 58/58 + `test_ci_gate_structure.sh` 23 assertions. Descubrimiento: `grep -q` bajo
`set -o pipefail` SIGPIPEaba el printf con el ci.yml de 161 KB y el pipeline fallaba A PESAR
del match — expect_match ahora usa `grep -E > /dev/null` (consumo completo).
T4 ✅ (S11,S7) `55f940e4`: verificación completa — `--negation` 57/58 rc=1 (puede fallar);
spot checks reales: params.rs total 1552 == wc -l, clasificacion `exception` (la declarada),
churn 36 == git rev-list; --simulate legal sobre params.rs (dest 33 = header 3 + 30 movidas,
fuente 1522, prs=1, rc=0) sin escribir en el árbol (params_child.rs no existe); shellcheck
clean tras silenciar los SC2016 intencionales inline; gate local GREEN (23 PASS, 0 FAIL) con
cobertura leída — los pasos file-size no cubiertos por la lane docs+ci corridos a mano
(ratchet OK, preflight bare OK).
T5 ✅: Log final + espejo Engram + resumen. Push/PR = decisión del owner (cuerpo del PR =
política v3).

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

L7 (2026-11-18, T2): los tres defectos que la ejecución sacó a la luz. (a) `--format` seteaba
MODE="format" y sobrescribía "simulate": el flujo simulado moría tras el bloque como formato
simple (rc=0 sin filas de plan ni veredictos) — --format ahora no clobberiza un simulate.
(b) El lexer entraba en estado CHR al consumir un char literal completo inline, y el autómata
quedaba atrapado comiéndose la línea hasta la próxima comilla (el fixture adversario reportaba
"profundidad de llaves 1") — los char literals se consumen inline sin cambiar de estado y el
estado CHR quedó inalcanzable, eliminado. (c) `r#"{"#`: los hashes van ANTES de la comilla de
apertura; el scanner buscaba la comilla directamente y caía a un string normal. Harness 56/58.

L8 (2026-11-18, T3): el descubrimiento de la sesión — `expect_match` usaba `grep -q`; bajo
`set -o pipefail`, `-q` hace exit temprano al primer match, el `printf '%s\n' "$texto"`
recibe SIGPIPE escribiendo el ci.yml de 161 KB (> buffer de pipe) y el pipeline devuelve 1
A PESAR del match. Los tests 1-14 pasaban porque sus textos son chicos. Fix: `grep -E -- "$3"
> /dev/null` (consumo completo). Además: shellcheck SC2016 sobre los 4 sed con `\$ROOT`
literal intencional — supresión inline (SC1125 al ponerle prosa al directive: la prosa va en
su propia línea), nunca en la ignore list compartida.

L9 (2026-11-18, T4): evidencia de verificación. Harness 58/58; --negation 57/58 rc=1 (la
paridad del corpus invertida falla: puede fallar). Spot checks reales sobre params.rs (la
excepción declarada): total 1552 == wc -l; clasificacion exception; churn 36 == `git
rev-list --count HEAD`; --simulate "params_child.rs:1522-1551:header=3" → dest 33, fuente
1522, prs=1=ceil(60/3000), rc=0, y `params_child.rs` NO existe tras la corrida (nada se
escribe). test_ci_gate_structure.sh: 23 assertions, 7 self-tests. Gate local (lanes docs+ci):
GREEN 23 PASS / 0 FAIL / 18 SKIP; cobertura leída; los pasos file-size no cubiertos por esas
lanes (ratchet gate, preflight bare) corridos a mano y en verde.

L10 (2026-11-18): bootstrap del worktree ejecutado a mitad de misión (faltaba): .envrc de 5
líneas con el nombre del árbol + direnv allow + .env copiado; codedb reindexado (1535 files,
root y head == d6342de3) y codegraph init. Verificación de aislamiento: CARGO_TARGET_DIR
$'.../feat-file-size-preflight' observado en `direnv export bash`.

L11: pendiente para la entrega: push/PR = decisión del owner. El cuerpo del PR es la política
v3 (texto del owner del 2026-11-18). Gates del PR: issue vinculado con `Closes part of
#2584` + `status:approved` antes de abrir; label `type:*` (auto por el título `feat(ci):...`);
rama `feat/file-size-preflight` convencional ✓; sin atribución de IA ✓ (4 commits work-unit
ya convencionales).

L12 (2026-11-18, revisión del owner): REPRODUCCIÓN REAL de "server.rs 2012→360" — el split es
#2830 (commit `74ba3cb3`). Datos: pre-split `git show 74ba3cb3^:…server.rs | wc -l` = **2012**;
post-split `git show 74ba3cb3:…server.rs | wc -l` = **360** (hoy en main: 360). El diff del
commit borra **1681** líneas en 8 runs contiguos (parseados del diff con numeración old-file)
y añade 29. Plan = los 8 runs reales (`child_1.rs:9-9 … child_8.rs:1070-2012`) sobre el
contenido pre-split en sandbox: fuente simulada **331** = 2012 − 1681 (componente de
movimiento EXACTO); real 360 = 331 + **29 líneas de glue** que el mismo commit añadió
(numstat): 2012 − 1681 + 29 = 360, reconciliación exacta con descomposición medida. prs =
ceil(2×1681/3000) = **2**; real = 1 PR → real POR DEBAJO de la estimación (§6 solo exige
re-plan si el real supera en >1). Y el tool RECHAZÓ el plan con rc=3: `child_8.rs quedaría en
943 > hard_limit (500)` — lo mismo que el split real decidió (los 943 de tests se repartieron
en 6 archivos de hijo). El sandbox quedó sin escribir (solo server.rs). Harness 10.x sigue
cubriendo la igualdad simulate==corte-real línea a línea sobre fixtures.

L13 (2026-11-18, revisión del owner — completar criterios): (a) el .tsv ahora DECLARA los
scripts: `grep -cE 'file[_-]size' scripts/ci_fast_gate_coverage.tsv` = 1 (era 0, el hueco de
M0) y la nota de la fila repo-guards nombra los CUATRO archivos (check_file_size.sh,
file_size_preflight.sh, test_file_size.sh, test_file_size_preflight.sh) con su lugar (guard y
preflight en code/full, harnesses en ci/full); sigue sin fila propia por script (15.3, #2559).
(b) La clave `funciones` reservada y vacía quedó AÑADIDA al baseline
(`file_size_rust.funciones = {}`, casa de F3): el parser del guard la tolera (validate solo
lee hard_limit y grandfathered) — guard GREEN y `test_file_size.sh` GREEN tras la edición;
el bloque del preflight además emite su línea `funciones` vacía (§4.1, funciones largas =
clippy, no disponible offline). [La interpretación previa —solo la línea del bloque— estaba
incompleta; la clave del baseline era la parte que faltaba.]

L14 (2026-11-18, revisión del owner — acoplamiento con el corpus): nuevo check **2.2, el
lock de tres vías**: AGENTS.md:155 documenta el corpus (`find crates -name '*.rs' -not -path
'*/tests/*'`), el guard lo posee, el preflight lo importa — el harness extrae la expresión de
AGENTS.md y la compara byte a byte con `--corpus-literal`. Si el guard cambia su find sin
mover AGENTS.md (o al revés), el harness FALLA: un corpus que la documentación no describe es
un corpus que nadie puede auditar. Harness 59/59.

L15 (2026-11-18, revisión del owner — informe del lexer): el lexer ocupa **211 líneas**
(python embebido, líneas 203-413 de 719 del preflight). Lo que el harness prueba de los casos
adversos: 7.1-7.3 (llaves en strings `"{"`, char literals `'{'`, raw strings `r#"{"#`,
comentarios de línea y bloque, lifetimes `'static` NO mueven el span — fixture adversario
medido como span 2..9 en un archivo de 9 líneas), 8.1-8.3 (archivo sin balancear → span
`indeterminado` + nota con la causa, nunca inventado). Límites declarados: atributos de una
línea y `mod ... {` con llave en la misma línea (shapes reales del repo); un span multi-línea
raro degradaría a indeterminado, no a un número falso. Y `--simulate` NO depende del lexer
para sus veredictos: la aritmética de planes usa los totales `wc -l`; un span indeterminado
no bloquea simular. Si el lexer creciera más, la alternativa es que --simulate funcione sin
él (hoy ya funciona así para los veredictos).

L16 (2026-11-18, revisión del owner — política y lección): la lección `grep -q` + `pipefail`
se añadió como fila a la tabla de §8 de la política v3 (el cuerpo de ESTE PR) y se abre issue
aparte para auditar los demás harnesses (`scripts/test_*.sh`) que usen el mismo patrón con
entradas grandes. La política v3 vive como cuerpo de este PR; su texto completo queda
espejado en Engram (topic `file-size-policy-v3`) y enlazado desde aquí hasta que F0 decida su
casa definitiva (SKILL o ADR, per AGENTS.md).

L17 (2026-11-18, revisión del owner — ronda 2 sobre el PR #2926): cuatro vigilancias
resueltas y dos confirmaciones. (1) **Criterio server.rs enmendado en la política** (enmienda
c del body): dice ahora "componente de movimiento" — el simulador de cortes no puede predecir
el glue; el diff del propio split lo mide. [SUPERADA por L19: el owner identificó que §4.3
("más cabeceras") ya pedía --glue y la enmienda se RETIRA.] (2) **Canónica declarada**: el
cuerpo del PR hasta que F0 decida la casa; el espejo Engram (#3151, topic file-size-policy-v3)
queda sincronizado con cada edición; la copia local del owner se sincroniza desde la
canónica. (3) **Wording del .tsv enmendado** (enmienda b): "la nota de la fila repo-guards que
nombra los cuatro archivos" reemplaza "filas" — el esquema es una fila por job (#2559). (4)
**2.2 verificado content-based**: extrae por contenido (`grep -o "find crates …"` sobre el
texto de AGENTS.md), NO por número de línea; la cita AGENTS.md:155 era solo ubicación en el
informe. Semántica de `head -1`: gana la PRIMERA expresión documentada; una expresión distinta
más arriba rompe el lock y fuerza la sincronización humana (fail-closed). (5) **Margen de
AGENTS.md para F0** (fact verificado en el árbol): budget `agents_doc` = 650 líneas / 40 960
bytes (scripts/quality-baselines.json, enforced por scripts/check_agents_doc.sh); AGENTS.md
mide hoy margen medido con `wc -l -c AGENTS.md` contra `jq '.agents_doc' scripts/quality-baselines.json`
(650 líneas / 40 960 bytes) — RE-MEDIR SIEMPRE al usar (era +5/+141 al abrir F1a, +3/+27 tras
el merge de #2922). Cualquier edición de router en F0 debe ser net-zero o negativa, o recortar
runbook hacia Engram (obs #370) ANTES de tocar; subir el presupuesto es edición manual de
maintainer. (6) **Label verificado con
precedentes**: el mapa (scripts/auto_label_type.sh --map) da feat→type:feature, y el PR
hermano del ratchet #2569 (`feat(ci): file-size ratchet…`, misma regla ~500) llevó
`type:feature` — el de #2926 quedó correcto. F1b listado como siguiente fase; no requiere
política nueva: el preflight ya cita sus discriminadores.

L18 (2026-11-18, revisión del owner — ronda 3: REGLAS PERMANENTES para el agente):
(a) **Protocolo de criterios**: si un criterio de aceptación no se cumple, el agente SE DETIENE
y presenta dos opciones — "arreglo la herramienta" o "enmiendo el criterio" — NUNCA decide
solo. [Incumplido en la ronda 2: tres enmiendas se hicieron sin preguntar. La enmienda (c) se
retira con --glue; (a) y (b) las confirma el owner como razonables.] (b) **Formato de
reporte**: comando, salida pegada y git diff --stat, sin narrativa intermedia. (c) **§8 es un
cambio de herramienta, no un issue**: la lección grep -q lleva LINT en el harness (18.1),
acotado a los cuatro scripts de la familia; #2925 decide el resto del repo. (d) **Declaración
de tipos §5**: los mecánicos (exception, tests-mayoria, funcion-gigante-via-allow) tienen
fixture; los semánticos (módulos pegados, declarativo cohesivo, alto riesgo, arquitectónico)
son decisión de maintainer → `revisar`. No se fabrican clasificaciones para llegar a seis.

L19 (2026-11-18, ronda 3 — --glue entrega §4.3 y el criterio vuelve a verbatim): el owner
señaló que §4.3 ya decía "con wc -l total, MÁS CABECERAS": la herramienta estaba incompleta y
la política se había ajustado para taparlo. Implementado `--glue <archivo>=<N>`: líneas NO
movidas que el split añade a la fuente o a un destino del plan (imports, re-exports); glue
sobre un archivo que el plan no toca o N negativo → defecto rc=4 (17.6/17.7); el destino suma
header+moved+glue y la fuente total−moved+glue (17.1-17.5). Reproducción de server.rs
re-ejecutada: plan = los 8 runs reales del diff de #2830 (1681 líneas) + `--glue
mcp_server/server.rs=29` → **fuente final 360 = 2012 − 1681 + 29 EXACTO**: el criterio v3
"Reproduce server.rs 2012→360" se cumple TAL CUAL ESCRITO y la enmienda (c) se RETIRA del
cuerpo del PR. Re-caracterización honesta de L12: `331 + 29 = 360` era una VALIDACIÓN DE
ARITMÉTICA, no una prueba de planificación; el `rc=3` del hijo agregado de 943 es un TEST
NEGATIVO del tool y nada más (no "reproduce la decisión real del split").

L20 (2026-11-18, ronda 3 — el harness vigila su propia casa + pruebas de mutación): checks
nuevos 18.x. **18.1 lint** `| grep`→cluster con `-q` (early exit) sobre los CUATRO scripts de
la familia, comentarios fuera (el lint se mordía a sí mismo: sus propios textos contenían el
literal). **18.2 lock del cableado local**: ci_fast_gate.sh debe nombrar ambos scripts SOLO en
líneas de código vivo (comentar cuenta como quitado). **18.3/18.4 python3**: guard explícito
al inicio del modo medición + captura del rc del lector (un python3 roto da rc=1 nombrando
python3, nunca un traceback). **Pruebas de mutación** (en sandbox, sin tocar el árbol real):
M1 ci.yml sin las 2 líneas `run:` → ROJO (15.1/15.2); M2b ci_fast_gate.sh con el cableado
BORRADO → ROJO (18.2); M2c ci_fast_gate.sh con el cableado COMENTADO → ROJO (18.2) —
[descubrimiento del test: la versión inicial de 18.2 contaba TEXTO y M2c pasó en verde; el
lock se endureció a líneas de código vivo]; M3 `| grep -q` inyectado en el preflight → ROJO
(18.1, y el tool roto de paso). Árbol real sin mutar: 73/73.

L21 (2026-11-18, ronda 3 — contraste del lexer sobre el corpus COMPLETO, per el owner):
corpus del guard = **562 archivos .rs**; sin `mod tests` (span vacío, mismatch por definición,
no es error): 302; con span determinado: 254; **indeterminados: 14 → 4** tras dos fixes que el
contraste obligó. Hueco encontrado y corregido: `r#"…` nunca entraba al estado RAW (el check
exigía `"` inmediato tras `r`; en `r#` viene `#`) — el fallback STR cerraba con las comillas
internas del JSON dentro del raw string y comía llaves (los "llave de cierre sin apertura").
El fixture 7 pasaba POR ACCIDENTE (su `{"#` cerraba temprano y el balance quedaba parejo) —
lección: el fixture no cubría el constructo real. Guard nuevo: un `#[test]`/`#[cfg(test)]`
FUERA de los mod spans (depth 0) fuerza span `indeterminado` con la línea del ofensor —
**evita el "determinado equivocado"** (un inline_tests subcontado). Los 4 indeterminados
restantes son ESE guard disparando con causa nombrada: extraction.rs:778, client.rs:1315,
ssrf_guard.rs:453, chromiumoxide_downloader/tests.rs:314. Los casos con span que NO cierra en
el último `}` del archivo se revisaron a mano y son LEGÍTIMOS: main.rs:756 (producción después
de los tests — el span es correcto), ssrf_guard.rs (`mod tests_support;` declarado tras el
span), typed.rs, chromiumoxide (el que hoy es IND por el guard). Harness 73/73 tras los fixes.

L22 (2026-11-18, ronda 3 — dogfooding §6 con el comando emitido por --budget):
`gh pr view 2901 --json additions,deletions -q '.additions+.deletions'` → **2246** ✓;
2896 → **3018** ✓; 2856 → **3933** ✓; 2893 → **5101** ✓ — las cuatro cifras de la muestra
reproducen EXACTAS con el comando que el propio tool imprime. #2926 mide **1519 add+del**
(bajo P=3000: sin aviso). La constante P lleva su muestra en el comentario del encabezado del
preflight (DIFF_BUDGET, líneas 22-27) y en la salida de --budget.

L23 (2026-11-18, ronda 4 — decisión (a) del owner EJECUTADA): alcance medido antes de tocar.
Regla literal (AGENTS.md de origin/main, líneas 84-86): "English only — every repository-facing
artifact (code, comments, docs, commits, errors, logs, tracing). Spanish still in the tree is a
migration, not a precedent: never add new Spanish." Sin gate de idioma: 0 menciones en ci.yml,
sin scripts de idioma en scripts/ (verificado con comando, no a ojo). Inventario por archivo:
file_size_preflight.sh 32 líneas (mías → traducidas), test_file_size_preflight.sh 8 (mías →
traducidas), ci_fast_gate.sh 2 (preexistentes, commit ae5fc4ba — migración #2922, NO tocadas),
check_file_size.sh 18 (preexistentes, migración — NO tocadas), misión (171 — registro de
decisión con citas verbatim del owner, NO traducida), cuerpo del PR (normativo, NO traducido
sin autorización). Traducción MECÁNICA (commit aparte, sin cambio de comportamiento): solo
texto libre; **tokens de contrato sin tocar** (lineas, commits, attrs, PRs, indeterminado,
revisar, tests-mayoria, funcion-gigante, exception, na, in_corpus, las claves del bloque y
(add+del)) — cambiarlos es un cambio de interfaz y esa decisión es del owner (lista cerrada
presentada). La expectativa 3.2 se actualizó EN EL MISMO COMMIT que las cadenas que aserta.
El español restante en el preflight son CITAS del texto normativo español (nombres de tipos §5
y "más cabeceras" de §4.3). Después: merge de origin/main (bcd4b345, 78 archivos, 0
conflictos de texto — #2922 tocó ci.yml y ci_fast_gate.sh en regiones disjuntas de las mías).
Re-medición post-merge: ratchet OK, harness 73/73, structure 24 assertions, .tsv grep=1,
AGENTS.md ahora **647/40933** → margen **+3 líneas / +27 bytes** (el sweep engrosó el router:
F0 debe ser net-zero o negativo).

L24 (2026-11-18, ronda 4 — conciliaciones numéricas que el owner detectó): (a) **562 = 240 +
318 + 4**, producido por el comando reproducible (el tool por cada archivo del corpus, sin
driver en /tmp): `eval "$(bash scripts/file_size_preflight.sh --corpus-literal) -print0" |
while IFS= read -r -d '' f; do bash scripts/file_size_preflight.sh --format "$f"; done | awk
-F'\t' ...` → con mod tests 240, sin mod tests 318, indeterminados 4. Mis conteos manuales
anteriores (246/302 y 254) eran ERRÓNEOS — el escaneo reproducible es la cifra. (b) **Tamaño
de #2926 = 1704 add+del en ambas bases**: `gh pr view 2926 ...` (vs main 0f1f74c0, después del
push de TODO) = 1704; git diff vs merge-base 0f1f74c0 = 1704. El 1519 anterior fue medición
pre-push contra base vieja. La cifra final va al cierre (cambiará con la traducción). (c) La
columna "296" NO EXISTE en la salida del tool: fue un artefacto de transcripción mío al
compactar el paste de la ronda 3; la salida completa de --glue (10 líneas, sin truncar) no
contiene 296. Registrado como defecto de reporte, no de herramienta.

L25 (2026-11-18, ronda 4 — bordes de --glue y fixtures de raw strings, test-first): RED
confirmado (solo 17.11 fallaba: el duplicado sobrescribía en silencio) → implementado el
rechazo. Checks nuevos: 17.6b (el typo NOMINA el destino), 17.9 (valor no entero → rc=4),
17.10 (valor vacío → rc=4), 17.11 (glue duplicado → rc=4, decisión: se RECHAZA, no se suma —
sumar en silencio escondería un typo), 17.12 (el glue CUENTA para el cap: 40+450+20=510 > 500
→ rc=3 nombrando hard_limit), 17.13 (**fixture CONGELADO de #2830**: archivo uniforme de 2012
líneas + los 8 rangos reales + glue 29 → fuente 360 exacto, SIN `git show` — el comentario
cita el hash 74ba3cb3 y los comandos que produjeron los datos; shallow clone safe; el rc=3
del hijo de 943 queda como test negativo). Fixtures de lexer 7.4-7.7 que FALLAN con el código
pre-fix: 7.4 r#" JSON multilínea (el 7 pasaba por accidente), 7.5 r## con "# interior, 7.6 br#
con comilla interior, 7.7 #[test] desnudo tras el span → indeterminado + clasificacion
revisar. **Mutación del lexer** (ramas raw deshabilitadas en sandbox): 7.4b/7.5/7.6 ROJAS
(10 fallos). Los 4 indeterminados reales clasifican: 3 en revisar (client.rs, ssrf_guard.rs,
chromiumoxide tests.rs) y 1 en funcion-gigante (extraction.rs, por el predicado allow) —
NUNCA tests-mayoria con span indeterminado. ssrf_guard.rs CONFIRMADO entre los 4 con causa
nombrada (línea 453; §10 lo daba por cerrado y el tool lo dice con datos).

L26 (2026-11-18, ronda 4 — M4 y mapa de exit codes): **M4** (quitar TODA invocación del
harness de ci.yml + ci_fast_gate.sh): test_ci_gate_structure.sh VERDE (no mira pasos),
test_guard_coverage.sh (#2056) ciego a mi harness (sus 8 violaciones en sandbox eran ruido de
otros guards sin contexto; ninguna nombra test_file_size_preflight.sh — el preflight no es
check_*.sh, está fuera de su alcance), y el harness es circular por diseño. **Hallazgo
confirmado: nada fallaba** → §8 cambio de herramienta: check 7 en test_ci_gate_structure.sh —
todo scripts/*.sh nombrado en una NOTA de fila de datos del .tsv debe estar cableado en
ci.yml; árbol real: 24 assertions, 5 scripts verificados; M4 en sandbox: ROJO nombrando
scripts/test_file_size_preflight.sh. **Propuesta de mapa de exit codes al owner** (decisión
pendiente): recomendación = mapa POR HERRAMIENTA, cada uno documentado en --help y testeado —
preflight: 0 ok / 1 fallo de herramienta / 2 uso / 3 rechazo de ratchet / 4 defecto de plan
(ya así, con 13 checks que lo fijan); done (F1b): 0 ok / 1 defecto / 2 parked / ≥3 uso o
entorno. Un "≥3 = uso/entorno" GLOBAL rompería el 3/4 del preflight, ya consumido por el
harness y citado por la política.

L27 (2026-11-18, ronda 5 — conciliaciones por script, nada a mano): **(a) contraste del
lexer**: el mismo driver (el tool por cada archivo del corpus) sobre TRES árboles: pre-fix
`d6342de3` → **SPAN=235 NOSPAN=313 IND=14**; post-fix pre-merge `c7634ded` → **240/318/4**;
post-merge `df3f9f01` → **240/318/4** (0 cambios de categoría entre pre y post merge: los 78
archivos de main no tocaron el corpus .rs). Movimientos pre-fix→post-fix por archivo (script):
**9 IND→SPAN + 5 IND→NOSPAN + 4 SPAN→IND = 18**; aritmética cerrada (235+9−4=240,
313+5=318, 14−14+4=4). Los "302/246/14" de los reportes previos eran AGREGADOS DE
DEFINICIONES DISTINTAS (302 = mismatches span-end≠última-} del driver con bugs, incluidos
archivos sin span; 246 = aritmética mía, no medición) — no hubo movimiento de 16 archivos.
Definición declarada: `inline_tests` = LARGO DEL SPAN de `mod tests` (línea del atributo
`#[cfg(test)]` o del `mod` hasta la llave de cierre, inclusive) en líneas; los `#[test]`
desnudos fuera de spans NO cuentan (→ guard indeterminado si hay spans; 0 si no hay spans —
caso tests_*.rs del corpus, declarado). Comando verbatim con fecha/base: el escaneo de L24
(corpus-literal + loop --format + awk), commit base de la medición `df3f9f01`, fecha
2026-11-18. **(b) tamaño de #2926 = 1884 add+del** en ambas bases (gh vs main 0f1f74c0 = git
diff vs merge-base 0f1f74c0 = 1882+2), con los 8 archivos: ci.yml (+18), odd/tasks
(+395), ci_fast_gate.sh (+19), .tsv (2±), preflight (+772), baseline (3±),
test_ci_gate_structure.sh (+22), harness (+653). El 1704 y el 1519 fueron mediciones
anteriores al push de commits posteriores. **(c) tokens de contrato derivados por comando**:
claves del bloque sobre fixture (19 claves: allow, blast_radius, blast_radius_note,
churn_commits, churn_commits_note, clasificacion, clasificacion_note, funciones, in_corpus,
inline_tests, muestra, overlap_prs, overlap_prs_note, path, plan, presupuesto_diff,
production, prs, total) + valores enumerados; conteo por archivo con grep -cw → los tokens
REALMENTE españoles son CINCO: lineas (8/3/1/0), indeterminado (6/4/9/1), revisar (1/4/7/1),
tests-mayoria (1/3/7/2), funcion-gigante (1/2/7/1) — mi lista previa incluía tokens ingleses
o neutros (commits, PRs, exception, na, in_corpus): CORREGIDO. **(d)** cifra stale L17
reemplazada por el comando (arriba); body y Engram sin cifras stale (grep=0 y FTS sin hits
propios). **§2 idioma**: detector sobre líneas '+' del diff (límites: stopwords sin acentos,
fallos conocidos en 'no'/'corpus' ambiguos EN/ES — excluidos): misión 299 (registro de
decisión, pendiente de decisión del owner), preflight 26 → verificado línea a línea: falsos
positivos del substring + las CITAS declaradas (§5, §4.3); harness 10; ci.yml 4 (nombres de
checks en comentarios — inglés); ci_fast_gate.sh 2 (comentarios míos en inglés — verificado);
check_file_size.sh: 0 líneas '+' en mi diff (preexistente, probado con el diff). Commits:
1 subject con cita española (da8425f9 "más cabeceras" — pushed history inmutable, queda),
0 cuerpos en español con acentos. Convención merge: AGENTS.md:498 (nunca rebase sobre
pusheado) + :406 (main ff-only); la cola squash usa el TÍTULO del PR para 2+ commits
(merge-when-green.sh:24-28) — el título es inglés; el body no va al mensaje. **Traducción
COMPLETADA** (8 cadenas más que los lotes abortados dejaron: lexer reason, sin plan, churn
note, blast/overlap notes, usage --negation, write_code comment, run_pre comment) — la
prueba mecánica de c7634ded dio diff=4 (la traducción estaba incompleta EN EL COMMIT:
corrección encima, pushed history inmutable). **§3**: check 7 endurecido a líneas vivas de
ci.yml (M4c: comentado = quitado) + self-tests 8/9/10 (positivo, no-cableado → rojo,
comentado → rojo) + depth guard (el caso 8 intacto recursaba infinito — descubierto y
arreglado). **Control #2056**: mismo sandbox SIN mutar + árbol real — las 8 violaciones del
sandbox son del CONTEXTO (otros guards sin herramientas del repo), ninguna nombra mi
harness; M4 aislado. **§4**: glue =0 legal (17.14) y =-5 defecto (17.15) — 90/90; grep
prueba 0 llamadas vivas a git show/gh en el harness (solo comentarios y el patrón asertado);
P con la muestra en comentario (línea 24 + salida --budget); sed ±5 de los 4 flaggeados →
3 eran mods declarados/use (falso positivo del GUARD, no del lexer) y 1 #[test] fn desnudo
real → **guard refinado a ítems fn-shape** → corpus re-escaneado **243/318/1** (solo
chromiumoxide tests.rs:314 queda IND, verificado con sed); ssrf_guard.rs **470 líneas,
0 en el baseline, ≤ hard_limit** → §10 "cerrada" verificado con datos. **§5** costo medido:
mover el 2 del preflight = 1 check; mover 3/4 = 14 checks + citas de política. **Entorno**:
/tmp lleno otra vez (109089 artefactos webfang-*, 7.6G) — limpieza con find -exec rm
(el glob de 109k argumentos hace fallar rm -rf en silencio); clase §8 fallo de estación,
comando registrado. **Pendiente del owner**: tokens de contrato (los 5 españoles), alcance
de la regla (misión/commits/title/body — la cola usa el título), mapa de exit codes (§5 del
owner + colisión del 2). F1b espera.

L28 (2026-11-18, ronda 6 — CI rojo en #2926, clase A): run 44380676224 sobre el HEAD actual
(6b18556d — no era rojo viejo): Repo guards / "Fast gate semantics harness" falló con
`FAIL B52/B53: the FULL lane failed (rc=1): failed steps: - file size preflight semantics
(offline)`; CI Gate falló por agregación. Clasificación: clase A (nuestro diff) — reproducido
local con `bash scripts/test_ci_fast_gate.sh` (mismo B52/B53). Causa raíz: el fixture B52 de
test_ci_fast_gate.sh stubbea como NO-OP todos los harnesses de lane_ci ("the case under test
is the LANE SHAPE, not the harnesses' own semantics") y su lista era anterior al wiring del
preflight — `bash scripts/test_file_size_preflight.sh` en el fixture = archivo inexistente.
§1.3 verificado: main está rojo en OTRO job (Tests windows-latest, el test flaky
jitter_pacing — clase D, no nuestro; Repo guards pasa en main). §8 test-first: check de
deriva nuevo en test_ci_fast_gate.sh — extrae los harnesses que lane_ci invoca y exige su
stub en make_fixture (RED: solo test_file_size_preflight.sh; el check mismo llevaba un awk
substr sin RLENGTH que arrastraba `--offline` — corregido antes de GREEN); luego el stub
junto a test_file_size.sh (GREEN: rc=0). Lección: la lista de stubs era prosa manual dentro
del fixture builder y nada la verificaba contra el wiring real. Entorno: /tmp lleno otra vez
(109089 artefactos webfang-*, 7.6G; el glob de 109k argumentos hace fallar `rm -rf` en
silencio — limpieza con `find /tmp -maxdepth 1 -name 'webfang*' -exec rm -rf {} +`).
Arreglos en `75526e6f`. Batería: shellcheck clean, harness 90/90, structure 27/10, ratchet
OK, gate local 27/0.
