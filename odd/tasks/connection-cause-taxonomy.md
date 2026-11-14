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
