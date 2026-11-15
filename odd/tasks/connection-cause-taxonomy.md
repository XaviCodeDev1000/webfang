# Feature: #2784 — the connection-failure taxonomy (PR 1)

**Issue:** #2784 (`status:approved`) — expose the real cause of a connection failure in the
error chain
**Labels:** `type:feature`
**Branch:** `fix/connection-cause-taxonomy`
**Worktree:** `~/Projects/Rust/webfang-worktrees/fix-connection-cause-taxonomy`
**Delivery:** one PR, scoped to the classification. The per-cause backoff policy in batch
(DNS treated differently from a block) is **PR 2**, on its own issue and its own review: it
changes behavior, not just reporting.
**Runner:** `cargo nextest run --test connection_cause_taxonomy`, `cargo clippy --all-targets
--all-features -- -D warnings`, `cargo fmt --all -- --check`

## Why this exists

The laboratory measured six connection-failure causes and the tool reported **three** messages on
the batch path and **four** on the `--url` path. `client error (Connect) (status: None)` covered
**four** different causes; `request timed out after 30s` covered **three**. There is no
machine-readable cause field at all — only a message string, and the two CLI paths do not even
report the same failure the same way.

The field run's signature (`client error (Connect) (status: None)` at exactly 10.0 s) was
reproduced in the laboratory by **two** causes — a dropped SYN and a blackholed resolver — so
the tool could never attribute the field failures. That is the cost of the missing taxonomy,
measured.

Evidence: [#2784](https://github.com/XaviCodeDev1000/webfang/issues/2784#issuecomment-6870356863)
and the controlled re-run
([comment](https://github.com/XaviCodeDev1000/webfang/issues/2784#issuecomment-6870790451)).

## The contract (fixed before the first line of code)

The trace field is a **public contract**: names and values are stable from the first release.

**Field name:** `cause`, on the error events of the JSONL trace.

**Values** (snake_case; a value never changes meaning, a new class is a new value):

| value | meaning | user-facing message (Spanish) |
|---|---|---|
| `dns_resolution` | the name could not be resolved, or the resolver did not answer | "no se pudo resolver el nombre del dominio" |
| `connect_refused` | the peer refused the connection (RST) | "el servidor rechazó la conexión" |
| `connect_timeout` | the TCP connect did not complete in time (the SYN never got through) | "la conexión no se completó a tiempo" |
| `read_timeout` | the connection was established and the response never arrived | "el servidor no respondió a tiempo" |
| `connection_closed` | the peer closed the connection before answering (EOF) | "el servidor cerró la conexión antes de responder" |
| `tls_handshake` | the TLS handshake failed | "falló el handshake TLS" |
| `unknown` | the failure could not be classified | "fallo de red no clasificado" |

`unknown` is a **value, not a shrug**: it means the classifier ran and could not decide, and it
is the honest answer for a cause the taxonomy does not cover. It never means "the classifier did
not run".

### Which measured cause proves which value

Every value has a laboratory case behind it — the case, the control, and the message the tool
prints today:

| value | laboratory case | today's message (batch) |
|---|---|---|
| `dns_resolution` | `dns_blackhole` (the resolver swallows the query) | `client error (Connect)` at 10,0 s |
| `dns_resolution` | `dns_fail` (the resolver answers NXDOMAIN) | `client error (Connect)` at 0 s |
| `connect_refused` | `rst` (the peer resets) | `client error (Connect)` at 0 s |
| `connect_timeout` | `syn_drop` (the SYN never gets through) | `client error (Connect)` at 10,0 s |
| `read_timeout` | `read_timeout` (the server accepts and never answers) | `operation timed out` at 30 s |
| `connection_closed` | `eof` (the server accepts and closes) | `client error (SendRequest)` at 0 s |
| `tls_handshake` | `tls_close` (the server accepts and closes during the handshake) | `client error (Connect)` at 0 s |

Seven measured causes, **three** messages: four of them collapse into `client error (Connect)`.
`tls_handshake` used to have no case at all — it now has one (`tls_close`), measured with the
positive control, and the measurement is what proves the class is needed: the handshake failure
prints the same `Connect` message as the refused connection.

### The read timeout and the EOF travel through different wrappers

On the `--url` path the read timeout's run ends with the engine's `crawl stalled: a worker
exceeded the 70s wait bound`, and the EOF surfaces as `client error (SendRequest)`. Both are
**wrappers**, not causes: the taxonomy must carry the cause *through* the wrapper, so the
operator reads "the server did not answer in time" and not "a worker stalled".

## The surfaces

- **A new domain module** `crates/webfang_core/src/domain/error/cause.rs`: the `NetworkCause`
  enum, its stable values, the Spanish messages, and the classifier's contract. A new file keeps
  `crates/webfang_core/src/error.rs` (395 lines today) under the ~500-line rule — that file is a
  **declared exception** in #2584's program ("declarative/cohesive exceptions (e.g. `error.rs`)
  stay grandfathered"), so nobody is splitting it, but adding to it would still push it.
- **The error chain**: `ScraperError::Network` carries the classified cause instead of a bare
  message.
- **The trace**: the error events gain the `cause` field. The schema change is announced in the
  PR body, as a public contract, and documented next to the trace schema
  (`docs/src/debugging.md`). The maintainer's instruction — "Documéntalos junto al esquema de la
  traza" — is the authorization for that file: the field and its values belong next to the
  schema they extend.
- **The value set is snapshotted**: a snapshot test of the whole set of values, so adding one is
  a **visible** change (the snapshot diff) and not a silent extension of a public contract.

## The two risks that decide the design

### 1. The blackholed resolver must not be classified as a connect timeout

The pair that motivated the whole taxonomy — a **blackholed resolver** and a **dropped SYN** —
both surface today as `client error (Connect)` at exactly 10,0 s. If the resolver hangs *longer*
than the connect timeout, the connect timeout fires first, the resolver never gets to record
its own failure, and the case is classified as `connect_timeout`. The DNS class would be
unreachable exactly where it matters.

**A resolver timeout would be a BEHAVIOR change, and PR 1 is classification only.** A shorter
resolver timeout makes a slow-but-valid resolution fail earlier than today (glibc's defaults are
5 s per attempt and 2 attempts, so even 8 s would cut resolutions that succeed now). So **PR 1
introduces no timeout at all**; if a resolver timeout is ever wanted, it belongs to its own PR,
with a documented, configurable and justified value.

**The design instead records the PHASE, per request.** The resolver marks each resolution as
"started" and "finished" (ok or failed). When the **outer** timeout fires — the client's, the
same one that fires today — the classifier reads the marker: "started and not finished" means the
request was still waiting for the name, so the cause is `dns_resolution`; "finished" means the
resolution was already done and the timeout belongs to the connect phase, so the cause is
`connect_timeout`.

The marker must be **per request, not global**: a shared global flag would attribute one
request's resolution to another one's timeout. The correlation mechanism (a request-scoped
tracker shared between the resolver and the classifier — a token, a task-local, or an explicit
per-resolution cell) is an implementation detail, and the **pair test is what pins it**: if the
correlation is wrong, the pair test fails.

**Nothing about the observable behavior changes**: the same requests fail at the same moment
with the same exit codes. Only the *name* of the cause changes.

**The RED requires the pair**, and it must not depend on the namespace — the CI runners deny
it, so a namespace-only pair would be skipped exactly where it protects most. The pair is
covered **in process**, with the namespace version as an extra confirmation:

| half of the pair | in-process construction | namespace confirmation |
|---|---|---|
| the blackholed resolver | a `Resolve` implementation that **never resolves** (the repository already has the `NameSource` seam for it) | the fake resolver that swallows the query |
| the dropped SYN | a listener with a **minimal backlog** whose accept queue is already full: on Linux the kernel then drops the following SYNs, and the client's connect never completes. **Measured here**: the control connect completes in 0,00 s, and the case does not complete at all | the `nft` rule that drops the SYN |

The SYN half is verified with a **positive control** like every other injection: the same client
against a normal listener connects instantly, so the hang is the injection and not the path.

### 2. Classify by type, never by text

The classifier walks the error chain with `downcast` — our own typed errors, `std::io::Error`'s
`ErrorKind`, and the foreign error types' own predicates — and **never compares message
strings**. The messages change between dependency versions; a text-matching taxonomy would
break silently, and the breakage would look like a network change.

**The pin against a `wreq` update:** the classifier's tests drive it with **real errors produced
by the real dependency** — an in-process refused connection to `127.0.0.1:9` yields the actual
`wreq`/hyper error, and the test asserts the verdict. When a dependency update changes the
error's shape, the mapping stops matching and the test fails — that is the pin, and it needs no
string comparison to work.

## The repo guards (do not repeat #2752's round)

The new module adds **intra-crate dependency sites**, and touching the trace documentation can
activate the **documentation guards**. Before the PR:

- `scripts/refresh_real_tree_golden.sh`, with the refreshed golden **in the same commit** as
  the tree that changed;
- the two checks the local gate already runs (the intra-crate direction check and the CLI
  reference drift check) — since #2763 the gate runs them itself, so the requirement is simply
  **not to skip the gate**;
- the golden set and the baselines are refreshed in the same commit, never in a follow-up.

## The RED phase

The test does not need new API to be written: it runs the binary against a **closed port** and
asserts the trace's error event carries `cause=connect_refused`. Today the field does not exist,
so the test fails for the right reason — the behavior is missing, not the API.

1. `crates/webfang_core/tests/connection_cause_taxonomy_test.rs` — the E2E RED (a closed port →
   `connect_refused`).
2. The laboratory's six cases become the **classification matrix** the implementation is checked
   against; the ones reproducible in-process become repository tests, and the rest stay in the
   harness as the oracle.

## Rules this branch follows

- The user-facing error text is **Spanish**; the tracing fields and logs are **English**.
- No `.unwrap()` / `.expect()` in production code.
- The trace field's values are **stable**: a later change that renames one is a breaking change
  and needs its own issue.
- PR 1 does **not** touch the backoff policy: that is PR 2, with its own issue.

## Log

### L1 — The decision (2026-11-14)

The maintainer: encara el fix sobre el propio #2784, sin issue nuevo ("abrir otro fragmentaría
la discusión"), recortado a lo que el issue pide primero: la clasificación de la causa. PR 2
apartado: la política de backoff por causa en batch, que cambia comportamiento y merece su
propio issue y su propia revisión. Y una instrucción explícita: el campo de la traza es un
contrato público → elegir nombres y valores estables desde el principio y avisar del cambio de
esquema en el PR.

### L2 — The RED observed (2026-11-14)

`cargo nextest run --test connection_cause_taxonomy_test`:

```
thread 'a_refused_connection_is_classified_in_the_trace' panicked at
  crates/webfang_core/tests/connection_cause_taxonomy_test.rs:74:5:
the trace must classify the refused connection as `connect_refused`; causes found: []
```

The failure is the RIGHT one: the field does not exist in the trace, so the behavior is missing
and not the API. The test is deliberately left **uncommitted** until the implementation lands, so
the branch never carries a red commit: the RED → GREEN slice ships as one work unit (the test,
the taxonomy, the message and the trace field together), with the RED evidence recorded here.

(The target is named after the file: `connection_cause_taxonomy_test`, not
`connection_cause_taxonomy` — cargo derives it from the stem, and the repo's own convention
keeps the `_test` suffix on files that have one.)

### L3 — The GREEN, first slice: the vocabulary and the classifier (2026-11-14)

**Landed** (`63449239`): the taxonomy's vocabulary and the type-based classifier, with 14
new tests, `cargo fmt` clean and `cargo clippy --all-targets --all-features -D warnings`
clean.

- `crates/webfang_core/src/domain/error/cause.rs` — the stable values, the Spanish messages,
  the `ClassifiedNetworkError` carrier (which KEEPS the original error as its `#[source]`, so
  the cause is added to the chain and never substituted for it), `DnsResolutionError` (the
  resolver's own typed failure) and `cause_in_chain` (the chain walk, by TYPE).
- `crates/webfang_core/src/infrastructure/downloader/cause.rs` — the classifier and the phase
  it needs: `FetchPhase::Connect` (before the response) or `FetchPhase::Body` (after it). It
  walks the chain by `io::ErrorKind` and by the domain's typed errors, and **never** compares
  message text.

**The classification, by type:**

| signal (by TYPE) | cause |
|---|---|
| `DnsResolutionError` anywhere in the chain | `dns_resolution` |
| `io::ErrorKind::ConnectionRefused` | `connect_refused` |
| `ConnectionReset` / `UnexpectedEof` / `BrokenPipe` | `connection_closed` |
| `TimedOut` in the connect phase | `connect_timeout` |
| `TimedOut` in the body phase | `read_timeout` |
| `InvalidData` in the connect phase (the TLS stack's handshake violation) | `tls_handshake` |
| anything else | `unknown` |

**The wiring that remains** (the next slice, with its RED):

1. the **resolver** raises `DnsResolutionError` instead of a bare I/O error
   (`infrastructure/ssrf.rs`, `gai_lookup`);
2. the **downloader** classifies at the ORIGIN — where the phase is known — and lets the cause
   travel: the non-timeout failures become `DownloadError::Network(Box::new(classified))`, and
   the timeout keeps its `DownloadError::Timeout(secs)` variant (so the retry rule and the exit
   code do not move) with the classified error wrapped around it;
3. the **trace** gains the `cause` field: `log_scrape_error` takes an optional cause and emits
   it as a field, omitted when there is none;
4. the RED's two tests go green, the value-set snapshot lands, and the pair tests (the
   in-process never-resolving resolver and the accept-queue SYN drop, with the positive control
   and the written skip reason) join the slice.

**Why the wiring is not in this commit.** The retry rule reads `dl_err.classify()`, and the
classification of a `Network` variant and of a `Timeout` variant is not identical — so wrapping
the timeout changes what the domain classifier sees. That is exactly the kind of change that
must not be rushed: it needs its own RED (the retry behavior pinned) before it moves.

**The RED stays uncommitted** (`crates/webfang_core/tests/connection_cause_taxonomy_test.rs`),
with its patch and copy at `~/.cache/webfang-lab/red/`: the branch never carries a red commit.

### L4 — The wiring, half landed: the cause dies at the CrawlError boundary (2026-11-14)

The wiring was written and probed, and the probe found exactly where it stops. **Uncommitted**
on purpose (the branch stays at the green `63449239`; the patch and the RED are at
`~/.cache/webfang-lab/red/`).

**What the wiring does today:**

- `WreqDownloader::send_request` classifies at the origin (the phase it knows: Connect) and lets
  the cause travel inside a `ClassifiedNetworkError`, keeping the variant underneath (`Timeout`
  keeps its shape, so the retry rule and the exit code do not move);
- `read_body_capped` classifies the body phase;
- `ValidatingResolver`'s failed lookup raises `DnsResolutionError` — **and the SSRF scan's
  rejection is deliberately NOT wrapped** (a refusal is not a DNS failure, and the guard's
  callers must keep seeing what they see today);
- `log_scrape_error_with_cause` emits the stable wire name as the `cause` field, omitted when
  there is none. The old 5-argument `log_scrape_error` is preserved (it delegates with `None`)
  because there are **9+ call sites**: changing all of them would have made this a much wider
  diff than PR 1 should be, and two of them (the batch crawl site and the fetch site) are the
  ones that matter for the field.

**Where it stops — measured, not assumed.** With the entry-guard hatch armed and the batch
against port 9:

```
{"error": "network error: error sending request for uri (...): client error (Connect) ...", "stage": "fetch"}
```

No `cause` field, and the message is the **unclassified** one. Two discoveries:

1. the batch path does not run through `WreqDownloader::send_request` — the crawler's
   `fetch_page` path produces the error, and its conversion to `CrawlError` flattens the cause
   away (the same flattening D4 removed from the *message*, which the cause must also cross);
2. the cause therefore has to survive the conversion boundary, not just the downloader.

**The next slice** (its own RED, its own GREEN): find the fetch path the batch actually uses,
carry the cause through that conversion, add the field to the event that path logs, and turn the
two RED tests green together with the value-set snapshot.

**One lesson about the harness itself.** The first manual probe without the entry-guard hatch
failed at the guard, not at the network — the test asserted "a refused connection" while the
tool had refused to even try. That is the positive-control rule paying for itself a second time:
**the injection must reach the layer under test**, or the case proves nothing. Both RED tests
now arm the documented test-only hatch explicitly, with a comment naming why.

### L5 — Where the cause actually dies, measured (2026-11-14)

Two probes with a **freshly built binary** (`cargo build -p webfang_cli`, then the batch against
port 9 with the entry-guard hatch armed):

```
{"error": "network error: error sending request for uri (...): client error (Connect) (status: None)", "stage": "fetch"}
```

The message is the **unclassified** `DownloadError::Network(Box<wreq::Error>)` display — so the
wreq error never passed through the classified origin. The path, now traced end to end:

```
ProductionPageFetcher::fetch_page          (application/crawler/ports.rs)
  → router.fetch(url)  OR  fallback.fetch_url(url.as_str(), config)
     → the FALLBACK static fetcher is the live branch for the batch path
       → application/http_client (the HttpClient) performs the request
         → HttpError::Connection(msg)        ← A STRING
           → CrawlError::Connection(msg)     ← A STRING (domain/error/crawl_error.rs)
             → the trace site reads a chain that has nothing typed left
```

**This is the boundary the maintainer predicted**: `HttpError::Connection(String)` flattens the
transport error into text, and everything below it sees only the text. The least intrusive fix,
as instructed, is to keep the original error as `#[source]` in the existing variant — **without
adding variants** — and to classify at the `HttpClient`'s own send boundary, where the wreq
error still exists.

**What that slice must verify (regression-pinned before it moves anything):**

| contract | pin |
|---|---|
| exit codes unchanged (69 for connection failures) | a test that drives the real failed fetch and asserts the code |
| the retry rule and its error classes unchanged | the existing retry/classify tests stay green |
| the message texts others may parse | the existing assertions on those texts stay green, or the change is moved to its own PR |

**Still pending, and the reason the PR must not claim the pair is solved:** the resolver's
**"started and not finished" marker**. Without it, the blackholed resolver and the dropped SYN
both surface as `connect_timeout` — the exact two causes this whole taxonomy exists to separate.
The vocabulary and the classifier landed green (`63449239`) **cover the resolver's own typed
failure**, not the marker case. The commit message and the PR description say so; the pair test
is what will close it.

**Also pending:** confirming with the laboratory's `tls_close` fixture that wreq's handshake
failure really arrives as `io::ErrorKind::InvalidData`. If it does not, the `tls_handshake`
value is unreachable and the test must say so, with everything else falling into `unknown`.

### L6 — The cause now crosses the boundary; RED → GREEN (2026-11-14)

**What landed (uncommitted→committed as one work unit):**

- The classifier (`FetchPhase`, `classify`) moved to the **domain** (`domain::error::cause`),
  `pub(crate)` module, no `pub use` re-export — the application layer must not import
  infrastructure, and a public re-export/module entry makes rustdoc's search index tip over
  an internal rustdoc ICE under `--all-features` (see "Known blocker" below).
- `HttpError::Connection`, `CrawlError::Connection` and `CrawlError::Network` now carry
  `source: Option<Box<dyn Error + Send + Sync>>` **beside** the message. The `Display` of
  every one of them is byte-identical to the former tuple shape, pinned by tests
  (`test_http_error_connection_display_is_unchanged`, `test_crawl_error_connection_display_is_unchanged`,
  plus `test_connection_carries_its_cause_through_the_conversion`).
- Classification happens at every send boundary where the phase is known, and **only the
  transport `Network` arm gains the cause** — a builder error stays `InvalidUrl`, a timeout
  stays `Timeout`, `BodyTooLarge` stays `BodyTooLarge`, so no class, retry rule or exit code
  moves:
  - `infrastructure/crawler/http_client::fetch_url` — the batch/crawl fallback path — send
    (`FetchPhase::Connect`) and body (`FetchPhase::Body`);
  - `application/http_client` (`map_send_error`, the raw send path, the port default);
  - `WreqDownloader::send_request` / `read_body_capped`.
- `From<CrawlError> for ScraperError` keeps the synthetic `io::ErrorKind` (that is what makes
  the class transient) and rides the classified cause **inside** it as its source, so
  `cause_in_chain` finally reads it at the trace site.

**Route coverage (the PR must state this — point 4):** the `cause` field exists on

| route | connect | body |
|---|---|---|
| batch / crawl page fetch (static fallback `fetch_url`) | ✔ | ✔ |
| `--url` scrape (`WreqDownloader` via `FetchRouter`) | ✔ | ✔ |
| application `HttpClient` (MCP tools, robots, sitemap) | ✔ | ✘ (`HttpError::Request` still flattens) |

**Not covered yet, said honestly:** `HttpError::Timeout` / `CrawlError::Timeout` carry no
source slot, so a timeout on those routes reports no `cause`; and the resolver's
"started and not finished" marker is still pending, so a blackholed resolver and a dropped
SYN both surface as `connect_timeout` until the pair test lands.

**Verification:** 3025 lib tests, the RED pair (25/25, both `connect_refused` and
`dns_resolution` now in the trace), `exit_code_integration` 32/32, `ssrf_rfc1918_e2e` 22/22,
`scraper_service_test` 56/56, `sitemap_fetch_timeout_test` 24/24, `harness_403_counts` 25/25,
`ssrf_layer2_coverage_test` 7/7; `cargo fmt --check` and clippy `--all-targets --all-features
-D warnings` clean.

**Known condition (not a blocker — corrected):** `env RUSTDOCFLAGS=-D warnings cargo doc
--workspace --all-features --no-deps` prints several rustdoc ICEs on this branch (the known
`search_index::PathData` scoped-tls panic, obs #2707) — **but the command exits 0**, so the
`doc-quality` job is GREEN. The ICEs are noise from a known rustdoc 1.91.1 bug, not a lint in
this change. See L7 for the full picture.

### L7 — The rustdoc ICE, diagnosed; and the breaking surface (2026-11-14)

**The ICE, bisected per the maintainer's order.** First, the correction that matters: the
command **exits 0** — the ICEs are printed by rustdoc's search-index rendering and the run
still succeeds, so the `doc-quality` job is **green**. (My first reading of the printed
`error:` lines as "the gate goes red" was wrong: I had not checked the exit code. This is the
pre-existing panic of obs #2707 — same stack, same shape, reproduced on another branch and on
its base.) With that out of the way, the bisect:

It is a **rustdoc internal panic** in the pinned toolchain, not a lint in this change:

| probe | result |
|---|---|
| `main` @08a323c2, `--all-features`, fresh target dir | 0 ICEs (8+ runs) |
| `main` + a dummy documented public item | 0 ICEs |
| `main` + a public item with ~4000 extra doc lines | 0 ICEs (not a size threshold) |
| this branch, `--all-features` | ICE on **every** run (12+), deterministic — and **exit 0** |
| `main`, `--workspace --all-features`, fresh target AND a forced re-doc of all five crates (`touch` each `lib.rs`, so the cache cannot mask it) | 0 ICEs, exit 0 — so in THIS tree the panic correlates with the vocabulary commit `63449239`, not with the cache or the environment |
| this branch, default features | only the 2 PRE-EXISTING feature-gated links in `domain/repository.rs` (also red on `main`) |
| **this branch on NIGHTLY (rustc 1.100.0-nightly, 2026-08-15)** | **0 ICEs** — the same source documents, with only 2 pre-existing lint errors in `infrastructure/converter/{obsidian,wikilinks}.rs` (untouched by this change) |

So the panic is a **rustdoc 1.91.1 bug that a newer rustdoc does not have**. The trigger sits
in commit `63449239` (the vocabulary + the classifier), **not** in the wiring: the pre-wiring
state ICEs too.

Mitigations measured, all ineffective: a type alias for the source field; `#[doc(hidden)]` on
the field (the sanctioned temporary exit); removing the shim's re-export; renaming the module
or the enum; stripping the new module's docs. Minimal reproductions attempted on `main` — a
public enum with a struct variant carrying a boxed source (with and without `thiserror`), and
a `pub(crate)` module plus a private shim re-exporting its items — do **not** reproduce it.

**Consequence:** none for the gate — and note the two lessons this cost: (a) a printed
`error:` is not a gate failure, the exit code is (0 here); (b) the causal test against the base
must FORCE the re-documentation, because a cached target dir makes the base look clean (obs
#2875 spells out both). **Search Engram before bisecting:** obs #2707 and obs #2875 already
described this panic, its non-fatality and its proof, and the right way to isolate it.

**Consequence:** none for the gate — the command exits 0, the job stays enabled and green,
and nothing was turned off. What remains is a **known rustdoc 1.91.1 noise** on this repo
(obs #2707): the same panic prints on other branches and on their base. On nightly (a newer
rustdoc) the panic is gone entirely, which is the eventual fix for the noise.

**The breaking surface (public API change).** `HttpError::Connection`, `CrawlError::Connection`
and `CrawlError::Network` change from tuple to struct variants, so downstream matches on the
old `String` payload break. `webfang_core` is a published crate and the whole workspace
(`mcp`, `cli`, `test_utils`, `benchmark`) compiles green against the change.

- The marker is `feat(domain)!:` — on the commit **and** on the PR title, because the branch
  has 8 commits and SQUASH keeps the title.
- Local guard, offline mode: `bash scripts/check_breaking_surface.sh --fixture <file>
  --strategy squash` → **OK** with the marker on the title, **red** without it (the negation
  proves the check bites).
- The label the auto-label job derives from a `!` title is `type:breaking-change`.

### L8 — The resolver marker, and the two CI reds (2026-11-14)

**The marker (the maintainer's point 2).** A hung resolver used to read as
`connect_timeout` — a CONCRETE but wrong attribution, worse than `unknown` because it is
consumable. Now the resolver marks the host while a resolution is in flight
(`domain/error/resolution.rs`): the mark is dropped when the resolution ends (even on
cancellation), and a connect-phase timeout is refined with it:

| marker state | cause |
|---|---|
| a resolution for that host started at/after this request began and has NOT finished | `dns_resolution` |
| a resolution is in flight but it started BEFORE this request (another request's) | `unknown` |
| no resolution in flight (the name resolved) | `connect_timeout` (unchanged) |

The request's start instant is what makes the attribution per request instead of per host. The
concurrent case is the honest one: `unknown`, never a false attribution.

**Two CI reds that only the FULL run showed** (the local checklist did not):

1. **Code quality (duplication ratchet)**: +46 duplicated lines. Fixed by making the DOMAIN
   own the mappings (`domain/error/mapping.rs`: `http_failure`, `download_failure`,
   `crawl_network_failure`, `http_connection_failure`, `with_cause`) so each call site is ONE
   line, and by giving the two mocks a single `HttpError::without_cause`. Now **8211 ≤ 8219**.
2. **Repo guards**: the file-size ratchet. The typed cause adds a field to three PUBLIC
   variants and the marker lives at the resolver's own boundary, so six grandfathered files
   grew. Everything that COULD leave them did (the classifier, the mappings and the tracker
   moved into the domain), leaving **+42 lines in total** — measured per file: processor.rs +2,
   ssrf.rs +5, crawl_task.rs +1, wreq_downloader.rs +7, crawl_error.rs +18, client.rs +9. The
   fields and their doc comments are mandatory under `deny(missing_docs)`, rustfmt expands the
   struct literals, and an enum's variants cannot be split across files, so the residual is
   structural. The six entries were raised by a MANUAL JSON edit — the mechanism the policy
   names ("a maintainer decision visible in review") — with the measurement in the notes.

`domain/error/cause.rs` also had to be split (it passed the 500-line hard limit): the
vocabulary and the classifier stay in `cause.rs`, the mappings in `mapping.rs`, the resolution
tracker in `resolution.rs`.

### L9 — The pair, proven end to end in the laboratory (2026-11-14)

The laboratory harness (`~/.cache/webfang-lab/case.sh`: each cause isolated in an unprivileged
`unshare -rmn` namespace, every injection with a positive control) re-measured the full matrix
against the PR's binary, batch route, `TIMEOUT_SECS=20` so the CONNECT deadline fires:

| injected cause | trace `cause` | elapsed |
|---|---|---|
| `dns_blackhole` (the resolver never answers) | **`dns_resolution`** | 10.1s |
| `syn_drop` (the SYN is dropped) | **`connect_timeout`** | 10.1s |
| `dns_fail` (NXDOMAIN) | `dns_resolution` | 0.1s |
| `rst` (connection refused) | `connect_refused` | 0.1s |
| `eof` (the connection closes mid-request) | `unknown` | 0.1s |
| `read_timeout` (the server stalls) | `unknown` | 20.1s |

**The pair the whole line exists for is proven**: the hung resolver is `dns_resolution` and the
dropped SYN is `connect_timeout` — the two causes this taxonomy exists to separate, and the one
a bare classifier could NOT tell apart (wreq reports the hung-resolver deadline neither as a
timeout nor with an `io::Error` in its chain, because the resolution never came back to be
wrapped).

Two honest gaps remain, both caught by `unknown` rather than mislabelled:

- `eof` and `read_timeout` are connect-phase/body-phase failures whose wreq chains expose no
  `io::Error`, so the classifier cannot read their kind. They are `unknown` — the classifier
  ran and could not decide — never a wrong cause. Follow-up: read wreq's own error kinds
  (`SendRequest`/body-timeout) in the classifier.

**Two defects the laboratory found in the marker itself**, both fixed:

1. The RAII guard erased the evidence: a resolution the client ABANDONED (the connect deadline
   firing) dropped its mark, so the hung-resolver case looked like "nothing in flight". The
   mark now survives abandonment and only leaves the tracker when the resolution COMPLETES.
2. The refinement only engaged on `ConnectTimeout`, but the hung resolver's failure is
   `Unknown` (see above). It now engages on either, gated by the connect phase — which the
   caller knows and passes.

At the repo level the taxonomy's six other causes were already proven by unit tests; the
laboratory is the venue for the process-level pair, and #2784 stays open until that fixture
lives in the repo suite.

### L10 — The maintainer's pre-merge review (four findings)

The maintainer asked for four things before merging, and one of them was a
real defect:

1. **The checks**: verified by hand, not asserted — `headRefOid` of the PR and
   the `headSha` of every run agree on `84184d4c`, and the four workflows
   (`CI`, `PR Validation`, `Docs`, `Mutation Testing`) report `success` on it.
2. **The tracker could grow without bound** — CONFIRMED, and fixed. The bound
   that existed covered only the marks of ONE host (16); the map of HOSTS had
   no cap and no expiry, so a batch against thousands of hosts (or a long run
   with a broken DNS) grew the tracker forever. It is now bounded on three
   axes: 16 marks per host, 256 hosts, and a mark that is older than 5 minutes
   is no longer evidence at all (an expired mark cannot classify anything; it
   stops being evidence and the classifier's own cause stands). Three tests
   pin the bounds, and the drop is observable (`tracing::debug!`).
3. **The fail-closed scan** — confirmed unchanged, and now PROVEN by a test:
   `gai_lookup` marks the resolution and then runs the scan, so the marker
   cannot alter the scan; and an SSRF refusal keeps its class and carries NO
   cause, so it can never be read as a DNS failure.
4. **The pair now lives in the repo**: `infrastructure/ssrf/resolution_pair.rs`
   proves the hung resolver IN PROCESS (injectable lookup seam, no network,
   no privileges, 0.3 s), and the other side — the accept queue that swallows
   the SYN — is declared, skipped and written out with its probe, because
   dropping a SYN needs a kernel no CI should have.

The debugging leftovers are gone: zero `eprintln!`/`dbg!`/`println!` in the
added lines of the whole diff (the one that broke three stderr snapshots is
removed).

### L11 — The maintainer's review: the cause was computed and thrown away (2026-11-15)

The maintainer reviewed the whole PR and asked for changes. The verdict: *"la arquitectura es
correcta; la honestidad de las claims no lo es"*. Six blocking points, of which one was a real
defect in the code and the rest were claims the PR could not carry.

**The defect (§2): on the `--url` route the cause was computed and then DISCARDED.**

`download_failure` narrowed the transport error into a variant FIRST (`Io`, `Dns`, `Tls`,
`Timeout`) and classified the NARROWED error; only the `Network` variant carried the carrier.
So every failure that narrowed into another variant reached the trace with no cause at all —
including the refused connection, the most common failure of all. Two things had to change:

1. **the cause is decided on the RAW `wreq::Error`**, before the narrowing erases the
   resolver's typed failure and the I/O kinds the classifier reads. The retry rule reads the
   error CLASS, never the cause, so classifying earlier moves no verdict;
2. **every transport variant carries the cause**, not only `Network`:

   | variant | where the cause travels |
   |---|---|
   | `Network` | the carrier IS the payload |
   | `Io` | the carrier is the I/O error's payload — the `kind` (the retry rule's input) survives |
   | `Dns` / `Tls` / `Timeout` | the carrier is the variant's `source` |

   The `Io` variant needed no shape change: an `io::Error` keeps its payload AND its kind, and
   the payload is where the carrier travels. What it does NOT do is expose that payload as a
   source — `io::Error::source()` returns the payload's *own* source — so the chain walk has to
   look into `get_ref()`. That is one extra check in `cause_in_chain`, pinned by a test.

**The RED was observed in this tree, not asserted.** With the fix reverted (the source patch
alone; the new tests stayed), the three new tests fail:

```
the `--url` route must classify the refused connection as `connect_refused`; causes found: []
the refused connection's message must name its cause; messages:
  network error: error sending request for uri (http://127.0.0.1:9/): client error (Connect) (status: None)
```

The second line is the issue's own complaint, verbatim: every failure of a real run printed the
same sentence. The batch route's two tests stayed green through the revert — the batch already
worked, exactly as the review's table said.

**What else changed in this round:**

- **the message names the cause** (the issue's first acceptance criterion): every classified
  failure reads `"{cause in Spanish}: {the transport's own words}"`, on every route. The
  `cause` FIELD stays the machine contract; the text is prose that says the same thing.
- **three values are declared, not measured** (`read_timeout`, `connection_closed`,
  `tls_handshake`): the transport does not expose the evidence they need, so they arrive as
  `unknown`. The doc and the enum now say so, instead of presenting all seven as equally
  observed.
- **the ignored half of the pair asserts the full verdict**: `assert_eq!(ConnectTimeout)`
  instead of "not a DNS failure" — an `unknown` would have satisfied the old assertion.
- **`Clone` keeps the class** and loses only the cause, and that is pinned by a test.
- **`docs/src/debugging.md`**: the claim "the field's absence means 'not a network failure'"
  was FALSE and is gone; the absence means "not classified". The field is the contract; the
  text is not.

**Decisions that are the maintainer's, and are NOT taken here:** the `last_success` veto (the
marker is per HOST with a veto, not per request as the spec promised) and the file-size raise
(+59 lines over six grandfathered files). Both are open in the review thread.

**Verification:** the whole workspace suite green (5,564 tests) after the five snapshots that
carry the new `cause` field were reviewed by hand — the DNS one now reads
`cause: "dns_resolution"` on the `--url` route, which is the defect itself, visible in a
pinned snapshot.
