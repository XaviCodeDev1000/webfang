# PRD — Migración de webfang a Rust 1.100 + edition 2024

**Documento de Requisitos de Producto (migración técnica)**

| Campo | Valor |
|---|---|
| ID | `PRD-MIG-2026-01` |
| Versión del documento | 1.0 |
| Fecha | 2026-11-14 |
| Estado | **Proposed** — pendiente de aprobación del maintainer |
| Producto | `webfang` — `XaviCodeDev1000/webfang` |
| Baseline auditado | `main` @ `08a323c29769` (2026-11-14 02:15 UTC) · workspace `v7.7.1` |
| Fuente de entrada | Informe *"De Herramienta Personal a Plataforma Industrial"* (2026-11) |
| Alcance de entrega | Fases F0–F3 (migración de toolchain, edición, cadena de suministro y superficie de APIs) |
| Fuera de alcance de entrega | F4 (plataforma de plugins y agente autónomo) — capturada como épica con decisión bloqueante |
| Anexos | `anexos/baseline-webfang.md` · `anexos/audit_webfang.py` · `anexos/config/*` · `anexos/backlog-migracion.csv` |

> **Cómo leer este documento.** §2 es la línea base medida; §3 corrige el informe de origen
> (varias de sus premisas no sobreviven al contacto con el repo); §6 son los requisitos
> ejecutables con criterios de aceptación; §9 el plan con gates de salida; §13 cómo aterriza
> en el pipeline SDD/RDD que ya usa el proyecto. Todo número citado sale de
> `anexos/audit_webfang.py` sobre el SHA de baseline: si el repo se mueve, se re-corre el
> script, no se edita la cifra a mano.

---

## 1. Resumen ejecutivo

1. **El salto de toolchain es viable y barato; la edición es el desbloqueo real.** El workspace
   está en `1.91.1` / `edition 2021`. Rust `1.100.0` se publicó el **2026-11-12** (hace 2 días).
   El salto acumula nueve releases, pero el repo ya está extraordinariamente bien preparado:
   el riesgo número uno de `edition 2024` (`env::set_var` pasa a ser `unsafe`) **ya está pagado**:
   las 25 mutaciones de entorno viven en un único dueño sancionado (`webfang_test_utils::env_set`,
   `env_remove`, `EnvGuard`) y **ya están envueltas en `unsafe {}` con `#[allow(unsafe_code)]` y
   comentario SAFETY**.
2. **El informe subestima y sobrestima a la vez.** Subestima la superficie de `assert_matches!`
   (474 candidatos reales, no 272: el grep de una línea se pierde 202 casos multilínea) y
   sobrestima el impacto en rendimiento de `from_utf8_lossy_owned` (26 casos, **24 de ellos en
   tests**, 2 en `src/`). También atribuye a `edition 2024` dos cosas que no son de la edición.
3. **La parte de IA del informe está desactualizada respecto al repo.** `webfang` ya tiene
   `LlmPort` + `--extract-with-llm`, `EmbeddingPort`/`RemoteEmbeddingAdapter`, AXTree, Set-of-Mark
   y un servidor MCP sobre `rmcp 3.9`. "Integrar un primer LLM" no es una fase futura: ya shipped.
   Y "plugin de `llama.cpp` local" **contradice ADR-0004** (ONNX local retirado, remoto como única vía).
4. **La arquitectura de plugins FFI del informe choca con una política dura del repo:**
   `unsafe_code = "deny"` a nivel de workspace, más lanes de Miri y sanitizers. `abi_stable` +
   `*mut c_void` + `catch_unwind` no es una decisión de implementación: es una excepción de
   política que requiere ADR y aprobación explícita. Este PRD propone una alternativa que
   conserva la extensibilidad sin romper el invariante (§8.3).
5. **Lo que el informe no vio y sí importa:** el bump invalida **todos los seeds de `target/`**
   (la clave incluye la versión de rustc), el nuevo layout de build-dir de Cargo 1.100 toca
   justo el árbol que el mecanismo de seeds poda y mide, hay **110 snapshots insta + 5 fixtures
   trybuild `.stderr`** que la edición y los cambios de diagnóstico de 1.100 van a mover, y
   el MSRV bump interacciona con las **líneas de soporte** (7.7 STABLE / 7.6 MAINTENANCE) que
   siguen aceptando parches de seguridad compilables con 1.91.

**Decisión que se pide al maintainer:** aprobar F0–F3 como un tren de migración de ~9 semanas
con reversiones por fase, y abrir ADR-0021 (toolchain/MSRV), ADR-0022 (edition 2024) y ADR-0023
(seam de extensibilidad) antes de escribir código de F4.

---

## 2. Línea base verificada

Medida con `python3 anexos/audit_webfang.py repo-webfang` sobre `08a323c29769`.

### 2.1 Superficie y configuración

| Hecho | Valor | Fuente de verdad |
|---|---|---|
| Versión del workspace | `7.7.1` | `Cargo.toml` → `[workspace.package] version` |
| Edición | `2021` (heredada por los 5 miembros) | `Cargo.toml` → `edition`; `edition.workspace = true` |
| MSRV declarado | `1.91` | `Cargo.toml` → `rust-version` |
| Toolchain pinned | `1.91.1` | `rust-toolchain.toml` → `channel`; `mise.toml` → `rust` |
| Lint de seguridad | `unsafe_code = "deny"` | `Cargo.toml` → `[workspace.lints.rust]` |
| Lints de pánico | `unwrap_used`, `expect_used` = deny (carve-out `cfg(test)` por raíz) | `Cargo.toml` → `[workspace.lints.clippy]` |
| Miembros | `webfang_core`, `webfang_mcp`, `webfang_cli`, `webfang_test_utils`, `webfang_benchmark` | `[workspace] members` |
| Excluidos del workspace | `fuzz/`, `tools/intra-crate-lint/` (edition `2021` propia) | `[workspace] exclude` |
| Tamaño | 577 ficheros `.rs`, ~222.465 LOC | auditoría |
| Dependencias lockeadas | 623 paquetes, `Cargo.lock` v4 | `Cargo.lock` |
| Workflows | 17 | `.github/workflows/` |
| Snapshots / fixtures de diagnóstico | 110 `.snap` (insta) · 5 `.stderr` (trybuild) | auditoría |

LOC por crate: `webfang_core` 179.522 · `webfang_mcp` 36.281 · `webfang_benchmark` 3.853 ·
`webfang_test_utils` 1.568 · `webfang_cli` 1.241.

### 2.2 Pruebas

| Métrica | Valor |
|---|---|
| Atributos de test unitario en el árbol (`#[test]` / `#[tokio::test]`) | **4.743** |
| Inventario de tests de integración (`docs/test-inventory.json`) | **1.230** tests en 182 ficheros, 6 ignorados |
| Por crate (inventario) | core 890 · mcp 263 · benchmark 76 · test_utils 1 |
| Lanes de verificación relevantes | `fmt`, `clippy`, `test-core`, `test-full`, `test-crossplatform`, `cfg-windows-lint`, `feature-matrix`, `mcp`, `miri`, `miri-infra-fast`, `miri-infra-slow`, `coverage`, `flake-detection`, `security`, `code-quality`, `repo-guards` |
| Matriz de compatibilidad | 5 combos requeridos + 1 pairwise (`COMPATIBILITY-MATRIX.md`, `scripts/check_compatibility.sh`) |
| Mutación | `cargo-mutants 27.1.0`, 96 shards semanales, ~1.640 mutantes en el scope gateado |

> El `README` del repo habla de "1.337 tests"; el inventario y el conteo de atributos dicen otra
> cosa. **No usar esa cifra en material de producto** sin regenerar el inventario
> (`scripts/ci_test_inventory.py`).

### 2.3 Superficie de refactor (multilínea-aware)

| Patrón | Total | `src/` | `tests/` |
|---|---:|---:|---:|
| `assert!(matches!(..))` → `assert_matches!` | **474** | 367 | 107 |
| … de ellos en una sola línea (lo que ve un `grep`) | 272 | 203 | 69 |
| `from_utf8_lossy(..).into_owned()` | **26** | **2** | 24 |
| `from_utf8_lossy(..).to_string()` | 12 | 2 | 10 |
| `if let` (total workspace) | 627 | 565 | 62 |
| `if let` en `webfang_core/src` | **482** | — | — |
| Candidatos reales a let-chain (`if let` cuyo primer statement es otro `if`) | **112** | 106 | 6 |
| Brazos vacíos `_ => {}` / `() => {}` | 18 | 11 | 7 |
| `strip_prefix(..).unwrap_or(..)` → `trim_prefix` | 17 | 11 | 6 |
| `env::set_var` / `remove_var` | **25** | 25 | 0 |
| `#[allow(unsafe_code)]` | 9 | 9 | 0 |
| bloques `unsafe {}` | 15 | 15 | 0 |
| `static mut` | **0** | — | — |
| `-> impl Trait` (RPIT, captura de lifetimes en 2024) | 10 | 8 | 2 |
| identificador `gen` (reservado en 2024) | **0** | — | — |
| `cfg_if!` / `cfg_if::` | **0** | — | — |

Concentración de `assert_matches!` (top): `webfang_mcp/tests/mcp_params_validation_test.rs` (34),
`webfang_core/src/error/tests_conversions.rs` (22), `src/cli/error.rs` (17),
`src/application/http_client/client.rs` (16), `src/infrastructure/downloader/spa_detector.rs` (16).

Los 9 `#[allow(unsafe_code)]` están en exactly tres ficheros:
`webfang_core/src/cli/crash_points.rs` (2), `webfang_core/src/infrastructure/platform/windows_console.rs` (3),
`webfang_test_utils/src/lib.rs` (4).

### 2.4 Puntos de anclaje de la migración en CI

| Superficie | Estado hoy | Qué la rompe / la protege |
|---|---|---|
| `dtolnay/rust-toolchain` | 35 pasos, 3 SHAs: `6bed0761…` (29 pasos, línea *master*, input `toolchain` con default `stable`), `02cb101e…` (3, ci.yml), `be39649a…` (3, fuzz/sanitizers, default `nightly`) | `scripts/check_toolchain_drift.sh` (R1: paso con `targets:` debe declarar `toolchain:`; R2: todo `toolchain:` explícito == `channel`, salvo `nightly*`) + su autotest `scripts/test_toolchain_drift.sh` |
| `toolchain:` explícitos | **3**: `ci.yml` (job mcp, `target: x86_64-unknown-linux-gnu`), `release.yml` ×2 (matrix de targets y provenance) | son los únicos que hay que editar a mano |
| Pasos que instalan targets | **2** (`ci.yml`, `release.yml`) | el fallo histórico `E0463` que vació `v7.0.0` |
| Pins de versión en narrativa | `README.md` (badge + prerequisitos), `AGENTS.md` (Stack), `docs/src/installation.md`, `docs/src/overview.md`, `release.yml` (nota de release) | `scripts/check_docs_version_pins.sh` (fail-closed; el bump de pins de *versión de producto* lo owns el flujo de release) |
| Seeds de `target/` | `scripts/seed_recipe.sh` / `seed_target.sh` / `seed_publish.sh`; clave incluye versión de rustc; **rechazan** cualquier `build-dir` configurado; podan por embedding de ruta en fingerprints | bump de rustc ⇒ `seed: cold` para todos los worktrees; el nuevo layout de Cargo 1.100 toca el árbol medido |
| Warnings | `cargo clippy … -D warnings` + `RUSTDOCFLAGS=-D warnings` | `CARGO_BUILD_WARNINGS=deny` (Cargo ≥1.97) es la alternativa que no re-keyea la caché |
| Dependencias muertas | `cargo-machete 0.9.2` (job `code-quality`) | Cargo 1.100 trae `[lints.cargo]` + `cargo::unused_dependencies` |
| SLOs de CI | Tier 0 ≤2 min · Tier 1 ≤10 min · Tier 2 ≤30 min (p90) | `docs/ci-slo.md`, `scripts/ci_metrics.sh` |
| Líneas de soporte | 7.7 STABLE (bugfix+security) · 7.6 MAINTENANCE (security) · ≤7.5 EOL | `.github/support.json` → `SUPPORT.md` |

---

## 3. Correcciones al informe de origen

El informe es una buena tesis estratégica y una mala especificación. Estas son las correcciones
que cambian el plan (verificado = comprobado contra fuente primaria o contra el repo).

| # | Afirmación del informe | Realidad verificada | Efecto en el PRD |
|---|---|---|---|
| C1 | "Rust 1.100" como destino | ✅ `1.100.0` publicado el 2026-11-12 | El destino es real, pero tiene **2 días**: F1 fija `1.100.0` y deja la línea 7.7 en `1.91.1` para backports |
| C2 | `min-publish-age` se activa con "una línea" | ✅ existe (RFC 3923), pero la clave es `[registry] global-min-publish-age = "3 days"` (o `registries.<name>.min-publish-age`). **No afecta a versiones ya en `Cargo.lock`** | RF-08: la política protege *resoluciones nuevas*, no audita el lock actual; `cargo-audit`/`cargo-deny` siguen siendo necesarios |
| C3 | "`edition 2024` neutraliza el riesgo del never-type fallback" | ❌ En **1.100 el fallback cambia a `!` en TODAS las ediciones**; `Infallible` pasa a ser alias de `!` | El riesgo es de **F1 (toolchain)**, no de F2 (edición). RF-05 lo trata como breakage de compilación/inferencia |
| C4 | "`edition 2024` estabiliza los `if-let guards`" | ❌ `if_let_guard` sigue inestable; lo que desbloquea 2024 son **let chains** (1.88) | RF-16 se limita a let chains; nada de guards en `match` |
| C5 | 272 `assert!(matches!` | ⚠️ 272 es solo la forma de una línea. Total real: **474** | RF-14 dimensiona 474 sustituciones en ~8 lotes, no 272 en 3 |
| C6 | 34 `from_utf8_lossy(...).into_owned()` con "impacto positivo en el rendimiento general" | ⚠️ **26**, y **24 en tests**; solo 2 en `src/` (`application/elastic_ingestion.rs`, `infrastructure/observability/test_log_capture.rs`) | RF-15 se reclasifica como **higiene**, prioridad P2, sin promesa de rendimiento |
| C7 | 482 `if let` refactorizables a let chains | ⚠️ 482 es el total en `core/src`; candidatos reales de anidamiento: **112** en 53 ficheros | RF-16 con ratchet, no conversión masiva |
| C8 | `cfg_select!` reemplaza macros externas tipo `cfg-if` | ⚠️ **0 usos** de `cfg_if` en el workspace | Sin requisito. Se documenta como disponible, no como trabajo |
| C9 | `pop_front_if` simplifica `SessionCap::try_admit` | ✅ `1.93`; el patrón `while front().is_some_and(..) { pop_front(); }` existe en `webfang_mcp::mcp_server::server::SessionCap::try_admit` | RF-17, con la invariante de capacidad pre-reservada intacta |
| C10 | `DropGuard` elimina guardas RAII artesanales | ✅ `core::mem::DropGuard` en 1.100; hay 2 candidatos: `adapters::downloader::InFlightGuard`, `chromiumoxide_downloader::HandlerGuard` | RF-18 **opcional**: `HandlerGuard` aborta un `JoinHandle`, no es un `Drop` puro; evaluar antes de sustituir |
| C11 | "Fase 2: integrar un primer LLM" | ❌ ya existe: `LlmPort`, `--extract-with-llm`, `EmbeddingPort`/`RemoteEmbeddingAdapter`, AXTree, SoM, MCP (`rmcp 3.9`) | F4 se reformula como **orquestación**, no como integración |
| C12 | Plugin con `llama.cpp` local | ❌ contradice **ADR-0004** (ONNX local borrado; embeddings remotos única vía) | Requiere superseder ADR explícito; no se cuela por un plugin |
| C13 | Plugins dinámicos con `abi_stable`, `*mut c_void`, `catch_unwind` | ❌ choca con `unsafe_code = "deny"` + Miri + provenance de release | §8.3: comparativa y recomendación (proceso hijo MCP). ADR-0023 bloqueante |
| C14 | `path_jail` como dependencia de seguridad | ⚠️ crate no verificado; el repo ya tiene guardas de ruta (`adapters::url_path`, `domain::crawler_port::filename`, `infrastructure::export::file_exporter`) | RF-24: reutilizar lo existente; ninguna dependencia nueva sin `cargo-deny` + ADR |
| C15 | "Rust: 5× menos memoria, 70% de los CVEs" | ⚠️ cifras de marketing, no medibles aquí | Fuera de los KPI. Si se quiere el argumento, se mide con `webfang_benchmark` |
| C16 | Ataque a `arrayref`, 86 minutos | ⚠️ no verificado | No es premisa de ningún requisito; `min-publish-age` se justifica por diseño, no por anécdota |
| C17 | "Actualizar SHA pins" como parte del bump | ⚠️ matiz importante: el SHA mayoritario (`6bed0761…`) es de la línea *master*, donde `toolchain` **es un input**. Los SHAs de rama de versión (`1.100.0` → `9522411a9bce8484da960823fc2f46e52f981f71`) **no exponen ese input**: lo hardcodean | RF-03: **no repointar** a rama de versión (rompería la premisa del drift guard); editar los 3 `toolchain:` explícitos |

---

## 4. Objetivos, no-objetivos y éxito

### 4.1 Objetivos

- **O1** — Compilar y publicar el workspace con `1.100.0` sin regresión funcional ni de SLO.
- **O2** — Migrar los 5 miembros (+ `fuzz`, `tools/intra-crate-lint`) a `edition 2024`.
- **O3** — Convertir la cadena de suministro en política declarativa y verificable (cooldowns,
  warnings sin `RUSTFLAGS`, lints de Cargo).
- **O4** — Pagar la deuda de superficie que la nueva edición/stdlib desbloquea, con ratchets.
- **O5** — Dejar el MSRV y las líneas de soporte con una política escrita y un gate que la enforce.
- **O6** — Decidir (no implementar) el seam de extensibilidad y la orquestación de agente, con ADR.

### 4.2 No-objetivos (explícitos)

- No reescribir a microservicios ni cambiar el modelo de despliegue (binario + MCP stdio/HTTP).
- No sustituir `wreq` por `reqwest` (política dura del repo, TLS fingerprint).
- No reintroducir inferencia local (ADR-0004) ni unificar dependencias deliberadamente
  duplicadas (`dashmap 5/6`, `selectors 0.35/0.37/0.38`).
- No introducir FFI/`unsafe` nuevo en `webfang_core`/`webfang_mcp` como parte de esta migración.
- No tocar `CHANGELOG.md` (escritor único: el Release PR) ni `docs/` salvo los pins que el
  propio bump obliga y con la superficie autorizada.
- No lanzar producto, pricing ni API pública en este PRD (F4 es decisión, no entrega).

### 4.3 Criterios de éxito (KPI con gate)

| KPI | Baseline | Objetivo | Gate que lo mide |
|---|---|---|---|
| K1 Compilación limpia | clippy `-D warnings` | 0 warnings con `CARGO_BUILD_WARNINGS=deny`, sin `RUSTFLAGS` global | `clippy`, `test-core`, `test-full` |
| K2 Tier 1 p90 | ≤10 min (SLO) | ≤10 min tras el bump; pico transitorio ≤+20% admitido 2 semanas | `scripts/ci_metrics.sh`, `docs/ci-metrics/fast-gate.log` |
| K3 `assert!(matches!` | 474 | **0** (ratchet descendente) | nuevo contador en el gate de higiene (§13.3) |
| K4 `from_utf8_lossy(..).into_owned()` | 26 | 0 | mismo contador |
| K5 let-chains | 112 candidatos | ≤20 restantes justificados | mismo contador (no bloqueante) |
| K6 Snapshots | 110 `.snap` | 0 diffs no revisados; cada churn con causa escrita | `cargo insta review` + `test-core`/`test-full` |
| K7 trybuild | 5 `.stderr` | regenerados y justificados uno a uno | `compile_fail_cases_fail_to_compile` |
| K8 Determinismo | verde | byte-idéntico | `webfang_benchmark/tests/corpus_determinism_test.rs` |
| K9 UB | 0 | 0 nuevo | `miri`, `miri-infra-fast`, `miri-infra-slow`, `sanitizers.yml` |
| K10 Mutación | ~1.640 mutantes / 96 shards | score no decrece | `mutants.yml` |
| K11 Compatibilidad | 5 combos + 1 pairwise | 5/5 + 1/1 | `scripts/check_compatibility.sh`, `feature-matrix` |
| K12 Windows | verde | verde (los `cfg(windows)` no los ve ningún gate Linux) | `scripts/check_windows_platform.sh`, `cfg-windows-lint` |
| K13 Coherencia de toolchain | verde | verde en el commit del bump | `check_toolchain_drift.sh` + `test_toolchain_drift.sh` |
| K14 Seeds | `seeded` | `seeded` restaurado tras re-publicar (no `cold` permanente) | línea `seed:` de `seed_target.sh` |

---

## 5. Actores y necesidades

| Actor | Necesidad | Qué le da esta migración |
|---|---|---|
| Maintainer / owner | Que el bump no cueste una release vacía como `v7.0.0` | Gates de drift, reversión por fase, tags de recuperación |
| Agente implementador (SDD/RDD) | Lotes ≤400 líneas, criterios verificables, sin ambigüedad | §6 con RF + criterios + lotes en §14 |
| Contribuidor local | Que `mise`/`just`/worktrees sigan funcionando el día 1 | RF-02, RF-04, seeds re-publicados |
| Usuario de binario | Que el binario no cambie de comportamiento | K8, K11, matriz de compatibilidad |
| Consumidor de crates (MSRV) | Saber con qué compilador puede buildar | O5, política MSRV + nota de release |
| Operador de CI | Presupuestos de caché y tiempos | RF-09..RF-12, K2, K14 |
| Cliente enterprise (futuro F4) | Aislamiento de código de terceros | §8.3, ADR-0023 |

---

## 6. Requisitos

Formato: **EARS** (`WHEN … THE SYSTEM SHALL …`) + criterios de aceptación verificables.
Prioridad: **P0** bloqueante de fase · **P1** obligatorio del PRD · **P2** deseable.
Esfuerzo en lotes de trabajo (work units ≤400 líneas, la convención del repo).

### E0 — Línea base y reversibilidad (F0)

**RF-01 (P0, 1 lote) — Congelar la línea base.**
WHEN se abra el tren de migración THE SYSTEM SHALL tener un tag de recuperación por fase y un
inventario de superficie versionado.
- [ ] Tag `migration/pre-1100` sobre el SHA de partida (convención del repo: tag de recuperación antes de publicar, como `onnx-tree/pre-step5` en ADR-0004).
- [ ] `anexos/audit_webfang.py` ejecutado y su salida adjunta al issue padre (no como gate nuevo sin aprobación).
- [ ] Inventario de tests regenerado (`scripts/ci_test_inventory.py`) para que K3–K5 se midan contra una cifra viva.
- **Aceptación:** `git tag --list 'migration/*'` muestra el tag; el issue padre tiene el baseline adjunto con SHA.

**RF-02 (P0, 1 lote) — Verde local antes de tocar nada.**
WHEN se empieza F1 THE SYSTEM SHALL tener `bash scripts/ci_fast_gate.sh` en verde y su informe
de cobertura de lane leído.
- [ ] Verde en el SHA de partida, con el informe de `gate.needs` que cubre / cubre en parte / no toca.
- [ ] `git config core.hooksPath scripts/githooks` activo en el clon de trabajo.
- **Aceptación:** log del gate adjunto; los jobs que el informe marca como "no cubre" y que este tren toca se listan explícitamente para corrida manual.

### E1 — Salto de toolchain 1.91.1 → 1.100.0 (F1)

**RF-03 (P0, 1 lote) — Bump atómico de la declaración de toolchain.**
WHEN se cambie el canal THE SYSTEM SHALL cambiarlo en todos los puntos que el drift guard lee,
en un solo commit.
- [ ] `rust-toolchain.toml` → `channel = "1.100.0"` (componentes `rustfmt`, `clippy` intactos).
- [ ] `mise.toml` → `rust = "1.100.0"` (parche exacto, no `1.100`).
- [ ] `Cargo.toml` → `rust-version = "1.100"` (ver RF-20 para el semver).
- [ ] Los **3** `toolchain:` explícitos → `1.100.0`: job `mcp` de `ci.yml`, matrix de targets de `release.yml`, paso de provenance de `release.yml`.
- [ ] Los `nightly-2026-08-27` (miri/fuzz/sanitizers) **no se tocan** en este lote.
- [ ] **No** repointar el SHA `6bed0761…` a la rama `1.100.0` (`9522411a…`): esa rama no expone el input `toolchain` y rompería R1/R2 del guard. Si se quiere repointar, es un lote aparte con ADR.
- **Aceptación:** `bash scripts/check_toolchain_drift.sh` → `OK: toolchain pins coherent (channel=1.100.0, steps=35)`; `bash scripts/test_toolchain_drift.sh` verde; `rustc -V` == 1.100.0 en CI y local.

**RF-04 (P0, 1 lote) — Actualizar la narrativa que el bump hace falsa.**
WHEN el canal cambie THE SYSTEM SHALL dejar coherentes los pins de versión legibles por humanos.
- [ ] `AGENTS.md` (línea *Stack*), `README.md` (badge + prerequisitos), `docs/src/installation.md`, `docs/src/overview.md`, nota de release en `release.yml`.
- [ ] Comentarios históricos que citan `1.91.1` como *hecho pasado* se dejan (son historia, no receta) y se marcan si el gate lo exige.
- **Aceptación:** `bash scripts/check_docs_version_pins.sh` verde; `git grep -n '1\.91' -- ':!Cargo.lock' ':!CHANGELOG.md' ':!odd/**'` no deja ninguna *receta* apuntando a 1.91.

**RF-05 (P0, 2–4 lotes, elástico) — Absorber las notas de compatibilidad de 1.92→1.100.**
WHEN el compilador sea 1.100 THE SYSTEM SHALL compilar sin errores ni warnings nuevos, con cada
cambio de comportamiento justificado en el commit.
Riesgos concretos a barrer (de las notas de 1.100):
- [ ] **never type fallback → `!` en todas las ediciones** + `Infallible` como alias de `!`: revisar inferencia en closures/`?`/`Result<_, Infallible>`; los lints `never_type_fallback_flowing_into_unsafe` y `dependency_on_unit_never_type_fallback` son deny-by-default desde **1.92** y ya señalan los sitios afectados (incluidos los 3 ficheros con `unsafe` sancionado).
- [ ] **Captura de closures en `if let` más precisa**: puede producir errores nuevos de borrow; 627 `if let` en el árbol.
- [ ] **`Box` generalizado a allocators**: nuevos errores de inferencia en `Box::…`; auditar los `-> impl Trait` (10) y conversiones `Box<dyn …>`.
- [ ] **dyn-compatibility comprobada en más sitios**: revisar los `dyn` ports (`LlmPort`, `EmbeddingPort`, `Downloader`, `StateStorePort`).
- [ ] **`str::escape_debug` escapa grapheme extenders** + **Unicode 18.0.0**: mueve snapshots y clasificación de caracteres → K6/K8.
- [ ] **`Iterator::{min,max}(_by_key)`** cambia qué métodos de `Ord` usa: solo importa si algún `Ord` es inconsistente; el corpus de determinismo es el detector.
- [ ] **`box` patterns ya no son sintaxis válida**; `aligned_fields_in_packed` deny-by-default; `repr(..)` repetido lintado.
- [ ] **ARMv7 hardfloat sin `d32` en baseline (1.94→1.100)**: si algún target de la matrix es ARMv7, documentar `-Ctarget-feature=+d32` o el target afectado.
- **Aceptación:** `cargo check --workspace --all-features` y `cargo clippy --all-targets --all-features -- -D warnings -W clippy::cognitive_complexity -W clippy::too_many_lines` en verde; cada error absorbido tiene su párrafo en el mensaje de commit.

**RF-06 (P0, 1 lote) — Re-medir el mecanismo de seeds con el nuevo layout.**
WHEN Cargo 1.100 cambie el layout de artefactos intermedios THE SYSTEM SHALL demostrar que la
poda y la publicación de seeds siguen siendo correctas, o rechazar el seeding de forma ruidosa.
- [ ] La clave de `seed_recipe.sh` incluye la versión de rustc ⇒ todos los seeds anteriores quedan inválidos: medir el coste de build frío y **re-publicar** un seed 1.100.
- [ ] Verificar la poda por embedding de ruta (`seed_publish.sh`) contra el nuevo layout: el árbol que se mide (`<target>/debug/build/`, salida de build-scripts, BoringSSL) es exactamente el que cambia.
- [ ] Mantener el rechazo de `build-dir` configurado (política, no workaround) y actualizar el comentario que hoy dice "WebFang is on 1.91".
- [ ] `include` en config de Cargo está estabilizado desde 1.93: el rechazo existente ya lo cubre; confirmar que sigue fail-closed con 1.100.
- **Aceptación:** dos corridas consecutivas de `seed_target.sh` imprimen `seed: seeded reason=reflink …`; `seed_publish.sh` publica y el self-check pasa; el tiempo de build frío queda registrado en el issue.

**RF-07 (P1, 1 lote) — Actualizar el arnés que asume versiones.**
WHEN el toolchain suba THE SYSTEM SHALL dejar coherentes los comentarios/guards que citan MSRV
de herramientas.
- [ ] `scripts/check_duplication.sh` (jscpd v5 con MSRV 1.96 vía `+stable`): con workspace en 1.100 se puede simplificar; decidir y documentar (no obligatorio).
- [ ] `cargo-nextest 0.9.131`, `cargo-edit 0.13.5`, `typos 1.42.3`, `insta 1.48.0`: revisar pins que se justificaban por MSRV 1.91; cada despin es un lote con `cargo-deny` verde.
- **Aceptación:** `mise.toml` sin comentarios que afirmen un MSRV falso; `cargo deny check` verde.

### E2 — Cadena de suministro y CI (F1)

**RF-08 (P0, 1 lote) — Cooldown de dependencias.**
WHEN Cargo resuelva versiones nuevas THE SYSTEM SHALL ignorar las publicadas hace menos de 3 días.
- [ ] `.cargo/config.toml` += `[registry] global-min-publish-age = "3 days"` (ver `anexos/config/`).
- [ ] Documentar el alcance: **no** re-audita `Cargo.lock`; es protección de resolución, no escáner.
- [ ] Documentar la salida de emergencia (bump de seguridad urgente): override puntual por registry o `cargo update -p <crate> --precise <ver>` con justificación en el PR.
- **Aceptación:** `cargo update --dry-run` con una dependencia recién publicada la excluye; el runbook de excepción está en el issue y en el comentario del config.

**RF-09 (P0, 1 lote) — Warnings sin `RUSTFLAGS`.**
WHEN CI quiera deny de warnings THE SYSTEM SHALL usar `CARGO_BUILD_WARNINGS=deny` en lugar de
`RUSTFLAGS=-D warnings`.
- [ ] Aplicado a los jobs de compilación (`clippy`, `test-core`, `test-full`, `feature-matrix`, `mcp`, `coverage`) vía `env:` a nivel de job.
- [ ] `RUSTDOCFLAGS=-D warnings` se mantiene (rustdoc no lo cubre esa variable).
- [ ] No se introduce `RUSTFLAGS` global nuevo; los `--cfg tokio_unstable` de los aliases `cargo console`/`cargo debug` quedan intactos.
- **Aceptación:** un job con un warning provocado falla; `Swatinem/rust-cache` mantiene hits entre un job con la variable y otro sin ella (medir hit-rate antes/después).

**RF-10 (P1, 1 lote) — Lints de Cargo.**
WHEN el workspace esté en 1.100 THE SYSTEM SHALL declarar `[lints.cargo]` con
`cargo::unused_dependencies` y decidir su relación con `cargo-machete`.
- [ ] Añadir la tabla al `Cargo.toml` raíz y herencia por miembro (`[lints] workspace = true` ya existe).
- [ ] Decisión explícita: coexistencia (machete sigue siendo el gate) o sustitución (entonces se retira el job en el mismo lote y se actualiza `scripts/ci_fast_gate_coverage.tsv`).
- **Aceptación:** el lint dispara con una dependencia muerta plantada; el gate de `code-quality` sigue siendo un solo productor de verdad.

**RF-11 (P1, 1 lote) — Presupuesto de caché tras el bump.**
WHEN todos los seeds se invaliden THE SYSTEM SHALL mantener el pool de caché dentro del
presupuesto existente (≤9,5 GB, sin entradas merge-ref de PRs cerrados).
- [ ] Medir el pool tras la primera semana con `cache-hygiene.yml` en marcha.
- [ ] Confirmar que `CARGO_INCREMENTAL=0` en CI sigue siendo la política (el layout nuevo no la cambia).
- **Aceptación:** informe de `gh cache list` antes/después; sin crecimiento neto >10%.

**RF-12 (P2, 1 lote) — Ventana de adopción de toolchain.**
WHEN se planifique el siguiente bump THE SYSTEM SHALL tener una política escrita (§9.4) que
evite adoptar un `.0` el día de su publicación sin red.

### E3 — Migración a edition 2024 (F2)

**RF-13 (P0, 3–5 lotes) — Migración mecánica por crate, en orden topológico.**
WHEN se migre la edición THE SYSTEM SHALL hacerlo crate a crate, con `cargo fix --edition` y
sin cambios de comportamiento en el mismo commit.
- [ ] Orden: `webfang_test_utils` → `webfang_core` → `webfang_mcp` → `webfang_cli` → `webfang_benchmark`; después `fuzz/` y `tools/intra-crate-lint/` (edition propia, fuera del workspace).
- [ ] `cargo fix --edition --all-features --allow-dirty` por crate, luego `edition = "2024"` en `[workspace.package]`.
- [ ] Barrido dirigido de los cambios de 2024: `unsafe` en `env::set_var`/`remove_var` (**ya pagado** en `webfang_test_utils`), `unsafe(no_mangle/export_name)`, `unsafe extern` blocks, captura de lifetimes en RPIT (10 sitios), `gen` como palabra reservada (**0 usos**), `static mut` (**0 usos**), precedencia de `impl Trait`/`dyn`, cambios en `match` ergonomics y en el orden de `Drop` de temporales en `if let`/`tail expr`.
- [ ] `rustfmt`: fijar la política de `style_edition` **explícitamente**. Si se adopta el estilo 2024, el reformateo es un commit separado y puramente mecánico (577 ficheros en el peor caso) — nunca mezclado con semántica.
- **Aceptación por crate:** `cargo check --all-features` + `cargo clippy --all-targets --all-features -D warnings` + `cargo nextest run` verdes; `cargo fmt --all -- --check` verde; 0 diffs de snapshot sin revisar.

**RF-14 (P0, 1 lote) — Regenerar el arnés de diagnóstico.**
WHEN la edición y el compilador cambien los diagnósticos THE SYSTEM SHALL regenerar los
fixtures con justificación individual.
- [ ] 5 `.stderr` de trybuild (`tests/compile_fail/page_state/*`, `batch_manager/zero_concurrency`) revisados uno a uno: el mensaje debe seguir expresando *la misma* prohibición de tipo, no una nueva.
- [ ] 110 `.snap` de insta: `cargo insta review` con causa escrita por grupo (escape_debug, Unicode 18, formato de `!`/`Infallible`).
- [ ] El test `compile_fail_cases_fail_to_compile` sigue fallando por la razón correcta (no por un mensaje distinto).
- **Aceptación:** diff de fixtures con un párrafo por fichero en el PR; K6/K7 en verde.

**RF-15 (P1, 1 lote) — Confirmar que la edición no cambió semántica observable.**
WHEN todos los miembros estén en 2024 THE SYSTEM SHALL demostrar equivalencia funcional.
- [ ] `scripts/check_compatibility.sh` (5 combos + pairwise) verde.
- [ ] `corpus_determinism_test.rs` byte-idéntico.
- [ ] `scripts/check_windows_platform.sh` verde (brazos `cfg(windows)` invisibles a los gates Linux).
- [ ] Lanes Miri + sanitizers verdes.
- **Aceptación:** matriz de compatibilidad actualizada con la columna de edición; evidencias adjuntas.

### E4 — Superficie de APIs estables (F3)

**RF-16 (P1, 6 lotes) — `assert_matches!` (1.96).**
WHEN un test afirme forma de valor THE SYSTEM SHALL usar `assert_matches!` para que el fallo
imprima el `Debug` del valor.
- [ ] 474 sustituciones, en 6 lotes por área: (1) `webfang_mcp/tests` (34 solo en `mcp_params_validation_test.rs`), (2) `core/src/cli/*` + `core/tests`, (3) `core/src/error/*`, (4) `core/src/application/*`, (5) `core/src/infrastructure/*`, (6) resto de `core/src` + `benchmark` + `test_utils`.
- [ ] Prohibido cambiar la aserción: misma forma, mismo patrón; si un patrón ligaba variables, se conserva el ligamento.
- [ ] `debug_assert_matches!` donde hoy no hay aserción en release: **no** se introduce (0 usos hoy).
- **Aceptación:** contador del patrón = 0; cada lote con `cargo nextest run` del área verde; una prueba negativa plantada demuestra el nuevo mensaje de fallo.

**RF-17 (P2, 1 lote) — `from_utf8_lossy_owned` (1.99).**
WHEN se convierta bytes a `String` THE SYSTEM SHALL usar `String::from_utf8_lossy_owned` para
evitar la copia cuando el búfer ya es UTF-8 válido.
- [ ] 26 sitios `into_owned` (2 en `src/`, 24 en tests) + evaluar los 12 `.to_string()` (no equivalentes mecánicos: decidir caso a caso).
- [ ] Sin promesa de rendimiento medible: el impacto está en el arnés, no en el crawler.
- **Aceptación:** contador = 0; benchmarks de `webfang_benchmark` sin regresión (ruido, no mejora esperada).

**RF-18 (P2, 1 lote) — `pop_front_if` (1.93) en `SessionCap::try_admit`.**
WHEN se pode la ventana de admisión THE SYSTEM SHALL expresar el prune con `pop_front_if`.
- [ ] Sustituir `while front().is_some_and(..) { pop_front(); }` manteniendo: sección crítica corta (prune + test + push), capacidad pre-reservada a `max`, sin `.await` bajo lock, `now` inyectado.
- **Aceptación:** `try_admit_refuses_past_the_cap_and_reports_the_live_count` y el resto de tests de `SessionCap` verdes sin modificar sus expectativas; `retry_after_secs` intacto.

**RF-19 (P2, 1 lote) — `trim_prefix`/`trim_suffix` (1.100).**
WHEN se recorte un prefijo con fallback THE SYSTEM SHALL usar `trim_prefix` donde sea
semánticamente idéntico.
- [ ] 17 candidatos `strip_prefix(..).unwrap_or(..)` (11 en `src/`): revisar uno a uno; `strip_prefix` devuelve `Option` y `trim_prefix` devuelve el original — solo son intercambiables cuando el fallback es el original.
- [ ] `Path::trim_prefix` en código de rutas: no tocar los guards de traversal (`adapters::url_path`, `crawler_port::filename`) sin test de escape.
- **Aceptación:** tests de rutas y de export verdes; 0 cambios en la política de nombres de fichero.

**RF-20 (P2, 1 lote) — Let chains (edition 2024).**
WHEN haya `if let` anidados THE SYSTEM SHALL aplanarlos a `if let … && let …` donde mejore la
lectura sin cambiar el scope de los ligamentos.
- [ ] 112 candidatos; objetivo ≤20 restantes justificados (ratchet, no conversión total).
- [ ] Prohibido aplanar cuando el `else` del interno no es vacío, o cuando hay temporales cuyo `Drop` cambie de orden.
- [ ] Los 18 brazos `_ => {}` vacíos: eliminar solo si el `match` era un `if let` encubierto.
- **Aceptación:** contador ≤20 con lista de excepciones; complejidad cognitiva no sube (`clippy.toml`: umbral 30/100, ratchet descendente).

**RF-21 (P2, 1 lote, opcional) — `DropGuard` (1.100).**
WHEN una guarda RAII sea puro "ejecutar al salir" THE SYSTEM SHALL evaluar `core::mem::DropGuard`.
- [ ] `InFlightGuard` (`adapters::downloader`) y `HandlerGuard` (`chromiumoxide_downloader`): **evaluar**, no sustituir a ciegas. `HandlerGuard` aborta un `JoinHandle` (supervisión, `tokio::spawn` está en `disallowed-methods`): si la guarda documenta la supervisión, se queda.
- **Aceptación:** decisión escrita por guarda; si se sustituye, tests de cancelación/abort verdes.

### E5 — MSRV, soporte y versionado (F1–F3, transversal)

**RF-22 (P0, 1 lote) — Política MSRV escrita.**
WHEN el proyecto declare un MSRV THE SYSTEM SHALL tener una política con cadencia, criterio y
proceso de bump.
- [ ] Propuesta: MSRV = *latest stable* en el momento del corte de minor, con pin exacto de parche; bump como máximo una vez por trimestre; nunca en un parche de seguridad.
- [ ] Documento en `docs/` **solo con autorización explícita** (regla del repo: no tocar `docs/` sin pedirlo); si no, en el ADR-0021.
- [ ] Gate: el drift guard ya cubre toolchain↔workflows; añadir la comprobación `rust-version` ↔ `channel` si no existe (lote pequeño, con autotest).
- **Aceptación:** ADR-0021 Accepted con la política; el gate falla si `rust-version` y `channel` divergen.

**RF-23 (P0, 1 lote) — Decisión de semver y superficie del marker.**
WHEN el MSRV suba de 1.91 a 1.100 THE SYSTEM SHALL tratarlo como decisión de ruptura del
maintainer y poner el marker donde la estrategia de merge lo conserva.
- [ ] Recomendación: publicar como **8.0.0** (`feat!:`/`chore!:`), no como minor, porque un consumidor de `webfang_core` con 1.91 deja de poder compilar.
- [ ] Matrix de `scripts/breaking_surface_lib.sh`: SQUASH con 1 commit → marker en el subject del commit; SQUASH con ≥2 → en el título del PR; MERGE → en los commits. `type!(scope):` es malformado; el footer `BREAKING CHANGE:` solo no basta.
- [ ] Coordinar con `release-plz` (escritor único de versión y `CHANGELOG.md`) y con la rotación de líneas de soporte.
- **Aceptación:** `breaking-surface` verde en PR y en `merge_group`; la release resultante es la prevista.

**RF-24 (P0, 1 lote) — Backports compatibles con 1.91.**
WHEN una línea 7.7/7.6 reciba un parche de seguridad THE SYSTEM SHALL poder compilarlo con su
toolchain original.
- [ ] Regla: nada de let-chains, `assert_matches!`, `from_utf8_lossy_owned`, `trim_prefix` ni APIs ≥1.92 en commits destined a las líneas viejas.
- [ ] Documentar el flujo de backport (cherry-pick + reescritura de la parte idiomática) en el ADR-0021.
- **Aceptación:** un backport de prueba compila con `1.91.1`; `.github/support.json` sigue coherente con `SUPPORT.md`.

### E6 — Seam de extensibilidad y agente (F4, decisión bloqueante)

**RF-25 (P0 de F4, 1 lote) — ADR-0023 de extensibilidad.**
WHEN se quiera un sistema de plugins THE SYSTEM SHALL decidir el mecanismo contra las políticas
duras del repo antes de escribir código.
- [ ] Comparativa obligatoria de §8.3, con veredicto y criterio de abandono.
- [ ] Declarar explícitamente el impacto en `unsafe_code = "deny"`, Miri, `deny.toml`, provenance de release y dirección intra-crate (ADR-0010/0011/0012).
- **Aceptación:** ADR-0023 con estado Accepted o Proposed con bloqueantes nombrados; ningún código de plugins antes.

**RF-26 (P1 de F4, 1 lote) — Reformular la épica de agente sobre lo que ya existe.**
WHEN se planifique la orquestación THE SYSTEM SHALL partir del inventario real (`LlmPort`,
`EmbeddingPort`, AXTree, SoM, MCP) y no de una integración desde cero.
- [ ] Inventario de herramientas MCP expuestas hoy (fuente: `crates/webfang_mcp/tests/stdio_handshake.rs`) y de los puertos de dominio.
- [ ] Definir el bucle de agente como orquestación sobre esos puertos, con presupuesto (tokens/llamadas) y política de egress existente (ADR-0018, guardas SSRF).
- [ ] Nada de inferencia local sin superseder ADR-0004.
- **Aceptación:** documento de exploración (`odd/<slug>/`) con el inventario y el diseño del bucle; pricing fuera de este PRD.

---

## 7. Requisitos no funcionales

| ID | Requisito | Verificación |
|---|---|---|
| RNF-01 | **Cero `unsafe` nuevo** fuera de los 3 ficheros sancionados | `unsafe_code = "deny"` + revisión; contador de `#[allow(unsafe_code)]` no sube de 9 |
| RNF-02 | **Cero `.unwrap()`/`.expect()` en producción** | lints de workspace + carve-outs `cfg(test)` por raíz |
| RNF-03 | **Invariante ENV_LOCK**: ningún `env::set_var` fuera de `webfang_test_utils` | `crates/webfang_test_utils/tests/env_invariant_scan.rs` + `clippy.toml disallowed-methods` |
| RNF-04 | **Determinismo del corpus** byte-idéntico | `corpus_determinism_test.rs` |
| RNF-05 | **Dirección de dependencias** Clean Architecture intacta | `scripts/check_dependency_direction.sh`, `check_intra_crate_direction.sh` |
| RNF-06 | **SLOs de CI** (Tier0 2 / Tier1 10 / Tier2 30 min p90) | `scripts/ci_metrics.sh` |
| RNF-07 | **Tamaño de lote ≤400 líneas** salvo excepción aprobada | convención SDD/RDD del repo |
| RNF-08 | **Cero duplicación nueva** | `scripts/check_duplication.sh` (ratchet) |
| RNF-09 | **Cobertura y mutación no decrecen** | `coverage`, `mutants.yml` |
| RNF-10 | **Mensajes de usuario en español; logs/tracing en inglés** | revisión (regla del repo) |
| RNF-11 | **Sin dependencias nuevas** sin `cargo-deny` + justificación | `deny.toml`, job `security` |
| RNF-12 | **Reversibilidad por fase**: tag + procedimiento de vuelta atrás probado | §12 |

---

## 8. Arquitectura objetivo y decisiones pendientes

### 8.1 Política de toolchain (ADR-0021)

```
rust-toolchain.toml   channel = "1.100.0"      # pin exacto, fuente de verdad del canal
mise.toml             rust    = "1.100.0"      # mismo parche, para que rustfmt no derive
Cargo.toml            rust-version = "1.100"   # contrato con el consumidor
workflows             toolchain: 1.100.0       # solo los 3 pasos explícitos; nightly aparte
```

Regla: **una sola fuente de verdad** (`rust-toolchain.toml`) y un gate que compara todo lo demás
contra ella. El drift guard existente ya cubre workflows; falta cubrir `rust-version` y `mise.toml`.

### 8.2 Política de edición (ADR-0022)

- `edition = "2024"` en `[workspace.package]`, heredada por los 5 miembros; `fuzz/` y
  `tools/intra-crate-lint/` se migran en el mismo tren (tienen edición propia).
- `style_edition` explícito en `rustfmt.toml`: separar el reformateo del cambio semántico.
- Migración por crate en orden topológico, `cargo fix --edition` como red mecánica, revisión
  humana de cada diff (el fixer no cubre cambios de `Drop` de temporales ni de captura en RPIT).

### 8.3 Seam de extensibilidad (ADR-0023) — comparativa

| Mecanismo | Aislamiento | Coste de ABI | Encaje con las políticas del repo | Veredicto |
|---|---|---|---|---|
| **FFI dinámica (`abi_stable`, `.so`/`.dll`)** | ninguno (mismo proceso) | alto: `StableAbi`, `RBox`/`RStr`, estado por `*mut c_void`, `catch_unwind` en cada entrada | **rompe `unsafe_code = "deny"`**, invisible a Miri, compliciza provenance y `cargo-deny`; un panic mal contenido tumba el host | ❌ **No por defecto**. Solo con ADR que acepte la excepción de política y un plan de contención |
| **WASM (Component Model)** | fuerte (sandbox lineal) | medio: `wit` + runtime (`wasmtime`) | sin `unsafe` propio, pero **añade un runtime grande** al grafo (623 paquetes hoy), coste de build y superficie de auditoría; I/O de red del plugin choca con la política de egress/SSRF | ⚠️ Viable a medio plazo; requiere medición de build/CI y ADR de dependencias |
| **Proceso hijo MCP (JSON-RPC)** | fuerte (proceso aparte, límites del SO) | **cero FFI**: contratos = esquemas JSON | reutiliza `rmcp 3.9` + `axum` ya presentes; sin `unsafe`; compatible con Miri (el plugin no es Rust del workspace); encaja con la dirección de puertos/adaptadores | ✅ **Recomendado como primer seam**: "plugin = servidor MCP hijo" con handshake, timeouts y presupuesto |
| **Registro estático por features** | ninguno (compile-time) | cero | es lo que ya hace el repo (`chromium`, `mcp`, …); 5 combos de compatibilidad | ✅ Complemento: el seam interno sigue siendo `dyn Puerto`, no un plugin binario |

**Recomendación:** el seam interno es y sigue siendo el **puerto de dominio** (`LlmPort`,
`EmbeddingPort`, `Downloader`, `StateStorePort`). La extensibilidad de terceros se ofrece primero
**fuera del proceso** (MCP), no dentro del binario. Así la promesa comercial ("añade un proveedor
sin recompilar") se cumple sin pagar ABI ni `unsafe`.

---

## 9. Plan de ejecución

### 9.1 Fases y gates de salida

| Fase | Semanas | Contenido | Gate de salida (todos obligatorios) |
|---|---|---|---|
| **F0** Línea base | 0–1 | RF-01, RF-02 | tag creado · gate local verde · inventario regenerado · baseline adjunto |
| **F1** Toolchain 1.100.0 | 1–3 | RF-03..RF-12 | drift guard + autotest · docs pins · clippy/check/test verdes · compat 5/5 · Miri · seeds re-publicados (`seed: seeded`) · Tier1 p90 medido |
| **F2** Edition 2024 | 3–6 | RF-13..RF-15 | los 5 miembros + 2 excluidos en 2024 · fixtures regenerados con justificación · `corpus_determinism` byte-idéntico · Windows typecheck · sanitizers |
| **F3** Superficie de APIs | 6–9 | RF-16..RF-21 | contadores K3=0, K4=0, K5≤20 · mutación y cobertura no decrecen · ratchet de duplicación verde |
| **F4** Plataforma | 9+ | RF-25, RF-26 | ADR-0023 Accepted · inventario de puertos y herramientas MCP · **sin código de plugins antes del ADR** |

Transversal: RF-22..RF-24 se cierran **antes** de publicar la release que contiene F1.

### 9.2 Orden de aterrizaje (por qué así)

1. **Toolchain antes que edición.** Separar las dos variables es lo que hace diagnósticable un
   fallo: si 1.100 rompe inferencia, no quieres que el ruido de la edición lo tape.
2. **Edición antes que refactors.** Los let-chains necesitan 2024; `assert_matches!` necesita
   1.96 (ya cubierto por F1) pero se hace después para no mover 474 sitios a la vez que el compilador.
3. **Refactors al final, en lotes por área.** Son los de mayor churn y menor riesgo: van últimos
   para que un rollback de F3 no arrastre F1/F2.

### 9.3 Calendario y ventanas externas

- `1.100.0` tiene 2 días a fecha de este PRD. **La línea 7.7 se queda en 1.91.1** para backports (RF-24).
- `1.101.0` llega el **2026-12-24**: cae dentro de F2/F3. Política: no se persigue; el siguiente
  bump se planifica tras cerrar F3 (trimestral, RF-22).
- Publicar el corte **8.0.0** al cerrar F2 (toolchain + edición juntos, un solo evento de ruptura
  para el consumidor), y F3 como minors posteriores.

### 9.4 Política de adopción de toolchain (propuesta para ADR-0021)

| Regla | Valor |
|---|---|
| Cadencia de bump | ≤1 por trimestre, siempre en un minor/major, nunca en un parche de seguridad |
| Ventana de enfriamiento | ≥14 días desde la publicación del `.0` antes de adopting en `main` (excepción: CVE del compilador) |
| Pin | parche exacto, idéntico en `rust-toolchain.toml` y `mise.toml` |
| Nightly | pin por fecha (`nightly-2026-08-27`), se mueve en lote propio con Miri/fuzz/sanitizers verdes |
| Reversión | tag por fase + procedimiento de §12 |

> Nota de coherencia: hoy F1 arranca a 2 días del `.0`, lo que **viola** la ventana de 14 días
> propuesta. Decisión explícita del maintainer: o se adopta 1.100.0 ya (con la línea 7.7 como red),
> o F1 empieza el 2026-11-26. El PRD recomienda lo segundo si no hay urgencia de producto.

---

## 10. Registro de riesgos

| ID | Riesgo | P | I | Detector | Mitigación / rollback |
|---|---|---|---|---|---|
| R-01 | Release vacía por drift de pin (el fallo de `v7.0.0`) | M | **Crítico** | `check_toolchain_drift.sh` + autotest | RF-03 atómico; no repointar a rama de versión; probar la matrix de targets antes del tag |
| R-02 | Never-type fallback / `Infallible` alias rompen inferencia | **A** | Alto | `cargo check --all-features` en F1 | RF-05; lote elástico reservado; revert de F1 si >2 días de bloqueo |
| R-03 | Churn de 110 snapshots + 5 `.stderr` mal revisado (falso verde) | **A** | Alto | `cargo insta review`, trybuild | RF-14 con causa por fichero; prohibido `INSTA_UPDATE=always` a ciegas |
| R-04 | Seeds inválidos ⇒ CI lento permanente | **A** | Medio | línea `seed:` de `seed_target.sh`, `ci_metrics.sh` | RF-06 re-publicación; K2 con pico admitido 2 semanas |
| R-05 | Nuevo layout de Cargo invalida la poda del seed (silencioso) | M | Alto | self-check de `seed_publish.sh` | RF-06; si no se demuestra, **rechazar seeding** (cold ruidoso) antes que sembrar mal |
| R-06 | Reformateo masivo por `style_edition` tapa el diff semántico | **A** | Medio | `cargo fmt --check` | commit de formato separado; `git diff -w` en revisión |
| R-07 | `env::set_var` unsafe en 2024 | **Baja** (ya pagado) | Alto | `env_invariant_scan`, clippy | verificar que las 25 llamadas siguen tras `unsafe {}` y `ENV_LOCK` |
| R-08 | MSRV bump rompe consumidores y backports | M | Alto | `breaking-surface`, `.github/support.json` | RF-23 (8.0.0) + RF-24 (backports 1.91-compatibles) |
| R-09 | Cooldown bloquea un parche de seguridad urgente | M | Medio | `cargo update --dry-run` | override puntual documentado (RF-08) |
| R-10 | Let-chains cambian el orden de `Drop` de temporales | M | Alto | tests de cancelación/shutdown, Miri | RF-20 prohíbe aplanar con temporales sensibles; lote reversible |
| R-11 | `cargo::unused_dependencies` duplica el gate de machete | M | Bajo | `ci_fast_gate_coverage.tsv`, `test_ci_gate_structure.sh` | RF-10 decide coexistencia o sustitución en el mismo lote |
| R-12 | Presión de F4 por meter FFI "solo un poco" | M | **Crítico** | revisión + `unsafe_code = deny` | ADR-0023 bloqueante; §8.3 como alternativa aprobable |
| R-13 | Unicode 18 / `escape_debug` cambian extracción real | M | Alto | `corpus_determinism_test`, snapshots de contenido | RF-15; si cambia salida visible, es cambio de producto y se documenta en la release |
| R-14 | ARMv7 / cambios de ABI por target | Baja | Medio | `test-crossplatform`, matrix de release | RF-05 revisa targets; `+d32` documentado si aplica |

---

## 11. Estrategia de pruebas y evidencias

| Capa | Qué se corre | Cuándo |
|---|---|---|
| Estática | `cargo check`, `cargo clippy --all-targets --all-features -- -D warnings -W clippy::cognitive_complexity -W clippy::too_many_lines`, `cargo fmt --all -- --check`, `RUSTDOCFLAGS=-D warnings cargo doc` | cada lote |
| Local gate | `bash scripts/ci_fast_gate.sh` + leer el informe de cobertura de lane | antes de cada push |
| Unit / integración | `cargo nextest run` (área del lote) · `test-core` / `test-full` en CI | cada lote |
| Snapshots | `cargo insta review` (nunca aceptar en bloque) | F2, F3 |
| Tipos negativos | trybuild `compile_fail` (5 fixtures) | F2 |
| Compatibilidad | `scripts/check_compatibility.sh` (5 combos + pairwise), `cargo hack --each-feature` | F1, F2 |
| Plataformas | `scripts/check_windows_platform.sh`, `test-crossplatform`, `cfg-windows-lint` | F1, F2 y cualquier toque de `cfg(windows)` |
| UB / concurrencia | `miri`, `miri-infra-fast`, `miri-infra-slow`, `sanitizers.yml` | F1, F2, F3 (lotes de concurrencia) |
| Determinismo | `corpus_determinism_test.rs` | F1, F2, F3 |
| Mutación | `cargo-mutants` (diff en PR, 96 shards semanal) | F3 y cualquier toque de la espina de extracción |
| Seguridad | `cargo-deny`, `cargo-audit`, `zizmor`, `actionlint`+`shellcheck` | cada lote con config/CI |
| Gobierno | `check_toolchain_drift.sh`, `test_toolchain_drift.sh`, `check_docs_version_pins.sh`, `check_test_inventory.sh`, `check_duplication.sh`, `breaking-surface`, `check-topology.sh` | cada PR |

**Evidencia mínima por lote:** log del gate local, lista de tests corridos, diff de snapshots con
causa, y medición cuando el lote toque tiempos (semilla, caché, SLO).

---

## 12. Reversibilidad

| Fase | Tag de recuperación | Rollback |
|---|---|---|
| F1 | `migration/pre-1100` | revert del commit de bump (atómico por diseño) + re-publicar seed 1.91.1; los seeds viejos siguen siendo válidos si no se borraron |
| F2 | `migration/pre-edition2024` | revert por crate (la migración es por crate precisamente para esto); `edition` vuelve a 2021 en el miembro afectado |
| F3 | `migration/pre-api-surface` | revert por lote de área; los contadores vuelven a su valor anterior, sin efecto funcional |
| Release | tag de la línea previa (`v7.7.1`) | la línea 7.7 sigue en 1.91.1 y acepta bugfix/security (`.github/support.json`) |

Regla: **ninguna fase se publica sin su tag creado antes** (la convención que ADR-0004 usó con
`onnx-tree/pre-step5`). Un rollback no toca `CHANGELOG.md`: lo hace el flujo de release.

---

## 13. Gobierno: cómo aterriza en el pipeline del repo

### 13.1 SDD/RDD

El repo trabaja `explore → propose → spec → design → tasks → apply → verify → archive`, con
presupuesto de ~400 líneas por work unit y excepciones aprobadas por el maintainer. Este PRD
ocupa el hueco **propose/spec**: cada RF se convierte en un issue con su `odd/<slug>/tasks.md`
y su evidencia. Los lotes de §14 están dimensionados para ese presupuesto.

### 13.2 Reglas que este tren debe respetar (y que el informe ignoraba)

- Sin PRs apilados (`scripts/check-topology.sh`); un worktree por lote, con su `.envrc`.
- `CHANGELOG.md`: escritor único (Release PR). `docs/`: no tocar sin autorización (RF-04 la pide explícitamente para los pins).
- Citas por **símbolo o ruta**, nunca por número de línea (este documento cita `SessionCap::try_admit`, no `server.rs:841`).
- No copiar estado mutable en prose: los contadores se re-miden con el script del anexo.
- Marker de ruptura en la superficie que la estrategia de merge conserva (`breaking_surface_lib.sh`).
- `git config core.hooksPath scripts/githooks` en cada clon nuevo.

### 13.3 Ratchets nuevos (requieren aprobación)

K3/K4/K5 necesitan un contador. Regla del repo: **un gate nuevo debe declararse** en
`scripts/ci_fast_gate_coverage.tsv` y su estructura la valida `scripts/test_ci_gate_structure.sh`.
Propuesta: un único script `scripts/check_api_surface_ratchet.sh` con un fichero de presupuesto
(versionado, descendente) para los tres contadores, en el lane `code-quality`, **fail-closed**.
Sin aprobación del owner, se mide a mano y se adjunta la evidencia al PR.

---

## 14. Backlog propuesto

Los títulos siguen la convención de commits del repo. `!` marca ruptura. Detalle importable en
`anexos/backlog-migracion.csv`.

**Volumen:** 33 issues, **38–41 work units** de ≤400 líneas (F0: 2 · F1: 13–15 · F2: 10–11 ·
F3: 11 · F4: 2). La elasticidad está en MIG-05/06 (absorción de compatibilidad) y MIG-14
(edition en `webfang_core`, 179.522 LOC): si esos lotes se pasan de presupuesto, se parten,
no se pide excepción.

| ID | Título | Épica | Fase | Depende de | Lotes |
|---|---|---|---|---|---|
| MIG-01 | `chore(migration): freeze baseline, recovery tag and test inventory` | E0 | F0 | — | 1 |
| MIG-02 | `chore(ci): record fast-gate lane coverage before the toolchain train` | E0 | F0 | MIG-01 | 1 |
| MIG-03 | `chore(toolchain)!: bump channel, mise and rust-version to 1.100.0` | E1 | F1 | MIG-02 | 1 |
| MIG-04 | `docs(pins): align README, AGENTS and install recipes with 1.100` | E1 | F1 | MIG-03 | 1 |
| MIG-05 | `fix(core): absorb never-type fallback and Infallible alias inference` | E1 | F1 | MIG-03 | 1–2 |
| MIG-06 | `fix(core): absorb 1.100 closure capture, Box allocator and dyn-compat changes` | E1 | F1 | MIG-03 | 1–2 |
| MIG-07 | `chore(scripts): re-measure seed prune and publish a 1.100 seed` | E1 | F1 | MIG-03 | 1 |
| MIG-08 | `chore(deps): enable registry global-min-publish-age cooldown` | E2 | F1 | MIG-03 | 1 |
| MIG-09 | `chore(ci): move deny-warnings to CARGO_BUILD_WARNINGS` | E2 | F1 | MIG-03 | 1 |
| MIG-10 | `chore(cargo): declare [lints.cargo] and decide machete coexistence` | E2 | F1 | MIG-09 | 1 |
| MIG-11 | `chore(ci): re-measure cache budget after seed invalidation` | E2 | F1 | MIG-07 | 1 |
| MIG-12 | `chore(tooling): revisit MSRV-driven tool pins` | E1 | F1 | MIG-03 | 1 |
| MIG-13 | `refactor(test_utils)!: migrate to edition 2024` | E3 | F2 | MIG-05, MIG-06 | 1 |
| MIG-14 | `refactor(core)!: migrate to edition 2024` | E3 | F2 | MIG-13 | 1–2 |
| MIG-15 | `refactor(mcp)!: migrate to edition 2024` | E3 | F2 | MIG-14 | 1 |
| MIG-16 | `refactor(cli,benchmark)!: migrate to edition 2024` | E3 | F2 | MIG-15 | 1 |
| MIG-17 | `chore(fuzz,tools): migrate excluded crates to edition 2024` | E3 | F2 | MIG-16 | 1 |
| MIG-18 | `style(fmt): pin style_edition and land the mechanical reformat` | E3 | F2 | MIG-17 | 1 |
| MIG-19 | `test(fixtures): regenerate trybuild stderr and insta snapshots with causes` | E3 | F2 | MIG-18 | 1 |
| MIG-20 | `test(compat): prove edition parity across the 5 required combos` | E3 | F2 | MIG-19 | 1 |
| MIG-21 | `test(mcp): adopt assert_matches! in mcp and cli error surfaces` | E4 | F3 | MIG-20 | 2 |
| MIG-22 | `test(core): adopt assert_matches! in error, application and infrastructure` | E4 | F3 | MIG-21 | 4 |
| MIG-23 | `refactor(core): adopt from_utf8_lossy_owned and trim_prefix` | E4 | F3 | MIG-20 | 1 |
| MIG-24 | `refactor(mcp): express SessionCap prune with pop_front_if` | E4 | F3 | MIG-20 | 1 |
| MIG-25 | `refactor(core): flatten let-chain candidates under edition 2024` | E4 | F3 | MIG-20 | 2 |
| MIG-26 | `refactor(downloader): evaluate DropGuard for InFlightGuard and HandlerGuard` | E4 | F3 | MIG-20 | 1 |
| MIG-27 | `docs(adr): ADR-0021 toolchain and MSRV policy` | E5 | F1 | MIG-01 | 1 |
| MIG-28 | `docs(adr): ADR-0022 edition 2024 adoption and style_edition` | E5 | F2 | MIG-27 | 1 |
| MIG-29 | `chore(ci): gate rust-version and mise against the toolchain channel` | E5 | F1 | MIG-03 | 1 |
| MIG-30 | `chore(release)!: cut 8.0.0 with the MSRV breaking marker on the kept surface` | E5 | F2 | MIG-20, MIG-27 | 1 |
| MIG-31 | `chore(support): keep 7.7/7.6 backports compilable on 1.91.1` | E5 | F1 | MIG-27 | 1 |
| MIG-32 | `docs(adr): ADR-0023 extensibility seam decision (no FFI by default)` | E6 | F4 | MIG-30 | 1 |
| MIG-33 | `docs(explore): inventory ports and MCP tools for agent orchestration` | E6 | F4 | MIG-32 | 1 |

---

## 15. Anexos

### A — Comandos de verificación

```bash
# baseline reproducible (read-only)
python3 anexos/audit_webfang.py <ruta-repo>            # informe Markdown
python3 anexos/audit_webfang.py <ruta-repo> --json     # métricas crudas

# coherencia del bump
bash scripts/check_toolchain_drift.sh
bash scripts/test_toolchain_drift.sh
bash scripts/check_docs_version_pins.sh

# gate completo (el espejo local de CI)
cargo check --workspace --all-features
cargo clippy --all-targets --all-features -- -D warnings \
  -W clippy::cognitive_complexity -W clippy::too_many_lines
cargo fmt --all -- --check
env RUSTDOCFLAGS=-D warnings cargo doc --workspace --all-features --no-deps
bash scripts/ci_fast_gate.sh

# migración de edición, por crate
cargo fix --edition --all-features --allow-dirty -p webfang_test_utils
# … luego el resto en orden topológico, y solo entonces edition = "2024"

# paridad funcional
bash scripts/check_compatibility.sh
bash scripts/check_windows_platform.sh
cargo nextest run --test corpus_determinism_test
cargo insta review
```

### B — Configuración propuesta

Ficheros listos para revisar en `anexos/config/`:

- `rust-toolchain.toml` — canal 1.100.0 con el comentario de por qué el pin es exacto.
- `cargo-config.append.toml` — bloque `[registry] global-min-publish-age` para añadir a `.cargo/config.toml`.
- `workspace-lints.append.toml` — bloque `[workspace.lints.cargo]` para el `Cargo.toml` raíz.
- `ci-warnings.append.yml` — patrón de `env:` por job para `CARGO_BUILD_WARNINGS=deny`.

### C — Superficies de edición exactas del bump

| Fichero | Qué cambia |
|---|---|
| `rust-toolchain.toml` | `channel` → `1.100.0` (+ comentario) |
| `mise.toml` | `rust` → `1.100.0` (+ comentario del parche exacto) |
| `Cargo.toml` | `rust-version` → `1.100`; `edition` → `2024` (en F2); `[workspace.lints.cargo]` (RF-10) |
| `.github/workflows/ci.yml` | `toolchain:` del job `mcp` → `1.100.0`; `env:` de warnings; comentarios de MSRV |
| `.github/workflows/release.yml` | 2 × `toolchain:` → `1.100.0`; nota de prerequisitos de la release |
| `.cargo/config.toml` | `[registry] global-min-publish-age` |
| `AGENTS.md`, `README.md`, `docs/src/installation.md`, `docs/src/overview.md` | pins de versión (con autorización para `docs/`) |
| `scripts/seed_target.sh`, `seed_recipe.sh`, `seed_publish.sh` | comentarios de política + re-medición de la poda |
| `crates/*/Cargo.toml` | nada (heredan `edition.workspace`), salvo `fuzz/` y `tools/intra-crate-lint/` |

### D — Fuentes primarias

- Rust 1.100.0 (2026-11-12): notas de release y changelog — never type estable, `core::mem::DropGuard`,
  `str`/`slice`/`Path::trim_prefix`, allocator API, `[lints.cargo]` + `cargo::unused_dependencies`,
  cooldowns (RFC 3923), nuevo layout de artefactos intermedios; notas de compatibilidad (fallback `!`
  en todas las ediciones, `Infallible` alias, captura en `if let`, `Box` con allocators, Unicode 18).
  <https://releases.rs/docs/1.100.0/>
- Rust 1.96.0 (2026-05-28): `assert_matches!` / `debug_assert_matches!`. <https://blog.rust-lang.org/2026-05-28/Rust-1.96.0/>
- Rust 1.97.0 (2026-07-09): `CARGO_BUILD_WARNINGS`. <https://blog.rust-lang.org/2026-07-09/Rust-1.97.0/>
- Rust 1.99.0 (2026-10-01): `String::from_utf8_lossy_owned`. <https://blog.rust-lang.org/2026-10-01/Rust-1.99.0/>
- Rust 1.93.0 (2026-01-22): `VecDeque::pop_front_if` / `pop_back_if`. <https://releases.rs/docs/1.93.0/>
- Rust 1.92.0 (2025-12-11): `never_type_fallback_flowing_into_unsafe` y
  `dependency_on_unit_never_type_fallback` pasan a deny-by-default.
  <https://blog.rust-lang.org/2025-12-11/Rust-1.92.0/>
- RFC 3923 — Cargo min-publish-age. <https://rust-lang.github.io/rfcs/3923-cargo-min-publish-age.html>
- `dtolnay/rust-toolchain`: rama `1.100.0` = `9522411a9bce8484da960823fc2f46e52f981f71` (verificado por
  `git ls-remote`); la rama de versión hardcodea `env: toolchain: 1.100.0` y **no** expone el input.
- Repo: `AGENTS.md`, `COMPATIBILITY-MATRIX.md`, `docs/ci-slo.md`, `docs/adr/0004-onnx-local-removal.md`,
  `docs/adr/0020-shared-llm-transport-primitives.md`, `.github/support.json`,
  `scripts/check_toolchain_drift.sh`, `scripts/breaking_surface_lib.sh`, `clippy.toml`.

---

## 16. Aprobaciones

| Rol | Decisión requerida | Estado |
|---|---|---|
| Maintainer / owner | Aprobar F0–F3 y el destino 1.100.0 | ☐ |
| Maintainer | ¿F1 arranca ya o tras la ventana de 14 días (2026-11-26)? | ☐ |
| Maintainer | MSRV bump como **8.0.0** (ruptura) y política de backports | ☐ |
| Maintainer | Autorización para tocar `docs/` en MIG-04 | ☐ |
| Maintainer | Ratchet nuevo (`check_api_surface_ratchet.sh`) sí/no | ☐ |
| Maintainer | ADR-0023: seam MCP fuera de proceso como recomendación | ☐ |