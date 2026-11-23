# Programa: reducir los archivos grandfathered del ratchet de tamaño

Plan persistido, **no ejecutado**. El ratchet (`scripts/check_file_size.sh`,
misión en `odd/tasks/file-size-ratchet.md`) congela a los ofensores históricos
en su conteo exacto; este programa es la dirección descendente: partirlos por
seams reales hasta que cada entrada salga del baseline con `--remove`.

El 500 es un guardrail local de AGENTS.md, **no una regla de Rust**: la doc
oficial (The Rust Book, cap. 7) pide agrupar funcionalidad relacionada y
limitar el detalle que hay que mantener en la cabeza, sin fijar líneas; el
único umbral numérico oficial es de **función**, no de archivo
(`clippy::too_many_lines`, default 100, y `clippy::cognitive_complexity`,
default 25, ambos ya activos en el gate de CI). Cerrar una misión exige que la
cohesión y la complejidad bajen de verdad; `<=500` es el disparador de revisión,
no la meta en sí.

El listado vigente NO se copia acá (estado mutable): la fuente de verdad es
`file_size_rust.grandfathered` en `scripts/quality-baselines.json`.

## Principios

- Split por seams reales, nunca por aritmética de líneas. Un split que rompe
  cohesión es peor que el archivo gordo, y el reviewer lo ve.
- Dos sombreros, nunca mezclados: cada split revisa lógica, eficiencia,
  seguridad y mantenibilidad, pero los cambios semánticos se separan o se
  acotan y se declaran; el PR dice qué fue movimiento y qué fue corrección.
- Comportamiento preservado: `refactor(scope)` puro, tests verdes por unidad
  de trabajo, snapshots intactos. El descenso del baseline viaja en el MISMO
  PR que el split que lo produce — la review ve el descenso medible junto al
  código que lo genera.
- Objetivo de cada split: todas las partes <=500 -> `--remove` de la entrada
  original. Una parte nueva >500: se sigue partiendo, o entra al baseline por
  edición manual revisada por maintainer (visible en review).
- Criterio de cierre (co-equal): además del guard de líneas, las funciones
  tocadas deben pasar `clippy::too_many_lines` (100) y
  `clippy::cognitive_complexity` (25) sin supresiones nuevas, y el `cx` de
  `jscpd --complexity` no debe subir. Un archivo de 450 líneas con funciones
  cortas está mejor que uno de 300 con una función de 180: si solo bajó el
  contador de líneas, el split no arregló nada.
- Excepciones documentadas: archivos declarativos/enumerativos cohesionados
  (candidatos: `error.rs`, `mcp_server/params.rs`, posiblemente
  `cli/args/mod.rs`) no se parten; quedan grandfathered con el porqué en las
  `notes` del JSON. La decisión se toma POR archivo en su misión, no acá.
  No son un caso excepcional que haya que rejustificar cada vez: basta que el
  archivo sea una sola unidad (enum, state machine, tabla o `match` exhaustivo)
  con lógica funcional mínima y que el gate de funciones esté limpio.
- Net-zero en grandfathered: mientras un archivo esté en el baseline, crecerlo
  una línea falla el guard — todo cambio que lo toque termina neto en cero o
  lo parte. Por eso las misiones de Ola 1 se secuencian antes que cualquier
  feature que necesite tocar esos archivos.
- Confirmar que mejoró: líneas con el guard, `cx` con `jscpd --complexity` y
  los lints de función del gate de CI sobre los archivos tocados — el binario
  pinneado 5.4.0 (instalado con
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

Dentro de cada banda el orden NO lo da el conteo de líneas: lo dan el churn
(`git log --follow --format=%ad --date=short -- <archivo>`), el historial de
bugs y la complejidad medida. Un archivo de 1400 líneas que cambia cada semana
va antes que uno de 1900 que nadie toca; "el peor y el hot path" fue el
criterio que puso `engine.rs` al frente de la Ola 1.

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
  - Progress (2026-11-22, batch O3-B1 / issue #2963, branch `refactor/ola3-b1`):
    the three `webfang_mcp` entries of the band are closed. `schema_bridge.rs`
    1015 → 259, `validation.rs` 1163 → 341 and `state.rs` 1131 → 487 all
    `--remove` (every part under the 500 limit: children
    `schema_bridge/{properties,overrides}.rs`, `validation/{reason,path}.rs`,
    `state/limits.rs`, plus the `*_tests.rs` siblings extracted by
    `scripts/split_tests.sh` with the multiset proof — state.rs's 566-line
    inline test mod was regrouped into two themed mods first so neither
    extracted sibling becomes a new over-500 offender). Behavior preserved:
    full `webfang_mcp` suite 648/648 green before and after the moves. The
    `needs-review` seam decisions for schema_bridge and validation — and the
    state.rs backpressure seam that the close-out tool demanded — are
    recorded on #2963 before each first cut.
  - Progress (2026-11-22, batch O3-B5 / issue #2967, branch `refactor/ola3-b5`):
    the two `application/crawler` entries of the band are closed.
    `discovery.rs` 1026 → 453 `--remove` (production untouched, including the
    band's single `allow(clippy::too_many_lines)` that stays F3 input; the
    580-line inline test mod regrouped at its own section seams into two themed
    mods, then extracted by `scripts/split_tests.sh` with the multiset proof —
    19 = 9 + 10, 9 = 0 + 9 — into `discovery_tests.rs` 279 and
    `discovery_scrape_tests.rs` 299). `sitemap_discovery.rs` 1400 → 407
    `--remove`: the `needs-review` decision (glued modules, four
    responsibilities) is recorded on #2967 before the first cut; the hub keeps
    the crawl orchestration plus the URL-building/filter helpers and three
    sibling seams move out — `sitemap_discovery_parser.rs` 122
    (SitemapParserPort boundary + error mapping), `sitemap_discovery_probe.rs`
    233 (robots.txt/fallback/sub-path URL probing) and
    `sitemap_discovery_subpath.rs` 135 (sub-path crawl fallback) — every moved
    item byte-verbatim except `pub(super)` on the four functions the hub
    calls (build_sitemap_parser, parse_sitemap, discover_sitemap_url,
    crawl_with_subpath_sitemaps); both public entry points stay in the hub so
    the MCP full paths and
    the `discovery.rs` → `lib.rs` re-export chain keep resolving unchanged. The
    573-line inline test mod regrouped into three themed mods (the
    `all(test, not(miri))` gate rewritten in its equivalent stacked
    `#[cfg(test)]` + `#[cfg(not(miri))]` form because `split_tests.sh`'s mod
    detection requires a `cfg(...test...)` attribute — semantics identical) and
    extracted with the multiset proof — 15 = 8 + 7, 8 = 4 + 4, 4 = 0 + 4 — into
    `sitemap_discovery_tests.rs` 217, `sitemap_discovery_entry_tests.rs` 148
    and `sitemap_discovery_ssrf_tests.rs` 211. Every part ≤ 500, both baseline
    entries removed in the same commit as the split that produced them, golden
    `real_tree_sites.txt` refreshed per split. Behavior preserved: the 35
    affected crawler discovery/sitemap tests green before and after every work
    unit.

## Método por archivo (por misión, en su worktree)

1. **CodeGraph explore** (callers + blast radius) antes de tocar — nunca
   editar a ciegas.
2. **Pregunta previa de frontera**: el Google Rust Style Guide manda decidir
   los límites de crate antes que los submódulos. Si el seam real pide un crate
   nuevo, eso es una decisión de arquitectura y va a su propia misión/PR —
   nunca se cuela en un split mecánico (ver protocolo de dos sombreros).
3. **Nombrar el driver real**: ¿varios módulos pegados? ¿una función con
   varias responsabilidades? ¿un `match` gigante? El driver elige la
   estrategia: extract module / extract function / tabla.
4. **Split por seams**, un commit de unidad por extracción, tests verdes
   entre unidades.
5. **Verificación triple**: el guard (líneas) + `jscpd --complexity` (binario
   pinneado 5.4.0, `cx` debe bajar de verdad) + los lints de función ya
   activos en CI (`clippy::too_many_lines` 100, `clippy::cognitive_complexity`
   25), sin supresiones nuevas.
6. **Cerrar en el ratchet**: `--update` a las partes que quedaron >500 pero
   más chicas; `--remove` a las <=500; parte nueva >500 = seguir partiendo o
   entrada revisada.
7. **Revisión compañera obligatoria**: revisar lo movido con las cuatro lentes
   (lógica, eficiencia, seguridad, mantenibilidad) y dejar el resultado en el
   PR; "sin hallazgos" es una respuesta válida, pero debe ser explícita.

## Protocolo de revisión por misión (dos sombreros)

El split sigue siendo mecánico por defecto. La revisión existe para detectar
deuda y riesgos reales, no para reescribir el sistema dentro del PR.

1. **Caracterizar antes de mover**: fijar comportamiento con tests/snapshots
   existentes o agregar caracterización mínima si faltan. Sin red, no hay split.
2. **Movimiento primero**: extracción mecánica con tests verdes entre commits.
   Si el cambio exige editar semántica para que compile, primero se resuelve
   la dependencia o se detiene la misión y se decide el seam correcto.
3. **Revisión por lentes** sobre el código movido:
   - **Lógica**: errores tragados, límites, estados imposibles, defaults
     silenciosos, manejo de `None`/`Err`, invariantes del seam.
   - **Eficiencia**: clones/asignaciones obvias, parseos repetidos, loops
     cuadráticos en hot paths, locks amplios; solo se actúa con medición o
     reducción clara de complejidad.
   - **Seguridad**: inputs externos, redirects/SSRF, paths, credenciales,
     permisos, temp files, prompt injection; todo fix exige test negativo.
   - **Mantenibilidad**: nombres, tamaño de función, duplicación, acoplamiento,
     cohesión del módulo extraído.
4. **Triaje de hallazgos**:
   - **Bloqueante** (correctness/security que impide corte seguro): detener el
     split o resolverlo en un cambio separado con test de regresión.
   - **Mejora chica y local**: puede acompañar como commit separado; el PR la
     lista bajo "Corregido / mejorado" con evidencia.
   - **Especulativa, amplia o transversal**: issue nuevo; no entra por la
     puerta de servicio en el split.
5. **Presupuesto de cambio semántico**: si lo no mecánico supera ~20–30% del
   diff del split, la entrega se parte o se extrae la corrección a otro PR.
6. **Reglas de evidencia**:
   - lógica = test rojo → verde;
   - seguridad = test negativo que reproduce el caso;
   - performance = medición antes/después o complejidad claramente inferior;
   - refactor = snapshots/tests intactos y diff explicable.
7. **Fuera del seam**: hallazgos en archivos no tocados se anotan como issue, no
   se "aprovechan" silenciosamente.

## Gobernanza

- Cada split = issue propia + PR con `Closes part of #2584` (el paraguas del
  programa), label `type:refactor` (el vocabulario exacto vive en
  `scripts/auto_label_type.sh --map`), base `main`, entrega secuencial.
- Cada split declara en el PR dos listas: `Movido sin cambios` y
  `Corregido / mejorado` (aunque la segunda sea "sin hallazgos"). Toda
  corrección semántica enlaza su test/issue; el reviewer ve separado qué fue
  refactor y qué cambió comportamiento.
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
- Mezclar splits con cambios de comportamiento en el mismo commit — y, salvo
  inseparabilidad acotada, tampoco esconderlos en el mismo PR sin lista y tests.
- Seguridad sin test negativo, lógica sin rojo→verde y performance sin medida o
  reducción clara de complejidad.
- "Ya que estoy" fuera del seam: hallazgos ajenos se registran como issue, no
  se cuelan en el split.
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
- L4 (2026-11-03): tras revisar el informe del agente, se decide que cada
  misión de split gane mantenibilidad sin volver el PR no revisable. Decisión
  de usuario: "vamos a actualizar lo que ha hecho el agente para ganar las
  dos cosas a la vez: el repositorio baja su deuda estructural y no convertís
  cada split en una fiesta de cambios no revisables." Se agrega el Protocolo
  de revisión por misión: dos sombreros (movimiento mecánico vs corrección),
  revisión obligatoria por lentes con resultado explícito, triaje de
  bloqueantes/mejoras locales/especulativas, presupuesto ~20–30% para cambio
  semántico, reglas de evidencia (rojo→verde, test negativo, medida o
  complejidad menor) y registro de hallazgos ajenos como issues. El PR de
  cada split declara `Movido sin cambios` y `Corregido / mejorado`.
- L5 (2026-11-16): el usuario pidió contrastar el plan con la documentación
  oficial y la guía de expertos: "que recomiendan los expertos y la doc oficial
  ? y comparalo con mi plan de OLAS." y autorizó el cambio: "adelante hazlo en
  un worktree,". Verificado: ninguna fuente oficial fija líneas por archivo
  (The Rust Book cap. 7, Google Rust Style Guide, Microsoft M-SMALLER-CRATES);
  el único umbral numérico es de función en Clippy. Se elevan los lints de
  función y `cx` a criterio de cierre co-equal, se explicita la excepción de
  unidades declarativas cohesionadas, se agrega la pregunta de frontera de
  crate antes del split y se ordenan las olas por churn/complejidad dentro de
  cada banda. Sin cambio de alcance: el 500 sigue siendo guardrail y la
  catraca queda intacta.
- L6 (2026-11-22): O3-B2 closed — the first Ola 3 / T4 batch (issue #2964,
  branch `refactor/ola3-b2`, base `04a34612`). Both band files left the
  ratchet with `--remove` in the same commit as the split that produced it:
  `crates/webfang_core/src/cli/error.rs` 1117 → 453 plus three themed test
  siblings extracted through `split_tests.sh` (multiset proof 47 = 13 + 11 +
  23: `error_tests.rs` 165, `error_matrix_tests.rs` 162,
  `error_overrides_tests.rs` 347); `crates/webfang_test_utils/src/lib.rs`
  1438 → 179 (the recorded −20 drift closed by the removal) split by
  responsibility into `env.rs` 436, `redaction.rs` 92, `bin_path.rs` 133 with
  the crate-root re-exports preserving every `webfang_test_utils::*` path,
  and its 29 inline tests extracted by theme (29 = 4 + 10 + 15:
  `bin_path_tests.rs` 110, `env_tests.rs` 335, `redaction_tests.rs` 194).
  Every resulting part measures <= 500; `file_size_done.sh` exits 0 for both
  batch paths; the intra-crate golden gained +28 sites with the error.rs
  split (+27 in `error_overrides_tests.rs`, +1 in `error_matrix_tests.rs`)
  and the full gate battery (check, strict clippy, fmt --check,
  rustdoc -D warnings, nextest, the four size/duplication/dead-test
  guards and the dependency-direction gate) is green. The 47 + 29 = 76
  affected test leaf names are unchanged modulo the five declared mod-path
  renames (three in test_utils, two in error). Needs-review decision for
  the harness file recorded on issue
  #2964 before the first cut (driver: glued modules; strategy: extract
  module per responsibility + re-export surface).
- L7 (2026-11-22, T4/O3-B3 progress, batch issue #2965): batch O3-B3 of the
  Ola 3 band split in worktree `refactor-ola3-b3` (base `152add03`).
  `cli/scrape_flow.rs` (1444, needs-review, churn 64) and `cli/args/crawler.rs`
  (1375, tests-majority, churn 46) both closed in the ratchet via `--remove`:
  every resulting part measures <= 500 lines. `scrape_flow.rs` driver recorded
  on the issue before the first cut (three glued responsibilities: resume/state
  wiring, the scrape flow with fetch-guard stage-2 pacing, fingerprint
  persistence); seam strategy = test mods to `*_tests.rs` siblings through
  `split_tests.sh` with partition proofs, then two production children
  (`scrape_resume.rs` with path-preserving re-exports, `scrape_fingerprint.rs`)
  keeping the flow and the pacing untouched. `crawler.rs` seam = test mods to
  siblings, then `CrawlerArgs` + clap impls to `crawler_args.rs` while the
  value parsers stay (full-path callers in `normalization.rs`/`args/mod.rs`).
  Full verification matrix green on the batch branch; no push yet (serialized
  delivery per the Ola 3 plan).
- L8 (2026-11-22, correction to L7): the O3-B3 batch was rebased before
  delivery — the recorded base `152add03` is historical; the delivered
  lineage sits on `main@cecfd1b8` (branch HEAD `86892efc`). The two test
  helper widenings in `crawler_spec_parity_tests.rs` were narrowed from
  `pub(crate)` to `pub(super)` after the risk review (sibling-only use).

- L9 (2026-11-22, O3-B9 progress entry — ENGLISH): batch O3-B9 (issue
  #2971, umbrella #2584) split `domain/options_spec/mod.rs` (1110 lines)
  and `application/vault_search.rs` (1076 lines) on
  `refactor/ola3-b9` (base `origin/main@04a34612`, rebased onto `main@52e4395b` before delivery). Needs-review decision
  for `options_spec/mod.rs` recorded on #2971 before the first cut:
  driver = glued modules (declarative model + error taxonomy + derivation
  engine + 432-line test mod), class = glued modules, strategy = sibling
  submodules mirroring the existing per-flag-group pattern, public API
  preserved by re-exports. Splits (one work-unit commit per seam, tests
  green between commits): `options_spec/error.rs` (83),
  `options_spec/types.rs` (279), tests to `options_spec_tests.rs` (429)
  via `split_tests.sh` (19 items, partition proof), `options_spec/validation.rs`
  (309); `mod.rs` = 45-line facade, `--remove` in the commit where it
  crossed <=500. `vault_search.rs`: 673-line test mod split into
  `vault_search_support_tests.rs` (235), `vault_search_search_tests.rs`
  (213), `vault_search_sync_tests.rs` (230) via `split_tests.sh`
  (21 items = 12+9, partition proof); source 414 lines, `--remove`.
  All parts <=500 (no `--update` needed). Characterization: 108 affected
  tests green before the first cut; 51 focused tests green after each
  seam. Visibility corrections (declared two-hats): `NumericPolicy::
  INTEGER_PARSE_DETAIL` -> `pub(super)`; shared vault test stubs -> `pub(super)`.
  Golden `real_tree_sites.txt`: +12 lines net (13 records in the three new
  vault test files, minus relocated base test-mod records; intra-crate
  direction check green after refresh). The branch was rebased onto
  `main@52e4395b` before delivery.
- L10 (2026-11-22, T4/O3-B4 progress, batch issue #2966): batch O3-B4 of the
  Ola 3 band split in worktree `refactor-ola3-b4` (base `52e4395b`).
  `cli/batch_flow.rs` (1250 baseline / 1212 real, needs-review, churn 18) and
  `domain/entities/content.rs` (1155, tests-majority, churn 12) both closed in
  the ratchet via `--remove`: every resulting part measures <= 500 lines
  (batch_flow 481; content 474; children batch_spool 199, batch_manager 276,
  scraped_content 65; test siblings batch_flow_tests 118, batch_spool_tests
  193, content_tests 464, scraped_content_tests 179). The recorded −38 drift
  on batch_flow was re-measured in the cut tree, confirmed, and closed with
  the split — no floor entry. Needs-review decision for batch_flow recorded
  on the issue before the first cut (driver: three glued responsibilities the
  earlier WU extractions carved around; strategy: mechanical responsibility
  split — spool acquisition child `batch_spool.rs`, manager assembly child
  `batch_manager.rs` including the crawl-config projection, lifecycle stays
  in the parent; external paths `crate::cli::batch_flow::{run_batch,
  load_batch_manager, BATCH_SPOOL_FILE}` preserved by re-exports). All mod
  tests moved through `split_tests.sh` with partition proofs
  (10=5+5, 5=0+5, 40=27+13, 27=0+27); content seam = two entities in one
  file, `ScrapedContent` moved to `scraped_content.rs` with a `pub use`
  facade in `content.rs` keeping the full path
  `entities::content::ScrapedContent` resolvable for a webfang_mcp consumer.
  Full verification matrix green on the batch branch; no push yet (serialized
  delivery per the Ola 3 plan).
- L11 (2026-11-22, correction to L10): the O3-B4 batch was rebased before
  delivery — the recorded base `52e4395b` is historical; the delivered
  lineage sits on `main@3ae03d5f` (branch HEAD after the golden regen).
  The batch_flow facade figure in L10 was corrected from 480 to 481.
- L12 (2026-11-22, T4/O3-B7 progress, batch issue #2969): batch O3-B7 of the
  Ola 3 band split in worktree `refactor-ola3-b7` (base `52e4395b`, rebased onto `main@2592333a` before delivery).
  `infrastructure/ssrf.rs` (1287, tests-majority, security carve-out S5)
  and `infrastructure/persistence/sqlite.rs` (1432, needs-review, churn 15)
  both closed in the ratchet via `--remove` in the same commit as the split
  that produced them; every resulting part measures <= 500. `ssrf.rs`
  keeps its 448-line production region byte-identical (guard stages 3b/3c
  untouched) while the inline test mod regroups at its own section seams
  into three themed siblings through `split_tests.sh` with the multiset
  proof (28 = 12 + 16, then 16 = 9 + 7, then 7 = 0 + 7):
  `ssrf_tests.rs` 315, `ssrf_resolver_env_tests.rs` 142,
  `ssrf_wiring_tests.rs` 329; the wiremock allow/deny parity pairs stay in
  one file and every negative test stays adjacent to its positive.
  `sqlite.rs` decision recorded on the issue before the first cut (glued
  modules, not a declarative unit); the production region splits along its
  section banners into `persistence/sqlite/` children (schema 159,
  connection 77, blob 57, vector_repository 297, note_repository 302,
  retention 48) with parent re-exports preserving every
  `persistence::sqlite::*` path including fingerprint.rs's
  `enforce_schema_version` call; the 490-line test mod extracts to
  `sqlite_tests.rs` (proof 18 = 0 + 18). Work-unit commits `17087eb2`,
  `2aa41b5d`, `559aa3c5`; full verification matrix green; no push yet
  (serialized delivery per the Ola 3 plan).
- L13 (2026-11-22, T4/O3-B6 progress, batch issue #2968): batch O3-B6
  (umbrella #2584) split `application/crawler/session.rs` (1006 lines) and
  `application/crawler/bounded_sink.rs` (1103 lines) on `refactor/ola3-b6`
  (base `origin/main@3ae03d5f`). Needs-review decisions recorded on #2968
  before each first cut. `session.rs`: test mod re-parented to
  `session_tests.rs` (376) via `split_tests.sh` (14 items = 0 + 14, partition
  proof), error taxonomy to `session_error.rs` (70), construction path
  (`CrawlSessionBuilder` + `checkpoint_interval_of`) to `session_builder.rs`
  (154); source 445 lines, `--remove` in the commit where it crossed <=500.
  `bounded_sink.rs`: test mod re-parented to `bounded_sink_tests.rs` (447;
  16 items = 0 + 16), byte-bounded `Backlog` (+`Admit`) to
  `bounded_sink_backlog.rs` (124), `spool_writer` (+`write_page`) to
  `bounded_sink_spool_writer.rs` (123); source 445 lines, `--remove`.
  `container.rs` measured 1071/0 in the cut tree: no inline test region is
  left to move (already path-declared `container/tests.rs`, 457 lines, the
  O2-T7 precedent), production cannot move under the ADR-0011 permanent
  substring exemption — no split this batch, file untouched, declared a
  candidate for the maintainer's exception list (recorded on #2968).
  Declared two-hat adjustments: `Engine::run` intra-doc link re-pointed to
  the full crate path from the builder child; `CrawlSessionBuilder::
  extra_seeds`, `Admit`, `Backlog` (+ called methods) and `spool_writer`
  widened to `pub(super)` (sibling-only use); one test helper reflowed at
  the mod-level dedent; `session_tests.rs` imports `ScraperError` directly
  (it reached the type through `use super::*` before the taxonomy moved).
  Test module paths unchanged, so no test renames: 50 affected tests
  (14 session + 16 sink + 20 container) green before the first cut and
  green after every seam. `file_size_done.sh` exit 0 per split file
  (verdict `closed`, base `3ae03d5f`). Golden `real_tree_sites.txt`
  refreshed in every commit that added production files.
- L12 (2026-11-22, O3-B10b progress entry — ENGLISH): batch O3-B10b
  (issue #2973, umbrella #2584) split
  `infrastructure/network/session_pool.rs` (1156 lines) and
  `infrastructure/observability/file_trace_layer.rs` (1113 lines) on
  `refactor/ola3-b10b` (base `origin/main@3ae03d5f`, rebased onto `main@2592333a` before delivery). Both are
  tests-majority (mechanical first cut; no S5 carve-outs apply).
  `session_pool.rs`: 605-line test mod extracted verbatim via
  `split_tests.sh` (27 items, partition proof) to
  `session_pool_tests.rs`; the extracted file (600) then split at the
  config seam into `session_pool_tests.rs` (330) +
  `session_pool_config_tests.rs` (273) (27 = 15 + 12, name multiset
  identical). Production seams: bounded-eviction mechanics
  (`evict_expired`, `enforce_domain_cap`) to `session_pool_eviction.rs`
  (62) and the balanced-pick mechanism (`PICK_ANCHOR`, `pick_start`) to
  `session_pool_pick.rs` (47); source 479 lines, `--remove`.
  `file_trace_layer.rs`: 677-line test mod extracted via
  `split_tests.sh` (24 items, partition proof), then split at the span
  seam into `file_trace_layer_tests.rs` (293; event/file-record
  contracts plus the five shared JSONL reader helpers) +
  `file_trace_layer_span_tests.rs` (385; span lifecycle contracts incl.
  span_close records) (24 = 12 + 12, name multiset identical); no
  production byte changed, source 441 lines, `--remove`. All parts
  <=500. Characterization: 71 session_pool/file_trace_layer-named unit
  tests plus 24 tests across `session_pool_integration`,
  `adversarial_stress_matrix` and `span_attribution_across_await`
  green before the first cut; the same focused sets green after every
  seam. Visibility corrections (declared two-hats): `evict_expired`,
  `enforce_domain_cap`, `pick_start` and the shared test helpers
  (`slots`, the five JSONL readers) -> `pub(super)`; the extracted
  session_pool test file imports `MAX_TRACKED_DOMAINS` directly
  instead of receiving it through the parent's use-glob. Golden
  `real_tree_sites.txt`: +1 row (the `MAX_TRACKED_DOMAINS` use in
  `session_pool_eviction.rs`) + 3 line-number shifts in
  `session_pool.rs`; extracted test files emit no rows (#2980).
- L13 (2026-11-22, correction to L12): the delivered hub figures are
  `session_pool.rs` 483 (not 479) and `file_trace_layer.rs` 443 (not 441) —
  the recorded values predate the appended `#[path]` test-wiring blocks.
  The golden delta is net 0 rows: −1 (`session_pool.rs` MAX_TRACKED_DOMAINS
  use relocated) +1 (`session_pool_eviction.rs`), plus 3 line-number shifts
  in `session_pool.rs`. Follow-up noted, outside this slice: the TTL-heal
  loop in `evict_expired` duplicates the inline loop in
  `SessionManager::evict_stale` (pre-existing).
- L14 (2026-11-22, correction to L12 in B10b): the B10b branch was synced
  with `main@5b8e4130` (merge commits, not rebase) before delivery: first
  O3-B5 + O3-B7 (`02aa7a8f`), then the mutants-gate fix
  `fix(ci): count MISSED across every mutation shard` (`5b8e4130`). The
  bookkeeping conflicts were resolved by removing both splits' closed
  ratchet entries (`session_pool.rs`, `file_trace_layer.rs`, `sqlite.rs`);
  the golden set was regenerated with `scripts/refresh_real_tree_golden.sh`
  on the merged tree.
- L15 (2026-11-22, correction to L12/L14 in B10b): the mutants-gate shards
  found two TTL survivors in the eviction seam; the pin tests landed
  (`a958e317`) and the branch was synced again with `main@76c72fcf`
  (O3-B6 `e7965e08`, O3-B8 `76c72fcf`). The golden set was regenerated on
  the new merged tree.
- L16 (2026-11-22, T4/O3-B10a progress): `infrastructure/export/record_store.rs`
  (1393, tests-majority, churn 15) split in worktree `refactor-ola3-b10a`
  (base `76c72fcf` + the 2-line `pub(super)` helper widening `3da9147c`). The
  751-line inline test mod was regrouped at the E5 section boundary and
  extracted through `split_tests.sh` with the multiset proof (27 = 19 + 8,
  then 8 = 0 + 8) into `record_store_tests.rs` (468) and
  `record_store_recovery_tests.rs` (316). The production region shed the
  on-disk envelope / injectable FS backend / lock sentinel / v1→v2 migration
  into `record_store_file.rs` (215) along the "file representation" seam,
  leaving the public persistence API in `record_store.rs` (453). Every part
  <= 500; `--remove` landed in the same commit as the split. Declared
  two-hat corrections: `StoreFile` (+ five fields) and
  `record_store_file::migrate_v1` widened to `pub(super)` for parent/sibling
  use, `RawRecord` / `PageStatus` / `std::io::Write` imported directly in the
  two extracted test files (previously inherited via `use super::*`), one
  unused `ErrorClass` import dropped, an explicit `#[path]` added on the new
  production child (Rust 2018 submodule lookup), and the module-level
  `[`RawRecord`]` intra-doc link re-pointed to the full crate path after the
  import moved. Golden
  `real_tree_sites.txt` refreshed in the same commit; 27/27 focused tests
  green.

- L17 (2026-11-22, T4/O3-B10a progress): `infrastructure/export/vector_exporter.rs`
  (1252, tests-majority, churn 13) split in the same worktree. The 658-line
  inline test mod was regrouped at the Task 4.4 boundary and extracted through
  `split_tests.sh` with the multiset proof (25 = 11 + 14, then 11 = 0 + 11)
  into `vector_exporter_tests.rs` (279) and
  `vector_exporter_append_tests.rs` (403). The production region shed the six
  byte-window/header helpers (`read_header_bytes`, `find_subslice`,
  `parse_count_field`, `count_documents`, `extract_created_at`,
  `patch_header_field`) plus `HEADER_SCAN_BYTES` into
  `vector_exporter_header.rs` (121) along the "header representation" seam,
  leaving the exporter and its write path in `vector_exporter.rs` (497). Every
  part <= 500; `--remove` landed in the same commit as the split. Declared
  two-hat corrections: the six helpers and the const widened to `pub(super)`
  for parent use, shared fixtures mirrored per themed test mod, one unused
  fixture dropped, dedent reflows applied, and an explicit `#[path]` added on
  the new child. The existing `#![allow(clippy::incompatible_msrv)]` stays in
  the parent (no new allow). Golden `real_tree_sites.txt` refreshed in the
  same commit (+2 sites); 25/25 focused tests green.
