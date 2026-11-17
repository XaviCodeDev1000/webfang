# Obscura L2 honesty — Phase 1 of #2722

Objective: make Layer 2 (Obscura) stop lying about URL/identity and classify its own
output, without touching REQ-OBS-03 or the SSRF posture. Branch `feat/obscura-l2-honesty`
off main (9ebcecbf). Delivery: `ask-on-risk` (no push/PR unless asked). Test runner:
`cargo nextest run --test <name>` + unit tests in-module.

## Specs

Requirements from issue #2722 (Phase 1, H3+H4+H5) as corrected by the 2026-11-17
verification against upstream v0.2.6/v0.2.7 (Engram `obscura/issue-2722-analysis`).
Verbatim user strings are quoted; corrections are marked [CORRECCIÓN].

- **S1 — UA propagation (H5, recortado)**: issue verbatim: "propagate
  `--user-agent`, `-H/--header`, `--accept-language`, `--allow-private-network`
  to `obscura fetch` (no `--cookie`)". [CORRECCIÓN] upstream no tiene
  `--header`/`--accept-language`/`--cookie` ni flag de red privada en el fetch
  renderizado (verificado en el struct `Fetch` de v0.2.6/v0.2.7); webfang no tiene
  flag de red privada que propagar (y relajar SSRF en L2 contradecería el guard).
  Alcance real: propagar **solo `--user-agent`** cuando el operador lo fijó
  (`DownloaderSpec.user_agent = Some`); `None` mantiene el comportamiento actual
  (obscura usa su UA por defecto).
- **S2 — URL final (H4)**: issue verbatim: "derive real `FetchedPage.url` (final)
  and `status` from Obscura's `Page loaded: <url> - \"<title>\"` stderr or batch
  JSON". [CORRECCIÓN] el status NO se reporta en el fetch renderizado (el batch
  JSON es solo `--dump original`, sin browser). Alcance real: parsear **solo la
  URL** de la línea stderr `Page loaded: <url> - "<título>"` (formato verificado
  en el binario 0.2.7 y en el source v0.2.6/v0.2.7). URL absoluta http(s) → se usa
  como `FetchedPage.url`; parse fallido → se conserva la URL pedida + WARN (la
  fetch nunca falla por una línea de stderr ausente).
- **S3 — status honesto**: el `status` de L2 sigue siendo 200 sintetizado — upstream
  no lo expone para el fetch renderizado. Se documenta en el módulo y se marca en
  el log estructurado (`status_source = "synthesized"`). Feature-request upstream
  (envelope JSON o línea de status) queda como follow-up, no en este slice.
- **S4 — body cap (hallazgo nuevo, etapa 5 del guard chain)**: verbatim del
  análisis: "ObscuraDownloader::fetch_inner lee el stdout del hijo con
  `wait_with_output()` SIN cap — viola la etapa 5". El stdout se lee con cap
  `max_page_bytes` (default `DEFAULT_MAX_PAGE_BYTES`); excedido →
  `DownloadError::BodyTooLarge { limit }`. El stderr se lee con un cap pequeño
  auxiliar (64 KiB, truncado, nunca fatal).
- **S5 — clasificación del body L2 (H3)**: issue verbatim: "body-classify L2 HTML
  for WAF/SPA and propagate `WafChallenge`". El router ejecuta la MISMA
  `evaluate_fetch` (SPA+WAF sobre el body) sobre la página de L2:
  `WafBlocked` → `WafChallenge` (aborta, respeta `ignore_waf` REQ-WAF-07),
  `SpaDetected` → escala a L3, `StaticContent` → acepta.
- **S6 — logs estructurados**: issue verbatim: "emit structured log fields (final
  url, status, layer) so the jq hit-rate stops being fragile". El mensaje existente
  `Layer 2 returned N bytes` conserva su texto (el jq de docs/ lo consume) y gana
  campos estructurados (`layer`, `final_url`, `status`, `status_source`).
- **S7 — guardas de scope (intactas)**: REQ-OBS-03 (sin cookies, sin `--storage-dir`,
  L2 sessionless), sin `--obey-robots`, sin exigir build stealth, piso de código
  0.2.5 sin cambio, `wreq` solo en L1. Sin cambios en `docs/` (regla del repo).

Acceptance criteria:
- Los tests unitarios de `obscura_downloader` pin: argv con `--user-agent` solo
  cuando está pinned; `FetchedPage.url` = URL final del stderr; fallback WARN;
  `BodyTooLarge` al superar el cap; cookies vacías y headers text/html intactos.
- Los tests del `HybridRouter` (stubs) pin: L2 con body de challenge → `WafChallenge`
  sin llamar L3; L2 con body SPA → escala a L3; L2 limpio → acepta.
- Gate completo verde: `cargo check`, `cargo clippy --all-targets --all-features`,
  `cargo fmt --all -- --check`, rustdoc con `-D warnings`, `cargo nextest` (módulo
  afectado), `bash scripts/ci_fast_gate.sh` + lectura del coverage report.

## Tasks

- T1 — S1: `with_user_agent` en `ObscuraDownloader`, wiring `build_obscura_layer`
  ← `build_fetch_router` (Hybrid). Route: inline. RED: argv test.
- T2 — S2: parse `Page loaded:` del stderr con fallback WARN. Route: inline.
  RED: fixture stderr → url final.
- T3 — S4: lectura capped de stdout/stderr (`take`), `BodyTooLarge`. Route: inline.
  RED: fixture grande → BodyTooLarge.
- T4 — S5: `evaluate_fetch` sobre la página L2 en `hybrid_router::fetch_inner`.
  Route: inline. RED: stubs router.
- T5 — S6+S3: campos estructurados en los debug! de L2. Route: inline (viaja con
  los diffs de T2-T4). Sin test nuevo (los snapshots existentes cubren).
- T6 — fixtures/tests: `write_fake_obscura` emite `Page loaded:` opcional; argv
  assertion de UA; version echo del fixture a 0.2.7. Route: inline (viaja con T1-T3).

Risk: none — no toca datos persistidos, ni límites de seguridad (SSRF intacto: L2
no gana capacidad de dial privado), ni contratos públicos (`ObscuraDownloader` y
`build_obscura_layer` son `pub(crate)`); el cambio de `FetchedPage.url` de L2 es el
pedido explícito (H4, contrato ya documentado en el tipo); concurrencia sin cambio
(mismo patrón kill_on_drop); los tests descritos cubren las regresiones.

## Log

- L1 (2026-11-17): usuario: "vamos a analizar la issue #2722, consultando siempre
  la documentacion oficial antes de implementar" → análisis verificado contra
  upstream v0.2.6/v0.2.7 (3 correcciones a la issue + hallazgo del body cap;
  Engram `obscura/issue-2722-analysis`). Usuario: "adelante con la implementacion"
  → Phase 1 con alcance corregido (S1-S7 arriba). Phase 0 ya ejecutado (obscura
  0.2.7 instalado, preflight hybrid verde).
- L2 (2026-11-17): decisión de diseño registrada — el protocolo del
  `obscura-worker` (status real) queda FUERA de este slice: es un contrato no
  documentado upstream y acoplarlo contradiría la guarda "Obscura must remain
  replaceable"; se resuelve con S3 (status sintetizado documentado) + follow-up
  upstream. Evidencia: worker.rs v0.2.7 expone {title,url,status} pero sin
  contrato estable ni flag de allow-private-network en el worker.
- L3 (2026-11-17): worktree creado desde 9ebcecbf (main == origin/main), bootstrap
  completo (.envrc 5 líneas, direnv allow, .env copiado, codegraph + codedb
  indexados, seed cold exit 0). Identity proxy OK.
- L4 (2026-11-17) — T1-T3+T6 (obscura_downloader): RED observado (3 tests nuevos
  fallan contra el contrato actual: argv sin `--user-agent`, url pedida en vez de
  la final, stdout sin cap) → implementación → GREEN 16/16
  (`cargo nextest -p webfang_core --lib obscura_downloader`), incluye el pin
  XP-S-05 (`timed_out_fetch_kills_the_obscura_child`) con el nuevo patrón de
  lectura concurrente. Riesgo verificado: kill_on_drop preservado.
- L5 (2026-11-17) — T4-T5 (hybrid_router + fetch_router): RED observado (2 tests
  nuevos del router fallan; los 15 existentes siguen verdes — sin daño colateral)
  → implementación (evaluate_fetch sobre el body L2 + campos estructurados en el
  evento de aceptación) + wiring (build_obscura_layer con UA + cap) → GREEN
  185/185 en `cargo nextest -p webfang_core --lib downloader` (todo el módulo).
- L6 (2026-11-17) — gate pre-commit: `cargo fmt --all -- --check` GREEN (tras
  fixer), `cargo clippy --all-targets --all-features` con la tabla de lints GREEN,
  rustdoc `-p webfang_core --all-features -D warnings` GREEN.
- L7 (2026-11-17) — commits por unidad (identity proxy OK en ambos):
  - T1-T3+T6 → `5a68a29c` feat(http): propagate pinned UA, final URL and a
    stdout cap on obscura L2 (obscura_downloader.rs, +299/−21).
  - T4-T5 → `4c023be0` feat(crawler): classify the L2 body for WAF/SPA in the
    hybrid router (hybrid_router.rs + fetch_router.rs, +105/−9).
- L8 (2026-11-17) — verificación de integración: 72/72 en los 6 targets
  afectados (preflight_diagnostics, ssrf_layer2_coverage_test,
  engine_js_strategy_timeout_test, downloader_headers_test, waf_fixtures_test,
  integration_engine_tests), cero diffs de snapshot. Differential check: los
  símbolos modificados (ObscuraDownloader, build_obscura_layer, Hybrid L2 arm)
  solo alcanzan la estrategia Hybrid (callers verificados con codedb); los arms
  Static/Full y su llamada WreqDownloader::new quedaron byte-idénticos, y los
  tests de preflight/SSRF con binarios falsos ejercitan la ruta híbrida real
  contra la misma corpora en ambos lados del cambio.
- L9 (2026-11-17) — el gate local destapó 3 rojos del propio diff (corrección
  en la misma rama, sombreros separados por commit):
  - Ratchet de tamaño: obscura_downloader.rs (777), fetch_router.rs (517) y
    hybrid_router.rs (680 > baseline 623, grandfathered) → splits mecánicos
    por el seam real (módulo de tests a subdirectorio, patrón
    wreq_downloader/wreq_body): `0759c371` (338+438, golden refresh incluido),
    `91f01223` (fetch_router 298+217; hybrid_router 211+468 + --remove del
    baseline con la edición descendente sancionada + golden). 185/185 green
    entre commits.
  - jscpd: +4 líneas duplicadas sobre baseline (8223 > 8219) — clones arrange
    de los 8 tests fake-binary → helper compartido `fake_fetch` → 8178 ≤ 8219.
    `4c2294d1` (test refactor, mismo comportamiento, 16/16 green).
  - intra-crate lint: golden set de sitios 2263 vs 2262 (un sitio nuevo en el
    árbol) → refresh con el script sancionado, en el mismo commit que cada
    split que cambió el conteo (contrato del propio test).
- L10 (2026-11-17) — re-ejecución del gate local completa tras las correcciones:
  **fast-gate GREEN** (PASS=45 FAIL=0 SKIP=2 — ambos SKIP declarados: pinned
  lint por changed-scope y windows type-check sin módulos de plataforma
  tocados). Notas advisory (nunca rojas): 25 tests "sin referencia issue/PR" —
  falso positivo del checker por los archivos de tests nuevos (los nombres son
  pre-existentes movidos mecánicamente; los tests nuevos citan #2722 en sus
  docs). Oddity ambiental observada: rustdoc emitó ICEs internos
  (scoped-tls/search-index) durante la lane pero el paso rustdoc reportó PASS;
  el run directo previo sobre webfang_core fue limpio — no atribuible al diff.
- L11 (2026-11-17) — cierre: rama con 6 commits (5a68a29c feat L2 honesty
  downloader, 4c023be0 feat router classification, e4e34df5 docs misión,
  0759c371 split obscura, 91f01223 splits routers + ratchet --remove,
  4c2294d1 dedup tests). Delivery `ask-on-risk`: sin push ni PR — la apertura
  de PR queda para el maintainer (issue #2722 necesita status:approved antes
  de abrir PR por la gate de metadatos).
