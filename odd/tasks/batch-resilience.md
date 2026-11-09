# batch-resilience — export incremental y resume de batch

**Objetivo:** cerrar #2609 y #2611 — el batch persiste/exporta solo al cierre limpio; matar una corrida
larga pierde todo. Slice 1 (`recover` + librería) → merge → slice 2 (flush incremental + resume).
**Branch:** `feat/batch-resilience` · **Worktree:** `~/Projects/Rust/webfang-worktrees/feat-batch-resilience`
**Entrega:** secuencial (sin PRs apilados), `Closes part of #2611` (slice 1), `Closes #2609 + Closes #2611` (slice 2).
**Runner:** `cargo nextest run --test <name>` · Gate: `bash scripts/ci_fast_gate.sh` + coverage report.

## Specs

S1 (#2611, verbatim): «El flujo de batch captura cada página en el spool .webfang-batch-capture.jsonl de
forma incremental, pero la conversión a markdown y la escritura de archivos ocurre solo al cierre limpio
del batch.»
S2 (#2611, verbatim): «Export por checkpoint: cada N páginas capturadas (flag `--export-checkpoint <N>`,
default razonable tipo 50) correr la extracción+escritura de las páginas nuevas del spool y marcarlas
como exportadas.»
S3 (#2611, verbatim): «un subcomando/utilidad de recuperación (`webfang recover --spool <file> -o <dir>`)
que convierta un spool existente a markdown con el mismo pipeline (mismo extractor, mismo frontmatter)».
S4 (#2611, verbatim): «En graceful shutdown (SIGINT/SIGTERM), exportar el spool pendiente antes de salir.»
S5 (#2609, verbatim): «Si batch soporta resume: persistir el estado de URLs procesadas en el state-dir
conforme avanza el batch (o al menos por chunk/cierro limpio), y que el relanzamiento salte las ya hechas.»
S6 (diseño aprobado v7, resumen ejecutivo): librería `convert_spool` única (recover/flush/salvage);
watermark = bytes durables del spool (`durable_bytes`, flush por página; `flush()` entrega al SO, no es
fsync); sidecar `.webfang-batch-capture.progress.json` escrito al arrancar (offset 0) con identidad del
spool (`first_line_sha256: Option`), `output_dir` y `ExtractionOptions` via `From<&ScraperConfig>` sin
`..`; orden por URL `.md` → COMMITTED → watermark; fallo de página avanza watermark, fallo de E/S no y
bloquea discard; namespace `batch-<hash8(canonical_output_key(output_dir))>`; auto-salvage solo con
sidecar `Parsed` válido, todo lo dudoso → `OrphanSpool` (nunca borrar con duda); arranque unificado:
open(create+append sin truncar) → try_lock → decide → mismo fd pasa al sink (`set_len(0)` tras salvage);
OrphanSpool crea spool nuevo en la ruta original; `CliExit::Busy` → 75 (EX_TEMPFAIL) tras verificar
variantes existentes; cierre con timeout 30 s y mensaje explícito de spool conservado.
S7 (test (a), invariantes aprobadas): (i) toda URL committed tiene su `.md`; (ii) toda línea
pre-watermark exportada y parsea; (iii) committed ⊆ pre-watermark ∪ lote en curso; (iv) cero doble
descarga (hits wiremock == 1 por URL). Kill sincronizado esperando N `.md`, nunca el reloj.
S8 (descopeado): slice 3 — default `XDG_STATE_HOME` (fallback `data_local_dir`), unificación de los 3
constructores de state-dir, migración backup-first, issue propio con `status:approved`. Follow-up
registrado, sin compromiso.
S9 (ascunción marcada como tal): el `content_hash` de un COMMITTED de flush es el sha256 de los bytes
del `.md` escrito (análogo al flush-proof del export.jsonl). Verificar en WU contra el uso que hace
`CommitSession::decide` del hash_index.

## Tasks

T1 (S1,S3) `application/batch/spool_export.rs::convert_spool` — extract_batch_content delega. Route: inline. Commit: 2aaa44d6
T2 (S3) subcomando `recover` (args/mod.rs, dispatch main.rs, commands/recover.rs, exit codes 74/3/69+strict). Route: inline. Commit: 485c13f7
T3 (S3) tests recover: fixture→.md idénticos a batch, corrupta, inexistente(74), --strict, offset>0, snapshot. Route: inline. Commit: 485c13f7
T4 (S1) gate completo + ci_fast_gate + PR slice 1. Route: inline. Commit: <sha>
T5 (S6,S9) WU1: progress.rs + adquisición unificada (fd lockeado, tabla colapsada, Busy→75). Route: inline. Commit: <sha>
T6 (S6) WU2: flush por página + durable_bytes. Route: inline. Commit: <sha>
T7 (S2,S4,S6) WU3: --export-checkpoint + tarea de flush (watch, tick 120s, spawn_blocking, cierre timeout 30s). Route: inline. Commit: <sha>
T8 (S5,S6) WU4: resume (namespace batch-<hash8>, filter_committed, CommitSession, recover+sidecar). Route: inline. Commit: <sha>
T9 (S6,S7) WU5: export_phase incremental + tests (a)-(e) + verifier independiente. Route: inline. Commit: <sha>
T10 (S1) gate + PR slice 2 (Closes #2609, Closes #2611). Route: inline. Commit: <sha>

## Log

L1 (2026-11-09, verbatim): «adelante» — autorización de implementar el plan batch-resilience v7
(recuperación de spool varado + flush incremental + resume de batch), tras auditoría que verificó
#2609/#2611 contra el repo y 5 rondas de review del diseño.
L2 (2026-11-09, maintainer, feedback plan v1): 8 puntos — opciones de extracción viajan con el spool;
watermark por BYTES (no líneas); orden `.md`→COMMITTED→watermark; flush fuera del loop (spawn_blocking);
verificar is_committed_proven y riesgo de discovery truncado → namespace propio; normalización de URLs
sucias en test; lock/PID para spool concurrente; flush por tiempo además de por conteo. Slice 3
descopeado a follow-up.
L3 (2026-11-09, maintainer, feedback v2): sidecar registra output_dir+opciones; SIGKILL vs BufWriter
(punto 1); COMMITTED ≠ exportado a este output dir → `batch-<hash8(output_dir)>`; sidecar al arrancar;
watch en vez de canal acotado; spike de reconcile día 1; recover+resume → follow-up --commit-state;
crash-windows con puntos de inyección; flock advisory documentado; timeout con spool conservado;
Risk slice 1 = Low.
L4 (2026-11-09, maintainer, feedback v3): contradicción del test (a) corregida (invariantes S7); spool
sin sidecar utilizable → OrphanSpool (no salvage con -o ajeno); orden de apertura sin O_TRUNC
(create+append → try_lock → set_len(0)); fstat/stat antes de unlink; warn de orphans con ruta y tamaño;
exit de conflicto = EX_TEMPFAIL (75), no 78; cada commit pasa -D warnings (decisión de dead_code en WU1).
L5 (2026-11-09, maintainer, feedback v4): SalvageFromStart → OrphanSpool (sidecar que no describe el
spool no elige destino; offset>len solo por manipulación externa); slice 1 solo flags→defaults (sidecar
llega en slice 2); tabla de decisión sweep/salvage por estados (sidecar Parsed/Unreadable/Absent).
L6 (2026-11-09, maintainer, código): presentó `batch_progress.rs` (sidecar v1 + decide_sweep puro, 5
tests OK en crate externo) — adoptado; `first_line_sha256: Option` (spool vacío al arrancar); orden de
arranque: spool+flock ANTES del sidecar; sin deny_unknown_fields; decisión OrphanSpool aceptada.
L7 (2026-11-09, maintainer, feedback v5): sweep y apertura comparten EL MISMO fd lockeado (decide y
escribir sin soltar lock; OrphanSpool crea spool nuevo; fstat/stat nativo); cada commit de slice 2 pasa
-D warnings (WU1 consume todo el módulo); exit 75 verificado contra variantes CliExit existentes.
L8 (2026-11-09, spike #0): D2/COMMITTED = status + output_location + content_hash + sin last_error +
attempts≥1 — todo batch-producible via cadena de `CommitSession::commit_item`; infra `cli/crash_points`
existe para tests de crash-windows; store de batch no se lee a mitad de corrida (hoy no-op). VERDE.
L9 (2026-11-09, implementación T1-T3, desvíos menores del plan y hallazgos): (a) convert_spool quedó
dividido en `convert_spool` (puro, sin escritura) + `save_converted` (escritura) — extract_batch_content
NO debe escribir (eso vive en export_phase) y el doble-save rompería el assert anti-duplicación del
slice 2; recover compone ambas. (b) Fixtures de test requieren cuerpo sustancial: el guard de contenido
mínimo #706 rechaza páginas chicas en la rama de éxito de Readability. (c) HALLAZGO ENTORNO:
`cargo doc` bajo el shim de mbx (~/.local/share/mbx/bin/cargo) ICEa deterministamente
(scoped-tls panic en rustdoc CrateInfo::read_many); con el cargo real (~/.cargo/bin/cargo) compila
limpio. Diagnóstico: árbol limpio en HEAD también ICEa; main solo "pasaba" por cache. Todos los `doc`
locales de esta sesión corren con cargo real. (d) Reader: `read_until` reemplaza `Lines` — UTF-8
inválido ahora es Codec (corrupto-skipeable) y no Io (fatal): cambio aprobado por el diseño
"decodificación tolerante". T1: 2aaa44d6 (+492/−44). T2/T3: 485c13f7 — 6/6 tests recover incl. árbol
idéntico a export batch real; snapshot aceptado.
