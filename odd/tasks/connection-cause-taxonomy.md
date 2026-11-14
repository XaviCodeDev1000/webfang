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
| `dns_resolution` | the name could not be resolved | "no se pudo resolver el nombre del dominio" |
| `connect_refused` | the peer refused the connection (RST) | "el servidor rechazó la conexión" |
| `connect_timeout` | the TCP connect did not complete in time | "la conexión no se completó a tiempo" |
| `read_timeout` | the connection was established and the response never arrived | "el servidor no respondió a tiempo" |
| `connection_closed` | the peer closed the connection before answering (EOF) | "el servidor cerró la conexión antes de responder" |
| `tls_handshake` | the TLS handshake failed | "falló el handshake TLS" |
| `unknown` | the failure could not be classified | "fallo de red no clasificado" |

`unknown` is a **value, not a shrug**: it means the classifier ran and could not decide, and it
is the honest answer for a cause the taxonomy does not cover. It never means "the classifier did
not run".

## The surfaces

- **A new domain module** `crates/webfang_core/src/domain/error/cause.rs`: the `NetworkCause`
  enum, its stable values, the Spanish messages, and the classifier's contract. A new file keeps
  `crates/webfang_core/src/error.rs` (395 lines today) under the ~500-line rule — that file is a
  **declared exception** in #2584's program ("declarative/cohesive exceptions (e.g. `error.rs`)
  stay grandfathered"), so nobody is splitting it, but adding to it would still push it.
- **The error chain**: `ScraperError::Network` carries the classified cause instead of a bare
  message.
- **The classifier** lives where the error is still rich — the downloader's failure boundary,
  where the underlying error's source chain can still be inspected — not at the far end of the
  chain where the information has been flattened into a string.
- **The resolver** marks its own failures, so the DNS class survives the wrapping. This is the
  open question of the RED phase: the `--url` path names the DNS failure today and the batch
  path does not, and the difference has to be found before the classifier can be placed.
- **The trace**: the error events gain the `cause` field. The schema change is announced in the
  PR body, as a public contract.

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
