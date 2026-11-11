# Ola 1 (T2) — Plan de las tres misiones priorizadas por riesgo: `error.rs`, `ssrf_guard.rs`, `preflight.rs` (paraguas #2584)

Objetivo: cerrar tres de los ocho monstruos >2000 que quedan en Ola 1, ordenados por **riesgo de producción decreciente** y no por tamaño. Cada monstruo es una misión ODD propia con su worktree y su registro; este documento es el plan compartido de las tres y NO las sustituye — cada misión abre además su propio `odd/tasks/file-size-*.md`.

Orden: **M1 `error.rs` → M2 `ssrf_guard.rs` → M3 `preflight.rs`**.

Criterio de orden: la proporción test/producción decide el RIESGO; el tamaño sólo ordena dentro de una categoría. Medición (línea de `mod tests` ⇒ producción = línea−1, tests = total−línea+1):

| Archivo | Total | `mod tests` en | Producción | Tests | % tests |
|---|---|---|---|---|---|
| `error.rs` | 2187 | 857 | 856 | 1331 | 61 % |
| `ssrf_guard.rs` | 2288 | 1132 | 1131 | 1157 | 51 % |
| `preflight.rs` | 3274 | 1745 | 1744 | 1530 | 47 % |

Plan de record: `odd/tasks/file-size-reduction.md` — método, dos sombreros y gobernanza. Precedente inmediato: `odd/tasks/file-size-ola1-engine.md` (el primer monstruo, 14 PRs; de ahí salen las reglas S2, S4, S6, S8, S9, S10 y el formato de este documento).

Entrega: una misión por monstruo, SIN batch; entrega secuencial (el repo no permite PRs apilados).

**Regla de branches (precedente: la misión engine abrió 10 PRs así).** Una rama por PR, cada una desde el `main` vigente, con el worktree propio cuyo nombre es la rama con `/` → `-` (el proxy de identidad debe pasar sin excepción):

| PR | Branch | Worktree |
|---|---|---|
| M1 PR A (tests por tema) | `refactor/2584-t2-error` | `~/Projects/Rust/webfang-worktrees/refactor-2584-t2-error` |
| M1 PR B (corte 1) | `refactor/2584-t2-error-pr2` | `~/Projects/Rust/webfang-worktrees/refactor-2584-t2-error-pr2` |
| M1 PR C (corte 2) | `refactor/2584-t2-error-pr3` | `~/Projects/Rust/webfang-worktrees/refactor-2584-t2-error-pr3` |

El registro de esta ola (`file-size-t2.md`) viaja con la rama del PR A y llega a `main` con él; los PRs siguientes lo editan con su Log.

Ratchet: `scripts/check_file_size.sh`. Duplicación: `jscpd --complexity` (5.4.0 pinneado).

Runner: **declarado por misión y el mismo antes y después** de cada medición (precedente L9: un registro que dice "40/40 lib" sin nombrar el runner es un número heredado sin procedencia). La invariante `--list` no puede mezclarse entre runners porque cada uno imprime las rutas con su propio formato.

Estado: plan redactado. M1 (`error.rs`) sin empezar.

## Specs

**S1 — Orden por riesgo, no por tamaño.** La proporción test/producción es el eje de riesgo: extraer tests es movimiento mecánico, partir producción exige seam real. El tamaño ordena dentro de una categoría, no entre categorías.

**S2 — Corpus del ratchet.** `find crates -name '*.rs' -not -path '*/tests/*'` (`scripts/check_file_size.sh`). Un archivo nuevo >500 sin entrada grandfathered es ROJO merge-blocking. Un componente de path llamado exactamente `tests/` saca el archivo del corpus ⇒ **PROHIBIDO** para archivos de test nuevos: esconde la deuda en vez de reducirla. Consecuencia: los archivos de test nuevos se nombran `*_tests.rs` / `tests_<tema>.rs` como hermanos, nunca dentro de un directorio `tests/`.

**S3 — Cada archivo de test nuevo nace ≤500, partido POR TEMA.** No se acepta la "entrada grandfathered revisada" (L12 del plan del programa) para un archivo de test nuevo: convertir una entrada de 2187 en dos (856 + 1331) reparte la deuda y **sube** el conteo de grandfathered. El particionado debe dejar el archivo de producción por debajo de 500 Y ningún archivo nuevo sobre 500.

**S4 — Movimiento puro, demostrado.** Cada bloque movido se prueba verbatim: diff normalizado (strip indent + drop blanks) del bloque contra su span de origen, igualdad línea a línea. Cero cambios de lógica en los PRs de movimiento. `git diff --color-moved` es la lectura visual, no la prueba.

**S5 — Cero ensanches de visibilidad.** El módulo de test cuelga del módulo DUEÑO (hijo), nunca hermano: un hijo ve los privados del padre sin ensanchar nada. Si el módulo de test queda al mismo nivel que el código que prueba, los privados necesitan `pub(super)` ⇒ ensanchamiento ⇒ prohibido. El compilador NO detecta ensanchamientos: la lista de modificadores de visibilidad que cambian se reporta siempre, y vacía es el resultado esperado.

**S6 — La ruta de los tests cambia al partir por temas.** Invariante: mismo **conjunto de nombres de hoja** y mismo **conteo total**, con la REGLA de renombre declarada en el reporte (precedente L13: "la invariante se evalúa sobre el conjunto de nombres de hoja + conteo total, declarando los renombres de ruta"). "Byte-idéntico" sólo aplica cuando no se movió ningún test.

**S7 — Baseline sólo baja.** Nunca sube. Una entrada que sale se borra con `--remove` en el MISMO PR; una que baja se actualiza con `--update` descendente.

**S8 — Re-medición antes del primer movimiento.** Ningún span se copia de la prosa: se re-mide en el árbol del corte (precedente L14: el rango del plan era prosa stale y el corte real difiere). Este documento da candidatos, no medidas.

**S9 — Re-ubicación de helpers.** Un helper consumido por código que se queda NO se mueve (S9a). Los helpers de test viajan con su primer consumidor real; uno compartido por varios temas vive en el archivo del tema que lo consume, o en un `common` si lo consumen de verdad varios. Al reubicar, los `use super::*` en cadena se re-apuntan (precedente L17: "los nombres que `mod tests` alcanza por `use super::*` rompen el lib-test con E0412" — compilar el lib limpio ANTES de podar imports).

**S10 — Golden intra-crate e inventario, post-fmt y en el MISMO commit.** `tools/intra-crate-lint/tests/fixtures/real_tree_sites.txt` y `docs/test-inventory.json` (que rastrea por archivo: mover tests lo desincroniza).

**S11 — Los tests de paridad del guard SSRF no se separan.** `seed_guard_refusal_agrees_with_the_enforcing_guard_on_every_form` y `the_dial_layer_always_denied_set_covers_every_never_allowable_range` existen para detectar deriva entre la capa de dial y el guard de entrada: deben ver AMBOS lados. Viven en un archivo, y ese archivo cuelga del módulo que contiene los dos lados.

**S12 — Compuertas completas por PR.** La lista de § Compuertas es la MISMA para las tres misiones, sin recortes por tamaño de PR.

**S13 — Revisión.** Revisión por cuatro lentes con resultado explícito en el PR ("sin hallazgos" es respuesta válida, pero explícita) + ciclo nativo (assess → status → start → consent → collect → capture → acknowledge) por commit unitario. La revisión es una vista externa, nunca una autorización de entrega.

## Compuertas (por PR)

**A — compilador y estilo** (idéntico al pre-commit, que replica CI):
1. `cargo check --all-targets --all-features` — 0 warnings.
2. `cargo clippy --all-targets --all-features -- -D warnings -W clippy::cognitive_complexity -W clippy::too_many_lines` — limpio.
3. `cargo fmt --all -- --check`.
4. `env "RUSTDOCFLAGS=-D warnings" cargo doc --workspace --all-features --no-deps` — exit 0.

**B — pruebas:**
5. Suite completa con el runner declarado: mismos verdes que la base. Todo fallo pre-existente se prueba A/B contra la base con bytes restaurados.
6. Invariante `--list`: mismo conjunto de nombres de hoja y mismo conteo; renombres declarados por regla (S6).
7. `test_classification_matrix_unchanged` (`error.rs`) en verde antes y después.
8. Pruebas de paridad del guard SSRF intactas y en verde (S11).

**C — movimiento:**
9. Verbatim por bloque: diff normalizado vs span de origen, igualdad línea a línea (S4).
10. `git diff --color-moved` — lectura visual.
11. Diff de modificadores de visibilidad: lista explícita de los `pub` / `pub(crate)` / `pub(super)` que cambian; vacía = sin ensanchamientos (S5).

**D — ratchet e inventario:**
12. `scripts/check_file_size.sh` verde; baseline sólo baja; toda parte nueva ≤500 o entrada revisada.
13. `scripts/check_test_inventory.sh` verde.
14. Golden intra-crate refrescado post-fmt, en el mismo commit (S10).
15. `scripts/check_duplication.sh` — la métrica jscpd no sube; el programa exige que baje.
16. `scripts/check_dead_tests.sh` — ningún test queda muerto por la reubicación.
17. `scripts/check_dependency_direction.sh` + `scripts/check_intra_crate_direction.sh` verdes.
18. `bash scripts/ci_fast_gate.sh` GREEN y su reporte de cobertura leído: todo job "NOT covered" que se haya tocado se corre a mano.

**E — entrega:**
19. Revisión por cuatro lentes con resultado explícito en el PR.
20. Ciclo nativo RDD sobre el commit unitario.

> Nota de herramienta: `cargo public-api` NO está instalado en esta máquina. El sustituto efectivo es la compuerta A1 (una re-export faltante es un error de compilación, no una ruptura silenciosa de API) + la compuerta C11 (única que caza ensanchamientos).

## Tasks

| ID | Misión / Tarea | Specs | Ruta | Estado | Commit |
|----|-----|-------|------|--------|--------|
| M1-T0 | Worktree `refactor/2584-t2-error` + este plan + espejo Engram | S1,S12 | inline | done | (este commit) |
| M1-T1 | **PR A** — `error.rs`: extraer los tests por tema en archivos ≤500 colgando de `error` como hijos (`mod tests_<tema>;` — sin `#[path]`: al ser `src/error.rs` + `src/error/`, Rust resuelve el hijo solo). Cero cambios de producción. Reparto **medido** (S8): `tests_basics.rs` 861–1035 (178) · `tests_conversions.rs` 1036–1318 (286) · `tests_classification.rs` 1319–1690 (376) · `tests_network.rs` 1691–1814 + 2017–2186 (299, dos spans) · `tests_crawl_error.rs` 1815–2016 (205). `error.rs` 2187→871. Verbatim probado: 1226/1226 líneas normalizadas idénticas | S2,S3,S4,S5,S6,S8,S9 | inline | **done** | fa7b239d |
| M1-T2 | **PR B** — corte de producción 1: `impl ScraperError` (classify) + los 3 helpers de red (`is_transient_network`, `is_dns_or_tls_transport_failure`, `io_error_for_network_message`) → `error/classification.rs` con `pub use` de lo público. Medido 281–640 (360 líneas) ⇒ `error.rs` ~530 | S4,S5,S8 | por decidir | pending | — |
| M1-T3 | **PR C** — corte de producción 2: los 5 `impl From<…> for ScraperError` (655–854, 200 líneas) → `error/conversions.rs`. Los `From` pueden vivir en un módulo hijo SIN tocar el enum (coherencia: trait y tipo son locales al crate). `error.rs` → ~300 ⇒ `--remove` del baseline. **El enum NO se parte** | S4,S5,S7,S8 | por decidir | pending | — |
| M1-T4 | Verificación completa (§ Compuertas), revisión por lentes, ciclo nativo, PR con `Closes part of #2584` | S12,S13 | inline | pending | — |
| M2-T0 | Worktree + registro de misión propio + espejo Engram | S1 | inline | pending | — |
| M2-T1 | **PR A** — `ssrf_guard.rs`: tests por tema en archivos ≤500 (IP/política · literales/URL · registro/redirect), con los de paridad en un archivo y colgando del módulo que ve ambos lados (S11). 1132–2288 = 1157 líneas ⇒ ≥3 archivos | S2,S3,S4,S5,S6,S8,S11 | por decidir | pending | — |
| M2-T2 | **PR B** — corte de producción 1: bloque de clasificación IP → `ssrf_guard/classification.rs` con `pub use`. Candidato 188–816 (628 líneas). **Verbatim absoluto**: tres rutas de veredicto que deben coincidir (`is_forbidden_ip` / `is_always_denied_ip` / `is_forbidden_ip_with_policy`) y semántica no trivial (ISATAP, IPv4-compatible, orden Teredo antes que encapsulaciones). Nada se reordena ni se "limpia" en el movimiento | S4,S5,S8,S11 | por decidir | pending | — |
| M2-T3 | **PR C** — corte de producción 2, **OBLIGATORIO**: el corte IP solo deja el archivo en ~510, todavía sobre 500. Candidato: `ForbiddenLiteral` + sus impls (818–840) → `ssrf_guard/literal.rs`. Si no alcanza, tercer corte (política de redirect) | S4,S5,S7,S8 | por decidir | pending | — |
| M2-T4 | Verificación completa, lentes, nativo, PR | S12,S13 | inline | pending | — |
| M3-T0 | Worktree + registro de misión propio + espejo Engram | S1 | inline | pending | — |
| M3-T1 | **PR A..D** — `preflight.rs`, 4–6 PRs: costuras de producción una por PR. Spans candidatos (banner → banner): Normalization 21–816 · JS strategy dependency preflight 817–1584 · LLM extraction provider gate 1585–1641 · Pre-flight HTTP Connectivity 1642–1729 · Display Helpers 1730–1744. **La prosa de la revisión se solapa (817–1584 vs 1390–1641) y no es un corte ejecutable: re-medir.** | S4,S5,S8 | por decidir | pending | — |
| M3-T2 | **`mod.rs` con los re-exports EXACTOS** que hoy consumen otros módulos: `normalize`, `ArgSources`, `NormalizedConfig`, `merge_budget_overrides`, `check_*`, `resolve_chrome_binary`, `preflight_check`, `PreflightResult` — verificados contra los usos reales (`crates/webfang_cli/src/main.rs` usa `ArgSources`) | S5,S8 | por decidir | pending | — |
| M3-T3 | Tests por tema (≤500 cada uno). Estructura real medida: `mod tests` 1745–3274, con `mod normalization_pipeline_tests` anidado en 2703–3274 (572 líneas, `use super::*` en cadena — S9). La región 1745–2697 (953) parte en ≥2; la anidada (572) en ≥2 | S2,S3,S4,S6,S9 | por decidir | pending | — |
| M3-T4 | Verificación completa, lentes, nativo, PR | S12,S13 | inline | pending | — |

### Notas de ejecución que no son opcionales

- **La rama del PR A es la que lleva este plan.** Cada PR posterior abre worktree nuevo desde el `main` ya fusionado (el squash reescribe historia: la rama anterior no sirve de base). Ningún `git checkout`/`switch`/`stash` en toda la misión.
- **El bloque de clasificación IP de `ssrf_guard.rs` no es código trivial.** `is_forbidden_ip`, `is_always_denied_ip` e `is_forbidden_ip_with_policy` son **tres rutas que deben coincidir** en su veredicto, y el bloque contiene semántica de seguridad no obvia (ISATAP con posición fija del identificador ATOP, la forma IPv4-compatible obsoleta, y el orden de evaluación de Teredo **antes** que las encapsulaciones porque sus rangos se solapan). M2-T2 lo mueve **verbatim**: no se simplifica, reordena ni "se limpia" nada en el movimiento. Los tests de paridad (S11) son el guard de esa propiedad; el motivo de fondo está registrado en la memoria del proyecto (obs #2337, issues #2231/#2232).

- `stage_budget_overrides` es `fn` **privada** con 2 usos: la llamada de producción (`normalize`) y una de test. Sus tests deben quedar como **hijos** del módulo que la posee; si cuelgan de un módulo hermano, necesita `pub(super)` ⇒ ensanchamiento (S5).
- `FieldBook` es `pub(crate)`: si se mueve, la ruta crate-wide cambia y hay que re-exportarlo donde se use.
- `preflight.rs` tiene un módulo de tests **anidado** (`normalization_pipeline_tests` dentro de `mod tests`): al moverlo hay que re-apuntar su cadena de `use super::*` (S9, precedente L17).
- El `mod tests` de `preflight.rs` arranca en 1745 y el banner de la región de normalización está en 2698/2703: el corte por temas debe respectar esa anidación, no atravesarla.
- Los rangos de este documento son **candidatos** (S8). La medición va antes del primer movimiento, en el árbol del corte, y las diferencias se registran en el Log de la misión.

## Log

- **L1 (2026-11-11, verbatim)**: "Acepto la revisión: su corrección de la contabilidad es correcta y mi plan estaba mal planteado en ese punto. Lo comprobé en el repo."

  - "`scripts/check_file_size.sh` cuenta archivos, no líneas de producción. El corpus es `find crates -name '*.rs' -not -path '*/tests/*'`. Un `error_tests.rs` hermano **sí entra en el corpus**. Si nace con 1331 líneas y no está en el baseline, falla, porque el límite duro es 500."
  - "Las tres entradas existen hoy en `scripts/quality-baselines.json`: `preflight.rs` 3274, `ssrf_guard.rs` 2288 y `error.rs` 2187."
  - "Si el archivo grandfathered baja pero sigue sobre 500, pasa con aviso. Por eso extraer tests solo gana algo si los archivos de test nuevos nacen en ≤500."
  - "`docs/test-inventory.json` tiene clave `files` (181 entradas) y `tools/intra-crate-lint/tests/fixtures/real_tree_sites.txt` existe. Con `odd/tasks/file-size-*.md` hay además una convención de misiones por escrito. No comprobé si las tres rutas concretas están en el inventario, así que eso se mide al empezar."
  - "Mover los tests a un directorio `tests/` los sacaría del corpus. Técnicamente funciona, pero esconde la deuda en vez de reducirla. Yo lo descartaría y dividiría por tema en archivos ≤500, que además mantiene los privados visibles al ser hijos del módulo dueño."
  - "El reparto por tema sale natural. En `error.rs` serían los `From` (~1041–1320), `classify` y matriz (~1324–1690) y los helpers de red y aplanado de `CrawlError` (~1692–2187), cada uno por debajo de 500. En `ssrf_guard` los tests de IP y política, los de literales y URL, y los de registro y redirect."
  - "Mi 'queda en ~855' omitía que `error.rs` y `ssrf_guard.rs` siguen sobre 500 tras extraer los tests, y que necesitan un segundo corte de producción más adelante."
  - "'Un PR por archivo' no sirve para `preflight.rs`: es una misión de 4 a 6 PRs."
  - "El orden pasa a ser por riesgo: primero `error.rs`, luego `ssrf_guard.rs`, y `preflight.rs` al final como misión larga."
  - "Mi respuesta a su pregunta final: primero el plan por escrito en `odd/tasks/` (por ejemplo `file-size-t2.md`), con las compuertas fijadas antes de mover nada. Lo ordenaría así: 1. `cargo check --all-targets --all-features`, `cargo test -- --list | sort` antes y después, y `git diff --color-moved`. 2. Diff de modificadores de visibilidad, porque el compilador no detecta ensanchamientos. 3. `check_file_size.sh`, `check_test_inventory.sh` y el golden `real_tree_sites.txt` (refrescado post-fmt, en el mismo commit). 4. Tests de paridad del guard SSRF intactos y juntos."
  - "Acepto la revisión: su corrección de la contabilidad es correcta y mi plan estaba mal planteado en ese punto."

- **L2 (2026-11-11, verificación del plan antes de redactarlo)**: se verificó cada afirmación estructural contra el árbol de `main` en `afced909`.
  - `mod tests` en `error.rs` 857, `ssrf_guard.rs` 1132, `preflight.rs` 1745 ⇒ las tres proporciones test/producción de la tabla son exactas (±1 línea por el atributo `#[cfg(test)]`).
  - `stage_budget_overrides` (`preflight.rs`) es `fn` privada: 2 usos, la llamada de producción y una de test. No era doc-comment ni `pub(crate)` que se hubiera escapado.
  - El precedente `#[path]` existe y es el cited: `infrastructure/llm/client.rs` declara `#[cfg(test)]` + `#[path = "client_tests.rs"] mod tests;`, con nota de issue #2614.
  - Los tres tests de red de seguridad existen: `test_classification_matrix_unchanged` (`error.rs`), `the_dial_layer_always_denied_set_covers_every_never_allowable_range` y `seed_guard_refusal_agrees_with_the_enforcing_guard_on_every_form` (`ssrf_guard.rs`).
  - 70 archivos referencian `ssrf_guard` (cifra exacta de la revisión); 21 —no 31— sólo por la constante de entorno. La conclusión no depende del conteo: la extracción de tests con `#[path]` no cambia ninguna ruta de importación.
  - `cargo public-api` no está instalado ⇒ la compuerta C11 (visibilidad) más A1 (compilación) la sustituyen.
  - **Corrección 1 (mejora el resultado)**: `error.rs` **sí** cierra en esta ola. Layout de producción medido: enums + `Result` 17–279 · `impl ScraperError` (classify) + 3 helpers de red 281–640 · `impl SemanticError` 641–654 · 5 `impl From<…> for ScraperError` 655–854. Los `From` pueden vivir en un módulo hijo sin tocar el enum (coherencia: trait y tipo son locales al crate), de modo que con dos cortes (`error/classification.rs`, `error/conversions.rs`) `error.rs` queda en ~300 y **sale del baseline**. La afirmación de la revisión "no cierran con un segundo corte posterior" era correcta pero pesimista: el corte existe y es legal.
  - **Corrección 2**: `ssrf_guard.rs` **no** cierra con un solo corte: el bloque IP (188–816, 628 líneas) deja el archivo en ~510, todavía sobre 500 ⇒ segundo corte obligatorio (M2-T3). Motiva la postponición de M2, que la revisión ya proponía.
  - **Corrección 3**: los rangos por tema de `error.rs` en la revisión (1041–1320 / 1324–1690 / 1692–2187) **no tilean** la región de tests: 857–1040 queda sin dueño, y 1692–2187 queda en 496 líneas, al borde del límite duro. Re-medir antes de cortar (S8).
  - **Corrección 4**: los rangos de `preflight.rs` en la revisión se solapan ("Binarios 817–1584" vs "Checks 1390–1641", 195 líneas de solape). Los banners reales son: Normalization 21–816 · JS strategy dependency preflight 817–1584 · LLM extraction provider gate 1585–1641 · Pre-flight HTTP Connectivity 1642–1729 · Display Helpers 1730–1744. Además el archivo tiene un módulo de tests **anidado** (`mod normalization_pipeline_tests`, 2703–3274) cuyo corte no puede atravesar la anidación.
  - **Corrección 5 (compuertas)**: la lista de la revisión era correcta en su forma pero corta en contenido. Faltaban clippy `--all-features`, `fmt --check`, el doc gate, la **suite de tests real** (el `--list` prueba el inventario, no que los tests pasen), la prueba verbatim por diff normalizado (la prueba de machine de movimiento puro; `git diff --color-moved` es lectura visual), `check_dead_tests.sh`, `check_duplication.sh`, las dos direcciones, `ci_fast_gate.sh` con su reporte leído, y el ciclo nativo. Todo incorporado en § Compuertas.
  - **Corrección 6 (invariante `--list`)**: "en los dos de `#[path]` debe ser idéntico" no puede ser literally cierto si el módulo de tests se parte por temas: al partirlo, **toda** ruta de test cambia de segmento. La invariante es el conjunto de nombres de hoja + conteo total, con la regla de renombre declarada (precedente L13 del plan de la misión engine). "Byte-idéntico" queda reservado para los PRs que no mueven ningún test.
- **L3 (2026-11-11)**: este plan se escribe DENTRO del worktree de M1 (`refactor/2584-t2-error`) y no en el checkout de main, que lo dejaría huérfano y fuera del árbol (#1972). Viaja con el branch de M1 y llega a main con el primer PR de la misión. Las misiones M2 y M3 abrirán su propio registro y editarán este documento con su Log.
- **L4 (2026-11-11) — PR A ejecutado, y UNA PREMISA DEL PLAN CAMBIÓ**: al medir `error.rs` resultó que **ningún test llama a los helpers privados**. `is_transient_network` se llama sólo en 375 y 381, `is_dns_or_tls_transport_failure` sólo en 388 — las tres llamadas están DENTRO del `impl ScraperError` (281–468) que se mueve entero. En la región de tests (857–2187) los tres nombres aparecen sólo en comentarios (1556, 1564, 1737, 2120). Los tests los ejercitan a través de `err.classify()`, que es `pub`. **CONSECUENCIA**: la preocupación de "la visibilidad se rompe en PR B" (L5 de la revisión) no se materializa ⇒ **PR B y PR C no necesitan re-parentar ningún módulo de test, ni cambiar una sola visibilidad**. El módulo `tests` se parte una vez, en PR A, y sus rutas quedan fijas para siempre. Eso también reduce la invariante `--list` a un solo evento (PR A) en vez de uno por PR.
  - Segunda corrección: `io_error_for_network_message` (599–632) se llama desde 688, DENTRO de `impl From<CrawlError> for ScraperError` (668–774) ⇒ por S9a viaja con su consumidor ⇒ va a `error/conversions.rs`, NO al bloque de clasificación. Si se hubiera movido al bloque IP, el llamador quedaría en otro módulo y haría falta un `pub(super)` (ensanchamiento) — evitado.
  - Tercera corrección: el bloque `impl ScraperError` (281–468) contiene 11 constructores públicos además de `classify` (363). El corte de PR B se hace por la frontera del método, no del `impl`: los constructores quedan en `error.rs` y `classify` va a `error/classification.rs` — Rust permite varios `impl` inherentes del mismo tipo en módulos distintos, y la sintaxis de llamada (`err.classify()`) no cambia. `classify` es `pub` y `ErrorClass` es `pub` (`domain/error/error_class.rs`), así que no hay nada que re-exportar.
  - Verbatim PR A: los 5 bloques movidos son idénticos línea a línea a su span de origen tras normalizar indentación y quitar blancos — 161 / 257 / 341 / 276 / 191 = **1226 = 1226**, diferencia 0. El diff total de la región de tests es 0.
  - Nota de proceso: el primer intento de extracción perdió los saltos de línea (el dedent los consumía) y se detectó por el conteo de líneas; se restauró con `git restore` y se re-ejecutó. El script de extracción aborta si una línea no empieza por los 4 espacios del módulo, así que un literal multilínea no puede corromperse en silencio.
