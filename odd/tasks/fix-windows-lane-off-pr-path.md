# #2342 — la lane cross-platform flaquea con un conjunto que se mueve entre runs

Worktree: `/home/xavi/Projects/Rust/webfang-worktrees/fix-windows-lane-off-pr-path` · Branch: `fix/windows-lane-off-pr-path`

## Diagnóstico (corregido contra `main` de hoy)

La issue nombra tres opciones y pide evidencia. La evidencia recogida cambia dos de sus
premisas:

1. **El compile-fail de `page_state` ya está cubierto.** `.config/nextest.toml`
   (`[profile.default]`, heredado por `ci`) da a
   `illegal_page_transitions_fail_to_compile` `600s` con `terminate-after 1`. Su intento
   de 186.8 s en el re-run queda **dentro** del techo, no contra él.
2. **Los `batch_*` ya están presupuestados.** Corren bajo
   `assert_spawn_within(SPAWN_LATENCY_BUDGET = 20s)` con un reintento propio del harness.
   Que aun así flakeen dice que el fallo no es «tarda más de lo presupuestado»: bajo carga
   el primer intento no completa y nextest lo salva en el reintento.
3. **El resto son tests que miden reloj de pared** (`status_429_with_future_http_date…`,
   `an_overrunning_attempt_is_abandoned_at_its_budget`). Flakes de ambiente, no de código.

Con `flaky-result = "fail"` (#2306), un `TRY 2 PASS` **falla el job** por diseño. Y como el
conjunto se mueve entre runs (el mismo commit falló con dos conjuntos disjuntos), no hay
conjunto estable que declarar conocido-flaky: la opción (b) de la issue no es implementable,
y (a) no aplica a tests que pasan en 3-8 s al reintento.

## Decisión (#2342)

`test-crossplatform` sale del camino de PR: corre en `push`, `schedule` y
`workflow_dispatch`, no en `pull_request` ni `merge_group`.

- Supersede el trade de #1633/#2282 («stays gating, no shielded»): lo que cambió es la
  naturaleza del rojo. Desde #2306 este lane se pone rojo por `TRY 2 PASS`, un evento que no
  es atribuible al diff del PR, así que el rojo no aporta señal de review y sí pinta fallido
  cada PR de código.
- Se conserva que sigue fuera de `gate.needs`, y el disparo por `push` sigue path-gateado por
  `needs_crossplatform` (Cargo.toml/Cargo.lock, build.rs, `.cargo/**`, rust-toolchain.toml o
  `.rs` con marcador cfg/target). `schedule` y `workflow_dispatch` resuelven a scope FULL en
  `scripts/ci_plan.sh` (línea `schedule | workflow_dispatch) full_scope "non-diff-event"`),
  así que `needs_crossplatform` es `true` ahí.
- Riesgo aceptado: se pierde la ejecución Windows/macOS por PR; la señal queda en el push a
  main y en la corrida programada.

Fuera de alcance, con trackeo propio: los tests que miden reloj (los que mueven el conjunto)
y #1631 (inventario de fallos reales Windows/macOS).

## Verificación

- `python3 -c "yaml.safe_load(open('.github/workflows/ci.yml'))"` → 25 jobs, parsea.
- `actionlint .github/workflows/ci.yml` → limpio.
- `grep -rn test-crossplatform` fuera de `ci.yml`: sólo aparece en `odd/` (registros
  históricos); ningún workflow ni script depende de que corra en PRs.
- No está en `gate.needs` (#2333 sigue abierta: promoverla a required es trabajo futuro), así
  que quitarla del PR no deja ningún check requerido en `pending`.
