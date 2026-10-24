# nextest-bump-flaky-result — bump a 0.9.131 + flaky-result=fail (#2306)

Issue: #2306 (`status:approved`), completa el paso 3 de #2305. Worktree:
`chore-nextest-bump-0.9.131`. Base: `15d4436d` (main post-#2307).

## Specs

- S1 (#2306): "nextest >= 0.9.131 pineado en mise.toml y en todos los
  `taiki-e/install-action` de los workflows, con el MSRV del workspace
  consistente."
- S2 (#2306): "`flaky-result = "fail"` activo en `[profile.default]` y
  `[profile.ci]` (paso 3 de #2305), verificado corriendo la lane."
- S3 (#2306): "Con `inherits` disponible, evaluar colapsar las copias de
  perfiles (ci, flake-nightly) en herencia."
- S4 (#2306): "#2305 puede cerrarse."
- S5: MSRV real verificado contra el changelog: nextest 0.9.129 subió el
  MSRV a Rust **1.91** (0.9115 lo había llevado a 1.89; flaky-result entró
  en 0.9.131, después del salto). Latest 0.9.149, también 1.91. Pin
  elegido: **0.9.131** (mínima que cumple, mismo MSRV que latest).
- S6: semántica de `inherits` (doc oficial): la cadena se recorre para cada
  clave; los overrides del padre se consultan a través de la cadena. Los
  overrides viven SOLO en `default`; ci y flake-nightly no declaran los
  suyos, heredan.

## Tasks

- [ ] T1 · Toolchain 1.88 → 1.91: mise.toml, rust-toolchain.toml, Cargo.toml
  (rust-version), AGENTS.md (Stack), README (badge/Requires/Stack),
  docs/src/installation.md ×2 + overview.md (sólo la versión, líneas
  mecánicas), comentarios ci.yml (:255, :1720)
- [ ] T2 · nextest 0.9.114 → 0.9.131: mise.toml:19 + 6 pins de ci.yml
  (test-core, test-full, test-crossplatform, tokio-console-smoke,
  feature-matrix, flake-detection) + comentarios SC-11 ×4 reescritos
- [ ] T3 · .config/nextest.toml: `flaky-result = "fail"` en default;
  `inherits` en ci (solo diferencias intencionales) y flake-nightly
  (inherits ci + retries = 0); comentarios reescritos
- [ ] T4 · Validación liviana: binario 0.9.131 prebuilt (sin warning de
  claves; inherits parsea), `cargo fmt` con 1.91, gates offline
- [ ] T5 · PR: Closes #2306 + Closes #2305, label type:chore

## Log

- L1: "si adelante" (usuario, autorizando encolo #2307 y arranco #2306).
- L2: #2307 mergeado por la cola (squash 15d4436d, 19:39Z); post-merge
  runbook ejecutado (main ff, worktree/branch/prune limpios; target dir en
  ventana móvil).
- L3: binario cargo-nextest 0.9.131 prebuilt en /tmp/wf-nextest-131 para
  validar config sin compilar; Rust 1.91.0 preinstalado vía rustup.
- L4: El gate de docs version pins sólo controla la versión del PAQUETE,
  no el MSRV: no se dispara. Menciones narrativas de "Rust 1.88" a
  actualizar: README ×3, docs/src/installation.md ×2, docs/src/overview.md
  ×1 (docs/book/ es generado: no se toca).
- L5: Drift de patch detectado y corregido: mise resolvió rust="1.91" a
  1.91.1 mientras el channel decía 1.91.0 — el pin va exacto e idéntico en
  ambos (con 1.88 era imposible: un solo patch).
- L6: PR #2310 creado (Closes #2306 + #2305). Primer run de CI: 4 jobs
  rojos con dos causas. (a) trybuild: 4 snapshots .stderr desactualizados
  bajo rustc 1.91.1 (el label de E0599 ahora qualifica el path y el
  unused_imports agrega su grupo) — regenerados extrayendo los bloques
  ACTUAL OUTPUT del log de CI con calibración byte-exacta contra los
  commiteados. (b) flake destapado por el enforcement:
  crawl_site_with_options_span_records_outcome_and_summary_duration_ms
  TRY 1 FAIL → TRY 2 PASS en 2/4 lanes; cuarentena vía override per-test
  flaky-result="pass" + issue #2313 (el try-1 output no se imprime para
  tests que acaban pasando: la causa se investiga allá).
- L7: actionlint reporta shellcheck preexistente en release/sanitizers
  idéntico en main (diff verificado): no son míos ni bloquean CI real.
