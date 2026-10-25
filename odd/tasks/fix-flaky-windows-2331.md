# fix-flaky-windows-2331

Objective: remove the FLKY-FL flakes of issue #2331 by fixing their root class (the test helpers build the `webfang` binary on demand inside every test process), not with per-test overrides.
Branch: `fix/flaky-windows-2331` · Worktree: `~/Projects/Rust/webfang-worktrees/fix-flaky-windows-2331` · Delivery: single PR (small scope, ≤400 authored lines) · Runner: `cargo nextest`, `RUSTUP_TOOLCHAIN=1.91.1`.

## Specs

- **S1** — In the lane `Tests (windows-latest)`, these three become green without per-test overrides and stay green across consecutive runs:
  - `cli::crawl_test::crawl_js_strategy_respects_timeout_secs`
  - `the_retired_shim_env_vars_are_ignored`
  - `cli_vs_mcp_export_jsonl_record_parity` (`cli_export_records`)
- **S2** — The wall-clock contract of `crawl_js_strategy_respects_timeout_secs` must measure the CLI run, never the harness bootstrap (precedent: #1697, `SPAWN_LATENCY_BUDGET`).
- **S3** — Local runs keep the hermetic on-demand build (feature forwarding from #302); a runner may declare a pre-built binary through an explicit environment contract, and a declared-but-missing binary fails loudly instead of silently building.
- **S4** — No `.config/nextest.toml` override, no `#[ignore]`, no test deletion. The other mission's crawler-engine tests stay untouched.

Acceptance evidence: local green runs of the three tests, a `cargo check`/clippy/fmt pass, and the Windows lane rerun after the CI env change.

## Tasks

- **T1** — `webfang_test_utils::webfang_path`: honour `PREBUILT_BIN_ENV`, assert existence, cache the built path per process. **done** (unit test `only_the_exact_marker_declares_a_prebuilt_binary`).
- **T2** — `cli_harness::resolve_webfang_path`: same contract before spawning cargo; keep feature forwarding for the unset case. **done**.
- **T3** — `crawl_js_strategy_respects_timeout_secs`: build the `Command` before `Instant::now()`. **done** (PASS 5.29s with the binary absent and the helper build inside the function).
- **T4** — CI: declare the pre-built binary in the lanes whose pre-build matches the helper's build (`test-core`, `test-crossplatform`). **done** (step-level `env:` on «Unit + integration tests»; `actionlint` clean).
- **T5** — Open finding, out of this slice: in `--all-features` lanes (`test-full`, nightly `flake-detection`) the core helper builds with the five forwarded features while the MCP helper builds with default ones; both write `<target>/debug/webfang`, so concurrent test processes alternate feature sets and invalidate each other's link. That explains the nightly parity TIMEOUT (180s, cold cache, no pre-build) and needs its own design (one authoritative pre-build + contract, or per-feature target dirs). Not `WEBFANG_TEST_PREBUILT_BIN` on those lanes: the pre-build uses `--all-features` (chromium) and the helper deliberately does not forward `chromium`.

## Log

- **L1** (reconstructed after a compaction; the verbatim user text is not available in this context) — Implementar la issue #2331: eliminar los flakes FLKY-FL de la lane Windows sin overrides per-test, con causa estructural documentada. Restricción del usuario, verbatim: «ten cuidado en ejecutar comandos pesados que saturen el sistema».
- **E1** — Measured locally with temporary instrumentation (reverted): first `scraper_cmd()` in a fresh target dir = **42.238s**; later calls in the same process = 99µs (core harness `OnceLock`); the MCP helper had no cache.
- **E2** — `cargo nextest` does not build the CLI binary: binary deleted, `nextest` run, binary still absent.
- **E3** — Windows job log: the passing attempt of `the_retired_shim_env_vars_are_ignored` took **83.181s** including the helper build; nextest suppresses the failed attempt's output. Timeline is consistent with the 120s × terminate-after 2 slow-timeout killing an attempt blocked on the build lock, but that remains an inference, not a proven assertion.
- **E4** — Contract positive: with `WEBFANG_TEST_PREBUILT_BIN=1` the same test runs in **0.45s**. Contract negative: flag set + absent binary → failure in **0.007s** naming the path, no compiler spawn.
- **E5** — Timer hoist: binary deleted, flag unset, `crawl_js_strategy_respects_timeout_secs` PASS in **5.29s** (build outside the measured clock).
- **E6** — `cargo check` and clippy (`-D warnings`, cognitive-complexity, too-many-lines) green for `webfang_test_utils` all-targets and `webfang_core --test behavioral`; `cargo fmt --all -- --check` clean.
- **E7** — `webfang_path` call sites: 33 across 13 files; the MCP entry point is `webfang_binary()` in `crates/webfang_mcp/tests/common/mod.rs`.
- **E9** — Third flaky verified locally with the contract: `cli_vs_mcp_export_jsonl_record_parity` PASS in 0.24s.
- **E8** — CI scope confirmed with the maintainer: `test-core` and `test-crossplatform`, the two lanes whose pre-build is default features. `test-full` and nightly keep the on-demand build on purpose (see T5).
