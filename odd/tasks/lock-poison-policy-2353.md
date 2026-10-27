# lock-poison-policy-2353

Objective: one documented policy for `std::sync::Mutex` poisoning across `crates/` (issue #2353), replacing the styles that drifted apart.
Branch: `chore/lock-poison-policy-2353` · Worktree: `~/Projects/Rust/webfang-worktrees/chore-lock-poison-policy-2353` · Delivery: PR (owner instruction, supersedes L1).

## The policy

A poisoned lock is **never fatal and never silent**.

- Recover the guard with `PoisonError::into_inner()` and keep going — the guard still grants exclusive access, and every production site that already did this wrote down why (crawl content sinks, MCP session results, preflight notes).
- Where the function already returns `Result` and a recovered write could corrupt an output stream, propagate a typed error instead (`infrastructure::stream`, `sitemap_parser`, `jsonl_writer`).
- Banned in production: `.lock().expect(..)`/`.unwrap()` (one panic becomes a process-wide outage) and `if let Ok(..) = mutex.lock()` (silently drops the work).
- One sanctioned panic: `MockClock`. A poisoned test clock is a test bug, not runtime state.

Recorded in `AGENTS.md` under **Project rules**.

## What the issue got wrong

The issue names three coexisting styles, all in production. Measured: two of the three examples are `#[cfg(test)]` code — `elastic_ingestion.rs` (`.expect`) and `llm/client.rs` + `remote_embedding.rs` (`into_inner`) are test mocks, not production paths. Production carried **five** shapes, not three, and the two the issue did not name were the harmful ones:

| Shape | Where | Verdict |
|---|---|---|
| `if let Ok(guard) = lock()` — silently skip | downloader asset ledger (x3), file trace writer, MCP stdio error slot (x2) | **fixed** |
| `.lock().expect(..)` — panic | vector exporter dimension slot (x2) | **fixed** |
| `.lock().ok()` — silently treat as absent | crawl result repository writer handle | **fixed** |
| `into_inner()` recovery | crawl content sinks, bounded sink, MCP state/server/export/scraping, preflight notes | kept (already documented) |
| `map_err(..)` propagation | vector stream, sitemap parser, JSONL writer | kept (caller returns `Result`) |

## Tasks

- **T1** — Normalize the five silent-skip / panic sites to recovery, each with a one-line reason. **done**.
- **T2** — Document the rule in `AGENTS.md`. **done**.
- **T3** — Enforcement gate. **considered, rejected** — see below.

## Why no gate

`scripts/check_concurrency_lints.sh` is the natural home, but its scan is line-oriented by construction (its own header says so). A `.lock()` chain is routinely three lines:

```rust
self.state
    .lock()
    .expect("...")
```

Catching that needs lookback across arbitrary gaps, and the test-helper files (`test_log_capture.rs`, `test_support.rs`) are separate files that the script's test-module boundary deliberately does not exempt. The result would be a gate with allowlist holes big enough to make it decorative. Rejected on purpose: a rule in `AGENTS.md` plus the reason written at each site is honest; a grep that mostly passes is not.

## Log

- **L1** — Owner instruction, verbatim: «vamos a implementar las issues abiertas 2351/2352/2353/ cada una en su propio worktree y no abrimos pr's».
- **E1** — Census method: every `.lock()` in `crates/*/src/**` before the first `#[cfg(...test...)] mod` of its file. 24 production sites across 12 files.
- **E2** — After T1 the same census reports 4 sites, all `MockClock` (`domain/clock.rs`), each already carrying the "test utility, poisoning indicates a test bug" rationale.
- **E3** — `webfang_core` lib tests: 2986 passed. `webfang_mcp` lib tests: 368 passed.
- **E4** — `cargo clippy --all-targets --all-features -- -D warnings` green; the two `#[allow(clippy::expect_used)]` attributes in `vector_exporter` are gone with the `expect`s they covered.
- **E5** — Behavior change worth naming: the downloader ledger now recovers instead of skipping. Before, a poisoned ledger silently disabled FIFO eviction for the rest of the run (unbounded cache); now it keeps evicting.
- **L2** — Owner instruction, 2026-10-26, verbatim: «primero abrimos las pr's de los otros worktree si PR abierta». Delivery updated from local branch to PR.
