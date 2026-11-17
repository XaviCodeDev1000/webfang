# Obscura CI hybrid job — slice 2 of #2722

Objective: a REQUIRED CI job installs a real Obscura binary and exercises the
hybrid path end to end. Branch `ci/obscura-hybrid-job` off main `aa7717ee`.
Delivery: `ask-on-risk`. Test runner: `bash scripts/test_ci_gate_structure.sh`,
`bash scripts/ci_fast_gate.sh` (lane `ci`), then the full pre-commit gate.

## Specs

- **S1 — real binary, pinned**: issue #2722 verbatim: "a CI job installs Obscura
  0.2.6 and exercises `hybrid` against a controlled URL". [CORRECCIÓN] v0.2.7 is
  the latest verified upstream release and the version the unit fakes already
  report; the job installs **v0.2.7** from the official release asset
  `obscura-x86_64-linux.tar.gz`, pins its SHA256
  `11b53d27cd743e00e196e216920e7f3f794cf378e3f1792b8d25d3beabaf9cad`, and
  asserts `obscura --version` prints `obscura 0.2.7`. Tarball layout verified:
  `obscura` + `obscura-worker` at the root.
- **S2 — hybrid exercised against a controlled URL**: write an inline JS-shell
  fixture (empty `id="root"`, under the 50-char content threshold) into
  `$RUNNER_TEMP/obscura-fixture/index.html`, serve it with `python3 -m
  http.server` bound to `127.0.0.1` on a fixed port, poll with `curl` until
  ready, then run `./target/debug/webfang <url> --js-strategy hybrid
  --obscura-binary <path> --trace-file <file> -o <dir>` with the three loopback
  SSRF hatches (`WEBFANG_DISABLE_SSRF_ENTRY_GUARD`,
  `WEBFANG_DISABLE_SSRF_RESOLVER`, `WEBFANG_DISABLE_SSRF_REDIRECT_GUARD`) plus
  `OBSCURA_ALLOW_PRIVATE_NETWORK=1` (Obscura has its own SSRF guard and refuses
  loopback without it). Assert the exported markdown contains the JS-only
  marker `OBSCURA_RENDERED_MARKER_HYBRID_7Q4` (proves real JS rendering, not a
  static fallback), the trace contains `obscura_variant_checked` (proves the
  real binary passed the preflight probe) and `Layer 2 returned` (proves L2
  served the page). Always kill the server, including on failure.
- **S3 — required producer, wired end to end**: user's verbatim scope choice:
  "Requerido en CI (Recomendado)". The job is in `gate.needs`, in the narrow-scope
  skip loop, gated by `run_code_jobs == 'true'`, added to `FULL_PRODUCERS` in
  `scripts/ci_plan.sh` (NOT `NARROW_PRODUCERS`), and declared as one row in
  `scripts/ci_fast_gate_coverage.tsv` with no local lane (CI-only job).
- **S4 — delivery hygiene**: no new `rust-cache` consumer (review-blocked per
  `ci.yml`), all actions SHA-pinned, no `docs/`, no `CHANGELOG.md`, no
  `continue-on-error` on a producer, no path filter that could keep the job out
  of a required context.

Acceptance criteria:
- `scripts/test_ci_gate_structure.sh` green: `gate.needs == {change-scope} ∪ FULL_PRODUCERS`, success/skip lists still partition it, coverage TSV matches exactly.
- `bash scripts/ci_fast_gate.sh` CI lane green with its coverage report read.
- Full pre-commit gate green (`cargo check`, strict clippy, `fmt --check`, rustdoc `-D warnings`).
- YAML/actionlint + shellcheck steps in `repo-guards` green.

## Tasks

- T1 — S1+S2: new `obscura-hybrid` job in `.github/workflows/ci.yml`. Route: inline.
- T2 — S3: wire `gate.needs`, narrow-scope skip loop, `scripts/ci_plan.sh`
  `FULL_PRODUCERS`, one row in `scripts/ci_fast_gate_coverage.tsv`. Route: inline.
- T3 — specs verification: structural tests, `ci_fast_gate.sh` CI lane, full
  pre-commit gate; re-read the diff. Route: inline.

Risk: item 5 (delivery/CI — modifies the merge gate in `.github/workflows/ci.yml`;
a mistake can block or falsely unblock merges). Independent verifier required
after the change's own checks.

## Log

- L1 (2026-11-17): usuario: "Slice 2 (job de CI que instala obscura y ejercita
  hybrid) sale de un worktree nuevo después de mergear #2881 — el delivery
  secuencial del repo lo exige, y toca .github/ que es de alta riesgo por la
  lista de tu AGENTS.md."
- L2 (2026-11-17): usuario: "ya están mergeadas las pr's" — verificado vía GitHub
  API: #2880 MERGED (8fa33402), #2881 MERGED (aa7717ee); main ff-only a aa7717ee;
  cleanup post-merge de ambos worktrees completado (obs #2307, 7 pasos).
- L3 (2026-11-17): usuario eligió alcance "Requerido en CI (Recomendado)" para
  slice 2 — autorización explícita para tocar `.github/`.
- L4: mapeo read-only (gentle-ai-explore): 5 lugares a sincronizar para un
  productor requerido — job + `gate.needs` + skip loop en `ci.yml`,
  `FULL_PRODUCERS` en `scripts/ci_plan.sh:26`, fila en
  `scripts/ci_fast_gate_coverage.tsv`. Required contexts se leen en vivo desde
  GitHub (`merge-when-green.sh`), no hay lista estática que editar.
- L5: spot check local confirmó el mapa y los nombres de flags: `--js-strategy`
  (`options_spec/crawler.rs:796`), `--obscura-binary` (`:838`),
  `--trace-file` (`:386`), `-o` (`cli/args/export.rs:12`). Asset v0.2.7 descargado
  y verificado: SHA256 `11b53d27…`, `obscura --version` → `obscura 0.2.7`.
- L6 (2026-11-17): corrección de S2 por evidencia del dry run local. Con
  `sync.html` el router emitió `SPA check: static content — no escalation needed`
  y L1 sirvió la página: el marcador JS NO apareció y no hubo evento `Layer 2
  returned`. `detect_spa` (`spa_detector.rs:191-238`) escala sólo con mount point
  SPA o <50 chars de texto de contenido; los fixtures F52 tienen demasiada prosa.
  Segundo dry run con un JS shell `#root` + `OBSCURA_ALLOW_PRIVATE_NETWORK=1`
  (el binario rechaza loopback con su propio guard: "Access to private/internal
  IP address 127.0.0.1 is not allowed") → `SPA detected (MountPoint("React
  #root"))`, `Layer 2 returned 768 bytes`, marcador presente. S2 reescrito con el
  fixture inline; no se crea archivo nuevo fuera de `crates/`.
- L7: verificación verde — `test_ci_gate_structure.sh` (23 aserciones + 7
  self-tests), `test_ci_plan.sh` (45 aserciones), `ci_fast_gate.sh` carril CI
  (25 PASS / 0 FAIL; coverage report nombra `obscura-hybrid` como no cubierto
  localmente), `actionlint`, `shellcheck`, `bash -n`, pre-commit completo
  (check/clippy/fmt/doc). `zizmor` sólo reporta un finding informativo
  preexistente en el job `notify`. Verificador independiente: S1-S4 PASS, cero
  blockers; advisories preexistentes por diseño.
