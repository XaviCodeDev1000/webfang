# Ola 1 — adapters/downloader/mod.rs (paraguas #2584)

Misión del programa `odd/tasks/file-size-reduction.md` (T2 — Ola 1). Monstruo libre
(verificado: ningún worktree activo toca `adapters/downloader/`; `ola1-waf-engine` y
`ola1-mcp-scraping` están ocupados con otros archivos).

- Branch: `refactor/ola1-downloader-mod` · Worktree: `webfang-worktrees/ola1-downloader-mod`
- Base: `main` @ `1d7b5adf` (post-merge #2810)
- Ratchet: `scripts/check_file_size.sh`; entrada baseline: 2008 líneas

## Estructura medida (2026-11-15, main @ 1d7b5adf)

Total **2008 líneas**:
- Producción L1–L1040 (**1040 líneas**): consts/ledger/guard (L23–L122), tipos pub
  `DownloadedAsset`/`DownloadConfig` (L127–L219), `Downloader` + impl (L222–L878),
  helpers libres filename/filters/backoff (L879–L1040)
- Tests inline: `mod tests` L1041–L1676 (**636**), `mod memory_probe_tests` L1677–L1725
  (**49**), `mod bounded_cache_tests` L1726–L2008 (**283**)

## Estrategia (precedente export_factory: slices chicos por el budget de review)

- **Slice 1**: tests inline → hijos por tema (`tests_filename.rs`, `tests_filters.rs`,
  `tests_download.rs` (wiremock), `tests_cache.rs` = bounded_cache_tests, `tests_memory_probe.rs`,
  `tests_common.rs` si hay base compartida). ~968 líneas fuera ⇒ hub ≈1040.
- **Slice 2**: helpers de filename/filters/backoff (L879–L1040, ~160) → `naming.rs`.
- **Slice 3**: `Downloader` cache/eviction machinery (InsertionLedger, InFlightGuard,
  insert_cached_cell, evict_over_capacity) → `cache.rs`.
- **Slice 4+**: download_once/generate_filename → `fetch.rs`; hasta `--remove` (≤500).

Cada slice = commit propio; verificación por slice: `--list` invariante, nextest lib,
clippy `-D warnings`, fmt, file-size. RDD por slice con baseRef = commit anterior.

## Tasks

| ID | Tarea | Estado | Commit |
|----|-------|--------|--------|
| T1 | worktree + exploración + registro | done | — |
| T2 | Slice 1: tests inline → hijos ≤500 | pending | — |
| T3 | Slice 2: naming.rs (filename/filters/backoff) | pending | — |
| T4 | Slice 3: cache.rs (ledger/guard/eviction) | pending | — |
| T5 | Slice 4+: fetch.rs hasta --remove | pending | — |
| T6 | push + PR (confirmación explícita, S3) | pending | — |

## Log

L1 — Pedido (verbatim, 2026-11-15): "OK, Arrancamos la misión de downloader/mod.rs en un
worktree nuevo". Elegido porque server.rs (2012) y downloader/mod.rs (2008) son los dos
monstruos libres; downloader/mod.rs primero (webfang_core, método probado, el más chico).

## Log (continuación)

L2 — Slice 1 (`5ee3634c`): 3 mod tests inline (968 líneas) → 6 hijos (tests_config 118,
tests_naming 286, tests_download 251, tests_memory_probe 43, tests_bounded_cache 278,
tests_common 8 con `vu`). mod.rs 2008→1052. Nota entorno: /tmp (tmpfs 16G) se llenó con
el build de btls-sys del target propio → se usó CARGO_TARGET_DIR compartido de main
(`~/.cache/cargo-target/main`) con TMPDIR reubicado; seed_target cold (no-seed).
L3 — Slice 2 (`24fdd812`): helpers naming/filters/stream (162) → naming.rs (178). mod.rs →895.
L4 — Slice 3 (`3b4a82ec`): maquinaria de cache acotada (InsertionLedger/CellState/
InFlightGuard/capacity) → cache.rs (92). mod.rs →810.
L5 — Slice 4 (`1b6ba14f`): pipeline de descarga (download/retry/once/batch/
generate_filename, 398) → fetch.rs (421) como `impl Downloader` en hijo.
**mod.rs 810→398 ≤500 ⇒ `--remove` del baseline.** Verificación final: --list 36=36,
nextest lib 3012/5/0, clippy -D warnings 0, fmt OK, workspace check 0 errores,
file-size verde. Rama lista para RDD por slices + push + PR.
