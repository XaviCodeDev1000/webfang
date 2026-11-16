# AGENTS.md — WebFang

Production-ready web scraper. Clean Architecture, AI semantic cleaning, sitemap-based crawling.

**Stack:** Rust 1.91 · Tokio · wreq 6 (TLS fingerprint) · SQLite

> **How to use this file.** It is a **router**, not a manual. Every block either states the
> rule (the one you cannot derive from the code or the compiler) or points at **where the rest
> lives**. The long detail lives in Engram and in the harness.
>
> **Nothing is documented twice.** If a fact has a source of truth — a script, a
> `support.json`, a test, a lockfile — this file **links** that source, it does not copy it.
> A copy in prose is a copy that rots.

## 🔎 Consulting Engram

Everything that is not here is searched in Engram: `mem_search` (MCP) or
`engram search "<query>" --project webfang [--scope personal]`.

| I need | Query | Scope | Obs |
|---|---|---|---|
| Worktrees: bootstrap, seeds, cleanup, target dirs | `worktree target dir seed bootstrap envrc` | project | #2307 |
| Workstation build profile: mbx, mold, OOM, timings | `build profile mbx mold cargo target OOM` | personal | #2306 |
| Support lines, hotfix routing, versioning, governance | `support line routing hotfix branch taxonomy versioning` | project | #2308 |
| PR gates, pre-commit, merge, batch, CHANGELOG | `merge-when-green batch merge pre-commit changelog` | project | #2309 |
| Delivery gates: hooks, push to main, cloud verification | `delivery gates hooksPath main-push verify cloud timeout` | project | #2316 |
| Local gate coverage report (lane ↔ `gate.needs`) | `ci_fast_gate coverage report gate.needs lane partial` | project | #2980 |
| SSH :22 → :443 contingency | `ssh port 22 443 identity` | personal | #2311 |
| Contamination, immutable history, hard-deny | `contamination history immutable hard-deny governance` | project | #2312 |
| Native review (RDD): full cycle | `gentle-ai review assess start consent capture acknowledge` | project | #2213 + skill `rdd-defect-workflow` |
| Skill index with exact paths | `skill-registry skills paths` | project | #43 + `.atl/skill-registry.md` (generated per tree) |
| State persistence, `--resume`, StateStore | `StateStore ExportState load_or_default version bak` | project | #216 + `docs/persistence-resume.md` |
| Where project knowledge lives (owner decision) | `knowledge architecture AGENTS.md decision` | project | #370 |
| AGENTS.md best practices | `agents md best practices` | project | #368 |
| Full block → obs index | `knowledge index bloque destino query verificada` | project | #2313 |

**Sources of truth in the repo — link them, do not copy them:**

| Fact | Source of truth |
|---|---|
| Support-line state | `.github/support.json` (`jq`), rendered into `SUPPORT.md` |
| Inter-crate dependency graph | `cargo metadata` + `scripts/check_dependency_direction.sh` |
| MCP tool count | `crates/webfang_mcp/tests/stdio_handshake.rs` |
| Admitted branch topology | `scripts/check-topology.sh` |
| PR rules CI enforces | `.github/workflows/pr-validation.yml` |
| Version pins in docs | `scripts/check_docs_version_pins.sh` |

## 🔧 Commands

```bash
# Verification (the full CI gate; --all-features is mandatory)
cargo check
cargo clippy --all-targets --all-features -- -D warnings \
  -W clippy::cognitive_complexity -W clippy::too_many_lines
cargo fmt --all -- --check          # --check verifies; a bare `cargo fmt` is a fixer
env "RUSTDOCFLAGS=-D warnings" cargo doc --workspace --all-features --no-deps

# Tests
cargo nextest run --test <name>
cargo insta review                  # review every snapshot diff

# Local gate (lane-aware: docs / CI / code / full)
bash scripts/ci_fast_gate.sh

# Manual checks
scripts/merge-when-green.sh <PR>            # wait for green checks, then merge
scripts/ci_status.sh <PR>                   # read-only summary
scripts/ci_pr_overlap.sh <N1> <N2>          # disjoint files? (batch merge)
scripts/check_windows_platform.sh           # type-check cfg(windows) arms cross-target
                                            # (needs: rustup target add x86_64-pc-windows-msvc)
```

## 🚧 Project rules

What the compiler **cannot** catch. Everything else comes from the harness or the table
above.

- **Never `.unwrap()` in production** — use `?`, `match` or `.context()`.
- **`cfg(windows)` arms are invisible to every Linux gate.** Run `scripts/check_windows_platform.sh`
  on any hand-touched `cfg(windows)` arm (the fast gate runs it when `infrastructure/platform/`
  changes): it type-checks for `x86_64-pc-windows-msvc` in a throwaway crate mirroring the
  workspace lint table, needs only the target's std (no MSVC, no linking), and fails loudly
  naming the `rustup target add` command when missing.
- **A poisoned `std::sync::Mutex` is never fatal and never silent.** Recover with
  `PoisonError::into_inner()` (still grants exclusive access) and keep going; where a recovered
  write could corrupt an output stream and the function returns `Result`, propagate a typed
  error instead (`infrastructure::stream`). Banned in production: `.lock().expect(..)`/`.unwrap()`
  (one panic → process-wide outage) and `if let Ok(..) = mutex.lock()` (silently drops work).
  `MockClock` keeps its `expect`: a poisoned test clock is a test bug, not runtime state.
- **User-facing errors in Spanish; tracing fields and logs in English.**
- **HTTP: always `wreq`, never `reqwest`** — TLS fingerprint impersonation for WAF evasion.
  An agent that suggests `reqwest` is wrong.
- **Do not unify conflicting dependencies** (dashmap 5/6, selectors 0.35/0.37/0.38): they are
  deliberate, three different crates need three different versions.
- **Do not cite line numbers.** Cite the symbol (`WreqDownloader::fetch_inner`) or the path. A
  line-number citation is invalidated by every edit above it in the file.
- **Do not copy mutable state into prose.** If it changes on its own (versions, counts, support
  lines, labels), link the source.
- **Do not touch `CHANGELOG.md`** in a work PR — there is a single writer (the Release PR).
- **Do not touch `docs/`** unless explicitly asked.

### The local gate mirrors `gate.needs`

`scripts/ci_fast_gate.sh` is the local equivalent of the PR fast gate, **not** of the whole
workflow. Every run ends with a coverage report naming which `gate.needs` jobs the lane
covers, covers only in part, or never touches. **Read it before pushing:** when it names a job
you touched, run that job's check by hand — a green local gate is not evidence for what it
does not cover. The declaration is `scripts/ci_fast_gate_coverage.tsv`, kept in sync with
`gate.needs` by `scripts/test_ci_gate_structure.sh`. The deliberate local exceptions (by class)
and the lane model: **obs #2980**.

## 🏗️ Architecture

### Workspace map (5 crates)

```text
webfang/                          # virtual workspace root (no [package])
├── crates/
│   ├── webfang_core/             # domain + application + infrastructure + cli
│   ├── webfang_mcp/              # MCP server (tool count pinned by stdio_handshake.rs)
│   ├── webfang_cli/              # the `webfang` binary (main.rs)
│   ├── webfang_test_utils/       # shared test harness (not published)
│   └── webfang_benchmark/        # benchmark harness (leaf, tooling)
```

- **The CLI code lives in `crates/webfang_core/src/cli/`** (`error.rs`, `scrape_flow.rs`,
  `args/`, `preflight.rs`, `orchestrator.rs`). `crates/webfang_cli/src/main.rs` is the thin
  binary. Look for the module, not the crate.

### Dependency direction (POLICY, enforced by a gate)

| Crate | May depend on |
|---|---|
| `webfang_core` | — |
| `webfang_mcp` | `webfang_core` |
| `webfang_cli` | `webfang_core` |
| `webfang_test_utils` | `webfang_core` |
| `webfang_benchmark` | `webfang_core`, `webfang_test_utils` |

- The **direction is `cli → core`, `mcp → core`; `cli` does NOT depend on `mcp`**. If you
  need something from the MCP server in the CLI, that is an architecture decision, not an
  import.
- **Dev tier:** any crate may target `webfang_test_utils` from `[dev-dependencies]` (it is
  the shared harness and is never published). An edge from `[dependencies]` is prohibited.
- Gate: `scripts/check_dependency_direction.sh` (runs in the `repo-guards` job of `ci.yml`),
  with its semantics harness `scripts/test_dependency_direction.sh`.

### Intra-crate layers (Clean Architecture)

```text
infrastructure → adapters → application → domain   (inward only)
```

Domain defines ports (traits) → Infrastructure implements them → Application orchestrates. Copy
the closest equivalent module: trait in `application/crawler/engine.rs`, domain entity in
`domain/entities.rs` (constructor + `TryFrom` + `Display`/`Debug`/`PartialEq`), adapter in
`infrastructure/`, new error in the layer's error file.

**Avoid giant components:** no new file over ~500 lines; if you touch one that already exceeds
it, split it. Largest files: `find crates -name '*.rs' -not -path '*/tests/*' | xargs wc -l | sort -rn | head`.

### Error stratification

The application error is `ScraperError` (`crates/webfang_core/src/error.rs`) — a **funnel**, not
a chain of wrappers: `DomainError`, `InfraError`, `CrawlError` and `DownloadError` each implement
`From<…> for ScraperError` (they are **inputs**; `domain/error/domain_error.rs` defines no
`impl From` itself).

- `CliError` (`cli/error.rs`) is **independent** of `ScraperError`; it maps to exit codes
  (`EXIT_CONFIG = 78` for a configuration error).
- `HttpError` is a **domain** enum (`domain/http_error.rs`), not an `InfraError` variant.
  `WafError` is **not an error type** — it is a `FetchBehavior` variant
  (`application/crawler/crawl_task.rs`). `ParseError` does not exist here.
- A new variant enters through `From` towards `ScraperError`; its user-facing message is Spanish.
  Full matrix and exit-code rules: `docs/error-classification-matrix.md`.

### Fetch guard chain (MANDATORY ORDER)

Every fetch path — CLI, engine, MCP tool, benchmark, future adapter — must wire the protections
**in this order**; each stage assumes the previous one filtered. Deviating is a **bug**, not a
style choice (pacing after a doomed request burns rate budget; classifying retries after reading
the body wastes a full read of a response that will never be accepted).

```text
ValidUrl (entry) → rate limit (pacing) → per attempt:
    timeout → redirect policy → SSRF at the socket dial (every hop)
→ retry classification → body read
```

| # | Stage | Reference implementation | Why |
|---|---|---|---|
| 1 | Entry validation | `ValidUrl` / `try_from_url` (`domain/value_objects.rs`), built at the argv boundary | Untrusted URL text becomes a validated domain type before entering any flow. Never accept a raw `Url`/`String` as a fetch target. |
| 2 | Rate limit (pacing) | Scrape: token bucket in `scrape_urls` (`cli/scrape_flow.rs`, `build_scrape_rate_limiter`), with cancel-aware pacing via `until_ready_or_cancel_observed` (`application/rate_limiter.rs`). Crawl: `RateLimiterConfig` (`application/rate_limiter.rs`), constructed in `application/crawler/engine.rs` from `delay_ms` + budget burst | The wait happens **before** the network is touched: a cancelled wait counts as skipped, never as a blocked fetch. |
| 3a | Timeout | `Client::builder()` in `infrastructure/downloader/wreq_downloader.rs`: request timeout + connect timeout (the connect one is clamped in `infrastructure/downloader/fetch_router.rs`) | Per-request ceiling. |
| 3b | Redirect policy | `redirect_policy()` (`domain/ssrf_guard.rs`): 10-hop limit + synchronous stop when the redirect target is a forbidden literal IP | Every redirect hop re-enters the chain: it is a new dial, not a free pass. |
| 3c | SSRF at the dial | `ssrf_guard()` wrapping the `Client` (`infrastructure/downloader/wreq_downloader.rs`, implementation in `domain/ssrf_guard.rs`): resolver-level check on **every** connection; private / link-local / loopback ranges rejected before the socket exists | Nothing may connect to a forbidden address, neither by DNS name nor through a redirect hop. E2E in `crates/webfang_core/tests/ssrf_rfc1918_e2e_test.rs`: pre-socket rejection of RFC1918/CGNAT/NAT64/mapped targets, exit 69, zero outbound packets. |
| 4 | Retry classification | `fetch_inner` (`infrastructure/downloader/wreq_downloader.rs`): mid-body transients (`ConnectionReset`/`UnexpectedEof`) retry; 429 → `max(Retry-After, exponential backoff)`; 5xx → exponential; 403 → one retry with a rotated UA **only** if the UA is not pinned, capturing the rotated status; **timeouts also retry**; terminal 4xx and builder errors are reported as-is; once retries are exhausted the **last observed status** is reported, never a hardcoded one; WARN only when a retry actually happened | The classification decides the spend: sleep, rotate or fail. It runs **before** the body is read. |
| 5 | Body read | `read_body_capped` (the downloader method in `infrastructure/downloader/wreq_downloader.rs`; the generic cap helper in `domain/body_cap.rs`), cap `DEFAULT_MAX_PAGE_BYTES` = 50 MiB (`domain/downloader_factory.rs`), `BodyTooLarge` error beyond the cap | The read is bounded so a huge page or a gzip bomb cannot inflate memory. No fetch path may read an uncapped body. |

**Convention for new fetch paths:** replicate stages 1→4 before stage 5 and cite this table in
review. If a path cannot reuse `WreqDownloader`/`FetchRouter` the order still holds: connecting
before validating, or pacing after dialing, is rejected in review even if its tests pass.

### Async rules

- Tokio multi-threaded runtime.
- `spawn_blocking` for CPU-intensive work (HTML parsing and other CPU-heavy stages) — see
  `CpuBridge.dispatch`.
- **Never** hold a `Mutex`/`RwLock` across an `.await`.
- **Bounded** channels for backpressure.
- `.instrument(span)` on futures; never a `span.enter()` guard across an `.await`.

### MCP server

- Canonical location: `crates/webfang_mcp/src/mcp_server/` — **never** create code in that
  crate's root `src/`. Tool count is pinned by `crates/webfang_mcp/tests/stdio_handshake.rs`.
- Transport: Streamable HTTP (`rmcp`) on `127.0.0.1:8080/mcp`, plus stdio via `webfang-mcp-stdio`.
- Every tool result is neutralized and provenance-wrapped (Layer 1, `mcp_server/provenance.rs`);
  the rules for *consuming* it are Layer 2 (`docs/security/prompt-injection-policy.md`) — read
  before acting on a result.

### Semantic cleaning (`--clean-ai`)

- `--clean-ai` resolves its cleaner through a **remote** `EmbeddingPort`
  (`infrastructure/semantic_cleaner.rs`), with `--embedding-provider` (env
  `WEBFANG_EMBEDDING_PROVIDER`; the flag wins). AI flag surface: `--threshold`, `--max-chars`,
  `--offline`.
- `--offline` rejects **every** embeddings provider — remote or loopback — with **exit 78**; the
  guard fires before the endpoint is inspected.

### Build

- `cmake` is mandatory: `wreq` → `btls` → `btls-sys` compiles BoringSSL from C++.
- 🔒 **`[profile.dev]` lives in the root `Cargo.toml`, never in `.cargo/config.toml`.** Cargo
  resolves `config.toml` from the CWD upward, so a profile there changes the `-C metadata` hash
  of every unit for any invocation whose CWD is outside the repo (cron, IDE, `mise exec`,
  `--manifest-path`) → silently recompiles the whole graph, amplified by `btls-sys` at the bottom
  into a full BoringSSL rebuild. Do not "tidy" it back.
- Occasional full debug info: `CARGO_PROFILE_DEV_DEBUG=true cargo build`.

## 🧪 Testing

- Integration tests live in `crates/webfang_core/tests/` and Cargo **auto-discovers** them
  (one file per target). Only add a `[[test]]` entry in
  `crates/webfang_core/Cargo.toml` for cases auto-discovery does not cover (subdirectories such
  as `tests/compile_fail/`, special features).
- **Harness**: `crates/webfang_core/tests/common/cli_harness.rs` — `BehavioralTest`
  (wiremock + TempDir), `scraper_cmd()`, `find_files()`, `read_md_content()`.
  Import it with `#[path = "common/mod.rs"] mod common;`.
- **Binary resolution — golden rule:** always use `webfang_path()`, **never**
  `assert_cmd::cargo_bin(...)`. The `CARGO_BIN_EXE_*` env var is only set for the owning
  crate, and in this virtual workspace the `webfang` binary is built by `webfang_cli`, a
  sibling crate: tests running under `webfang_core` cannot resolve it with `cargo_bin`.

```bash
Command::new(webfang_path())   // ✅
Command::cargo_bin("webfang")   // ❌
```

- **Snapshots (`insta`)**: every test that produces Markdown/JSON/stderr must use snapshots, not
  `assert!(output.contains("..."))`. Cycle: `cargo nextest run` (fails with `.snap.new`) →
  `cargo insta review` (review every diff) → `cargo nextest run` (green). `.snap.new` is
  gitignored: never commit pending snapshots.
- **Mandatory sanitization**: `redact_nondeterministic()` normalizes the TempDir path →
  `[TEMP_PATH]`, ISO-8601 timestamps → `[TIMESTAMP]`, wiremock ports → `[PORT]`, ANSI →
  `[ANSI]`. For extra non-deterministic fields, `insta::with_settings!({ add_filter(...) })`.
- **Six-node diagnostic** for every test you write or modify:
  1. **Observable behavior** — exercise public ports, never internal state.
  2. **Ephemeral adapters** — wiremock for HTTP, TempDir for the filesystem. No real network.
  3. **Semantic assertions** — business invariants through snapshots, not data dumps.
  4. **Effort distribution** — maximum investment in the stable domain logic.
  5. **Arrange simplicity** — an Arrange longer than 5 lines is a production design defect, not
     a test defect.
  6. **Absolute determinism** — injected time and randomness, zero flakiness.
  If the Arrange got complicated, fix the production design, not the test.
- To create an integration test: a file in `crates/webfang_core/tests/` (no `[[test]]`), the
  harness import, `webfang_path()` for the binary, snapshots for the output, and
  `cargo nextest run --test <name>`.

## 🌭 Observability

**Iron rule:** every feature, hot path or behavior change ships with observability — code that
cannot be traced in production is not finished. Stack: `tracing` + **FileTraceLayer**
(`--trace-file out.jsonl`) + native correlation IDs. **No external metrics backend**: emit a
structured `tracing` event, query the JSONL (`docs/src/debugging.md`, `scripts/analyze-trace.sh`).

| Situation | Requirement |
|---|---|
| New hot path / operation | `#[instrument(skip(...), fields(url = %url, ...))]` with the fields that identify the operation |
| Error path | `log_scrape_error(&err, url, stage, correlation_id, "context")` — never a bare `warn!`/`eprintln!` |
| New crawl/batch flow | Generate a `CorrelationId` at the entry point and propagate it; each unit of work gets a `.child()` |
| Long-running operation | Periodic progress log + final structured summary (`total`, `succeeded`, `errors`, `duration`, `trace_id`) |
| Async spans | `.instrument(span)` on futures; never a `span.enter()` guard across an `.await` |

- **Structured fields, not string soup:** `tracing::info!(pages = n, url = %url, "msg")`.
- **Correlation:** every event carries a 16-hex `trace_id`, **ephemeral within the run** (never
  persist or join across runs; the durable cross-run identity is the `CorrelationId` UUID). It
  sits at **top level**, not `.fields.trace_id` (populated only on the error path); `span_close`
  records share the `.span` name, so per-stage counts exclude `.record == "span_close"`.
- **Deterministic snapshots:** `correlation_id`/`trace_id` are internal, `#[serde(skip)]`.

## 🔬 Intelligence Stack

Two complementary tools **mandatory** for all source-level work — pick by mission, never reach
for classical tools first. They replace `grep`/`rg`, `find`/`fd`, `cat`/`head`/`tail` and `Read`
for source code. **They are CLIs, not skills** — invoke them as commands, not by loading a skill.

| Moment | Tool | Why |
|---|---|---|
| "What is this task about?" — first orientation | **CodeDB** `context` | 1 call: keywords + symbol defs + ranked files + snippets. Replaces 3-5 sequential calls. |
| "Show me the code of X" — explore and understand | **CodeGraph** `explore` | Returns the verbatim source + call paths + blast radius in ONE call. Eliminates the grep→Read loop. |
| "Where is X defined?" — lookup | **CodeDB** `word` / `symbol` | Inverted index, O(1). |
| "Who calls X?" — tactical check | **CodeDB** `callers` | 1 round-trip: fuses the word index with the outline scope. |
| "How does X flow through the code?" — deeper | **CodeGraph** `explore` / `impact` | Call paths + blast radius summary. |
| Post-edit diagnostics | `cargo check` / `cargo clippy` | codedb has no diagnostics tool: the compiler is the gate. |
| Query a public GitHub repo (no clone) | **DeepWiki** MCP (`read_wiki_structure`, `ask_question`) | Separate remote MCP, registered by the codedb installer. codedb itself is local-only. |

**Pocket rule:** CodeDB to *find and read* (tactical, O(1)); CodeGraph to *explore and
understand* (returns source, call paths and blast radius).

### Non-negotiable gates

- **Mandatory instead of classical tools:** query `codedb` (`word`, `symbol`, `callers`, `find`,
  `glob`) or `codegraph explore` for any source definition, usage or flow. `grep`/`rg`,
  `find`/`fd`, `cat`/`head`/`tail` and `Read` are **prohibited** on source code.
- Before editing a symbol → `CodeGraph explore` (callers + impact); **never** edit blind. Before
  renaming → check **all** usages (`codedb callers` / `CodeGraph explore`).
- Before committing → `cargo check` + `cargo clippy` + `cargo fmt --all -- --check`, re-read the diff.
- **Legitimate exceptions to `grep`/`rg`:** logs, CI output, `.env`/config text, files outside the
  index — never source code.

### Absolute paths in worktrees (CRITICAL)

In a worktree **both** tools need its **absolute path** (`project=` for CodeDB MCP,
`projectPath=` for CodeGraph MCP) — the bare repo name silently resolves to main and is
ambiguous across worktrees. **CodeDB CLI is POSITIONAL-FIRST:** `codedb <abs-root> <cmd>`; only
`mcp` reverses (`codedb mcp <path>`). `status`/`reindex` **ignore a trailing path** and act on
the cwd project (exit 0), so verify with `codedb <abs-root> status`: `root` must be the worktree
and `head` must match `git -C <abs-root> rev-parse --short HEAD`.

## 🌳 Worktree policy

Every active branch lives in its own **sibling** directory of the repo (never inside it: in-repo
worktrees break file watchers, ripgrep and the code intelligence tools). The `.git` object store
is shared; working tree, index and HEAD are isolated.

### Iron rules

- **The CWD is the absolute boundary.** Never reach paths outside your worktree through
  `../<other-worktree>/`.
- **One worktree per session.** Never switch branches mid-task: create another worktree.
- **The mission record travels with the branch.** `odd/tasks/<feature>.md` is created and
  edited INSIDE the mission worktree, never in the main checkout: written in the main
  checkout it stays orphaned in `main` (untracked) and the finished work never reaches the
  tree (#1972).
- **`.git/worktrees/` is Git's internal state.** Never edit it by hand: use
  `git worktree add` / `remove` / `prune` / `repair`.
- **Forbidden commands:**
  - `git checkout` / `git switch` — they change the branch inside the current worktree; use
    `git worktree add`. The hatch `git checkout --ignore-other-worktrees -B <branch>` (flag
    **before** `-B`) is the same class of escape as `--force`: explicit human authorization only.
  - `git stash` (any form) — `refs/stash` is **shared by ALL worktrees**: a `pop` can apply
    another session's stash. To set something aside, commit it on a throwaway branch.
  - `git worktree move` / `lock` — use `remove` + `add`.
  - `git worktree add --force` — bypasses Git's guard against a branch already checked out in
    another worktree (two agents on one branch is exactly the failure it prevents).

### Placement, naming and the identity proxy

Layout: `~/Projects/Rust/webfang/` (main, always on `main`) and sibling
`~/Projects/Rust/webfang-worktrees/<dir>/` (globally gitignored), e.g. `feat-auth/` ↔ `feat/auth`.

**Never put a worktree under `/tmp`** — it is a 16 GB tmpfs here and a from-scratch `target/`
does not fit: the build dies with `Disk quota exceeded`, and a backgrounded build that dies
that way returns an **empty log** (looks like it never ran). For a throwaway tree, point
`CARGO_TARGET_DIR` at an existing worktree's `target/` instead of building a second one.

The convention is `/ → -` (`feat/auth` → `feat-auth`). **The invariant is that the commit lands
on the branch intended for the task/PR.** Run the name proxy before every commit in a worktree:

```bash
[ "$(basename "$PWD")" = "$(git branch --show-current | tr '/' '-')" ] || echo "MISMATCH"
```

### Identity-exception protocol

A failed proxy does not prove you are in the wrong worktree, nor does the branch name prove you
are in the right one. Committing under a directory/name mismatch is allowed only when **all
three** hold:

1. **Externally anchored expectation** — the intended branch is known from a source fixed
   outside this worktree *before* committing (PR metadata `gh pr view --json
   headRefOid,headRefName`, task metadata, an upstream ref, or an explicit human instruction). A
   branch that only exists inside the worktree proves nothing.
2. **Verifiable binding** — `git symbolic-ref --short HEAD` and `git worktree list` agree
   (path → branch); `git rev-parse @{u}` is an extra signal with an upstream.
3. **Ancestry, not equality** — pre-commit HEAD is at or a descendant of the expected tip/base
   (a strict `HEAD == tip` check breaks after the first commit of a sequence).

Every exception commit records the evidence in its body:

```text
Worktree-Identity-Exception: directory-name mismatch
Intended-Branch: <branch>
Expected-Head: <sha>
Evidence: <external source> + git symbolic-ref + git worktree list
```

If any condition fails, MISMATCH is a hard stop.

### Per-worktree bootstrap

The `.envrc` is **per worktree**, never shared, written **from the tree's own name** (never
derived from main's — a `sed` over main's silently points `CARGO_TARGET_DIR` at MAIN's target).
It is **exactly 5 lines**; `$TREE` is substituted when written, only `$HOME` stays literal — a
literal `$TREE` left in the file makes the isolation guard reject the build
(`reason=unregistered-target`). Then `direnv allow`, `cp` main's `.env`, and index the tree
(`codegraph init`; `codedb "$PWD" reindex && codedb "$PWD" status`). **Without both indexes the
agent is blind**: the tools resolve to main or return nothing; restart the MCP connection after
indexing. The heredoc, the seed contract (`seed_target.sh`: `cold` normal, `refused` not a
build) and the paid-for mistakes: **obs #2307**.

### Frequent commits (mandatory in worktrees)

Commit **after every completed step**: a `git mv`, a bulk replace, a passing `cargo check`, the
tests in green. Uncommitted work in a worktree can be lost silently if you lose context or a
checkout happens. The full pattern (what goes into a commit and what does not) is in the
`work-unit-commits` skill.

### Post-merge cleanup (the merge is not done until the repo is ready)

From the **main** repo, after verifying `state: MERGED`: sync main **ff-only** (if it fails,
main diverged — **STOP**, do not paper over it), remove the mission's worktree, delete its
local branch with `-D` (squash rewrites history; never touch `main`, `gh-pages`, `backup/*` or
the current branch), `git worktree prune`, then delete the mission's target dir **by
ownership** (the exact name you exported — never enumerate the parent). Re-check with a moving
window (file count stable across 60 s + zero live `cargo`/`rustc`/`cargo-nextest`) and remove it
as a **standalone** command with the literal absolute path, never `$HOME`/`~` and never chained
(`rm -rf ~` is hard-deny and blocks the chain). Verify the handoff: no leftovers **of the
mission you merged**. The 7-step runbook: **obs #2307**.

### Shared vs per-worktree resources

`.git/`, config/hooks and `Cargo.lock` are shared; `target/`, `.envrc`, `.env` and the indexes
are per worktree; `seeds/` read-only, `quarantine/` never a target, `refs/stash` shared
(**never** `git stash`). Full table: **obs #2307**.

### Reserved `~/.cache/cargo-target/` namespace

`<worktree-target>/` is build state owned by one agent; `seeds/` (published store) and
`quarantine/` (retired objects) are **reserved** and can never be a worktree's
`CARGO_TARGET_DIR`. Do not invent new categories (`scratch/`, `shared/`, `cache/`, `tmp/`): the
guard rejects three known identities and accepts anything else, so a name nobody thought of
gets in through the back door — a denylist does not stop invention, the explicit namespace
does. Detail: **obs #2307**.

### Contamination

If you operated outside your worktree, or a `git stash pop` applied unexpected changes:
**STOP** immediately; do **NOT** clean up (no `git reset`, force-push or manual patching); report
exactly *"Contamination detected. Worktree: `<path>`. Intruder commit: `<hash>` or unexpected
stash applied. Awaiting human instructions."*; wait for explicit human authorization before any
corrective action.

### Pushed history is immutable

Once a commit exists on `origin/<branch>` its SHA is public; rewriting it (`git reset HEAD~N` +
recommit) desynchronizes local from remote and the only way back is a force-push, which the
harness **hard-denies**. The correction lands **on top**, as a new commit.

| Situation | ❌ | ✅ |
|---|---|---|
| Pushed commit is defective | `git reset HEAD~N` + recommit | a new commit that describes the fix |
| Pushed commit needs a better message | `git commit --amend` over pushed history | a new commit (or `--amend` only if not pushed) |
| Commit **not** pushed | — | `git commit --amend` |

⚠️ The gate is asymmetric: `git reset --hard` is hard-denied but a bare `git reset HEAD~N` is not
gated — you can walk into a rewrite and only discover at the end that the push is blocked.

## 🏷️ Branching, releases and support lines

- **Taxonomy — only these exist:** `main` (development; bases `feat/*`, `fix/*`,
  `refactor/*`, `perf/*`, `docs/*`, `test/*`, `chore/*`, + `ci/*`, `build/*`, `style/*`,
  `revert/*`) · `release/X.Y` (temporary stabilization for a MINOR/MAJOR, cut by the maintainer
  from an explicit main SHA; accepts only `fix/*`; deleted when `vX.Y.0` ships; a PATCH never
  creates one) · `support/X.Y` (maintenance line, created on demand from the line's latest tag
  when it needs a patch while main is ahead; accepts only `hotfix/*`; deleted at EOL — but
  `support.json` is the source of truth, not the branch) · `hotfix/*` (MUST be based on
  `support/X.Y`; PR base = `support/X.Y`, never main).
- **The state of the lines is not here.** Read `.github/support.json` (jq). The full model, the
  hotfix routing and the governance scripts: **obs #2308**.
- **Forbidden operations (review-blocking, enforced in CI):** merging `main` INTO
  `support/*` · cherry-picking `main` → `support/*` without an approved issue · tagging
  `vX.Y.Z+1` from `main` while it is ahead · a hotfix PR with base `main` · creating
  `support/*` "just in case" · editing `SUPPORT.md` by hand · running `release-plz` on a support
  branch · batching PRs with different `baseRefName` (disjoint files is not enough).
- **Versioning:** version bumps are release acts, never part of a feature/fix commit.
  Inter-crate pins stay at `^old`; only `[workspace.package]` moves. `release-plz` governs
  `main`; on `support/*` the PATCH is manual, inside the hotfix PR.

## 🔒 Permissions and limits

### Allowed without asking

- Read any file in the repo.
- `cargo check`, `cargo clippy`, `cargo fmt`, `cargo nextest run`.
- Both intelligence tools (CodeDB, CodeGraph).
- Edit inside `crates/`, `crates/*/tests/`, `benches/`, `examples/`.
- Worktree management: `git worktree add`, `remove`, `list`, `prune`.
- Read-only cross-branch inspection: `git show <branch>:<file>`, `git log <branch>`.

### Ask first

- Adding or removing dependencies (`Cargo.toml`).
- Changing feature flags or profiles.
- Deleting files.
- `cargo build --release` or `cargo llvm-cov`.
- Modifying CI/CD (`.github/`).
- New files outside `crates/`, `crates/*/tests/`, `benches/`, `examples/`.
- Merging a PR (the maintainer decides the merge).

### Never

- Commit secrets, `.env` or credentials.
- `.unwrap()` in production.
- Force-push to main.
- Modify `target/`, `dist/`, `build/`.
- `git checkout` / `git switch` to change branches (use `git worktree add`).
- `git stash` in any form.
- Reach sibling worktrees through relative paths (`../feat-auth/...`).
- Commit in a worktree with a branch/directory mismatch without the exception protocol.
- Use the bare repo name for the intelligence tools in a worktree.
- Rewrite pushed history (`git reset` / `rebase` over commits already on
  `origin/<branch>`).

### Hard-deny: there is no approval path

Some command classes open a dialog the human approves; others are **hard-deny** and never show
one. Conversational authorization **cannot** change a hard-deny decision — there is no dialog to
press.

| Class | Examples |
|---|---|
| **Confirm** (dialog → approve → runs) | `git push`, `git rebase`, forced `git branch -D`, `npm publish`, `pi remove` |
| **Hard-deny** (nothing is approvable) | force-push (`--force`, `--force-with-lease`, `-f`), `git reset --hard`, `git clean -f -d`, `rm -rf /` or `~`, `chmod -R 777`, `chown -R` |

When a hard-deny blocks: **do not re-ask in conversation** (structurally useless) and **do not
route around the pattern** (splitting, reordering flags, wrapping in a script — the list is
deliberately non-configurable). **Report the block once**, with the exact command for the human
and a non-destructive equivalent you can run yourself.

## 📝 Commits, PRs and CI

**Format:** `type(scope): description` — type: `feat` | `fix` | `refactor` | `test` | `docs` |
`perf` | `chore` | `revert` — scope: `cli` | `crawler` | `ai` | `mcp` | `exporter` | `http` |
`domain` | `infra`.

**The 5 PR gates** (job `Validate PR metadata`, required, no `continue-on-error`):

1. **Linked issue** — body carries `Closes #N` / `Fixes #N` / `Resolves #N`. For a partial slice
   of an umbrella use `Closes part of #N` (passes validation, does **not** close the umbrella; a
   bare `Closes #N` auto-closes it mid-plan).
2. **The linked issue carries `status:approved`** before the PR is opened. Protected label:
   **never** self-approve. If the PR opened before the label landed, validation does not
   re-evaluate on its own: `gh run rerun <id> --failed`.
3. **Exactly one `type:*` label** — vocabulary distinct from commit types; the map lives in
   `scripts/auto_label_type.sh` (`--map` prints it), applied from the PR title by the auto-label
   job of `pr-validation.yml`.
4. **Conventional branch name** — the regex lives in `pr-validation.yml` (do not restate it).
5. **No AI attribution** in the PR's commits.

**The changelog line is the squash subject, and that is not always the PR title.** The queue
squashes: **two or more** commits → the PR title; **exactly one** → that commit's subject (the
title is ignored). `cliff.toml` keeps only the first line. **Set the commit subject of a
one-commit branch to the line you want in the changelog** — renaming the PR changes nothing.

**Pre-commit (must replicate CI exactly):**

```bash
cargo check && cargo clippy --all-targets --all-features -- -D warnings \
  -W clippy::cognitive_complexity -W clippy::too_many_lines \
  && cargo fmt --all -- --check \
  && env "RUSTDOCFLAGS=-D warnings" cargo doc --workspace --all-features --no-deps
```

- `--all-features` is a **safety flag**: `chromium`-gated code has its only consumers behind
  `#[cfg(feature = "chromium")]`, so clippy without it sees those imports as dead and
  `clippy --fix` would **delete** them. Never auto-fix against a narrower feature set.
- A bare `cargo fmt` is a **fixer** (rewrites the tree, exits 0 regardless): green from it is not
  evidence. Verify with `--check` and commit the result.
- The fourth command replicates CI's `doc-quality` job (denies rustdoc lints);
  `check`/`clippy`/`fmt` never document an item.

**Checklist before opening the PR:**

- [ ] `bash scripts/ci_fast_gate.sh` GREEN; coverage report read (every `NOT covered` job you touched run by hand)
- [ ] Full pre-commit gate above; `cargo nextest run` (at least the affected module)
- [ ] `git diff --stat main...HEAD` reviewed: only expected files changed
- [ ] Spanish user-facing errors; new public items documented; `CHANGELOG.md` untouched
- [ ] One `type:*` label + linked issue **with** `status:approved` + conventional branch
- [ ] Worktree branch matches the directory; no `checkout`/`switch`/`stash` this session

**Merge:** `scripts/merge-when-green.sh <PR> --queue` — waits for required checks, enqueues,
waits for `MERGED`. The queue is the normal path to `main` (no direct merge); never `--auto` or
`--admin`, no strategy flag (the queue owns the method). `BEHIND` is not fatal. Batches are queue
groups: same base, disjoint files (`scripts/ci_pr_overlap.sh`); the queue does not resolve conflicts.

**Watcher fallback:** if the script exits 4 while the PR is verifiably ready (`mergeStateStatus`
`CLEAN` **and** every required context reported green in the rollup), enqueue manually with a
bare `gh pr merge <N>` (no strategy flag) and poll `state` until `MERGED`. It replaces the
waiting, never the verification: a context that has not *reported* green is the #1011
false-green mode, not a candidate. The queue deletes the remote branch on merge (a vanished
`origin/<branch>` afterwards is expected). Diagnosis: `merge-when-green context read fallback
manual enqueue` in Engram.

**Breaking change (maintainer decision):** the `!` marker must sit on the surface the lander
keeps. SQUASH + **one commit** → the commit subject (`type!:`, `type(scope)!:`); SQUASH + **two
or more** → the PR title; MERGE → the branch commits survive, the title is discarded.
`type!(scope):` is malformed and a `BREAKING CHANGE:` footer alone does not satisfy the gate;
`merge-when-green.sh` refuses both (exit 5). Never leave the `!` only on a surface the chosen
strategy drops.

**The same invariant is a required CI check (`breaking-surface`).** The decision matrix lives in
`scripts/breaking_surface_lib.sh` (shared by the CI check and the local guard, so they cannot
drift); the merge method is read from the tracked mirror `.github/merge-queue.json`. Fail
closed. The local guard is defense in depth at the merge boundary. Operational detail: **obs
#2309**.

**Hooks and verification:** `pre-commit`/`pre-push` consult the review gate; enable the wiring on
a fresh clone with `git config core.hooksPath scripts/githooks`. Wait for CI with the non-blocking
pattern (`gh run list --workflow=ci.yml …`), never `gh run watch` under a short timeout; a push
that timed out did not necessarily fail (verify with `git ls-remote`). Detail: **obs #2316**.

**There are no stacked PRs in this repo.** `scripts/check-topology.sh` admits only the
head-prefix → base pairs of its table; **no feature head prefix may target another working
branch** (`fix/*` → `release/X.Y` and `hotfix/*` → `support/X.Y` are admitted — versioned lines,
not working branches). Use **sequential delivery**: slice 1 as a normal PR on `main` → merge →
slice 2 from the new `main` → repeat. Never retarget an open PR onto a working branch to "shrink
the diff"; rebuilding on `main` is always available and legal.

## 🧾 Native review (RDD)

The native review is an **outside view**, never a delivery authorization: after the
acknowledgement, commit / push / PR / release stay under ordinary repository policy.

- The **candidate** is one reviewable slice (a work-unit commit or PR slice) — never a TODO,
  never the accumulated branch. **Consent is per candidate**, not the global switch's kill
  switch: never fabricate an approval or re-enable the switch for the user.
- The full cycle, the 4 lenses, failure envelopes and recovery paths: **obs #2213** + the
  `rdd-defect-workflow` skill.

## 🗺️ Skills and CLIs

- **Skill index:** `.atl/skill-registry.md` — generated **per tree** and gitignored (run
  `gentle-ai skill-registry refresh` in a fresh clone). It is an **index** for delegators: the
  `SKILL.md` is the source of truth — **inject exact paths**, not summaries. The registry skips
  `sdd-*`, `_shared` and `skill-registry` by contract. **CodeDB and CodeGraph are CLIs, not
  skills** (§ Intelligence Stack).
- **Loading the right rules is mandatory:** before any Rust change, resolve the applicable skill
  by code context via the registry and load its `SKILL.md` (or installed `<id>`) **before**
  editing, forwarding the exact rule set to any delegated writer. `rust-skills` (265 rules / 26
  categories) and `rust-best-practices` (distilled checklist) cover the same ground at different
  granularity; `rust-review` for a cited review report. Other relevant skills: `branch-pr`,
  `work-unit-commits`, `chained-pr`, `rdd-defect-workflow`, `cognitive-doc-design`,
  `codebase-refactoring`, `dry-refactoring`, `jscpd`, `webfang-architecture`,
  `webfang-worktree-ops`, `webfang-ci-gate`, `webfang-testing`, `webfang-fetch-path`.
- **Every delegation to a subagent must include:** the skill paths to read, the intelligence
  mandate (explore impact before editing; `cargo check` before returning), the exact verification
  commands, and the absolute worktree path.

## 💾 Engram (correct usage)

The server resolves the active session from the working directory; concurrent sessions can
make `mem_save` / `mem_judge` fail with `multiple active runtime sessions match…`.

- **Never invent a `session_id`.** Register one with `mem_session_start` and pass that same ID
  to every `mem_save` / `mem_judge` of the session. **Close it when done**
  (`mem_session_end(id, summary)`): each registration writes a 30-minute runtime lease, and an
  expired lease drops out of candidate resolution by itself.
- If `mem_save` returns `judgment_required: true`, judge **each** candidate with **its own**
  `judgment_id` (from `candidates[]`), never the top-level one.
- On **any** engram error run `engram --help` first (then `engram <cmd> --help`) before retrying
  or inventing flags; the CLI (`engram save|search|context`) is the MCP fallback. Diagnose the
  collision with `engram doctor --check ambiguous_active_runtime_sessions --project webfang`
  (diagnostic only): two genuinely-alive sessions colliding is by design — end the finished one
  or pass `session_id`.
- **Project knowledge lives here**, not in the repo: decisions, direction, audit findings,
  reasoning. The repo holds the **rules** and their sources of truth (owner decision: obs #370).
