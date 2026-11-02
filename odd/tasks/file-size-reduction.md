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
- Confirmar que mejoró: líneas con el guard y `cx` con
  `npx jscpd --complexity` sobre los archivos tocados. Un split que solo
  mueve código no arregló nada, lo renombró.

## Olas (entrega secuencial; el repo no permite PRs apilados)

- **T1 — Ola 0, calibración:** banda 501–570 líneas, 14 archivos (de
  `infrastructure/llm/client.rs` 501 a `mcp_server/handlers/url_utils.rs`
  567). Extracciones chicas y de bajo riesgo; ejercita `--update`/`--remove`
  y el flujo completo de PR antes de tocar monstruos.
- **T2 — Ola 1, monstruos >2000:** 9 archivos (~23.6k líneas), uno por
  misión/PR. `application/crawler/engine.rs` (4063) primero — el peor y el
  hot path; puede requerir 2–3 PRs secuenciales solo él.
- **T3 — Ola 2, grandes 1500–2000:** ~11 archivos.
- **T4 — Ola 3, cola 1000–1500:** ~26 archivos. La banda 500–1000 (la
  mayoría del baseline) queda cubierta por la regla de AGENTS.md "si lo
  tocás y ya las excede, partilo" — oportunista, no dedicada.

## Método por archivo (por misión, en su worktree)

1. **CodeGraph explore** (callers + blast radius) antes de tocar — nunca
   editar a ciegas.
2. **Nombrar el driver real**: ¿varios módulos pegados? ¿una función con
   varias responsabilidades? ¿un `match` gigante? El driver elige la
   estrategia: extract module / extract function / tabla.
3. **Split por seams**, un commit de unidad por extracción, tests verdes
   entre unidades.
4. **Verificación doble**: el guard (líneas) + `npx jscpd --complexity` sobre
   los tocados (`cx` debe bajar de verdad).
5. **Cerrar en el ratchet**: `--update` a las partes que quedaron >500 pero
   más chicas; `--remove` a las <=500; parte nueva >500 = seguir partiendo o
   entrada revisada.

## Gobernanza

- Cada split = issue propia + PR con `Closes #N`, label `type:refactor` (el
  vocabulario exacto vive en `scripts/auto_label_type.sh --map`), base
  `main`, entrega secuencial.
- Cada ola es una misión ODD propia con su worktree y su registro de misión.
- Realismo: ~25–30 PRs distribuidos en el tiempo. No es un sprint; la
  catraca garantiza que cada avance es permanente.

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
