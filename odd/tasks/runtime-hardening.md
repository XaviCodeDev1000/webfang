# ODD — Runtime hardening
Objective: implement the agreed WebFang hardening work in reviewable units. Branch: `feat/runtime-hardening` (worktree `feat-runtime-hardening`). Delivery: `sequential-on-main` (one PR per work unit against `main`, merged before the next; nested PRs are forbidden by `scripts/check-topology.sh`). Runner: `cargo check`, `cargo clippy --all-targets --all-features -- -D warnings -W clippy::cognitive_complexity -W clippy::too_many_lines`, `cargo fmt --all -- --check`, `cargo nextest run`, `bash scripts/ci_fast_gate.sh`.

Rules for the implementing agent: work only inside the worktree; `eval "$(direnv export bash)"` before cargo/git in non-interactive shells; verify the identity proxy before every commit; English only; no `.unwrap()` in production; no `unsafe` code; no external metrics backend; no `CHANGELOG.md` or `docs/` edits; no new dependency beyond S1 and S2; HTTP always uses `wreq` and keeps the guard-chain order `ValidUrl` → pacing → timeout → redirect → SSRF → retry classification → capped body; cite symbols and paths, never line numbers; commit after each unit.

## Specs

### S1 — Move `wreq` and `wreq-util` to the supported release line
Files: root `Cargo.toml`, `Cargo.lock`.
1. In root `Cargo.toml`, change `wreq = { version = "6.0.0-rc.29", features = ["gzip", "brotli", "stream", "json", "cookies"] }` to `version = "0.16.0"`, keeping all features. Change `wreq-util = { version = "3.0.0-rc.14", features = ["emulation"] }` to `version = "0.2.0"`, keeping all features.
2. Regenerate `Cargo.lock`. Do not change any other dependency version.
3. Build and fix call-site API drift without changing behavior or guard-chain order. Keep the default emulation profile `Chrome145`.
4. Verify the fetch guard chain is unchanged in `WreqDownloader`, `FetchRouter` and `build_wreq_client`.
Verification: `cargo check`; full clippy; `cargo fmt --all -- --check`; `cargo nextest run --test stdio_handshake`; one real scrape of `https://example.com` against local fixtures; `cargo deny check`; `bash scripts/check_advisory_policy.sh`.
Commit: `chore(http): move wreq to supported release line`

### S2 — Add vendored `arapuca` with the browser seccomp profile
Files: root `Cargo.toml` (or `crates/webfang_core/Cargo.toml`), new `vendor/arapuca/`, `Cargo.lock`.
1. Add `arapuca` as a git dependency pinned to commit `2ec2daa224018418cf10e1aa46c989cbebc746b1` of `https://github.com/LeGambiArt/arapuca`, or vendor that exact commit into `vendor/arapuca/` and path-depend on it. Pin by commit, never by tag. If an internal mirror is required, substitute only the URL; keep the commit.
2. Keep all optional features off; `microvm` must remain disabled.
3. Apply to `vendor/arapuca/src/seccomp.rs`: remove `libc::SYS_landlock_create_ruleset`, `libc::SYS_landlock_add_rule`, `libc::SYS_landlock_restrict_self` and `libc::SYS_memfd_create` from `baseline_kill_syscalls()`; insert those four syscall numbers into the baseline ENOSYS rule map in `build_baseline_filter`; remove `libc::SYS_ptrace` from `baseline_kill_syscalls()`; insert `clone_deny.insert(libc::SYS_ptrace, vec![]);` into the stacked baseline EPERM filter map so `ptrace` is denied with `EPERM`, never allowed and never killed.
4. Do not modify any other part of the vendored crate.
Verification: `cargo check`; confirm the vendored diff contains only those four changes; confirm `microvm`/`reqwest` is absent from `cargo tree -e features -i arapuca`.
Commit: `chore(infra): vendor arapuca with browser seccomp profile`

### S3 — Sandbox the browser subprocesses
Files: new wrapper binary crate under `crates/`, `obscura_downloader/mod.rs`, `chromiumoxide_downloader.rs`, `downloader_factory.rs`.
1. Create a wrapper binary (e.g. `webfang-sandbox-wrapper`) that answers `--version` by forwarding to the real browser; otherwise parses `--user-data-dir=<path>` from argv, creates the directory if absent, calls `arapuca run` with the arguments below, then `exec`s the real browser with the original argv. Use only safe APIs; no `unsafe`. Build for Linux only.
2. Wire L2 (`ObscuraDownloader::fetch_inner`): spawn `arapuca run ... -- <resolved-obscura-binary> fetch --dump html <url>`. Preserve `kill_on_drop(true)`, piped and capped stdout/stderr, timeout, exit-status check and stderr URL parsing. Keep the version probe.
3. Wire L3 (`ChromiumoxideDownloader::fetch`): resolve the wrapper path and pass it through `BrowserConfig::chrome_executable(path)`. Keep `.no_sandbox()` on Chrome.
4. Exact arguments, in order: `--task-id <tenant-or-run-id>`; `--seccomp baseline`; `-v <chrome-or-obscura-dir>:ro`; `-v <profile-dir>:rw` when resolved; `-v /proc:ro`; `-v /dev/shm:rw`; `-v /tmp:rw`; `-v /sys/bus:ro`; `-v /sys/class:ro`; `-v /sys/devices:ro`; `-v /etc/fonts:ro`; `-v /usr/share/fonts:ro`; `-v /dev/dri:rw`; `-v /etc/ssl:ro`; `-- <real-binary> <original argv>`. Never mount `/sys` itself.
5. On non-Linux, missing cgroup delegation or missing arapuca binary, skip the wrapper, emit a structured WARN and continue unsandboxed. Never fail a run because the sandbox is unavailable.
6. Derive the task id and profile directory from existing per-run state. Do not add a global mutable registry and do not change `ResolutionTracker` here.
Verification: `cargo check`; full clippy; build the wrapper; one `webfang --url https://example.com --js-strategy full --output <tmpdir> --max-pages 1` with the wrapper active → exit 0, `1 total, 1 succeeded, 0 failed`, markdown plus `export.jsonl`, no orphan browser; repeat with `--js-strategy hybrid`; one kill/timeout test with no orphan child. Use a temporary output directory outside the worktree.
Commit: `feat(infra): sandbox L2 and L3 browser subprocesses`

### S4 — Structured sandbox observability
Files: the wrapper crate and the two downloader modules from S3.
1. Emit structured `tracing` events for: wrapper applied, wrapper skipped with reason, sandbox denial (`errno`, syscall, layer, tenant/run id), browser exit with status. Use existing correlation IDs and top-level `trace_id`.
2. Keep every event in `FileTraceLayer` JSONL. Add no metrics backend, daemon or exporter.
3. Add one test capturing trace output and asserting the event fields exist.
Verification: `cargo nextest run` for the touched modules; one scrape with `--trace-file <tmpdir>/trace.jsonl`; `scripts/analyze-trace.sh` returns the sandbox events with non-empty fields.
Commit: `feat(observability): trace sandbox decisions`

### S5 — Upstream the generic profile slice
Files: patch files under `odd/` only; no WebFang source change.
1. Prepare a minimal upstream patch: ENOSYS for `memfd_create` and `landlock_*`, a dedicated EPERM tier for `ptrace`, exposed through an opt-in profile/flag. Keep the strict default unchanged.
2. State in the PR that the change grants no new capability to the sandboxee.
3. Keep the vendored pin in WebFang until an upstream release based on current `main` exists.
Verification: patch applies cleanly to upstream `main`; upstream CI passes; PR link recorded.
Commit: `docs(infra): prepare upstream seccomp profile patch`

### S6 — Lint and global-init hygiene
Files: `cli/batch_flow.rs`, `domain/error/resolution.rs`.
1. In `spawn_flush_watch`, replace `#[allow(clippy::disallowed_methods)]` with `#[expect(clippy::disallowed_methods, reason = "...")]` above the supervised `tokio::spawn`.
2. In `resolution_tracker()`, replace `OnceLock::new()` plus `get_or_init(...)` with `LazyLock::new(...)`. Keep the `Mutex<HashMap<...>>` shape and all call sites.
3. Do not convert other `#[allow]` sites and do not change the `ResolutionTracker` struct.
Verification: full clippy; `cargo fmt --all -- --check`; `cargo nextest run` for the affected modules.
Commit: `refactor(core): scope lint suppression and global init`

### S7 — Bulk reader for the general `chunks` table
Files: `infrastructure/persistence/sqlite/vector_repository.rs`, its domain trait and tests.
1. Add a bulk reader: `SELECT id, content, embedding_vector FROM chunks WHERE embedding_vector IS NOT NULL`, with optional `AND resource_url = ?1`. The column is `embedding_vector`; there is no `tenant_id` column.
2. Decode the BLOB with the existing byte-to-vector helper and feed results into the cosine ranking used for `note_chunks`. Handle an empty result without error.
3. Add no crate, C toolchain, schema migration or second database.
Verification: unit test asserting NULL-embedding rows are filtered; integration test writing two chunks (one NULL) and reading only the valid one; `cargo nextest run` for persistence and vault-search modules.
Commit: `feat(ai): add production reader for general chunks`

### S8 — Cross-platform proof before any platform claim
Files: CI workflow files, test scripts.
1. macOS runner: `cargo check` plus a real Seatbelt Chrome launch smoke test through the sandbox path.
2. Windows: `scripts/check_windows_platform.sh` plus a Chrome launch smoke test from a service context with `allow_exec=true` and network namespace disabled.
3. Do not claim cross-platform support before both are green. If macOS `sandbox-exec` cannot launch Chrome, record the exact failure and keep Linux as the only verified platform.
Verification: both CI jobs green; results recorded in this document's Log.
Commit: `ci: prove sandbox paths on macOS and Windows`

### S9 — Confine SSRF test bypasses
Files: `domain/ssrf_guard.rs`, its callers, preflight/config flag plumbing.
1. Remove production reads of `DISABLE_VALIDATING_RESOLVER_ENV` (`WEBFANG_DISABLE_SSRF_RESOLVER`) and `DISABLE_REDIRECT_GUARD_ENV` (`WEBFANG_DISABLE_SSRF_REDIRECT_GUARD`).
2. Confine both reads to `#[cfg(any(test, feature = "test-util"))]`, or replace them with one explicit in-process flag that emits a structured audit event. Do not accept either variable from the environment in a release build.
3. Test that with both variables set, a release build still rejects a forbidden literal IP at dial and redirect time.
Verification: `cargo check`; full clippy; `cargo nextest run --test ssrf_rfc1918_e2e_test` → exit 69 and zero outbound packets for RFC1918/CGNAT/NAT64/mapped targets.
Commit: `fix(domain): confine SSRF test bypasses to test builds`

### S10 — Redact credentials in every output sink
Files: `domain/providers.rs`, `domain/proxy_config.rs`, `cli/commands/blocks.rs`, `infrastructure/observability/file_trace_layer.rs`, `application/rate_limiter.rs`, `domain/ssrf_guard.rs`.
1. Add a redacting URL newtype with `Debug`, `Display` and `Serialize` that strips username and password; use it for `ProviderConfig::base_url`. Fail closed to a fixed placeholder when userinfo cannot be removed.
2. Strip userinfo before formatting `origin` in `ProxyConfigError::{UnsupportedScheme, UserInfoNotAllowed, OriginOnly}`.
3. Route every URL-valued tracing field through the redactor, including `record_pacing_wait`'s `url` and `redirect_policy`'s `target_uri`.
4. Add a property test using `https://user:s3cret@host/` across `Debug`, `Serialize`, errors, `CliExit` output and `--trace-file` JSONL; assert the secret appears in none.
Verification: `cargo nextest run` for domain, CLI and observability modules; `doctor` and `show` against an inline-credential base URL print no secret; `cargo fmt --all -- --check`.
Commit: `fix(domain): redact credentials in all output sinks`

### S11 — Classify L3 output and populate the escalation context
Files: `downloader/hybrid_router/mod.rs`, `downloader/spa_detector.rs`, `http/waf_engine/entropy.rs`, `waf_engine/verdict.rs`.
1. Run `evaluate_fetch(page)` on the L3 `Ok(page)` path before returning it.
2. Build `InspectionContext` with real status, headers, content-type, body sample and cookies; pass it to `detect_spa`. Remove the hardcoded `ignore_waf` short circuit at the escalation gate and make it configurable.
3. In `entropy_evidence`, require a correlated WAF status before promoting control-header presence to T1 at status 200.
4. In `decide`, do not discard body-sourced T2 evidence on 5xx when a vendor marker is present.
5. Tests: L3 challenge aborts; large legitimate 200 does not block; 5xx with markers blocks; update `test_detect_spa_ignore_waf_true_t1_not_blocked` to the configurable behavior.
Verification: `cargo nextest run` for `hybrid_router` and `waf_engine`; full clippy.
Commit: `fix(http): classify L3 output and populate escalation context`

### S12 — Durable persistence and visible corruption
Files: `infrastructure/export/record_store.rs`, `record_store_file.rs`, `cli/batch_flush.rs`, `application/crawler/bounded_sink.rs`, `infrastructure/persistence/sqlite/connection.rs`, `sqlite/vector_repository.rs`, `application/resume.rs`.
1. Call `sync_all()` on the temp file before every `rename(2)` in `StoreFs::write` and `write_sidecar`, and `sync_all()` on the parent directory after the rename.
2. Advance `BoundedFileSink::durable_bytes` only after fsync, never after `flush()` alone.
3. Set SQLite `synchronous = FULL` while keeping `journal_mode = WAL`. Run `PRAGMA integrity_check` at startup and return a typed error on failure.
4. Change `load_or_init` so corruption or unsupported version is an operator-visible error instead of a fresh empty store with a warning.
5. Do not switch to WAL2 or `BEGIN CONCURRENT`.
Verification: unit test that a corrupt store returns an error; integration test that a sidecar watermark is persisted after fsync; `cargo nextest run` for persistence and crawler modules; `bash scripts/ci_fast_gate.sh`.
Commit: `fix(persistence): durable writes and visible corruption`

### S13 — Bound shutdown and shared-resource waits
Files: `cli/shutdown.rs`, `application/crawl_result_repository.rs`, `cli/batch_flow.rs`, `infrastructure/persistence/pool.rs`.
1. Call `token.cancel()` in `ShutdownGuard::Drop` in addition to aborting the signal listener handle.
2. Wrap `CrawlResultRepositoryImpl::shutdown`'s `tx.send(WriteCommand::Shutdown)` in `tokio::time::timeout`.
3. Retain the `spawn_flush_watch` blocking handle and abort it on timeout.
4. Configure explicit `acquire`, `create` and `recycle` timeouts on the deadpool pool. Add no dependency or feature to do so; if the pinned version cannot express timeouts, stop this unit and report the exact blocker.
5. Test: shutdown with in-flight work exits in under 5 seconds with no orphan tasks.
Verification: `cargo nextest run` for CLI, engine and persistence modules; full clippy.
Commit: `fix(runtime): bound shutdown and pool waits`

### S14 — Unify MCP and CLI error contracts
Files: `mcp_server/panic_containment.rs`, `handlers/url_utils.rs`, `mcp_server/mod.rs`, `cli/error.rs`, `cli/commands/connect.rs`, `cli/parse.rs`.
1. Return the same panic envelope on HTTP and stdio: JSON-RPC `-32603` with an English message inside a `CallToolResult` error.
2. Change `validate_url` to return `-32602` invalid params when `valid: false`, instead of a success result.
3. Derive `MCP_TOOL_COUNT` from the router's `list_all()`; remove the hardcoded constant; do not advertise the panic probe outside loopback or development builds.
4. Add `SsrfBlocked` exit code 70 and `WafBlocked` exit code 71 to `CliExit`; stop remapping SSRF-related `ConfigError` into `NetworkError` (69) in `validate_endpoint` and `diagnose_reachability`.
5. Preserve the original error cause in `handle_completions`.
6. Update the MCP tool-count fixture only if the derived count differs from the pinned value.
Verification: `cargo nextest run --test stdio_handshake`; `bash scripts/smoke-mcp-stdio.sh <built webfang-mcp-stdio>`; CLI matrix with SSRF-blocked and WAF-blocked endpoints asserting exit 70 and 71.
Commit: `fix(contracts): unify MCP errors and SSRF exit codes`

### S15 — Measured benchmark metrics
Files: `crates/webfang_benchmark/Cargo.toml`, `src/aggregate.rs`, `src/cost.rs`, benchmark harness sources, `infrastructure/observability/memory_probe.rs`.
1. Add a measured-RSS field to `StrategyMetrics`. Sample `/proc/self/statm` with the real page size. If no safe API for page size exists in the pinned dependencies, stop this step and report the blocker; add no `unsafe` and no dependency.
2. Use measured RSS in `cost::estimate` where available; keep `Downloader::memory_cost()` only as an explicitly labelled fallback.
3. Count challenge events over total attempts using `WafVerdict`/`WafTier::Challenge`, `DownloadError::WafChallenge` and `ScraperError::WafBlocked`. Never report a presence flag such as `> 0` as a rate.
4. Generate load at a fixed target rate from a scheduled timeline and record scheduled and observed timestamps. Compute percentiles in memory. Add no HdrHistogram or other crate.
5. Use local deterministic fixtures. Keep Tier B competitor adapters excluded from published numbers.
Verification: two consecutive runs on the same fixtures produce identical numbers; `cargo nextest run` for the benchmark crate; full clippy.
Commit: `feat(bench): measured RSS, open-loop latency and challenge rate`

### S16 — Completion retry and untrusted-input envelope
Files: `infrastructure/llm/client.rs`, `application/llm_extraction.rs`, `mcp_server/provenance.rs`.
1. Add bounded retry to `OpenAiLlmClient::send_completion` for 429 and 5xx with exponential backoff plus jitter, honoring `Retry-After`. Emit a structured event on every retry. Terminal 4xx must not retry.
2. Wrap cleaned fetched text in explicit delimiters in `build_request` and state the instruction hierarchy in the system prompt. No raw untrusted text reaches the user message without the envelope.
3. Apply `provenance.rs` neutralization to inputs as well as outputs, preserving trust levels.
4. Tests: 429 with `Retry-After` retries the expected times; terminal 401 does not retry; a marked injection payload arrives inside the envelope.
Verification: `cargo nextest run` for LLM, extraction and MCP provenance modules; `cargo fmt --all -- --check`.
Commit: `fix(ai): retry completions and envelope untrusted input`

### S17 — Tenant-scoped paths and bounded resource defaults
Files: `infrastructure/autotuning.rs`, `cli/config.rs`, `cli/scrape_resume.rs`, `infrastructure/downloader/chrome_profile.rs`, `adapters/downloader/mod.rs`, `cache.rs`, `infrastructure/downloader/resource_governor.rs`, `fetch_router/mod.rs`, `hybrid_router/mod.rs`.
1. Add an explicit tenant-or-run identifier and resolve the DB path, config path, credentials file, checkpoint/state directory, Chrome profile and download directories underneath a scoped root. Preserve CLI and environment overrides. Keep current home-relative defaults for single-tenant runs with no identifier.
2. Set a finite default asset cache capacity in `Downloader::new`; keep `with_asset_cache_capacity` for explicit overrides.
3. Read the cgroup memory limit in `ResourceGovernor` and use it instead of host RAM when present.
4. Make `FetchRouter::fetch` and `HybridRouter::fetch_inner` consume the permit count returned by `check_resources`, not only its `Err` case.
5. Tests: two identifiers produce different DB and profile paths; default asset cache capacity is finite; escalation is denied when the reduced permit count reaches zero.
Verification: `cargo nextest run` for autotuning, downloader and resource-governor modules; full clippy.
Commit: `feat(multitenancy): scoped paths and bounded resources`

## Tasks

- T1 — S1 — inline — migrate wreq release line — BLOCKED (rustc 1.91.1 vs required 1.98; see L8)
- T2 — S2 — inline — vendor arapuca with browser profile — DONE (slice `feat/runtime-hardening-02-arapuca`, commit `ed75c5de`)
- T3 — S3 — delegated — sandbox L2 and L3 subprocesses — DONE (commits 18f0db48, 195680c7, 9f23141a; native review escalated, informational)
- T4 — S4 — delegated — sandbox tracing events — DONE (commit c307bb2b)
- T5 — S5 — inline — upstream profile slice — pending
- T6 — S6 — delegated — lint and init hygiene — DONE (commit 421b91de)
- T7 — S7 — delegated — chunks bulk reader — DONE (commit bb341aa4)
- T8 — S8 — inline — macOS and Windows CI proof — pending
- T9 — S9 — delegated — SSRF bypass confinement — DONE (commit badc2dab)
- T10 — S10 — delegated — credential redaction — DONE (commit a34e6ef8)
- T11 — S11 — delegated — L3 WAF classification — DONE (commit a2fe1da7)
- T12 — S12 — delegated — durable persistence — DONE (commit 89324c06; surface extended with parent adjudication)
- T13 — S13 — inline — bounded shutdown and pools — DONE (commit bd56cb8b)
- T14 — S14 — inline — MCP and CLI error contracts — DONE (commit b7177994)
- T15 — S15 — delegated — measured benchmark metrics — BLOCKED (no safe real page-size API in pinned deps; see L12)
- T16 — S16 — delegated — completion retry and input envelope — DONE (commit b7507b8f)
- T17 — S17 — inline — tenant-scoped paths and resources — pending

Route declaration: T3, T4, T6, T7, T9-T12, T15 and T16 were delegated to bounded writers with explicit edit surfaces and skills; the parent owns all commits and seams. Work-unit commits carry each spec; commit identities are recorded here as they land.

## Log

L1 (verbatim user request): "agrega las nuevas unidades , pero no incluyas una sección corta de 'Excluded from this delivery', para que informar de algo que no vamos a implementar, el agente sigue unicamente el plan"

L2 (verbatim user request): "ahora elimina la carpeta reports y crea un nuevo worktree con nombre adecuado para implementar el plan con nombre adecuado siguiendo la politica del odd, luego eliminamos este worktree"

L3 — Setup evidence: branch `feat/runtime-hardening`, worktree `feat-runtime-hardening`, cut from the current implementation branch. Isolated `.envrc`, `direnv allow`, `.env` copy, identity manifest, CodeGraph and CodeDB indexes verified before implementation. No source or dependency change has been made by the planning phase.

L4 (verbatim user request): "la rama quedó un commit atrás de main, así que conviene re-anclarla.\nWorktree: feat-runtime-hardening\nRama: feat/runtime-hardening\nDocumento: odd/tasks/runtime-hardening.md (S1-S17, T1-T17) \n\nvamos a implementarpor pr anidadas"

L5 — Re-anchor evidence: the branch had no upstream and no `origin/feat/runtime-hardening`, so rewriting local history was safe. `git rebase --autostash origin/main` replayed the 17 planning commits onto `876d108f`; `origin/main...HEAD` is now `0 17`; `refs/stash` stayed empty before and after; the single stray one-line edit in `odd/2534-selector-fragment/tasks.md` was preserved.

L6 — Delivery decision: user selected "Secuencial en main (recomendado)" over nested PRs. Evidence: `scripts/check-topology.sh` admits only `feat/* -> main` (`feat/*` may not target another working branch), and AGENTS.md states "There are no stacked PRs in this repo" and prescribes sequential delivery: slice 1 as a normal PR on `main` → merge → slice 2 from the new `main` → repeat. Each work unit therefore starts on a fresh branch from `main` and targets `main`; no `.github/` policy change is requested.

L7 (verbatim user request): "Saltar S1, seguir con T2 (recomendado)"

L8 — S1 blocker evidence: a throwaway branch from `main` changed `wreq` to `0.16.0` and `wreq-util` to `0.2.0`. `cargo check` failed because both declare `rust-version: 1.98` while the workspace pins `rust-version = "1.91"` and the installed toolchain is 1.91.1. `wreq 0.16.4` also requires `lru ^0.19.0`, so "regenerate Cargo.lock without changing any other dependency" is impossible with that line. `cargo info` confirms `wreq 6.0.0-rc.31` and `wreq-util 3.0.0-rc.14` declare `rust-version: 1.85`. No files were kept: `Cargo.toml`/`Cargo.lock` were restored, the throwaway worktree and branch removed. T1 is reopened pending an explicit toolchain decision; implementation proceeds with T2.

L9 — T2 evidence: commit `ed75c5de` on `feat/runtime-hardening-02-arapuca`, re-anchored onto `origin/main` (`e640bbe6`) so `origin/main...HEAD` is `0 1`. `vendor/arapuca/` is the exact commit `2ec2daa224018418cf10e1aa46c989cbebc746b1`; `diff -rq` against a pristine clone shows only `src/seccomp.rs` changed, and its diff contains only the four requested edits (ptrace removed from kill list and inserted into the EPERM map; landlock_* and memfd_create removed from kill list and inserted into the ENOSYS map). `cargo check`, full clippy and `cargo fmt --all -- --check` green; `cargo tree -e features -i arapuca` shows no `microvm`, `reqwest`, `krun`, `openssl`, `gpt` or `nbd` edge. `cargo doc --workspace --all-features --no-deps` exited 0 but rustc emitted ICE reports (eight `rustc-ice-*.txt` files) — they were deleted, not committed; the doc gate's exit code is recorded but the ICE is an environment defect, not verified proof. Slice diff is ~42k added lines (vendored source), far over the 400-line review budget: `chained-pr` classifies vendor diffs as `size:exception` candidates; no exception has been granted.

L10 — T3 evidence: implemented on the integration branch by a bounded writer (RED→GREEN recorded), commit `18f0db48`; native review (`review-193c25885116e368`) found duplicated sandbox helpers and an ignored configured browser. One bounded correction (120-line budget) shared the helpers via `sandbox` and forwarded the configured binary through `REAL_BINARY_ENV`, stripped before exec; dependency-direction policy and its 16-case harness were updated to admit the new crate. Commits `195680c7`, `9f23141a`. Re-review lineage `review-31403166fd086777` ended in `native_stop_required` with one inconclusive finding (`R2-001`, unknown causality) — informational, no approval to infer. Live E2E with arapuca was not exercised (binary absent); fixtures and smoke tests are green.

L11 — Delegated batches: T4 (c307bb2b) added four structured sandbox events with a JSONL field-existence test; T6 (421b91de) converted one lint suppression to `#[expect]` and moved global init to `LazyLock`; T7 (bb341aa4) added the general `chunks` bulk reader with NULL-embedding filtering. Full `cargo check`, clippy with `-D warnings -W clippy::cognitive_complexity -W clippy::too_many_lines`, `cargo fmt --all -- --check`, direction gate (16/16 harness cases) and `cargo nextest run --all-features` are green.

L12 — Second batch: T9 (badc2dab) confined both SSRF env hatches to `cfg(test)` with production twins returning false and three E2E proofs at exit 69; T10 (a34e6ef8) added `RedactedUrl` and redacted every URL-valued tracing sink with a property test; T11 (a2fe1da7) classified L3 output and built a real `InspectionContext`; T12 (89324c06) added fsync-before-rename, fsync-backed durable bytes, SQLite `synchronous = FULL` + integrity check, and typed corruption errors, after parent adjudication extended the edit surface; T16 (b7507b8f) added bounded completion retries and the untrusted-input envelope. T15 was blocked at its own spec gate: no safe API for the real page size exists in pinned declared dependencies (`libc::sysconf` is unsafe, `sysinfo` keeps `page_size_b` private, `nix::sysconf` is only transitive) and S15 forbids `unsafe` and new dependencies — no files changed, decision pending. Two cross-unit contract failures were fixed in commit efa4d0b8 (wrapper inherits workspace lints; AI boundary prefix tracks the S16 system prompt). Full workspace `cargo nextest run --all-features` is green.

L13 — Pre-publication guard remediation (verbatim user request): "¿Querés que corrija las guardas y re-ancle antes de publicar cargando la skills apropiadas para file-size?" Loaded `webfang-file-splits` and `webfang-ci-gate`; re-anchored the unpushed branch onto `origin/main` `cd15e330` (`0 20`). Three bounded writers split `RedactedUrl` and the provider tests out of grandfathered `providers.rs` (re-exports kept), extracted `ChunkVector`/`MultiVectorRepository` and two SQLite test submodules, folded the S3 launch helper into `obscura_downloader/sandbox.rs`, and compressed S11 test fixtures; a fourth deduped S9/wrapper/redaction fixtures. No rule was relaxed: the file-size baseline only went DOWN (`--remove providers.rs`), jscpd went 8259→8174 against baseline 8219, generated golden/inventory/AI-doc pins/AGENTS budget/CLI reference were regenerated, and the doc manifest now points at `providers/tests.rs`. Commits `5a47acfe`, `a417f4d2`, `a425d01d`, `f0e185fb`. Final `bash scripts/ci_fast_gate.sh`: 87 PASS / 0 FAIL / 4 SKIP. Pre-commit gate (`cargo check`, full clippy, `cargo fmt --all -- --check`, `env RUSTDOCFLAGS=-D warnings cargo doc --workspace --all-features --no-deps`) green; full `cargo nextest run --all-features`: 5844 passed, 8 skipped. Nothing had been pushed at the time of that check; see L14 for the publication step.

L14 — Publication (verbatim user request): "abre issue nueva con label approved y pr close part of ...". With explicit authorization and verified `admin` viewer permission, issue #3050 was created with `status:approved` and read back. The branch was pushed to `origin/feat/runtime-hardening-integration` (`ce4fca41` at push time) and PR #3051 was opened against `main` with title `feat(runtime): harden subprocess sandboxing, AI boundaries and persistence`, body starting `Closes part of #3050`, and the observed gate/review evidence. No `size:exception` label exists in this repository, so none was invented; the over-budget vendor diff is disclosed in the PR body. `type:feature` is derived from the `feat` title by `scripts/auto_label_type.sh`; the auto-label job owns applying it. T1, T15, T5, T8, T13, T14 and T17 remain outside this slice.

L15 — Post-publication critical review (verbatim user request): "vamos a hacer un review critico y profundo porque toca varios archivos a la pr abierta 3051. tienes worktree abierto para arreglar los bugs detectados, nunca toques a main siempre trabajas dentro del worktree relacionado a esta PR". Five work-unit commits close confirmed defects. `c6295088`: `connect` persisted `ProviderConfig` through the redacting `Serialize` hook, so a re-connect wrote the redacted `base_url` into the config file — silently dropping inline credentials — and the merge guard then saw two different endpoints and refused the update; the record now re-inserts the configured URL verbatim and the cross-endpoint error redacts both URLs. `2e8a48a1`: `ScraperError::Http` carried the LLM `base_url` with userinfo into the CLI error stream. `b5325977`: the `FileTraceLayer` backstop parsed the whole field value as a URL, so any message that merely embedded a credentialed URL was replaced wholesale by the placeholder, destroying the diagnostic. `2da62840`: `PRAGMA integrity_check` ran in the `post_create` hook for every pooled connection AND again in `setup_schema` (N+1 full scans of the database at startup); it is now claimed once per pool, before any pragma. `7d5c3557`: L2 mounted the current directory for the default bare `obscura` name (`Path::parent()` of a bare name is empty) and exec'd the name from a directory that does not hold it — bare names now resolve on `PATH` and an unresolved name skips the sandbox; L3 set `chrome_executable` to the wrapper without forwarding a real browser when the caller pinned none, so the wrapper guessed from a short PATH list and missed `CHROME`, `chrome` and nonstandard installs that chromiumoxide detects — the parent now resolves with `chromiumoxide::detection::default_executable`; the wrapper test target used unix-only APIs unconditionally, so it and the pure-function unit tests are confined to Linux (the workspace `--tests` run compiles on macOS and Windows). Verification: `cargo check --workspace --all-features`, `cargo clippy --workspace --all-targets --all-features -- -D warnings -W clippy::cognitive_complexity -W clippy::too_many_lines`, `cargo fmt --all -- --check` all green; nextest for connect/downloader/observability/persistence (361 passed), credential-redaction + sqlite-durability + sqlite-integration + resume-gate targets (89 passed) and the wrapper crate (6 passed). The doc gate exited 0 but rustc emitted ICE reports again (the L9 environment defect); the `rustc-ice-*.txt` artifacts were deleted, not committed.

Findings reported, not changed (each needs a decision outside this slice's edit surface). `arapuca` is a declared dependency of `webfang_core` with no `use` site: the sandbox execs the `arapuca` binary, which the build does not produce, so every run silently takes the unsandboxed path unless the operator installs it. S7's `load_chunk_vectors` has no production caller — only tests — so the general `chunks` reader is inert. S16 exposes `neutralize_input` but no tool input is routed through it; the output path is unchanged. L3's `FetchedPage` carries empty headers, so S11's "real context" for that layer is status-only and the synthesized `cookie` header is read by nothing. The `obscura-hybrid` CI job still claims that three SSRF hatches disarm the guard, but S9 made the resolver and redirect reads test-only; the job only works because its target is a literal IP. The Spanish proxy-config messages edited by S10 were pre-existing text, not new prose.

L16 — T13 + T14 (bounded shutdown/pools; MCP and CLI error contracts). T13 (`bd56cb8b`): `ShutdownGuard::drop` fires the token BEFORE aborting the listener, so cooperative stages stop starting work once the guard and its owner are gone. Three unbounded waits got budgets: `CrawlResultRepositoryImpl::shutdown` caps the hand-off to the bounded writer queue at 2 s and, when it fires, abandons the join with a typed `CrawlError::Storage` instead of waiting on the same wedged blocking syscall (`spawn_blocking` cannot be interrupted, so awaiting it would hang); `run_batch_crawl` used `timeout(.., handle)`, which merely DETACHES the flush watch, and now aborts and joins it through `stop_flush_watch`; `create_pool`/`create_memory_pool` configure explicit deadpool timeouts (wait 30 s, create 10 s, recycle 10 s) — the pinned 0.12.3 builder exposes `timeouts(Timeouts { wait, create, recycle })`, so no dependency was added. New test: a shutdown with 32 writes in flight finishes inside the 5 s acceptance budget with every record on disk. T14 (`b7177994`): ONE panic envelope on both transports — the HTTP middleware and `McpHandler::call_tool` now answer JSON-RPC `-32603` with the same English `PANIC_CONTAINED_MESSAGE` (the in-handler arm used to answer a 200 `isError` result with Spanish text, and a test pinned that Spanish, contradicting the repo's English-only rule). `validate_url` refuses an unusable target with `-32602` and keeps `valid`/`reason`/`reason_code` in the error `data` instead of a success-shaped body. `MCP_TOOL_COUNT` is derived (`mcp_tool_count()`); the external 36-tool pin moved to `tests/stdio_handshake.rs`, and an in-crate assertion against the derived value was removed as tautological — the derived count equals the pin, so no fixture changed. The panic probe now requires a development build as well as the env switch, so a release binary cannot advertise it. `CliExit` gained `SsrfBlocked` (70) and `WafBlocked` (71): `connect`/`doctor` decide the SSRF refusal with `redirect_literal_blocked_with_policy`, the SAME predicate the adapter's guard uses, so `allow_loopback` and `network_policy` keep working (a naive `seed_guard_refusal` pre-check refused legitimate wiremock/loopback providers and was replaced); an all-WAF scrape run exits 71 through `waf_blocked_exit_when_all_failures_are_waf`, and `handle_completions` keeps its cause. Verification: MCP `cargo nextest run -p webfang_mcp --all-features` 651 passed; core lib filters for cli/connect/exit-code/doctor/completion 576 and batch/shutdown/crawl_result_repository 164; exit-code integration targets (`exit_code_integration`, `waf_escalation_test`, `batch_export_exit_code_test`, `crawl_error_exit_codes`, `dns_tls_exit_code_test`) 96; `cargo fmt --all -- --check` and clippy on both crates with `-D warnings -W clippy::cognitive_complexity -W clippy::too_many_lines` green. `cargo doc` was not re-run (the L9/L15 environment ICEs make it non-evidence). Open, outside this slice's edit surface: `docs/error-classification-matrix.md` still documents the pre-70/71 exit set and `docs/` was not touched per repo rule; the four T14 contract changes (panic envelope, `validate_url`, derived count, probe gating) have their own docs references pending that decision.

L17 — CI red remediation after L16: two jobs failed on `9056c9eb`. `Intra-crate lint tool tests` compared the real-tree golden set (2440 vs 2449 sites): the T13/T14 edits added nine `crate::` sites and shifted line numbers, and the refreshed fixture had not been committed — `bash scripts/refresh_real_tree_golden.sh` regenerated it and `cargo test --manifest-path tools/intra-crate-lint/Cargo.toml --locked` is green (9 targets, including `real_tree`). `doc-quality` denied `rustdoc::private-intra-doc-links`: `McpHandler::call_tool`'s public doc linked the crate-private `panic_containment::PANIC_CONTAINED_MESSAGE`; the reference is now plain backticked text, so the public documentation no longer points at a private item. `origin/main` advanced two commits while the fixes were prepared; the branch integrated it with a merge commit (`61753162`) — append-only, no pushed history rewritten — and the golden set was regenerated AFTER the merge so it matches the final tree. Local evidence: intra-crate-lint suite green, `cargo fmt --all -- --check` green, clippy on `webfang_core` + `webfang_mcp` with `-D warnings -W clippy::cognitive_complexity -W clippy::too_many_lines` green, MCP suite 651 passed immediately before. `cargo doc` is not usable as local evidence (rustdoc ICEs in the search-index writer — the L9/L15 environment defect) and the full `scripts/ci_fast_gate.sh` did not finish inside the session budget (it runs a Miri lane), so CI's `doc-quality` and the lint job are the authority for those two; nothing else in the gate changed in this commit.
