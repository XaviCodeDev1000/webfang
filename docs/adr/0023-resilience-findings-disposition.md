# ADR 0023: Disposition of the Five R4 Resilience Findings from #3000

- **Status:** Accepted — 2026-11-23.
- **Date:** 2026-11-23
- **Deciders:** Project owner (`Xavi`), R4 resilience lens
- **Related:** `ADR-0014` (persistence unification and hybrid D3 commit point), review lineage `review-a5a258b012f152bb` on PR `#2998`
- **Docs:** `docs/adr/README.md`, `AGENTS.md` § Observability
- **Issues:** `#3000` (this ADR); follow-up of `#2972` / `#2584`
- **Supersedes:** — (records pre-existing constraints; does **not** reopen review on PR `#2998`)

---

## 1. Context

The native RDD review of PR `#2998` (the Ola 3 B10a file split of the export
module) approved with zero blockers. Its approval envelope listed five
advisory resilience findings, all pre-existing in the pre-split code and only
relocated by the split. Issue `#3000` requires each to be either fixed with a
regression test or accepted as a documented constraint, without reopening the
review on PR `#2998`.

One finding was verified to be stronger than its issue text. The finding said a
torn append would fail to locate the insertion point; the actual code path
`VectorExporter::writer` only truncates when a `]` exists in the last 256 bytes.
A document truncated mid-write leaves no `]`, `read_append_state` reads only the
header counter and never parses document lines, and `close_json` appends `]}`.
The append therefore **succeeds** and produces invalid JSON. This was reproduced
with a RED test before any fix.

## 2. Decision

| Finding | Disposition | Where |
| :--- | :--- | :--- |
| R4-HeaderScanWindow | Fixed | `vector_exporter_header.rs`: adaptive scan, 512 B initial window doubling to a 64 KiB cap |
| R4-NoFsyncOnWrite | **Accepted as a constraint** | This ADR, § 3 |
| R4-AppendCrashRecoveryGap | Fixed | `VectorExporter::writer`: typed error + structured warning, file left byte-identical; § 4 |
| R4-LockSentinelWorldReadable | Fixed | `StoreLock::acquire`: `0o600` at creation and on every acquire on Unix |
| R4-MigrationBackupTimestampCollision | Fixed | `unique_suffix()` shared by temp and `.v1.bak.` names |

The header fix is deliberately a growth strategy, not a hard length rejection:
S1 asks for a bigger header to be *handled*. The scan grows only while the
`documents` marker is missing and the file is longer than the current window,
so the common case remains one bounded read.

## 3. Why no fsync is still the contract

`StoreFs::write` flushes to the OS page cache and `RealFs::rename` is the
commit point. Rust's `File::flush` is a no-op on Unix and Windows, and
`File::sync_all` would block until the filesystem confirms the data reached
disk. Adding it to every write would change the latency contract of a hot path
for a failure mode the design never claimed to cover.

The store promises **process-crash durability**, not power-loss durability.
A SIGKILL closes descriptors and leaves the rename atomicity intact; a power
loss can still lose a committed rename, and `load_or_init` then quarantines the
unreadable state and starts a fresh store. That quarantine behavior is the
documented recovery for the power-loss case, not a regression.

The relevant Rust standard library contracts were checked before accepting
this: `OpenOptionsExt::mode` (creation bits, default `0o666` masked by umask)
and `File::sync_all` / `sync_data` / `flush`. Source:
<https://doc.rust-lang.org/stable/std/os/unix/fs/trait.OpenOptionsExt.html>,
<https://doc.rust-lang.org/stable/std/fs/struct.File.html>.

## 4. Why torn vector appends fail closed

The JSONL export path already repairs a torn tail by truncating back to the
last newline and preserving the abandoned bytes. The vector format has no
equivalent, and adding one here would mean deleting bytes from a document that
may be otherwise recoverable by an operator. Rejecting the append converts
silent corruption into an explicit `ExporterError::WriteError` and leaves every
byte in place. Automatic vector torn-tail recovery is a separate design
decision, explicitly out of scope for `#3000`.

The SIGKILL crash-matrix harness continues to cover the clean-close flow; the
new unit test covers the torn-document flow that the harness did not assert.

## 5. Consequences

- Existing valid vector exports keep the same on-disk format and the same
  append behavior. Only a file with no closing bracket is newly rejected.
- A pre-existing `<state>.lock` sentinel is tightened to `0o600` on the next
  acquire; mutual exclusion semantics are unchanged (advisory `flock(2)` on
  Unix, `LockFileEx` on Windows).
- New v1 backups carry a monotonic per-process suffix after the millisecond
  stamp. No code reads the suffix, so this is additive.
- No item reopens review on PR `#2998`. This ADR is the citable record.

## 6. Alternatives rejected

- **Raise `HEADER_SCAN_BYTES` to a larger fixed constant.** Still truncates a
  header beyond the new value; the adaptive scan has the same fast path.
- **Add `sync_all` to `RealFs::write`.** Changes a hot-path latency contract and
  still does not fsync the parent directory, so it would not fully deliver
  rename durability.
- **Port `recover_torn_tail` to the vector exporter.** Destructive without a
  separate format-level decision; fail-closed preserves the evidence.
- **Reject oversized `created_at` at construction.** Would not help a legacy
  external file, which is where the truncation was actually observed.

## 7. Verification

Each fixed item has a regression test that was observed failing before the fix
and passing after it. `cargo nextest run -p webfang_core --lib` is green, and
the full pre-commit gate plus the local CI gate are recorded in the feature
document `odd/tasks/resilience-3000.md`.
