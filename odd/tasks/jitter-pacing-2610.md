# feat(cli): --jitter-ms — jitter ascendente del delay para romper la cadencia metronómica (issue #2610)

**Objective**: break the metronomic request cadence produced by `--delay-ms` so a
rate limiter (Cloudflare) cannot align its counting window with our request pattern.
**Branch**: `feat/jitter-ms-pacing` (worktree `~/Projects/Rust/webfang-worktrees/feat-jitter-ms-pacing`).
**Delivery strategy**: ONE PR, two work-unit commits (core → plumbing) — the maintainer's
call (L7). The authored delta is 777 lines (the rest is generated: insta snapshots + the
test inventory); the review's fine-grained target is the `rate_limiter.rs` gate, the rest is
mechanical plumbing (spec → argv → preflight → options).
**Test runner**: `cargo nextest run` (focused per task) + `scripts/ci_fast_gate.sh` at the end.

## Specs

- **S1** — "`--jitter-ms <MS>` (o `--jitter <PERCENT>`, a decidir en diseño) que aleatoriza el delay efectivo de cada request: **Base**: el `--delay-ms` configurado. **Rango asimétrico con sesgo hacia arriba** … Default: 0 (comportamiento actual) para no romper tests deterministas de pacing."
  Accepted form (design decision, 2026-11-12): absolute `--jitter-ms <MS>`; effective
  interval is uniform over `[delay_ms, delay_ms + jitter_ms]` — upward-only, never
  below the configured delay. Mean interval becomes `delay_ms + jitter_ms / 2`, so
  throughput drops against the configured `--delay-ms`; operators who need the same
  rate must lower `--delay-ms` accordingly.
  Acceptance: the flag parses from CLI, `WEBFANG_JITTER_MS`, and TOML `jitter_ms`;
  default 0 keeps today's behavior byte-for-byte.
  What the operator sees (stated in `--help` and in the flag's rustdoc): the FIRST request
  leaves immediately and the gate governs every interval after it, so a run of N requests
  takes about `(N-1) * (delay_ms + jitter_ms / 2)` — and with `0` a run that fits inside the
  burst is not paced at all.

- **S2** — "**Precedencia intacta**: el backoff con `Retry-After` del fetch guard chain (stage 4) manda sobre cualquier jitter — el jitter solo aplica al pacing normal, nunca a una espera dictada por el servidor."
  Acceptance: with jitter active, a `429` carrying `Retry-After: N` still waits ≥ N
  seconds before the retry (wiremock regression).

- **S3** — "**Secuencia por corrida/proceso**, no un patrón repetido entre invocaciones del batch (si el orquestador lanza chunks, cada proceso debe tener su propia secuencia)."
  Acceptance: the RNG is seeded from OS entropy per limiter instance (one per run),
  so two processes never replay the same sequence.

- **S4** — "Con `--jitter-ms` activo, los intervalos observados caen dentro del rango configurado (test con `MockClock`/inyección de azar, determinista vía seed)."
  Acceptance: the SCHEDULED gap between two dispatches is in
  `[delay_ms, delay_ms + jitter_ms]` — the schedule's slots are absolute, so a
  late wake-up can only make a measured gap SHORTER, never faster than the
  floor the schedule guarantees. The scheduling core is unit-tested
  deterministically with a seeded RNG (no clock, no sleeping).

- **S5** — "Default 0: los tests existentes de pacing no cambian."
  Acceptance: `jitter_ms == 0` builds NO jitter gate — the governor token bucket stays
  the only pacing mechanism, burst included, and every existing pacing test stays green
  untouched.

- **S6** — Burst decision (2026-11-12, maintainer): the burst is the other bot signature.
  With `jitter_ms > 0` the gate covers EVERY request from the first one — the first
  `concurrency` permits are NOT exempt. With `jitter_ms == 0` the burst is untouched
  (S5). `budget.burst()` on this machine is 8 (16 cores → `auto_crawl_table` → 8), so
  today the first 8 requests of a run leave simultaneously; jitter removes that.

- **S7** — Cancellation (2026-11-12, maintainer): a cancelled jitter wait must not leave
  a stale future slot that delays another task if the limiter is reused. The reserved
  slot is returned to the schedule when the cancelled reservation is still the latest one.

- **S8** — Scope check (2026-11-12): the `http_client` RPM limiter is NOT on the target
  fetch path. `HttpClientConfig::rate_limit_rpm` defaults to `None` and no production
  builder sets it, so `HttpClient::new` builds no limiter at all; and the
  `HttpClientPort::get` implementation never consults it. The only pacing on the target
  path is `SharedRateLimiter`. The `http_client` RPM limiter stays a non-goal.

## Tasks

- **T1** — limiter core: `jitter_ms` on `RateLimiterConfig` (+ `with_jitter`), the cumulative
  schedule gate in every `SharedRateLimiter` wait method, slot rollback on cancellation,
  unit tests. → S1, S3, S4, S5, S6, S7. Route: inline. Commit: `ded51438`.
- **T2** — CLI plumbing: `OptionSpec` entry + GROUP, `spec_command` binding + slot,
  `CrawlerArgs`, `NetworkOptions`, preflight stages, `ConfigDefaults`, `CrawlerConfig`,
  the three limiter builders. → S1, S5. Route: inline. Commit: `e757e22a`.
- **T3** — `Retry-After` regression with jitter active (wiremock) + the E2E pacing proof.
  → S2, S6. Route: inline. Commit: `e757e22a`.
- **T4** — verification: `cargo check` / `clippy --all-targets --all-features` / `fmt --check` /
  `cargo doc`, focused `nextest` suites, independent verifier (Risk: item 4), RDD assess.
  Route: inline. Evidence below.

**Risk: item 4** (concurrency — the jitter gate adds shared scheduling state and async
ordering to the shared rate limiter). Not item 3: a new flag and an optional config key are
explicitly excluded from the high-risk list.

### PR body (draft)

```
Closes #2610

## What

`--jitter-ms <MS>` randomizes the delay between requests so the cadence stops being a
signature. The value is an ADDITION to `--delay-ms`: the effective interval is uniform over
`[delay_ms, delay_ms + jitter_ms]`, so the mean interval becomes `delay + jitter/2` — the
request rate drops, and a run that wants its old rate must lower `--delay-ms`.

Default 0: today's behavior byte for byte (no gate is built at all).

```
--jitter-ms 500            # CLI
WEBFANG_JITTER_MS=500      # env
jitter_ms = 500            # TOML
```

What the operator sees:

- The **mean interval becomes `delay + jitter/2`** — the request rate DROPS against the
  configured `--delay-ms`, so a run that wants its old rate must lower `--delay-ms`.
- With the jitter on, **the burst is no longer a burst on the wire**: `budget.burst()` is 8
  on a 16-core box, so today the first 8 requests of a run leave simultaneously — with the
  jitter the first leaves at once and the other seven wait their slots.
- The **first request leaves immediately**; the gate governs every interval AFTER it, so a run
  of N requests takes about `(N-1) * (delay + jitter/2)`.

## The four design decisions

1. **Cumulative gate, not a sleep.** The shape is load-bearing: a sleep drawn AFTER the
   permit yields `delay + j_k - j_(k-1)`, whose difference of independent draws dips BELOW
   the configured delay; a sleep drawn BEFORE the permit is absorbed by the token bucket and
   barely moves the cadence. Only a cumulative schedule — `slot = max(next_slot, now)`,
   `next_slot = slot + delay + jitter` — keeps the floor AND decorrelates.
2. **The burst is covered.** The burst is the other half of the metronome: N requests
   leaving at once is as loud as N requests leaving on a fixed tick. With the jitter on, the
   token bucket's `concurrency` permits are paced too — the first leaves at once, the rest
   wait their slot. With the jitter off, the burst is untouched. (`budget.burst()` is 8 on a
   16-core box, so today the first 8 requests of a run leave simultaneously.)
3. **A cancelled wait returns its slot — but only if it is still the latest reservation.** A
   later reservation already built its own slot on top of that one; stealing it would
   reorder the schedule.
4. **The `http_client` RPM limiter is off the target path** (so it stays a non-goal):
   `HttpClientConfig::rate_limit_rpm` defaults to `None` and no production builder sets it,
   so `HttpClient::new` builds no limiter at all; and `HttpClientPort::get` never consults
   one even when it exists. The only pacing on the target path is `SharedRateLimiter`.

## AC2 — `Retry-After` is respected verbatim

How it is proven (not only structurally):

- **A regression test against the fixture server** —
  `retry_after_is_respected_while_the_jitter_is_active` in
  `crates/webfang_core/tests/jitter_pacing.rs`: the fixture answers `429` with
  `Retry-After: 1` on the first hit and `200` on the second, the client runs with the jitter
  ACTIVE, and the assertion reads the SERVER-SIDE arrival log: the retry must arrive at least
  a second later. A client that shortened, replaced or raced that wait would fail the test.
- **Structural isolation** (why the wait cannot be touched in the first place): the jitter
  lives in the limiter (fetch guard chain stage 2) and the server-dictated wait lives in
  `WreqDownloader::fetch_inner` (stage 4). The retry never re-enters pacing, so the jitter
  can only add time to a normal request — never shorten or replace the wait the server asked
  for.

## Known limitations (from the native review)

The review approved this candidate with zero BLOCKER and zero CRITICAL. Two findings are
known limitations of the shipped behavior, each tracked as its own issue:

- **#2749 — an OS-RNG failure at construction degrades to NO pacing at all**, not to
  token-bucket-only pacing: the failure removes the protection the operator asked for instead
  of removing only the randomization. Rare (it needs the OS entropy source to be unavailable)
  and the fix is small — fall back to the token bucket and WARN about the lost randomization.
- **#2750 — a cancelled reservation that is no longer the latest leaves its slot stranded**,
  so the next request waits one extra slot. Cancellation/shutdown paths only; the request is
  late, never early, so the pacing floor still holds.

## What a reviewer should read closely

Commit 1 (`ded51438`, the limiter core) is the fine-grained one — the gate, the slot
arithmetic, the cancellation rollback, the unit tests. Commit 2 (`e757e22a`) is mechanical
plumbing: spec → argv → preflight → options → the three limiter builders. Commit 3
(`8ad48189`) only spells the semantics out in `--help` and the docs. Commit 4
(`7de372ef`) is the mission record (`odd/tasks/jitter-pacing-2610.md`) — no product code.
Commit 5 (`74846cb7`) moves the gate into its own module (`rate_limiter/jitter.rs`) and
raises the file-size ratchet. Commit 6 (`90dcde59`) is the independent verifier's fix
batch — the binary's final merge now copies `jitter_ms`, the per-URL rebuild carries it,
and the zero-delay rule answers "no pacing" only when there is no jitter either.

Docs-only commits (no product code): commit 4 (`7de372ef`, the mission record), commit 7
(`f835df40`, the mission-record update with the verifier round and the review receipt),
commit 8 (`1b148275`, the PR-body alignment with the known limitations) and commit 9
(`45cc6e49`, the file-size baseline kept at main's value after the rebase). The generated
part of the diff is the insta snapshots plus `docs/test-inventory.json`; the authored part is
the gate, its plumbing and the tests.

## Verification

- `cargo nextest run --workspace --all-features` → 5508 passed, 7 skipped.
- `cargo clippy --all-targets --all-features -- -D warnings -W clippy::cognitive_complexity -W clippy::too_many_lines` → clean.
- `cargo fmt --all -- --check` → clean.
- RED proof: removing the jitter from `build_batch_rate_limiter` makes
  `jitter_paces_every_request_not_only_the_refills` fail — the E2E is not vacuous.
- `gentle-ai review assess --base-ref 92d9f241 --committed-only` → risk `high`, review due
  (the jitter gate adds shared scheduling state to the shared rate limiter).
- Native review: APPROVED (risk / resilience / reliability; zero BLOCKER, zero CRITICAL) —
  receipt `gentle-ai.review-acknowledged/v1`, lineage `review-3lens-a41f9c`, authority burned.
```

### Verification evidence

- `cargo nextest run --workspace --all-features` → **5508 tests run, 5508 passed, 7 skipped** (exit 0).
- `cargo clippy --all-targets --all-features -- -D warnings -W clippy::cognitive_complexity -W clippy::too_many_lines` → clean.
- `cargo fmt --all -- --check` → clean (after one `cargo fmt --all` fixer pass).
- `cargo doc --workspace --all-features --no-deps` with `RUSTDOCFLAGS=-D warnings` → exit 0, HTML generated.
  NOTE: rustdoc 1.91.1 ICEs once per crate while rendering the search index
  (`scoped-tls`: "cannot access a scoped thread local variable without calling `set` first").
  It fires for crates this change never touched (`webfang_test_utils`, `webfang_mcp`,
  `webfang_cli`, `webfang_benchmark`) and the command still exits 0 — a pre-existing
  toolchain bug, not a change-caused one.
- RED proof for the E2E pacing test: with the jitter removed from `build_batch_rate_limiter`,
  `jitter_paces_every_request_not_only_the_refills` FAILS (3 retries) — the test is not vacuous.
- `gentle-ai review assess --base-ref 92d9f241 --committed-only --escalate-item 4` → risk `high`,
  `review_due: true` (`high_risk`), 31 paths / 911 changed lines.


## Log

- **L1** (2026-11-12, verbatim) — issue #2610 "feat(cli): --jitter-ms — aleatorización asimétrica
  del delay para eliminar la firma metronómica":

  > ## Problema
  >
  > `--delay-ms` aplica una separación **fija** entre requests. Un ritmo perfectamente metronómico es por sí mismo una firma de bot: los intervalos son predecibles y pueden alinearse con la ventana de conteo de un rate limiter (Cloudflare evalúa ventanas de segundos a minutos por IP).
  >
  > Evidencia de campo (2026-11-05, corrida real contra un sitio detrás de Cloudflare con rate limiting agresivo): con delay fijo de 1,5 s y 4 s, el sitio tropezó repetidamente por tasa mientras que las peticiones aisladas (fingerprint TLS de wreq) siempre pasaron con 200. El fingerprint ya no es el cuello de botella; el patrón temporal sí aporta señal.
  >
  > ## Propuesta
  >
  > Flag `--jitter-ms <MS>` (o `--jitter <PERCENT>`, a decidir en diseño) que aleatoriza el delay efectivo de cada request:
  >
  > - **Base**: el `--delay-ms` configurado.
  > - **Rango asimétrico con sesgo hacia arriba**, p. ej. base 5.000 ms → efectivo uniforme entre 4.000 y 7.500 ms. Simétrico ±25% está mal: los picos hacia abajo acercan al umbral justo cuando el sitio está más sensible.
  > - **Precedencia intacta**: el backoff con `Retry-After` del fetch guard chain (stage 4) manda sobre cualquier jitter — el jitter solo aplica al pacing normal, nunca a una espera dictada por el servidor.
  > - **Secuencia por corrida/proceso**, no un patrón repetido entre invocaciones del batch (si el orquestador lanza chunks, cada proceso debe tener su propia secuencia).
  > - Default: 0 (comportamiento actual) para no romper tests deterministas de pacing.
  >
  > ## Criterios de aceptación
  >
  > - Con `--jitter-ms` activo, los intervalos observados caen dentro del rango configurado (test con `MockClock`/inyección de azar, determinista vía seed).
  > - Con jitter activo, `Retry-After` sigue respetándose textualmente.
  > - Default 0: los tests existentes de pacing no cambian.

- **L2** (2026-11-12) — Design review with the maintainer. Decision (a): upward-only
  `[delay, delay + jitter]` in ms; the issue's own example (`[4.000, 7.500]` for a 5.000 ms
  base) contradicts its own rationale and was retired in favour of a hard floor at the
  configured delay. Mechanism: cumulative schedule (`slot = max(next_slot, now)`,
  `next_slot = slot + delay + jitter`), never a post-grant sleep (which accumulates into
  sub-base dips) nor a pre-grant sleep (which the token bucket absorbs).
- **L3** (2026-11-12) — Maintainer questions resolved: burst is NOT exempt when jitter is on
  (S6); cancellation must return the reserved slot (S7); the `http_client` RPM limiter is
  off the target path (S8); AC2 gets a real wiremock test, not a structural argument.
- **L4** (2026-11-12) — Environment: worktree `feat-jitter-ms-pacing`; seeds are rustc-1.88
  artifacts and this tree needs 1.91, so the target dir is a cold build.
- **L5** (2026-11-12) — Implementation landed: `ded51438` (limiter core + unit tests) and
  `e757e22a` (CLI plumbing, E2E tests, snapshots, regenerated `docs/test-inventory.json`).
  The 911 changed lines are mostly generated (insta snapshots + the inventory); the authored
  delta is ~300. The repo forbids stacked PRs, so this ships as ONE PR on `main`.
- **L6** (2026-11-12) — Deliberately NOT exposed on the MCP surface: `scrape_batch` keeps
  `delay_ms` and its jitter stays 0. The MCP↔spec parity is one-directional (MCP-derived
  properties → spec ids), so a CLI-only spec does not break it.
- **L7** (2026-11-12) — Delivery budget: 777 authored lines (468 of them in
  `application/rate_limiter.rs`, half of that documentation and tests) — over the ~400
  heuristic. The maintainer votes **ONE PR with the two commits as they are**: a PR carrying
  only the engine would be inert (nobody can turn `jitter_ms` on without the flag, so a
  reviewer could not check AC1/AC2/AC3 against it), the RED proof crosses both commits, and
  this repo has no stacked-PR path — PR2 would have to wait for PR1 to merge, delaying the
  fix the issue asks for. The change is ONE behavior end to end, and the doc/comment budget
  was NOT cut to fit a number. If the verifier or the native review demands a separate
  iteration of the engine, the split is: PR1 = the engine + its unit tests, PR2 = the flag.
- **L8** (2026-11-12) — Pre-PR checks from the maintainer, both done: (1) the FIRST request
  leaves immediately — the gate governs every interval AFTER it, so a run of N requests
  takes about `(N-1) * (delay + jitter/2)`; pinned by `jitter_never_delays_the_first_request`
  and `jitter_gate_covers_the_first_request`, now also stated in the code docs, in the
  `--jitter-ms` rustdoc and in `--help`. (2) `--help` now says the mean interval becomes
  `delay + jitter/2` (the request rate drops) and that with `0` a run that fits in the
  burst is not paced at all; the help snapshots were regenerated.
- **L9** (2026-11-12) — Independent verifier (`gentle-ai-verify`) returned NEEDS_FIX: S2–S8
  held, S1 was broken by ONE blocker (the binary's final merge in
  `crates/webfang_cli/src/main.rs` never copied `jitter_ms`, so a TOML-configured jitter was
  silently dropped) plus two advisories (the per-URL rebuild lost the jitter; the zero-delay
  rule said "no pacing" while the crawl path clamped to 1 ms). All three are fixed in
  `90dcde59`, each with a regression test, and the RED proof was observed by reverting the
  fixes (the TOML test then saw `jitter_ms: 0` instead of 250, and the jitter-alone test
  found no wiring log at all).
- **L10** (2026-11-12) — Native review (RDD) of the corrected candidate: assess → risk
  `high` / review due; consent GRANTED by the maintainer; lineage `review-3lens-a41f9c`
  (risk + resilience + reliability — the readability actor produced no output through the
  OpenCode transport and was dropped from the scope). Result: **APPROVED**, zero BLOCKER and
  zero CRITICAL, receipt `gentle-ai.review-acknowledged/v1` with the authority burned.
  Advisory findings left for later work, never for this candidate: `R4-001` (an OS-RNG
  failure at construction degrades to "no pacing at all" instead of token-bucket-only
  pacing — WARNING) and `R4-002` (a cancelled reservation that is no longer the latest
  leaves its slot stranded — WARNING); plus suggestions on the `SLOT_HORIZON` fallback log,
  the mutex contention, and the documentation of `jitter_ms`.
- **L11** (2026-11-12) — `main` moved 9 commits past the branch fork point (`92d9f241` →
  `0e99822c`, the `ssrf_guard` test-module split), so the branch was REBASED onto the
  current `main` before opening the PR. The candidate patch is byte-identical — the reviewed
  diff still describes exactly what the PR proposes — and the only conflict was
  `scripts/quality-baselines.json` (both sides raised baselines), resolved as the union: our
  raises plus main's `ssrf_guard.rs` value, which stays at 1147. The branch was NOT pushed
  before the rebase, so no published history was rewritten. Re-verified on the rebased tree:
  `cargo nextest run --workspace --all-features` → 5508 passed / 7 skipped, clippy
  `--all-targets --all-features` clean, `cargo fmt --all -- --check` clean, file-size ratchet
  OK.
