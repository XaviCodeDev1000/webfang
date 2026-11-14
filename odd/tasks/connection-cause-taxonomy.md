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
  (`docs/src/debugging.md`, which the maintainer asked to update).
- **The value set is snapshotted**: a snapshot test of the whole set of values, so adding one is
  a **visible** change (the snapshot diff) and not a silent extension of a public contract.

## The two risks that decide the design

### 1. The blackholed resolver must not be classified as a connect timeout

The pair that motivated the whole taxonomy — a **blackholed resolver** and a **dropped SYN** —
both surface today as `client error (Connect)` at exactly 10,0 s. If the resolver hangs *longer*
than the connect timeout, the connect timeout fires first, the resolver never gets to record
its own failure, and the case is classified as `connect_timeout`. The DNS class would be
unreachable exactly where it matters.

**The design answers it with a phase marker, not with a longer timeout:**

- the resolver marks its own failure with a **typed error** (`DnsResolutionError`) — the
  "resolution started and did not finish" record, per request;
- the resolver gets **its own timeout, shorter than the connect timeout**, so the marker is
  always written before the connect phase can give up;
- the classifier reads the marker **out of the error chain**, so the DNS class survives every
  wrapper between the resolver and the operator.

**The RED requires the pair.** The test set must demand `dns_resolution` for the blackholed
resolver and `connect_timeout` for the dropped SYN — one pair, two different verdicts. The pair
lives in two places: the laboratory harness (the oracle, with the positive control) and a
repository test that re-execs itself inside `unshare -rmn`, **skipping with the written reason**
where the environment denies the namespace (the CI runners do — obs #2394, the same pattern the
crash-matrix test uses).

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
