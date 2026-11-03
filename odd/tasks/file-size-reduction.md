# Programa: reducir los archivos grandfathered del ratchet de tamaño

Plan persistido, **no ejecutado**. El ratchet (`scripts/check_file_size.sh`,
misión en `odd/tasks/file-size-ratchet.md`) congela a los ofensores históricos
en su conteo exacto; este programa es la dirección descendente: partirlos por
seams reales hasta que todas las partes queden <=500 y cada entrada salga del
baseline con `--remove`.

El listado vigente NO se copia acá (estado mutable): la fuente de verdad es
`file_size_rust.grandfathered` en `scripts/quality-baselines.json`.

## Principios

- Split por seams reales, nunca por aritmética de líneas. Un split que rompe
  cohesión es peor que el archivo gordo, y el reviewer lo ve.
- Comportamiento preservado: `refactor(scope)` puro, tests verdes por unidad
  de trabajo, snapshots intactos. El descenso del baseline viaja en el MISMO
  PR que el split que lo produce — la review ve el descenso medible junto al
  código que lo genera.
- Objetivo de cada split: todas las partes <=500 -> `--remove` de la entrada
  original. Una parte nueva >500: se sigue partiendo, o entra al baseline por
  edición manual revisada por maintainer (excepcional, visible en review).
- Excepciones documentadas: archivos declarativos/enumerativos cohesionados
  (candidatos: `error.rs`, `mcp_server/params.rs`, posiblemente
  `cli/args/mod.rs`) no se parten; quedan grandfathered con el porqué en las
  `notes` del JSON. La decisión se toma POR archivo en su misión, no acá.
- Net-zero en grandfathered: mientras un archivo esté en el baseline, crecerlo
  una línea falla el guard — todo cambio que lo toque termina neto en cero o
  lo parte. Por eso las misiones de Ola 1 se secuencian antes que cualquier
  feature que necesite tocar esos archivos.
- Confirmar que mejoró: líneas con el guard y `cx` con `jscpd --complexity`
  sobre los archivos tocados — el binario pinneado 5.4.0 (instalado con
  `cargo install jscpd --version 5.4.0 --locked`, ver
  `scripts/check_duplication.sh`), nunca `npx` (versión sin pin). Un split
  que solo mueve código no arregló nada, lo renombró.

## Olas (entrega secuencial; el repo no permite PRs apilados)

Los conteos por banda NO viven en esta prosa: se derivan del baseline (fuente
de verdad) con la misma query para todas las olas:

    jq '[.file_size_rust.grandfathered[] | select(. >= LO and . <= HI)] | length' \
      scripts/quality-baselines.json

Bandas: Ola 0 = [501, 570] · oportunista = [571, 999] · Ola 3 = [1000, 1499] ·
Ola 2 = [1500, 2000] · Ola 1 = (2000, ∞). Snapshot sobre
origin/main@fb411dac (2026-11-03): 15 / 51 / 30 / 8 / 9 = 113.

- **T1 — Ola 0, calibración:** banda [501, 570]. Extracciones chicas y de
  bajo riesgo; ejercita `--update`/`--remove` y el flujo completo de PR antes
  de tocar monstruos.
- **T2 — Ola 1, monstruos >2000:** uno por misión/PR.
  `application/crawler/engine.rs` primero — el peor y el hot path; puede
  requerir 2–3 PRs secuenciales solo él.
- **T3 — Ola 2, grandes [1500, 2000].**
- **T4 — Ola 3, cola [1000, 1499].** La banda oportunista [571, 999] queda
  cubierta por la regla de AGENTS.md "si lo tocás y ya las excede, partilo" —
  oportunista, no dedicada.

## Método por archivo (por misión, en su worktree)

1. **CodeGraph explore** (callers + blast radius) antes de tocar — nunca
   editar a ciegas.
2. **Nombrar el driver real**: ¿varios módulos pegados? ¿una función con
   varias responsabilidades? ¿un `match` gigante? El driver elige la
   estrategia: extract module / extract function / tabla.
3. **Split por seams**, un commit de unidad por extracción, tests verdes
   entre unidades.
4. **Verificación doble**: el guard (líneas) + `jscpd --complexity` (binario
   pinneado 5.4.0) sobre los tocados (`cx` debe bajar de verdad).
5. **Cerrar en el ratchet**: `--update` a las partes que quedaron >500 pero
   más chicas; `--remove` a las <=500; parte nueva >500 = seguir partiendo o
   entrada revisada.

## Gobernanza

- Cada split = issue propia + PR con `Closes part of #2584` (el paraguas del
  programa), label `type:refactor` (el vocabulario exacto vive en
  `scripts/auto_label_type.sh --map`), base `main`, entrega secuencial.
- Batching por ola: Ola 0 se batchea (misma base, archivos disjuntos —
  `scripts/ci_pr_overlap.sh` verifica el no-solape; ~4 archivos por batch);
  Ola 1 es una misión por monstruo, sin batch (engine.rs puede llevar 2–3
  PRs propios); Ola 2 y Ola 3 se batchean a 2–3 archivos por PR como máximo.
  La cuenta, sobre los conteos del snapshot (C0..C3 = 15/9/8/30):
  ⌈C0/4⌉ + (C1..C1+2) + ⌈C2/3⌉..⌈C2/2⌉ + ⌈C3/3⌉..⌈C3/2⌉ ≈ 4 + 9–11 + 3–4 +
  10–15 ≈ 26–34 PRs dedicados. La fórmula es la fuente; el número, su
  resultado sobre el snapshot — se re-deriva cuando el baseline baje.
- Cada ola es una misión ODD propia con su worktree y su registro de misión.
- No es un sprint; la catraca garantiza que cada avance es permanente.

## Qué no haremos

- Perseguir los 500 en archivos cohesionados por diseño.
- Mezclar splits con cambios de comportamiento en el mismo commit.
- Copiar el listado de ofensores a prosa (el baseline es la fuente).

## Log

- L1: "podemos crear un plan para solucionar los 113 existentes están
  congelados?" — pedido original. Decisión de usuario: persistir el plan sin
  arrancar ejecución.
- L2: Persistido en el worktree/PR del ratchet (`ci/file-size-ratchet`): el
  PR que introduce el enforcement es el vehículo natural del programa que
  hace descender el baseline.
- L3: corrección post-merge de #2569 (issue #2583): las olas decían
  14/9/~11/~26 y L2 del ratchet decía 8 — la partición real sobre
  origin/main@fb411dac es 15/9/8/30 (más 51 oportunistas) = 113. Los conteos
  salen ahora de la query jq; la prosa conserva solo las bandas. Se agregan:
  política de batching por ola, regla net-zero para grandfathered, y el
  binario `jscpd` pinneado (5.4.0) en lugar de `npx`.
