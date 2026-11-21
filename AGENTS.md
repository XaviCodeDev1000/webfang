# AGENTS.md — WebFang

Production-ready web scraper. Clean Architecture, AI semantic cleaning, sitemap-based crawling.

**Stack:** Rust 1.91 · Tokio · wreq 6 (TLS fingerprint) · SQLite

> **How to use this file.** It is a **router**, not a manual: every block states a rule the
> compiler cannot catch, or points at where the detail lives (Engram, harness, scripts).
> **Nothing is documented twice** — a fact with a source of truth is linked, never copied;
> a copy in prose is a copy that rots.

## 🔎 Consulting Engram

Everything not here is searched in Engram: `mem_search` (MCP) or `engram search "<query>" --project webfang [--scope personal]`.

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
| Skill index with exact paths | `skill-registry skills paths` | project | #43 + `.atl/skill-registry.md` |
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
cargo fmt --all -- --check          # --check verifies; bare fmt is a fixer
env "RUSTDOCFLAGS=-D warnings" cargo doc --workspace --all-features --no-deps

# Tests
cargo nextest run --test <name>
cargo insta review                  # review every snapshot diff

# Local gate (lanes: docs / CI / code / full)
bash scripts/ci_fast_gate.sh

# Manual checks
scripts/merge-when-green.sh <PR>            # wait green checks, then merge
scripts/ci_status.sh <PR>                   # read-only summary
scripts/ci_pr_overlap.sh <N1> <N2>          # disjoint files? (batch merge)
scripts/check_windows_platform.sh           # type-check cfg(windows) arms cross-target (needs: rustup target add x86_64-pc-windows-msvc)
```

## 🚧 Project rules

What the compiler **cannot** catch; the rest comes from the harness or the tables above.

- **Never `.unwrap()` in production** — use `?`, `match` or `.context()`.
- **`cfg(windows)` arms are invisible to every Linux gate.** Run `scripts/check_windows_platform.sh`
  on any hand-touched `cfg(windows)` arm (the fast gate runs it when `infrastructure/platform/`
  changes): it type-checks for `x86_64-pc-windows-msvc` in a throwaway crate mirroring the
  workspace lint table — only the target's std needed (no MSVC, no linking); fails loudly naming
  the `rustup target add` command when missing.
- **A poisoned `std::sync::Mutex` is never fatal and never silent.** Recover with
  `PoisonError::into_inner()` (still grants exclusive access) and keep going; where a recovered
  write could corrupt an output stream and the function returns `Result`, propagate a typed
  error instead (`infrastructure::stream`). Banned in production: `.lock().expect(..)`/`.unwrap()`
  (one panic → process-wide outage) and `if let Ok(..) = mutex.lock()` (silently drops work).
  `MockClock` keeps its `expect`: a poisoned test clock is a test bug, not runtime state.
- **English only — every repository-facing artifact** (code, comments, docs, commits,
  errors, logs, tracing). Spanish still in the tree is a migration, not a precedent:
  never add new Spanish.
- **HTTP: always `wreq`, never `reqwest`** — TLS fingerprint impersonation for WAF evasion.
- **Do not unify conflicting dependencies** (dashmap 5/6, selectors 0.35/0.37/0.38): they are
  deliberate, three different crates need three different versions.
- **Do not cite line numbers.** Cite the symbol (`WreqDownloader::fetch_inner`) or the path;
  line citations rot on every edit above them.
- **Do not copy mutable state into prose.** If it changes on its own (versions, counts, support
  lines, labels), link the source.
- **Do not touch `CHANGELOG.md`** in a work PR — there is a single writer (the Release PR).
- **Do not touch `docs/`** unless explicitly asked.
- **`docs/` holds living decisions; `evidence/<mission>/` holds frozen mission records** —
  annotate, never rewrite. Gate-read artifacts (`baseline.json`, markers) stay next to their
  evidence; download audits live outside the repo (`/webfang-audit/`); ADRs: `docs/adr/`.

### The local gate mirrors `gate.needs`

`scripts/ci_fast_gate.sh` is the local equivalent of the PR fast gate, **not** of the whole
workflow. Every run ends with a coverage report naming which `gate.needs` jobs the lane covers,
covers in part, or never touches. **Read it before pushing:** when it names a job you touched,
run that job's check by hand — a green local gate is not evidence for what it does not cover.
The declaration is `scripts/ci_fast_gate_coverage.tsv`, kept in sync with `gate.needs` by
`scripts/test_ci_gate_structure.sh`. Deliberate local exceptions and the lane model: **obs #2980**.

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
`domain/entities/` (constructor + `TryFrom` + `Display`/`Debug`/`PartialEq`), adapter in
`infrastructure/`, new error in the layer's error file.

**Avoid giant components:** no new file over ~500 lines; if you touch one that already exceeds
it, split it. Largest files: `find crates -name '*.rs' -not -path '*/tests/*' | xargs wc -l | sort -rn | head`.

### Error stratification

The application error is `ScraperError` (`crates/webfang_core/src/error.rs`) — a **funnel**, not
a chain of wrappers: `DomainError`, `InfraError`, `CrawlError` and `DownloadError` each implement
`From<…> for ScraperError` (**inputs**; `domain/error/domain_error.rs` defines no `impl From`).

- `CliError` (`cli/error.rs`) is **independent** of `ScraperError`; it maps to exit codes
  (`EXIT_CONFIG = 78` for a configuration error).
- `HttpError` is a **domain** enum (`domain/http_error.rs`), not an `InfraError` variant.
  `WafError` is **not an error type** — it is a `FetchBehavior` variant
  (`application/crawler/crawl_task.rs`). `ParseError` does not exist here.
- A new variant enters through `From` towards `ScraperError`; its user-facing message is in English.
  Full matrix and exit-code rules: `docs/error-classification-matrix.md`.

### Fetch guard chain (MANDATORY ORDER)

Every fetch path — CLI, engine, MCP tool, benchmark, future adapter — must wire the protections
**in this order**; each stage assumes the previous one filtered. Deviating is a **bug**, not a
style choice (pacing after a doomed request burns rate budget; retry classification after the
body read wastes a full read of a response that will never be accepted).

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
| 5 | Body read | `read_body_capped` (`WreqDownloader` method, `infrastructure/downloader/wreq_downloader/wreq_body.rs`; generic cap helper in `domain/body_cap.rs`), cap `DEFAULT_MAX_PAGE_BYTES` = 50 MiB (`domain/downloader_factory.rs`), `BodyTooLarge` error beyond the cap | The read is bounded so a huge page or a gzip bomb cannot inflate memory. No fetch path may read an uncapped body. |

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
- Compiler cache: **mbx** (content-addressed, per-tree targets, own GC). `sccache` is inert
  (`unset RUSTC_WRAPPER`, zero cross-tree hits); no target migration without new measurements: **obs #2306**.
- 🔒 **`[profile.dev]` lives in the root `Cargo.toml`, never in `.cargo/config.toml`.** Cargo
  resolves `config.toml` from the CWD upward: a profile there changes the `-C metadata` hash of
  every unit for any invocation whose CWD is outside the repo (cron, IDE, `mise exec`,
  `--manifest-path`) → silent full-graph rebuild, amplified by `btls-sys` into a full BoringSSL
  rebuild. Do not "tidy" it back.
- Occasional full debug info: `CARGO_PROFILE_DEV_DEBUG=true cargo build`.

## 🧪 Testing

- Integration tests live in `crates/webfang_core/tests/`, **auto-discovered** (one file per
  target). Add a `[[test]]` entry in `crates/webfang_core/Cargo.toml` only for cases
  auto-discovery misses (subdirectories such as `tests/compile_fail/`, special features).
- **Harness**: `crates/webfang_core/tests/common/cli_harness.rs` — `BehavioralTest`
  (wiremock + TempDir), `scraper_cmd()`, `find_files()`, `read_md_content()`; import with
  `#[path = "common/mod.rs"] mod common;`.
- **Binary resolution — golden rule:** always `Command::new(webfang_path())`, **never**
  `Command::cargo_bin("webfang")` / `assert_cmd::cargo_bin(...)`: `CARGO_BIN_EXE_*` is only set
  for the owning crate, and in this virtual workspace the `webfang` binary is built by the
  sibling `webfang_cli` — tests under `webfang_core` cannot resolve it with `cargo_bin`.

- **Snapshots (`insta`)**: every test producing Markdown/JSON/stderr uses snapshots, never
  `assert!(output.contains("..."))`. Cycle: `cargo nextest run` (fails with `.snap.new`) →
  `cargo insta review` (every diff) → `cargo nextest run` (green). Never commit `.snap.new`.
- **Mandatory sanitization**: `redact_nondeterministic()` normalizes TempDir paths, timestamps,
  wiremock ports and ANSI; for extra fields, `insta::with_settings!({ add_filter(...) })`.
- **Six-node diagnostic** for every test you write or modify:
  1. **Observable behavior** — exercise public ports, never internal state.
  2. **Ephemeral adapters** — wiremock for HTTP, TempDir for the filesystem. No real network.
  3. **Semantic assertions** — business invariants through snapshots, not data dumps.
  4. **Effort distribution** — maximum investment in the stable domain logic.
  5. **Arrange simplicity** — an Arrange over 5 lines is a production design defect, not a
     test defect: fix the design, not the test.
  6. **Absolute determinism** — injected time and randomness, zero flakiness.

## 🌭 Observability

**Iron rule:** every feature, hot path or behavior change ships with observability — untraceable
code is not finished. Stack: `tracing` + **FileTraceLayer** (`--trace-file out.jsonl`) + native
correlation IDs. **No external metrics backend**: emit a structured `tracing` event, query the
JSONL (`docs/src/debugging.md`, `scripts/analyze-trace.sh`).

| Situation | Requirement |
|---|---|
| New hot path / operation | `#[instrument(skip(...), fields(url = %url, ...))]` with the fields that identify the operation |
| Error path | `log_scrape_error(&err, url, stage, correlation_id, "context")` — never a bare `warn!`/`eprintln!` |
| New crawl/batch flow | Generate a `CorrelationId` at the entry point and propagate it; each unit of work gets a `.child()` |
| Long-running operation | Periodic progress log + final structured summary (`total`, `succeeded`, `errors`, `duration`, `trace_id`) |
| Async spans | `.instrument(span)` on futures; never a `span.enter()` guard across an `.await` |

- **Structured fields, not string soup:** `tracing::info!(pages = n, url = %url, "msg")`.
- **Correlation:** every event carries a 16-hex `trace_id`, **ephemeral within the run** (never
  persist or join across runs; durable identity = the `CorrelationId` UUID), at **top level** —
  not `.fields.trace_id` (error path only). Per-stage counts exclude `.record == "span_close"`
  (those records share the `.span` name).
- **Deterministic snapshots:** `correlation_id`/`trace_id` are internal, `#[serde(skip)]`.

## 🔬 Intelligence Stack

Two CLIs (**not skills**) **mandatory** for all source-level work, replacing `grep`/`rg`,
`find`/`fd`, `cat`/`head`/`tail` and `Read` on source code — pick by mission, never classical tools first.

| Moment | Tool | Why |
|---|---|---|
| "What is this task about?" — first orientation | **CodeDB** `context` | 1 call: keywords + symbol defs + ranked files + snippets. |
| "Show me the code of X" — explore and understand | **CodeGraph** `explore` | Verbatim source + call paths + blast radius in ONE call. |
| "Where is X defined?" — lookup | **CodeDB** `word` / `symbol` | Inverted index, O(1). |
| "Who calls X?" — tactical check | **CodeDB** `callers` | 1 round-trip: fuses the word index with the outline scope. |
| "How does X flow through the code?" — deeper | **CodeGraph** `explore` / `impact` | Call paths + blast radius summary. |
| Post-edit diagnostics | `cargo check` / `cargo clippy` | codedb has no diagnostics tool: the compiler is the gate. |
| Query a public GitHub repo (no clone) | **DeepWiki** MCP (`read_wiki_structure`, `ask_question`) | Remote MCP registered by the codedb installer; codedb is local-only. |

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
`projectPath=` for CodeGraph MCP) — the bare name silently resolves to main. **CodeDB CLI is
POSITIONAL-FIRST:** `codedb <abs-root> <cmd>`; only `mcp` reverses. `status`/`reindex` **ignore a
trailing path** and act on the cwd project (exit 0): verify with `codedb <abs-root> status` —
`root` must be the worktree, `head` must match `git -C <abs-root> rev-parse --short HEAD`.

## 🌳 Worktree policy

Every active branch lives in its own **sibling** directory of the repo (never inside it: in-repo
worktrees break file watchers, ripgrep and the intelligence tools). `.git` is shared; working
tree, index and HEAD are isolated.

### Iron rules

- **The CWD is the absolute boundary.** Never reach paths outside your worktree through
  `../<other-worktree>/`.
- **One worktree per session.** Never switch branches mid-task: create another worktree.
- **The mission record travels with the branch.** `odd/tasks/<feature>.md` is created and
  edited INSIDE the mission worktree, never in main: written in main it stays orphaned
  (untracked) and the finished work never reaches the tree (#1972).
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
`~/Projects/Rust/webfang-worktrees/<dir>/` — outside the repo, so no ignore rule is needed: Git
cannot track paths outside the worktree. The per-tree file that IS ignored is `.envrc`.
Example: `feat-auth/` ↔ `feat/auth`. Native review `candidate-views/` are detached HEADs under
`.git/gentle-ai/`, not worktrees for humans.

**Never put a worktree under `/tmp`** — a 16 GB tmpfs here: a from-scratch `target/` does not
fit (`Disk quota exceeded`), and a backgrounded build that dies that way returns an **empty
log**. For a throwaway tree, point `CARGO_TARGET_DIR` at an existing worktree's `target/`.

The convention is `/ → -` (`feat/auth` → `feat-auth`). **The invariant is that the commit lands
on the branch intended for the task/PR.** Run the name proxy before every commit in a worktree:

```bash
[ "$(basename "$PWD")" = "$(git branch --show-current | tr '/' '-')" ] || echo "MISMATCH"
```

`scripts/githooks/identity-gate.sh` runs **before** `review-gate.sh` in `pre-commit` and is
fail-closed; `review-gate.sh` stays fail-open for verdicts (#1036). Main needs canonical root **and**
branch `main`; rebase/cherry-pick defer; detached HEAD denies. Bootstrap writes the ignored
`.worktree-identity` via `scripts/worktree_identity.sh` (existing trees need it too). Non-interactive
shells must run `eval "$(direnv export bash)"` or an inherited `CARGO_TARGET_DIR` is denied.

### Identity-exception protocol

A mismatch commit is allowed only under the three-condition protocol (external anchor +
verifiable binding + ancestry, not equality), with the `Worktree-Identity-Exception` trailer in
the commit body. Any failed condition is a hard stop. Full text: **Engram #3201**.

### Per-worktree bootstrap

The `.envrc` is **per worktree**, never shared, written **from the tree's own name** (never
derived from main's — a `sed` over main's silently points `CARGO_TARGET_DIR` at MAIN's target).
It is **exactly 5 lines**; `$TREE` is substituted when written, only `$HOME` stays literal — a
literal `$TREE` makes the isolation guard reject the build (`reason=unregistered-target`). Then
`direnv allow`, `cp` main's `.env`, write the identity manifest with `scripts/worktree_identity.sh`,
and index the tree (`codegraph init`; `codedb "$PWD" reindex && codedb "$PWD" status`).
**Without both indexes the agent is blind**: the tools resolve to main
or return nothing; restart the MCP connection after indexing. The heredoc, the seed contract
(`seed_target.sh`: `cold` normal, `refused` not a build) and the paid-for mistakes: **obs #2307**.

### Frequent commits (mandatory in worktrees)

Commit **after every completed step** (a `git mv`, bulk replace, passing `cargo check`, green
tests): uncommitted work can be lost silently. Pattern: `work-unit-commits` skill.

### Post-merge cleanup (the merge is not done until the repo is ready)

From **main**, after `state: MERGED`: sync main **ff-only** (diverged = STOP), remove the mission
worktree, delete its branch with `-D`, `git worktree prune`, delete its target dir **by ownership**
(never enumerate/chained), verify no leftovers of that mission. 7 steps: **obs #2307**.

### Shared vs per-worktree resources

`.git/`, config/hooks and `Cargo.lock` are shared; `target/`, `.envrc`, `.env` and the indexes
are per worktree; `seeds/` read-only, `quarantine/` never a target, `refs/stash` shared
(**never** `git stash`). Full table: **obs #2307**.

### Reserved `~/.cache/cargo-target/` namespace

`<worktree-target>/` is one agent's build state. Reserved by canonical identity: `seeds/`,
`seeds-webfang/`, `quarantine/`, `quarantine-webfang/` — never a `CARGO_TARGET_DIR`, never
collected (suffixed dirs are separate stores, not prefix matches). Do not invent
new categories: the guard accepts any non-reserved identity. **obs #2307**.

`scripts/sweep_orphan_targets.sh` classifies orphans (dry-run by default). Its live registry is the
`CARGO_TARGET_DIR` declarations in `.envrc` files — comparing with worktree ROOT paths never
matches. It protects the four reserved identities and skips recent or `lsof`-held targets.

### Contamination

Outside-worktree work or an unexpected `git stash pop` means **STOP**, no cleanup, report the exact
**obs #2307** sentence and wait for explicit human authorization.

### Pushed history is immutable

A commit on `origin/<branch>` has a public SHA; rewriting it (`git reset HEAD~N` + recommit)
desynchronizes local from remote and the only way back is a force-push — **hard-denied**.
Corrections land **on top**, as new commits.

| Situation | ❌ | ✅ |
|---|---|---|
| Pushed commit is defective | `git reset HEAD~N` + recommit | a new commit that describes the fix |
| Pushed commit needs a better message | `git commit --amend` over pushed history | a new commit (or `--amend` only if not pushed) |
| Commit **not** pushed | — | `git commit --amend` |

⚠️ The gate is asymmetric: `git reset --hard` is hard-denied but a bare `git reset HEAD~N` is not
gated — you can walk into a rewrite and only discover at the end that the push is blocked.

## 🏷️ Branching, releases and support lines

- **Taxonomy — only these exist:** `main` (development; bases `feat/*`, `fix/*`, `refactor/*`,
  `perf/*`, `docs/*`, `test/*`, `chore/*`, `ci/*`, `build/*`, `style/*`, `revert/*`) ·
  `release/X.Y` (temporary MINOR/MAJOR stabilization, cut by the maintainer from an explicit main
  SHA; accepts only `fix/*`; deleted when `vX.Y.0` ships; a PATCH never creates one) ·
  `support/X.Y` (maintenance line, created on demand from the line's latest tag when it needs a
  patch while main is ahead; accepts only `hotfix/*`; deleted at EOL — `support.json` is the
  source of truth) · `hotfix/*` (MUST be based on `support/X.Y`; PR base = `support/X.Y`, never main).
- **The state of the lines is not here.** Read `.github/support.json` (jq). The full model, the
  hotfix routing and the governance scripts: **obs #2308**.
- **Forbidden (review-blocking, CI-enforced):** merging `main` INTO `support/*` · cherry-picking
  `main` → `support/*` without an approved issue · tagging `vX.Y.Z+1` from `main` while ahead ·
  a hotfix PR with base `main` · creating `support/*` "just in case" · hand-editing `SUPPORT.md` ·
  `release-plz` on a support branch · batching PRs with different `baseRefName` (disjoint files is not enough).
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

Some command classes open an approval dialog; **hard-deny** classes never show one —
conversational authorization cannot change them (there is no dialog to press).

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
- [ ] English only; new public items documented; `CHANGELOG.md` untouched
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
false-green mode, not a candidate. The queue deletes the remote branch on merge. Diagnosis:
`merge-when-green context read fallback manual enqueue` in Engram.

**Breaking change (maintainer decision):** the `!` marker must sit on the surface the lander
keeps. SQUASH + **one commit** → the commit subject (`type!:`, `type(scope)!:`); SQUASH + **two
or more** → the PR title; MERGE → branch commits survive, title discarded. `type!(scope):` is
malformed and a `BREAKING CHANGE:` footer alone does not satisfy the gate; `merge-when-green.sh`
refuses both (exit 5). Never leave the `!` only on a surface the chosen strategy drops.

**The same invariant is a required CI check (`breaking-surface`).** The decision matrix lives in
`scripts/breaking_surface_lib.sh` (shared by the CI check and the local guard — they cannot
drift); the merge method is read from the tracked mirror `.github/merge-queue.json`. Fail
closed; the local guard is defense in depth. Operational detail: **obs #2309**.

**Hooks and verification:** `pre-commit`/`pre-push` consult the review gate; enable the wiring on
a fresh clone with `git config core.hooksPath scripts/githooks`. Wait for CI with the non-blocking
pattern (`gh run list --workflow=ci.yml …`), never `gh run watch` under a short timeout; a push
that timed out did not necessarily fail (verify with `git ls-remote`). Detail: **obs #2316**.

**There are no stacked PRs in this repo.** `scripts/check-topology.sh` admits only the
head-prefix → base pairs of its table; **no feature head prefix may target another working
branch** (`fix/*` → `release/X.Y` and `hotfix/*` → `support/X.Y` are admitted — versioned lines,
not working branches). Use **sequential delivery**: slice 1 as a normal PR on `main` → merge →
slice 2 from the new `main` → repeat. Never retarget an open PR onto a working branch;
rebuilding on `main` is always available and legal.

## 🧾 Native review (RDD)

The native review is an **outside view**, never a delivery authorization: after the
acknowledgement, commit / push / PR / release stay under ordinary repository policy.

- The **candidate** is one reviewable slice (a work-unit commit or PR slice) — never a TODO,
  never the accumulated branch. **Consent is per candidate**, not the global switch's kill
  switch: never fabricate an approval or re-enable the switch for the user.
- The full cycle, the 4 lenses, failure envelopes and recovery paths: **obs #2213** + the
  `rdd-defect-workflow` skill.

## 🗺️ Skills and CLIs

- **Skill index:** `.atl/skill-registry.md` — generated **per tree**, gitignored (run
  `gentle-ai skill-registry refresh` in a fresh clone); an **index** for delegators: the
  `SKILL.md` is the source of truth — **inject exact paths**, not summaries. Skips
  `_shared` and `skill-registry` by contract. **CodeDB/CodeGraph are CLIs, not skills** (§ Intelligence Stack).
- **Loading the right rules is mandatory:** before any Rust change, resolve the applicable skill
  by code context via the registry and load its `SKILL.md` (or installed `<id>`) **before**
  editing, forwarding the exact rule set to any delegated writer. `rust-skills` (265 rules / 26
  categories) and `rust-best-practices` (distilled checklist) cover the same ground at different
  granularity; `rust-review` for a cited review report. Also: `branch-pr`,
  `work-unit-commits`, `chained-pr`, `rdd-defect-workflow`, `cognitive-doc-design`,
  `codebase-refactoring`, `dry-refactoring`, `jscpd`, `webfang-architecture`,
  `webfang-worktree-ops`, `webfang-ci-gate`, `webfang-testing`, `webfang-fetch-path`, `webfang-file-splits`.
- **Every delegation to a subagent must include:** the skill paths to read, the intelligence
  mandate (explore impact before editing; `cargo check` before returning), the exact verification
  commands, and the absolute worktree path.

## 💾 Engram (correct usage)

The server resolves the active session from the working directory; concurrent sessions can
make `mem_save` / `mem_judge` fail with `multiple active runtime sessions match…`.

- **Never invent a `session_id`.** Register with `mem_session_start`, pass that same ID to every
  `mem_save`/`mem_judge`, **close when done** (`mem_session_end(id, summary)`): each registration
  writes a 30-minute runtime lease; an expired lease drops out of candidate resolution by itself.
- If `mem_save` returns `judgment_required: true`, judge **each** candidate with **its own**
  `judgment_id` (from `candidates[]`), never the top-level one.
- On **any** engram error run `engram --help` first (then `engram <cmd> --help`) before retrying
  or inventing flags; the CLI (`engram save|search|context`) is the MCP fallback. Collision
  diagnosis: `engram doctor --check ambiguous_active_runtime_sessions --project webfang`
  (diagnostic only) — two genuinely-alive sessions colliding is by design: end the finished one
  or pass `session_id`.
- **Project knowledge lives here**, not in the repo: decisions, direction, audit findings,
  reasoning. The repo holds the **rules** and their sources of truth (owner decision: obs #370).
