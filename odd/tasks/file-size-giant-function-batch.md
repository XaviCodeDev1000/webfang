# Giant-function batch — five grandfathered files split by seam (paraguas #2584)

Objetivo: sacar cinco entradas del ratchet de tamaño partiéndolas por seams reales, sin
cambiar comportamiento. Rama: `refactor/file-size-test-batch` (worktree
`~/Projects/Rust/webfang-worktrees/refactor-file-size-test-batch`, desde `main@3492178a`).
Ratchet: `scripts/check_file_size.sh`; duplicación: `scripts/check_duplication.sh` (jscpd 5.4.0
pinneado). Runner declarado: `cargo nextest`.

## Specs

S1 (alcance): los cinco archivos que el usuario nombró como «giant-function»:
`crates/webfang_cli/src/main.rs`, `application/extraction.rs`, `application/scraper_service.rs`,
`cli/orchestrator.rs`, `infrastructure/downloader/wreq_downloader.rs`.
S2 (método): split por seams reales, un commit de unidad por archivo, tests verdes entre unidades,
movimiento mecánico primero (`odd/tasks/file-size-reduction.md`, § Método).
S3 (revisión): cuatro lentes con resultado explícito; el PR declara `Movido sin cambios` y
`Corregido / mejorado`.
S4 (cierre): `--remove` a lo que quede ≤500 y `--update` descendente a lo que quede >500, en el
MISMO commit que produce la bajada; golden del real tree refrescado en ese mismo commit.
S5 (guard chain): en `wreq_downloader.rs` la cadena de guards de fetch es intocable —
`fetch_inner` debe quedar byte-idéntico (AGENTS.md § Fetch guard chain).
S6 (sin PRs apilados): un solo PR, base `main`, `Closes part of #2584`, `type:refactor`; push y PR
solo con confirmación explícita del usuario.

## Tasks

| ID | Archivo | Seam | Resultado | Evidence |
|----|---------|------|-----------|----------|
| G1 | `application/scraper_service.rs` (1233) | hub + `types.rs` (DTOs) + `page_content.rs` (extracción por página + ensamblado) + `batch.rs` (fan-out) + tests hermano | entrada `--remove` | commit `ff8411ca`; lib 3116, `scraper_service_test` 56 verdes |
| G2 | `application/extraction.rs` (780) | hub + `selector.rs` (cluster CSS) + `readability.rs` (Readability/adaptive/pruner) + tests hermanos | entrada `--remove` | commit propio; lib 3116 verdes |
| G3 | `cli/orchestrator.rs` (1230) | hub + `persistence.rs` + `export_phase.rs` + `phases.rs` + 2 archivos de tests | entrada `--remove` | commit propio; orquestador 18 tests verdes |
| G4 | `webfang_cli/src/main.rs` (1172) | hub + `main/ai.rs` + `main/handlers.rs` + `main/setup.rs` + 2 archivos de tests | entrada `--remove` | commit propio; cli 6/6 (default) y 11/11 (all-features) |
| G5 | `infrastructure/downloader/wreq_downloader.rs` (764) | `cookies.rs` (cookies + header adapter + Retry-After) + `BoundedRead`/`WAF_SNIFF_MAX_BYTES` → `wreq_body.rs` | `--update` 764 → **632** | commit `413f6ed7`; 39 tests wreq + lib 3116 verdes; `fetch_inner` byte-idéntico |

## Log

- **L1 (verbatim del pedido)**: «vamos a continuar con el plan de reducir y revisar el codigo de los
  archivos grandes — giant-function (main.rs, extraction.rs, scraper_service.rs, orchestrator.rs,
  wreq_downloader.rs)».
- **L2 (corrección de método)**: el usuario recordó dos reglas que cambiaron la ejecución:
  «recuerda que debes trabajar en worktree y dejar a main limpio sin contaminar y cargar las skills
  adecuadas para la mision». Las ediciones que habían empezado en el árbol de `main` se rescataron
  al worktree de la misión y `main` se restauró limpio (`git status` vacío), antes de continuar. Se
  cargaron `webfang-file-splits` y `webfang-worktree-ops` como contrato de la misión.
- **L3 (herramientas)**: el usuario pidió «utilizar codedb y codegraph mas poderosas en lugar de las
  herramientas clasicas». El censo de callers por archivo se hizo con `codegraph query`; los gates
  siguen siendo el compilador y los scripts del repo.
- **L4 (delegación)**: alcanzado el backstop de contexto, la unidad pendiente de `orchestrator.rs`
  (`run_scrape_route` / `prepare_resume_route`) se delegó a un worker; escribió ambas funciones y
  agotó su tiempo antes de reportar, así que el cierre (cableado, imports, tests, verificación) se
  completó inline con evidencia propia.
- **L5 (verificación de movimiento)**: para cada archivo se comparó el cuerpo normalizado de cada
  función movida contra `HEAD`. Resultado: todos idénticos salvo `resolve_log_level_with_config`,
  cuyo único cambio es el reflow de firma de rustfmt (cuerpo idéntico), y `extract_cookies`, que
  pasó de método asociado a función libre con el call site actualizado. En `wreq_downloader.rs` el
  span de `fetch_inner` se comparó aparte: 10 577 bytes idénticos antes y después.
- **L6 (hallazgo durante el cierre)**: el primer intento de `wreq_downloader.rs` dejó el padre en
  653 líneas — dos más que su entrada del baseline, que un grandfathered no puede crecer ni una
  línea. Se corrigió moviendo `BoundedRead` y `WAF_SNIFF_MAX_BYTES` a `wreq_body.rs` (su único
  consumidor), lo que dejó el padre en 632 con `--update` descendente.

## Corregido / mejorado

Sin cambios de comportamiento. Correcciones de forma declaradas: el reflow de rustfmt en
`resolve_log_level_with_config`, `extract_cookies` como función libre, y el filtrado
`#[cfg(feature = "adaptive-selectors")]` explícito en los helpers de `main/ai.rs` y
`main/wiring_tests.rs` — antes solo compilaban porque los tests corrían con `--all-features`; ahora
el árbol compila limpio con y sin la feature (verificado con ambos `cargo check`).
