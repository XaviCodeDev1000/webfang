# Ola 2 — misión propia: `wreq_downloader.rs` (alto riesgo)

**Objetivo:** sacar `infrastructure/downloader/wreq_downloader.rs` de la banda `[1500, 2000]` del ratchet sin tocar el orden de la cadena de guards de fetch.

Rama: `refactor/ola2-wreq` · worktree: `~/Projects/Rust/webfang-worktrees/refactor-ola2-wreq` · base: `origin/main@c48f9f85` · runner declarado: `cargo nextest`. Entrega secuencial, un commit unitario por slice, PR propio con `Closes part of #2584`, `type:refactor`, base `main` (autorización explícita antes de push/PR).

## Specs

**S1 — Cadena de guards, inviolable.** `fetch_inner` (`WreqDownloader::fetch_inner`) conserva el orden entry → pacing → timeout/redirect/SSRF en cada dial → clasificación de reintento → lectura con cap, verbatim y contiguo, incluido `#[allow(clippy::too_many_lines)]` y sus comentarios. Ningún corte atraviesa esa función; los slices sólo mueven código que no altera ese cuerpo.

**S2 — Slice 1: tests por tema.** Región de tests medida en el árbol del corte: `test_support` 757–876 (120L, 6 helpers `pub(super)`), `tests` 877–999 (123L, 11 tests), `wiremock_tests` 1000–1853 (854L, 23 tests). Extraer a hijos `wreq_downloader/<tema>.rs` ≤500: `test_support.rs`, `tests.rs`, `wiremock_fetch_tests.rs` (1000–1430) y `wiremock_retry_tests.rs` (1432–1853). Declarar en el padre con `#[cfg(test)] mod …;`. Renombres de ruta de tests declarados en el PR; conjunto de hojas y conteo total invariantes.

**S3 — Slice 2 (sólo si hace falta): producción.** El resto (1–756) incluye `mod retry`/`mod wreq_body`, constantes, `lowercase_headers`, el struct, el builder `new()`, cookie parsing (`extract_cookies` + `parse_set_cookie`) y la cadena de guards. Candidato cohesivo a extraer: cookie parsing (~83L, funciones puras). NO tocar `fetch_inner`.

**S4 — Ratchet y golden.** `--update` descendente en el mismo commit; golden `real_tree_sites.txt` refrescado POST-fmt en el mismo commit; `check_file_size`, `check_test_inventory`, `check_ignored_guard`, `check_duplication`, `check_dead_tests`, direcciones ×2.

**S5 — Cuatro lentes + RDD.** Todo corte en este archivo es ítem de alto riesgo (AGENTS.md § Fetch guard chain; doc de la ola S8). Cuatro lentes explícitos (risk, resilience, readability, reliability) y ciclo nativo sobre el commit unitario cuando `review_due` lo pida. Consentimiento por candidato: nunca fabricado.

**S6 — Compuertas.** check · clippy `--all-targets --all-features` estricto · `fmt --check` · doc con `RUSTDOCFLAGS=-D warnings` · `--list` antes/después con el mismo runner · suite completa · `ci_fast_gate.sh` GREEN con reporte leído.

## Tasks

| ID | Tarea | Specs | Ruta | Estado | Evidencia |
|----|-------|-------|------|--------|-----------|
| W0 | Worktree + doc + índices | S1 | inline | done | proxy OK, codegraph+codedb en `refactor-ola2-wreq` |
| W1 | Captura `--list` base y censo de seams | S2,S6 | inline | done | 3111 líneas, 29 tests wreq |
| W2 | Slice 1: extraer 4 hijos de tests | S1,S2 | inline | done | ver L2 |
| W3 | Verificación completa y ratchet | S3,S4,S6 | inline | done | ver L2 |
| W4 | Commit unitario + cuatro lentes + RDD | S5 | inline | in progress | — |

## Log

- **L1 (2026-11-17, verbatim del pedido)**: "`wreq_downloader.rs` (misión de alto riesgo, cuatro lentes + RDD)".
- **L0 (2026-11-17, contexto)**: quinto y último ítem pendiente del programa #2584; el doc de la ola lo tiene como O2-T5, diferido originalmente por la cadena de guards y por el área recién movida por #2880.
- **L2 (2026-11-17, slice 1 ejecutado)**: `wreq_downloader.rs` 1853 → **764** líneas. Movimiento mecánico de los tres módulos de tests a cuatro hijos: `test_support.rs` (111), `tests.rs` (120), `wiremock_fetch_tests.rs` (428) y `wiremock_retry_tests.rs` (435). El bloque `wiremock_tests` (854L) se partió en el límite temático entre fetch/cabeceras/SSRF/cookies y reintento/rotación/cap de body; el helper compartido `RawHeaderEquals` se re-calificó a `pub(super)` (struct + 2 campos) en el primer hijo y se importa explícitamente en el segundo — única excepción al diff de visibilidad, declarada. Los banners `#2235` viajan con sus módulos. **`fetch_inner` y la cadena de guards quedaron byte-idénticos** (S1). Verificación: `--list` 3111=3111 con **mismo conjunto de nombres de hoja** (29 tests wreq) y 23 renombres de ruta declarados (`wiremock_tests::*` → `wiremock_fetch_tests::*` / `wiremock_retry_tests::*`); nextest `-p webfang_core --lib` **3111 passed / 6 skipped / 0 failed**; check `--all-targets --all-features`, clippy estricto, fmt, doc, dirección intra-crate, `check_ignored_guard`, `check_test_inventory`, `check_dead_tests`, `check_duplication` OK; ratchet `--update 1856→764` y golden refrescado (+15 sitios) en el mismo commit post-fmt. Sigue: W4 (cuatro lentes + RDD) y slice 2 opcional de producción (`extract_cookies` + `parse_set_cookie`, ~83L puras) si se quiere bajar por debajo de 500.
