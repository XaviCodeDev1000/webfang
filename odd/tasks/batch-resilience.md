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
T4 (S1) gate completo + ci_fast_gate + PR slice 1. Route: inline. Commit: 2258dfdf+dcee477e+2c44a317+66c34957 (gate local GREEN 40/0/2; PR #2631 abierto)
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
L10 (2026-11-09, T4 gate): `ci_fast_gate.sh` rojo en 6 pasos y cómo se cerró cada uno. (a) intra-crate
direction: `spool_export.rs` importaba `ObsidianOptions`/`markdown_relative_path`/`save_results` desde
infrastructure; fix 2258dfdf — `save_converted` se movió a `cli/commands/recover.rs` (application queda
pura), `SpoolExportConfig` perdió `output_dir`, `ConvertedPage` perdió `output_path`, y el variante
`OutputIo` se conserva porque el CLI lo construye. (b) file size: 4 archivos sobre el ratchet
(main.rs 1228>1212, args/mod.rs 1066>1042, bounded_sink.rs 1049>996, file_saver.rs 503>500); fix
dcee477e — splits a `provider_id.rs`, `args/commands.rs`, `bounded_sink/reader.rs`,
`file_saver/image_rewrite.rs`; baseline bajado a 1169/1014/961/963. (c) duplication 8352>8325; fix
dcee477e+2c44a317 (helper de tests): helper `recover_with_lines` en recover_test.rs y tests de spool_export
reescritos sobre `convert_lines`; quedó 8297≤8325. (d) test inventory desactualizado: regenerado en
66c34957. (e) nextest unit fallaba por link (`R_X86_64_32 ... recompile with -fPIC` en
`libbtls_sys`): el rlib envenenado lo había dejado el shim mbx; `cargo clean -p btls-sys` + cargo real
lo reconstruyó en 42 s y el gate corrió con `PATH=/home/xavi/.cargo/bin:$PATH`. Gate final: PASS=40
FAIL=0 SKIP=2, GREEN; cobertura leída (code lane: test-core partial y doc-quality partial son las
excepciones conocidas). Tests focalizados: spool_export 5/5, bounded_sink 13/13, image_rewrite 6/6,
recover_test 24/24, provider_id 3/3. PR slice 1 pendiente de autorización de push/publicación.
L11 (2026-11-09, cierre #2609 + arranque slice 2): #2609 cerrado con decisión de producto Camino 1
(persistir estado) — verificación experimental del 2026-11-09: `--batch-file --resume --state-dir`
escribe records COMMITTED en el state-dir y el relanzamiento salta las URLs (no re-descarga, no
re-exporta); `build_batch_resume_store` ya existía en main (rollup 20b27990). Camino 2 (exit 78)
descartado. Slice 2 (T5-T10) arranca desde main a875d9f5 (PR #2631 mergeado) en worktree
`webfang-worktrees/feat-batch-resilience`. Nota: el doc de especificación ya no incluye WU4 como
"implementar resume de cero" — el resume batch ya funciona en main; WU4 se reencuadra a: preservar
el resume existente (build_batch_resume_store → CommitSession) bajo el arranque unificado del
sidecar, y comprobar S9 (content_hash del COMMITTED de flush) contra `CommitSession::decide`.
L12 (2026-11-09, WU1 implementado — desvío declarado): `application/batch/progress.rs` (2e149d40):
BatchProgressV1 + SpoolIdentity + ExtractionOptions (selector es String en ScraperConfig, no Option —
desvío del plan v7 que asumía Option; se registra `Some(selector)` siempre) + SidecarState/LockState +
decide_sweep puro con la tabla colapsada + `spool_first_line_sha256` (hash de la primera línea con
`\n`; cola rota sin `\n` = sin identidad) + `orphan_spool_path` timestamped + 13 tests unit. El clamp
de watermark a `spool.len_bytes` está dentro de decide_sweep (aprobado: offset>len solo por
manipulación externa). Pendiente para WU1 completo: adquisición unificada del fd (open create+append →
try_lock → decide → mismo fd al sink) que sustituye a `sweep_stale_batch_spool`; se hace en el commit
siguiente junto con la integración CLI (WU1b).
L13 (2026-11-09, WU1b + corte de slice parcial): 3890d822 — adquisición unificada en `build_batch_sink`
(observe_spool_pair → decide_sweep → actuar sobre lo observado), orfan-quarantine con rename
timestamped (nunca borrar; rename fallido = run aborta con IoError, NUNCA trunca), disclosure español
apuntando a recover; el sweep viejo (`sweep_stale_batch_spool`, `StaleSpoolSweep`,
`STALE_SPOOL_MESSAGE`) y sus 4 tests fueron consumidos por el camino nuevo (sin dead_code). WU1
COMPLETO (T5). Slice acumulado 962 líneas > ~400 del corte: el flush por página (WU2) abre el
siguiente tramo de commits; batch_flow baseline del ratchet 963→1013 (crecimiento del orfan-camino).
L14 (2026-11-09, WU2 completo): 9a6e508e — `BoundedFileSink::durable_bytes` (Arc<AtomicU64>): el writer
flusha por página (canal, backlog y recv) y publica la longitud real del archivo (metadata().len())
con Ordering Release/Acquire; watermark en frontera de registro, monótono, 0 al arrancar; contrato
best-effort documentado (flush ≠ fsync). bounded_sink baseline 961→1103. 16/16 tests bounded_sink.
L15 (2026-11-09, WU3 core — flag + flush library): `cli/batch_flush.rs` — flush_spool_tail (tail
conversion via convert_spool + save_results en spawn_blocking; SpoolRead conserva el watermark del
llamador, OutputWrite también: retry re-exporta la misma cola atómicamente por archivo),
running/write/read/clear_sidecar (temp+rename), batch_namespace = batch-<hash8(output_dir)>.
Flag `--export-checkpoint <N>` (default 0 = desactivado) plumeado en 4 capas: options_spec/export.rs
(BATCH_CONCURRENCY→EXPORT_CHECKPOINT en GROUP, DefaultValue::Uint(0), uint_unbounded) → args/export.rs
→ args/mod.rs → BatchOptions.export_checkpoint. 5 tests batch_flush. Falta WU3-integración: la tarea
de flush (watch/tick 120s/spawn_blocking/cierre timeout 30s) que LLAMA a flush_spool_tail durante la
corrida, y WU5 (export_phase incremental + tests S7).
