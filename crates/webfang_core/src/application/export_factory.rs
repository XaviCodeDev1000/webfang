//! Export factory for creating exporters based on format
//!
//! Provides flexible factory methods for creating appropriate exporters
//! based on ExportFormat enum values.
//!
//! # D3 commit-point protocol (PR3)
//!
//! [`process_results`] drives each item through the frozen per-item sequence:
//!
//! ```text
//! 1. output stage append + flush Ok          ← OUTPUT DURABLE (flush barrier)
//! 2. record.advance(Processed → Exported)    ← in-memory
//! 3. record_store.save()                     ← EXPORTED CHECKPOINT DURABLE
//! 4. record.advance(Exported → Committed)    ← in-memory
//! 5. record_store.save()                     ← ★ COMMIT POINT ★
//! ```
//!
//! Until PR4 introduces the single-writer JSONL session, the exporter's
//! successful return IS the flush barrier (each `JsonlExporter::export`
//! flushes before returning `Ok`). No record ever claims more than what was
//! durably flushed.

use std::collections::HashSet;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;

use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::application::resume::{canonical_key, load_preserving, RunId};
use crate::domain::crawler_port::filename::confine_filename_component;
use crate::domain::page_state::{PageStatus, Stateful};
use crate::domain::persistence::{
    DomainRecords, LastError, RawRecord, RecordStoreError, RecordStorePort,
};
use crate::domain::record_transition;
use crate::domain::{
    entities::ExportFormat,
    exporter::{ExporterError, CHECKSUM_FIELD},
    Exporter, ExporterConfig,
};

/// Per-run resume/commit context handed to the export functions (D5 seams).
///
/// Groups the record store, this run's identity, and the skip-policy switch.
/// With `resume = false` the gate never consults prior history (fresh run
/// re-drives everything while old records stay preserved — A2/E10).
pub struct ResumeContext<'a> {
    pub(crate) store: &'a dyn RecordStorePort,
    pub(crate) run_id: RunId,
    pub(crate) resume: bool,
    pub(crate) cancel: Option<&'a CancellationToken>,
    /// Test-only observer fired after every record-store save with the
    /// persisted status (ordering proof: EXPORTED strictly before COMMITTED).
    pub(crate) persist_observer: Option<&'a dyn Fn(PageStatus)>,
}

impl<'a> ResumeContext<'a> {
    /// A context for `store` with a fresh [`RunId`] and no skipping.
    #[must_use]
    pub fn new(store: &'a dyn RecordStorePort) -> Self {
        Self {
            store,
            run_id: RunId::new(),
            resume: false,
            cancel: None,
            persist_observer: None,
        }
    }

    /// Enable `--resume` semantics: skip ONLY `COMMITTED`-proven records.
    #[must_use]
    pub fn with_resume(mut self, resume: bool) -> Self {
        self.resume = resume;
        self
    }

    /// Observe cooperative cancellation between items (drain-before-final-
    /// persist: the in-flight item always completes its full D3 sequence).
    #[must_use]
    pub fn observing_cancel(mut self, token: &'a CancellationToken) -> Self {
        self.cancel = Some(token);
        self
    }

    /// Test-only observer hook for the PR5 crash-matrix ordering proof
    /// (asserts EXPORTED persists strictly before COMMITTED). Unused until
    /// the harness lands.
    #[allow(dead_code)]
    #[must_use]
    pub(crate) fn with_persist_observer(mut self, f: &'a dyn Fn(PageStatus)) -> Self {
        self.persist_observer = Some(f);
        self
    }
}

/// Why an item took a particular route through the export phase.
enum ItemDecision {
    /// Record proven `COMMITTED` — skip entirely (type-level gate).
    AlreadyCommitted,
    /// Output flush proven via content-hash membership — promote to
    /// `COMMITTED` without appending (D3 recovery actions 3–4).
    PromoteFromFlushProof,
    /// Normal path: append, checkpoint `EXPORTED`, commit.
    DriveAndCommit,
}

/// Per-run commit session: the loaded record map plus the content-hash index
/// of the target output file (D3 recovery seam).
pub(crate) struct CommitSession<'a> {
    ctx: Option<&'a ResumeContext<'a>>,
    records: DomainRecords,
    /// `checksum_sha256` values parsed from every valid line already in the
    /// output file. Membership proves "bytes flushed" without timing guesses.
    hash_index: HashSet<String>,
    /// `(url, content_hash)` pairs already appended during THIS run (#1215).
    /// The JSONL exporter is append-only — it cannot retract a line — so a
    /// second identical item must skip its append, or the file holds duplicate
    /// records that differ only in `timestamp_utc` (F-02).
    driven: HashSet<(String, String)>,
    /// Lines of the output file that were skipped while indexing it because
    /// they were not valid UTF-8 (#1812). The export summary carries the
    /// count: the operator must SEE what was dropped.
    skipped_non_utf8: usize,
}

impl<'a> CommitSession<'a> {
    pub(crate) fn open(ctx: Option<&'a ResumeContext<'a>>, output_path: &std::path::Path) -> Self {
        let records = ctx.map_or_else(DomainRecords::new, |c| load_preserving(c.store));
        let (hash_index, report) = build_content_hash_index(output_path);
        Self {
            ctx,
            records,
            hash_index,
            driven: HashSet::new(),
            skipped_non_utf8: report.skipped_non_utf8,
        }
    }

    fn cancelled(&self) -> bool {
        self.ctx
            .and_then(|c| c.cancel)
            .is_some_and(|t| t.is_cancelled())
    }

    /// Consume the session into its export outcome (#2272): the URLs that
    /// were driven plus the count of lines the index skipped. The count
    /// travels BOTH in the structured summary log (the surface the operator
    /// reads) and in the outcome (the surface callers transport).
    fn finish(self, processed_urls: Vec<String>, summary: String) -> ExportOutcome {
        let skipped_non_utf8 = self.skipped_non_utf8;
        info!(total = processed_urls.len(), skipped_non_utf8, "{summary}");
        ExportOutcome {
            processed_urls,
            skipped_non_utf8,
        }
    }

    /// Claim one `(url, content_hash)` append for this run. Returns `false`
    /// when the pair was already appended — the caller must skip the export
    /// append so the file stays idempotent on that key (#1215). Same URL with
    /// different content (or vice versa) is a different pair and still exports.
    fn claim_append(&mut self, url: &str, content_hash: &str) -> bool {
        self.driven
            .insert((url.to_string(), content_hash.to_string()))
    }

    /// The resume-gate decision for one item. With `resume = false` every
    /// item re-drives (fresh-run semantics); with `resume = true` only
    /// `COMMITTED`-proven records skip.
    fn decide(&self, url: &str, fresh_hash: Option<&str>) -> ItemDecision {
        let Some(ctx) = self.ctx else {
            return ItemDecision::DriveAndCommit;
        };
        if !ctx.resume {
            return ItemDecision::DriveAndCommit;
        }
        match self.records.get(&canonical_key(url)) {
            Some(record) => {
                // Type-level gate: skip ONLY when the record reconciles into
                // the terminal state through the D2 table (SC2 mechanism).
                if Stateful::<RawRecord, crate::domain::page_state::Committed>::reconcile(
                    record.clone(),
                )
                .is_ok()
                {
                    return ItemDecision::AlreadyCommitted;
                }
                if let Some(hash) = &record.content_hash {
                    if self.hash_index.contains(hash) {
                        return ItemDecision::PromoteFromFlushProof;
                    }
                }
            },
            None => {
                // Flush-proof WITHOUT a record: a previous run may have died
                // after flushing this exact content but before persisting ANY
                // record. The record's own claim takes precedence when one
                // exists (an unproven EXPORTED must re-export).
                if let Some(fresh) = fresh_hash {
                    if self.hash_index.contains(fresh) {
                        return ItemDecision::PromoteFromFlushProof;
                    }
                }
            },
        }
        ItemDecision::DriveAndCommit
    }

    fn promote_from_flush_proof(&mut self, url: &str, output_location: String, content_hash: &str) {
        let key = canonical_key(url);
        let Some(ctx) = self.ctx else { return };
        let Some(record) = self.resolve_flush_proof_record(url, &key, &ctx.run_id) else {
            return;
        };
        let Some(exported) =
            Self::drive_to_export_flushed(record, url, &ctx.run_id, content_hash, &output_location)
        else {
            return;
        };
        self.records.insert(key.clone(), exported.record().clone());
        self.save_notifying(PageStatus::Exported);
        // ★ COMMIT POINT ★ — no re-append; the line is already on disk.
        // The machine-owned transition shared with the loom model.
        if record_transition::swap_to_committed(&mut self.records, &key) {
            info!(url, "flush-proof promotion: advancing to COMMITTED");
            self.save_notifying(PageStatus::Committed);
        } else {
            warn!(
                url,
                "commit-point transition rejected by the machine; record left EXPORTED"
            );
        }
    }

    /// Fetch the record backing a flush-proof promotion. A record-less
    /// promotion means the previous run died before ANY persistence:
    /// synthesize a fresh lifecycle so the URL still lands in the store
    /// Committed (invariant: every input URL present).
    /// Returns `None` when the identity is structurally invalid (#876).
    fn resolve_flush_proof_record(
        &mut self,
        url: &str,
        key: &str,
        run_id: &crate::application::resume::RunId,
    ) -> Option<RawRecord> {
        if let Some(record) = self.records.remove(key) {
            return Some(record);
        }
        match fresh_discovered(url, key, run_id) {
            Some(record) => Some(record.into_record()),
            None => {
                warn!(
                    url,
                    "record-less flush-proof promotion rejected: structurally invalid URL"
                );
                None
            },
        }
    }

    /// Advance a record to the EXPORTED state for a proven flush.
    /// EXPORTED records take their legal direct transition (commit);
    /// ONLY lower states walk the drive chain — driving an EXPORTED
    /// record would hit the terminal-state guard and lose its
    /// content_hash (the dedup identity of the flushed line).
    /// Returns `None` when the record identity is structurally invalid (#876).
    fn drive_to_export_flushed(
        record: RawRecord,
        url: &str,
        run_id: &crate::application::resume::RunId,
        content_hash: &str,
        output_location: &str,
    ) -> Option<Stateful<RawRecord, crate::domain::page_state::Exported>> {
        use crate::domain::page_state::Exported;

        let flushed_path = PathBuf::from(output_location);
        if record.status == PageStatus::Exported {
            if let Ok(exported) = Stateful::<RawRecord, Exported>::reconcile(record.clone()) {
                return Some(exported);
            }
            tracing::warn!(url, "EXPORTED record broke invariants; re-driving fresh");
            let mut processed = drive_to_processed(record, run_id)?;
            {
                let payload = processed.record_mut();
                payload.updated_at = now_millis();
                payload.content_hash = Some(content_hash.to_owned());
            }
            return Some(processed.export_flushed(flushed_path));
        }
        let mut processed = drive_to_processed(record, run_id)?;
        {
            let payload = processed.record_mut();
            payload.attempts += 1;
            payload.last_error = None;
            payload.updated_at = now_millis();
            payload.content_hash = Some(content_hash.to_owned());
            if payload.output_location.is_none() {
                payload.output_location = Some(output_location.to_owned());
            }
        }
        Some(processed.export_flushed(flushed_path))
    }

    /// Record an item failure honestly: attempts++, classified last_error,
    /// updated_at — persisted at the record's NON-advanced state (SC6).
    /// A never-seen URL gets a fresh DISCOVERED record carrying the failure.
    fn fail_item(&mut self, url: &str, class: crate::error::ErrorClass, message: &str) {
        let Some(ctx) = self.ctx else { return };
        let key = canonical_key(url);
        let record = match self.records.entry(key.clone()) {
            std::collections::btree_map::Entry::Occupied(entry) => entry.into_mut(),
            std::collections::btree_map::Entry::Vacant(entry) => {
                // #876 domain boundary: an empty URL is structurally
                // meaningless; reject instead of persisting a record that
                // could never address a page.
                let record = match RawRecord::new_discovered(
                    url,
                    &key,
                    ctx.run_id.as_str(),
                    now_millis(),
                ) {
                    Ok(record) => record,
                    Err(err) => {
                        warn!(url, error = %err, "rejecting record creation for structurally invalid URL");
                        return;
                    },
                };
                entry.insert(record)
            },
        };
        record.attempts += 1;
        record.last_error = Some(LastError {
            class,
            message: message.to_string(),
        });
        record.updated_at = now_millis();
        warn!(url, class = ?class, "item failed; status NOT advanced");
        self.save_quiet();
    }

    /// D3 steps 2–5 for one successfully flushed item.
    pub(crate) fn commit_item(
        &mut self,
        url: &str,
        content_hash: String,
        output_location: PathBuf,
        run_id: &RunId,
    ) {
        // Crash-injection point: exporter returned (flush ack received)
        // but EXPORTED checkpoint not yet saved - the critical D3 window.
        crate::cli::crash_points::hit(crate::cli::crash_points::POST_FLUSH_PRE_COMMIT);
        let key = canonical_key(url);
        // Drive the record to PROCESSED along its legal chain (fresh URLs
        // enter as DISCOVERED; re-drives resume from their recorded state).
        let existing = self.records.remove(&key);
        // A2 fresh-run semantics: a terminal-state record from a PREVIOUS
        // run re-drives under the new run_id — the new run starts its own
        // honest lifecycle from DISCOVERED.
        let existing = existing.filter(|record| {
            !matches!(record.status, PageStatus::Exported | PageStatus::Committed)
        });
        let mut processed = match existing {
            Some(record) => {
                let Some(processed) = drive_to_processed(record, run_id) else {
                    warn!(url, "record identity is structurally invalid; item left uncommitted and will re-drive next run");
                    return;
                };
                processed
            },
            None => {
                // #876 domain boundary: validated construction or nothing.
                let fresh = match RawRecord::new_discovered(
                    url,
                    &key,
                    run_id.as_str(),
                    now_millis(),
                ) {
                    Ok(fresh) => fresh,
                    Err(err) => {
                        warn!(url, error = %err, "rejecting record creation for structurally invalid URL");
                        return;
                    },
                };
                Stateful::<RawRecord, crate::domain::page_state::Discovered>::new(fresh)
                    .queue()
                    .start_fetch()
                    .fetched()
                    .extracted()
                    .processed()
            },
        };
        {
            let payload = processed.record_mut();
            payload.run_id = run_id.as_str().to_string();
            payload.content_hash = Some(content_hash);
            payload.output_location = Some(output_location.display().to_string());
            payload.attempts += 1;
            payload.last_error = None; // success clears last_error (SC6)
            payload.updated_at = now_millis();
        }
        // Step 2–3: EXPORTED checkpoint durable BEFORE the commit point.
        let exported = processed.export_flushed(output_location);
        self.records.insert(key.clone(), exported.record().clone());
        self.save_notifying(PageStatus::Exported);
        // Steps 4–5: ★ COMMIT POINT ★ — rename(2) persisting Committed,
        // strictly after the output flush ack + EXPORTED checkpoint.
        // Re-reads the just-checkpointed EXPORTED slot through the ONE
        // machine-owned transition shared with the loom model.
        if record_transition::swap_to_committed(&mut self.records, &key) {
            self.save_notifying(PageStatus::Committed);
        } else {
            // Unreachable by construction: the slot holds the EXPORTED the
            // typed machine just produced. Honesty wins over an assert —
            // skip the commit point and let the next run re-drive.
            warn!(
                url,
                "commit-point transition rejected by the machine; item left EXPORTED"
            );
        }
    }

    /// Commit this session's working set through the store's own
    /// transaction (#1230 / F-07).
    ///
    /// `self.records` was loaded once in [`CommitSession::open`]; writing it
    /// back with `save` would replace whatever a concurrent `--resume`
    /// process committed in the meantime. `update` re-reads under the store
    /// lock, so the merge happens against durable truth.
    ///
    /// Conflict rule: **per-URL last-writer-wins**. A record another writer
    /// advanced after our `open()` carries a later `updated_at` and
    /// survives; ours wins otherwise. Ties go to ours, because we are the
    /// writer that just observed the transition.
    fn merge_persist(&self, ctx: &ResumeContext<'_>) -> Result<(), RecordStoreError> {
        let mine = &self.records;
        ctx.store.update(&mut |current| {
            for (key, record) in mine {
                let newer_already_wins = current
                    .get(key)
                    .is_some_and(|existing| existing.updated_at > record.updated_at);
                if !newer_already_wins {
                    current.insert(key.clone(), record.clone());
                }
            }
            Ok(())
        })
    }

    fn save_and_notify(&self, observer: Option<&dyn Fn(PageStatus)>, status: PageStatus) {
        let Some(ctx) = self.ctx else { return };
        match self.merge_persist(ctx) {
            Ok(()) => {
                if let Some(f) = observer.or(ctx.persist_observer) {
                    f(status);
                }
            },
            Err(e) => tracing::error!(error = %e, "record-store save failed"),
        }
    }

    /// Drain-before-final-persist (E8): after cancellation stops new items,
    /// one final save persists any remaining honest state before exit.
    pub(crate) fn final_persist(&self) {
        let Some(ctx) = self.ctx else { return };
        if let Err(e) = self.merge_persist(ctx) {
            tracing::error!(error = %e, "final record-store persist failed");
        }
    }

    /// Save and fire the ordering observer with the persisted status.
    fn save_notifying(&self, status: PageStatus) {
        self.save_and_notify(None, status);
    }

    /// Save failure state WITHOUT firing the transition observer (a failed
    /// item did not advance; the observer tracks lifecycle transitions only).
    fn save_quiet(&self) {
        if let Some(ctx) = self.ctx {
            if let Err(e) = self.merge_persist(ctx) {
                tracing::error!(error = %e, "record-store save failed");
            }
        }
    }
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or_default()
}

/// Walk a raw record forward along its legal chain to `PROCESSED`. Every
/// arm reconciles at the recorded position first — the typestate guarantees
/// no illegal move is expressible even during recovery re-drives.
/// A reconciliation failure (impossible persisted state that slipped past
/// load-time quarantine) degrades to a fresh DISCOVERED lifecycle under the
/// current run instead of panicking. Returns `None` only when even that
/// fresh lifecycle cannot be constructed (#876: structurally meaningless
/// identity) — callers must skip the item rather than persist garbage.
fn drive_to_processed(
    record: RawRecord,
    run_id: &RunId,
) -> Option<Stateful<RawRecord, crate::domain::page_state::Processed>> {
    use crate::domain::page_state::{
        Discovered, Extracted, Fetched, Fetching, Processed, Queued, ReconcileError,
    };
    let url = record.url.clone();
    let canonical_url = record.canonical_url.clone();
    let result = match record.status {
        PageStatus::Discovered => Stateful::<RawRecord, Discovered>::reconcile(record)
            .map(|s| s.queue().start_fetch().fetched().extracted().processed()),
        PageStatus::Queued => Stateful::<RawRecord, Queued>::reconcile(record)
            .map(|s| s.start_fetch().fetched().extracted().processed()),
        PageStatus::Fetching => Stateful::<RawRecord, Fetching>::reconcile(record)
            .map(|s| s.fetched().extracted().processed()),
        PageStatus::Fetched => {
            Stateful::<RawRecord, Fetched>::reconcile(record).map(|s| s.extracted().processed())
        },
        PageStatus::Extracted => {
            Stateful::<RawRecord, Extracted>::reconcile(record).map(|s| s.processed())
        },
        PageStatus::Processed => Stateful::<RawRecord, Processed>::reconcile(record),
        // Terminal states are filtered upstream (A2 fresh-run re-drive).
        PageStatus::Exported | PageStatus::Committed => Err(ReconcileError::StatusMismatch {
            expected: PageStatus::Processed,
            found: record.status,
        }),
    };
    match result {
        Ok(processed) => Some(processed),
        Err(e) => {
            tracing::warn!(
            error = %e,
            "quarantining impossible persisted state; starting fresh lifecycle"
                        );
            let fresh = fresh_discovered(&url, &canonical_url, run_id)?;
            Some(
                fresh
                    .queue()
                    .start_fetch()
                    .fetched()
                    .extracted()
                    .processed(),
            )
        },
    }
}

/// Start a brand-new DISCOVERED lifecycle for one URL under `run_id`
/// (A2: a new run owns its records; history stays on disk until commit).
/// Returns `None` when the URL fails the #876 domain boundary validation.
fn fresh_discovered(
    url: &str,
    canonical_url: &str,
    run_id: &RunId,
) -> Option<Stateful<RawRecord, crate::domain::page_state::Discovered>> {
    let record = match RawRecord::new_discovered(url, canonical_url, run_id.as_str(), now_millis())
    {
        Ok(record) => record,
        Err(err) => {
            tracing::warn!(url, error = %err, "rejecting record creation for structurally invalid URL");
            return None;
        },
    };
    Some(Stateful::<RawRecord, crate::domain::page_state::Discovered>::new(record))
}

/// Counters collected while indexing an existing JSONL output file.
///
/// The report is what the export summary shows the operator: how many lines
/// were read, how many carried no checksum, and how many were skipped
/// because they were not valid UTF-8 (#1812).
#[derive(Debug, Default, Clone, Copy)]
pub struct JsonlIndexReport {
    /// Non-empty lines read from the file.
    pub lines: usize,
    /// Lines that parsed as a JSON object but carried no checksum field.
    pub without_checksum: usize,
    /// Lines that were not valid UTF-8 and were skipped.
    pub skipped_non_utf8: usize,
}

/// The [`CHECKSUM_FIELD`] index of an existing JSONL output file, plus the
/// counters collected while reading it.
#[derive(Debug, Default)]
pub(crate) struct JsonlChecksumIndex {
    /// Every checksum found in the file.
    pub(crate) hashes: HashSet<String>,
    /// What the read observed.
    pub(crate) report: JsonlIndexReport,
}

/// Read the [`CHECKSUM_FIELD`] index of an existing JSONL output file.
///
/// # Streaming (#1812)
///
/// The file is read LINE BY LINE through a [`BufReader`], never as one
/// string: an export larger than RAM must not be loaded into RAM to be
/// indexed. This is the ONE reader for the export file's checksum field
/// (#2393): the JSONL writer session no longer reads the output file at all
/// on open — only its torn tail — so the application layer is the single
/// place that parses lines for `CHECKSUM_FIELD`, and the resume gate's view
/// of the file is built exactly here.
///
/// # Damaged bytes (#1812)
///
/// A line that is not valid UTF-8 is SKIPPED and counted, never fatal: one
/// damaged byte must not abort the whole export. The caller reports the
/// count through [`JsonlIndexReport::skipped_non_utf8`].
///
/// # Errors
///
/// Returns an I/O error when the file exists but cannot be read. A missing
/// file is NOT an error — it yields an empty index, exactly like a first run
/// against a fresh output path.
pub(crate) fn read_jsonl_checksum_index(
    path: &std::path::Path,
) -> std::io::Result<JsonlChecksumIndex> {
    let mut index = JsonlChecksumIndex::default();
    let file = match File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(index),
        Err(e) => return Err(e),
    };
    let mut reader = BufReader::new(file);
    let mut line = Vec::new();
    loop {
        line.clear();
        if reader.read_until(b'\n', &mut line)? == 0 {
            break;
        }
        // Strip the line terminator: the `\n` and the `\r` of a CRLF file.
        while matches!(line.last(), Some(b'\n') | Some(b'\r')) {
            line.pop();
        }
        if line.is_empty() {
            continue;
        }
        index.report.lines += 1;
        let Ok(text) = std::str::from_utf8(&line) else {
            index.report.skipped_non_utf8 += 1;
            continue;
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
            continue;
        };
        match value
            .get(CHECKSUM_FIELD)
            .and_then(serde_json::Value::as_str)
        {
            Some(hash) => {
                index.hashes.insert(hash.to_owned());
            },
            None => index.report.without_checksum += 1,
        }
    }
    Ok(index)
}

/// Log a structured error that ALSO carries the [`std::io::ErrorKind`].
///
/// The shared [`log_scrape_error`](crate::infrastructure::observability::log_scrape_error)
/// renders `error = %error`, and an `io::Error`'s `Display` prints only its
/// message: the `ErrorKind` set on the value never reaches the event, so the
/// operator sees the message and the stage but not the KIND of the damage.
/// That helper's signature is frozen (other callers depend on it), so the
/// extra field is added HERE, at the call site.
///
/// The field set is the shared helper's, plus `error_kind`.
fn log_io_error_with_kind(
    error: &std::io::Error,
    url: &str,
    stage: &str,
    correlation_id: Option<&crate::domain::CorrelationId>,
    context: &str,
) {
    tracing::error!(
        error = %error,
        error_kind = ?error.kind(),
        url = %url,
        stage = %stage,
        trace_id = correlation_id.map(|c| c.trace_id().to_string()),
        "{context}"
    );
}

/// Rebuild the resume dedup index from an existing output file.
///
/// The field name comes from the pinned [`CHECKSUM_FIELD`] constant (#1617,
/// D3), not from a string literal at this site: the exporter writes it and
/// this is the ONE reader (#2393), so a rename on either side is visible
/// here. Lines that parse but carry no checksum are counted and reported, so
/// a writer/reader field-name mismatch is visible instead of silent.
///
/// The read itself is [`read_jsonl_checksum_index`]'s: STREAMING, and a line
/// that is not valid UTF-8 is skipped and reported instead of failing the
/// export (#1812).
fn build_content_hash_index(path: &std::path::Path) -> (HashSet<String>, JsonlIndexReport) {
    let index = match read_jsonl_checksum_index(path) {
        Ok(index) => index,
        Err(e) => {
            // The index only PROVES a flush for the resume gate; an
            // unreadable file proves nothing, but it must not abort the
            // export either. Report it and continue with an empty index.
            log_io_error_with_kind(
                &e,
                &path.display().to_string(),
                "jsonl_resume_index",
                None,
                "could not read the output file to rebuild the resume index; continuing with an empty index",
            );
            return (HashSet::new(), JsonlIndexReport::default());
        },
    };
    if index.report.skipped_non_utf8 > 0 {
        // The DECIDED policy (#1812): a line that is not valid UTF-8 is
        // skipped, the export COMPLETES, and the damage is reported — never
        // a silent skip and never a failed export.
        let error = std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "{} line(s) of the export file are not valid UTF-8",
                index.report.skipped_non_utf8
            ),
        );
        log_io_error_with_kind(
            &error,
            &path.display().to_string(),
            "jsonl_resume_index",
            None,
            "JSONL lines that are not valid UTF-8 were skipped while indexing the export file",
        );
    }
    if index.report.without_checksum > 0 {
        tracing::warn!(
            file = %path.display(),
            lines = index.report.lines,
            lines_without_checksum = index.report.without_checksum,
            field = CHECKSUM_FIELD,
            "output lines carry no checksum field; resume dedup will re-drive them (writer/reader field-name mismatch?)"
        );
    }
    info!(
        file = %path.display(),
        lines = index.report.lines,
        hashes = index.hashes.len(),
        skipped_non_utf8 = index.report.skipped_non_utf8,
        "indexed output file for resume dedup"
    );
    (index.hashes, index.report)
}

/// Report of the ONE reader of an existing JSONL output file: how many
/// non-empty lines it has, how many carry no checksum field, and how many
/// were skipped for not being valid UTF-8.
///
/// This is the public surface for callers outside a commit session — the MCP
/// layer transports the count instead of recomputing it with its own line
/// semantics (#2272).
///
/// # Tolerant by policy (#1812)
///
/// An unreadable file proves nothing, but it must not abort: the caller gets
/// the empty report and the I/O error is logged. Only a genuinely damaged
/// count can be lost, and the export summary never lies about lines it did
/// read.
pub fn jsonl_index_report(path: &std::path::Path) -> JsonlIndexReport {
    match read_jsonl_checksum_index(path) {
        Ok(index) => index.report,
        Err(e) => {
            log_io_error_with_kind(
                &e,
                &path.display().to_string(),
                "jsonl_resume_index",
                None,
                "could not read the output file to report the JSONL index; continuing with an empty report",
            );
            JsonlIndexReport::default()
        },
    }
}

/// Map an export-stage failure onto the SC6 taxonomy: item-data problems are
/// `DomainRecoverable` (record + continue); environment failures stay fatal.
fn classify_export_failure(err: &ExporterError) -> crate::error::ErrorClass {
    use crate::error::ErrorClass;
    match err {
        ExporterError::Serialization(_) | ExporterError::InvalidConfig(_) => {
            ErrorClass::DomainRecoverable
        },
        _ => ErrorClass::InternalFatal,
    }
}

/// Create exporter based on output format
pub fn create_exporter(
    output_dir: PathBuf,
    filename: &str,
    format: ExportFormat,
) -> Result<Box<dyn Exporter>, ExporterError> {
    match format {
        ExportFormat::Jsonl => Ok(create_jsonl_exporter(output_dir, filename)),
        ExportFormat::Vector => Ok(create_vector_exporter(output_dir, filename)),
        ExportFormat::Auto => create_auto_exporter(output_dir, filename),
    }
}

/// Build a JSONL exporter with append mode enabled.
fn create_jsonl_exporter(output_dir: PathBuf, filename: &str) -> Box<dyn Exporter> {
    let config = ExporterConfig::new(output_dir, ExportFormat::Jsonl, filename).with_append(true);
    info!("Creating JSONL exporter: {:?}", config.output_path());
    crate::application::container::build_jsonl_exporter(config)
}

/// Build a Vector exporter with append mode enabled.
fn create_vector_exporter(output_dir: PathBuf, filename: &str) -> Box<dyn Exporter> {
    let config = ExporterConfig::new(output_dir, ExportFormat::Vector, filename).with_append(true);
    info!("Creating Vector exporter: {:?}", config.output_path());
    crate::application::container::build_vector_exporter(config)
}

/// Auto-detect the export format from existing files (`export.jsonl` /
/// `export.json`), falling back to JSONL when neither exists.
fn create_auto_exporter(
    output_dir: PathBuf,
    filename: &str,
) -> Result<Box<dyn Exporter>, ExporterError> {
    // Auto-detect: checks if export.jsonl or export.json exists
    info!("Auto-detecting format...");

    // Confine before probing so a hostile filename can never escape
    // `output_dir` through the existence checks below (#1125).
    let filename = confine_filename_component(filename, "export");
    let jsonl_path = output_dir.join(format!("{filename}.jsonl"));
    let vector_path = output_dir.join(format!("{filename}.json"));

    if jsonl_path.exists() {
        Ok(create_detected_jsonl(output_dir, &filename, &jsonl_path))
    } else if vector_path.exists() {
        Ok(create_detected_vector(output_dir, &filename, &vector_path))
    } else {
        Ok(create_default_jsonl(output_dir, &filename))
    }
}

/// Build a JSONL exporter for the auto-detected `.jsonl` file.
fn create_detected_jsonl(
    output_dir: PathBuf,
    filename: &str,
    jsonl_path: &std::path::Path,
) -> Box<dyn Exporter> {
    info!("Detected JSONL format - {:?} exists", jsonl_path);
    create_jsonl_exporter(output_dir, filename)
}

/// Build a Vector exporter for the auto-detected `.json` file.
fn create_detected_vector(
    output_dir: PathBuf,
    filename: &str,
    vector_path: &std::path::Path,
) -> Box<dyn Exporter> {
    info!("Detected Vector format - {:?} exists", vector_path);
    create_vector_exporter(output_dir, filename)
}

/// Build a JSONL exporter when no existing export file is found.
fn create_default_jsonl(output_dir: PathBuf, filename: &str) -> Box<dyn Exporter> {
    info!("No existing export, using default Jsonl format");
    create_jsonl_exporter(output_dir, filename)
}

/// Outcome of one item through the export phase.
enum SingleItemOutcome {
    /// Counted as processed this run.
    Driven,
    /// COMMITTED-proven skip (not counted as processed).
    Skipped,
    /// Cooperative cancellation observed; stop the loop.
    Cancelled,
}

/// D3 sequence for exactly one scraped item (decide → flush → checkpoint
/// → commit, or honest failure recording). Extracted from
/// `process_results` to stay within complexity ratchets.
fn process_single_item(
    session: &mut CommitSession<'_>,
    exporter: &dyn crate::domain::Exporter,
    result: &crate::domain::ScrapedContent,
    output_path: &std::path::Path,
    run_id: &RunId,
) -> SingleItemOutcome {
    use crate::domain::entities::DocumentChunkUnvalidated;
    use sha2::{Digest, Sha256};

    let url_str = result.url.as_str().to_string();
    // Same digest the JsonlExporter stamps into the line; computed up
    // front so decide() can prove flush membership even with no record.
    let content_hash = format!("{:x}", Sha256::digest(result.content.as_bytes()));
    // #1215: the exporter is append-only, so a second identical item in the
    // same run would write a duplicate record (same url + hash, fresh
    // timestamp). Claim the pair first; the loser skips without counting.
    if !session.claim_append(&url_str, &content_hash) {
        info!(url = %url_str, "duplicate (url, content_hash) already exported this run; skipping append");
        return SingleItemOutcome::Skipped;
    }
    match session.decide(&url_str, Some(content_hash.as_str())) {
        ItemDecision::AlreadyCommitted => {
            info!(url = %url_str, "resume gate: COMMITTED-proven; skipping");
            return SingleItemOutcome::Skipped;
        },
        ItemDecision::PromoteFromFlushProof => {
            session.promote_from_flush_proof(
                &url_str,
                output_path.display().to_string(),
                &content_hash,
            );
            if session.cancelled() {
                return SingleItemOutcome::Cancelled;
            }
            return SingleItemOutcome::Driven;
        },
        ItemDecision::DriveAndCommit => {},
    }

    // Item-data problems are DomainRecoverable per SC6: record the
    // classified failure and continue - never abort the export.
    let chunk = DocumentChunkUnvalidated::from_scraped_content(result);
    let validated = match chunk.validate() {
        Ok(validated) => validated,
        Err(e) => {
            let err = ExporterError::InvalidConfig(e.to_string());
            let class = classify_export_failure(&err);
            session.fail_item(&url_str, class, &err.to_string());
            return SingleItemOutcome::Skipped;
        },
    };

    match exporter.export(validated) {
        Ok(()) => {
            session.commit_item(
                &url_str,
                content_hash.clone(),
                output_path.to_path_buf(),
                run_id,
            );
            if session.cancelled() {
                return SingleItemOutcome::Cancelled;
            }
            SingleItemOutcome::Driven
        },
        Err(e) => {
            let class = classify_export_failure(&e);
            session.fail_item(&url_str, class, &e.to_string());
            warn!(url = %url_str, error = %e, "item export failed; continuing");
            if session.cancelled() {
                return SingleItemOutcome::Cancelled;
            }
            SingleItemOutcome::Skipped
        },
    }
}

/// Convert scraped results into the target export format with exactly-once
/// semantics: every item flows through the D3 sequence (flush → EXPORTED
/// checkpoint → COMMITTED) under the resume gate.
///
/// # Errors
///
/// Returns [`ExporterError`] only for exporter construction failures;
/// item-level failures are recorded per-record and never abort the run.
pub fn process_results(
    results: &[crate::domain::ScrapedContent],
    output_dir: PathBuf,
    format: ExportFormat,
    filename: &str,
    ctx: Option<&ResumeContext<'_>>,
) -> Result<ExportOutcome, ExporterError> {
    info!("Processing {} results for export", results.len());

    // Fail-fast on uncreatable output_dir (ENOTDIR through a regular file,
    // permission denied, etc.): surface as ExporterError::DirectoryCreation
    // so the MCP/CLI caller gets an honest isError:true instead of a fake
    // success that swallows the per-item error (D3 swallow bug, #854).
    std::fs::create_dir_all(&output_dir).map_err(ExporterError::DirectoryCreation)?;

    // Confine before joining so a hostile filename can never escape
    // `output_dir` through the session index or the exporter (#1125).
    let filename = confine_filename_component(filename, "export");

    // Hash-index BEFORE the exporter touches the file, so membership
    // reflects exactly the bytes flushed by previous runs (D3 seam).
    let output_path = output_dir.join(format!("{filename}.jsonl"));
    let mut session = CommitSession::open(ctx, &output_path);
    let fallback_run_id = RunId::new();
    let run_id = ctx.map_or(&fallback_run_id, |c| &c.run_id);

    let exporter = create_exporter(output_dir, &filename, format)?;
    let mut processed_urls = Vec::new();

    for result in results {
        match process_single_item(
            &mut session,
            exporter.as_ref(),
            result,
            &output_path,
            run_id,
        ) {
            SingleItemOutcome::Driven => {
                processed_urls.push(result.url.as_str().to_string());
            },
            SingleItemOutcome::Skipped => {},
            SingleItemOutcome::Cancelled => break,
        }
    }
    // Crash-injection (PR5b): the drain-ordering kill window — after the items
    // that completed their full D3 sequence and BEFORE `final_persist()`.
    // Checked ONCE, after the loop, instead of only on the `Cancelled`
    // outcome: a cancelled run whose items were all skipped (AlreadyCommitted
    // or duplicate) never reaches that arm, and the drain still happens
    // (#2269 cavity). One site, so the drain line cannot be logged twice.
    if session.cancelled() {
        tracing::info!("cancellation observed; draining before final persist");
        crate::cli::crash_points::hit(crate::cli::crash_points::DURING_CANCEL_DRAIN);
    }
    session.final_persist();

    let total = processed_urls.len();
    Ok(session.finish(
        processed_urls,
        format!("✅ Export completado: {total} documentos procesados"),
    ))
}

/// Result of a batch export ([`process_results`]): the URLs that were driven
/// through the export plus the count of pre-existing JSONL lines the index
/// skipped because they were not valid UTF-8.
///
/// The count comes from the ONE reader of the export file — the same number
/// the export summary logs — so callers transport it instead of recomputing
/// it (#2272).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportOutcome {
    /// URLs whose items completed the drive-and-commit sequence.
    pub processed_urls: Vec<String>,
    /// Lines of the pre-existing output file skipped while indexing (#1812).
    pub skipped_non_utf8: usize,
}

/// Get domain from URL
///
/// Extracts the domain (host) from a URL string.
///
/// # Arguments
///
/// * `url` - URL string to extract domain from
///
/// # Returns
///
/// Domain string (e.g., "example.com" from `<https://www.example.com/docs/api/>`)
///
/// # Examples
///
/// ```
/// use webfang_core::application::export_factory::domain_from_url;
///
/// let domain = domain_from_url("https://www.example.com/docs/api/");
/// assert_eq!(domain, "www.example.com");
/// ```
pub fn domain_from_url(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|p| p.host_str().map(str::to_string))
        .unwrap_or_else(|| "unknown".to_string())
}

/// What the AI export path will do with one chunk.
///
/// The plan is computed BEFORE the batch export, so the commit point can
/// only mark what was actually written (#1812). The non-AI path
/// ([`process_single_item`]) is the reference: a chunk that cannot be
/// exported is a RECORDED FAILURE, never a silent commit — a committed
/// record that was never written makes the resume gate skip the page
/// forever.
enum AiChunkOutcome {
    /// The resume gate decided otherwise: this chunk is not driven.
    NotDriven,
    /// Validated and queued for the batch export. Carries the content hash
    /// the commit point must record.
    Exported(String),
    /// A within-run duplicate of an earlier chunk: the FIRST occurrence is
    /// the one exported (#1215) — never a failure, never a second append.
    Duplicate,
    /// Validation rejected the chunk: an honest, recorded failure.
    Invalid(String),
}

/// Plan every chunk of the AI export path before anything is written.
///
/// Returns the per-chunk outcome and the chunks that will be exported.
fn plan_ai_chunks(
    chunks: &[crate::domain::DocumentChunk],
    decisions: &[ItemDecision],
    content_hashes: &[String],
) -> (
    Vec<AiChunkOutcome>,
    Vec<crate::domain::DocumentChunkValidated>,
) {
    let mut driven_keys: HashSet<(String, String)> = HashSet::new();
    let mut outcomes = Vec::with_capacity(chunks.len());
    let mut validated_chunks = Vec::with_capacity(chunks.len());
    for ((chunk, decision), content_hash) in chunks.iter().zip(decisions).zip(content_hashes) {
        if !matches!(decision, ItemDecision::DriveAndCommit) {
            outcomes.push(AiChunkOutcome::NotDriven);
            continue;
        }
        // #1812: a chunk that did NOT validate never reaches the file, so it
        // must not claim the dedup key either — a claimed key would mask a
        // LATER valid chunk with the same identity, and the export would
        // write nothing at all ("duplicate ... already exported this run"
        // although nothing was exported). Validation therefore comes FIRST.
        let validated = match chunk.clone().validate() {
            Ok(validated) => validated,
            Err(e) => {
                outcomes.push(AiChunkOutcome::Invalid(e.to_string()));
                continue;
            },
        };
        // #1215: the exporter is append-only, so a second identical chunk in
        // the same run must not write a duplicate record. The first
        // occurrence wins; the loser is a skip, not a failure.
        if !driven_keys.insert((chunk.url.clone(), content_hash.clone())) {
            outcomes.push(AiChunkOutcome::Duplicate);
            continue;
        }
        outcomes.push(AiChunkOutcome::Exported(content_hash.clone()));
        validated_chunks.push(validated);
    }
    (outcomes, validated_chunks)
}

/// Commit the AI export path's per-chunk outcomes.
///
/// Mirrors [`process_single_item`]: only what was actually written is
/// committed, and a chunk that could not be exported is a recorded failure
/// at a non-advanced status (SC6) — never a silent commit (#1812).
fn commit_ai_outcomes(
    session: &mut CommitSession<'_>,
    processed_urls: &[String],
    content_hashes: &[String],
    decisions: &[ItemDecision],
    outcomes: &[AiChunkOutcome],
    output_path: &std::path::Path,
    run_id: &RunId,
) {
    for (((url_str, content_hash), decision), outcome) in processed_urls
        .iter()
        .zip(content_hashes)
        .zip(decisions)
        .zip(outcomes)
    {
        match decision {
            ItemDecision::AlreadyCommitted => continue,
            ItemDecision::PromoteFromFlushProof => {
                session.promote_from_flush_proof(
                    url_str,
                    output_path.display().to_string(),
                    content_hash,
                );
            },
            ItemDecision::DriveAndCommit => match outcome {
                AiChunkOutcome::Exported(hash) => {
                    session.commit_item(url_str, hash.clone(), output_path.to_path_buf(), run_id);
                },
                AiChunkOutcome::Invalid(error) => {
                    // The non-AI path's shape: an item that cannot be
                    // exported is a recorded failure, NOT a commit. A commit
                    // here would make `--resume` skip the page forever even
                    // though nothing was ever written to the file.
                    let err = ExporterError::InvalidConfig(error.clone());
                    let class = classify_export_failure(&err);
                    session.fail_item(url_str, class, &err.to_string());
                },
                AiChunkOutcome::Duplicate => {
                    info!(url = %url_str, "duplicate (url, content_hash) already exported this run; skipping commit");
                },
                AiChunkOutcome::NotDriven => {},
            },
        }
        if session.cancelled() {
            break;
        }
    }
    // Same drain-ordering kill window as the standard path (#2244): after the
    // chunks that completed their full sequence and before `final_persist()`.
    // Armed
    // HERE too so the point keeps its meaning under `--clean-ai`. Checked
    // once, after the loop, so a cancelled run with NOTHING left to drive
    // still observes the drain: every chunk AlreadyCommitted, or a cleaning
    // phase that was itself cancelled and produced no chunks at all (#2270).
    if session.cancelled() {
        tracing::info!("cancellation observed; draining before final persist");
        crate::cli::crash_points::hit(crate::cli::crash_points::DURING_CANCEL_DRAIN);
    }
}

/// Process pre-cleaned document chunks and export them.
///
/// This function is used when `--clean-ai` is enabled. It accepts
/// `DocumentChunk` instances that already have embeddings populated
/// by the `SemanticCleaner`, bypassing the simple field-mapping conversion.
pub fn process_results_with_chunks(
    chunks: &[crate::domain::DocumentChunk],
    output_dir: PathBuf,
    format: ExportFormat,
    filename: &str,
    ctx: Option<&ResumeContext<'_>>,
) -> Result<ExportOutcome, ExporterError> {
    use sha2::{Digest, Sha256};

    info!("Processing {} cleaned chunks for export", chunks.len());

    std::fs::create_dir_all(&output_dir).map_err(ExporterError::DirectoryCreation)?;

    // Same join-time confinement as `process_results` (#1125).
    let filename = confine_filename_component(filename, "export");

    let output_path = output_dir.join(format!("{filename}.jsonl"));
    let mut session = CommitSession::open(ctx, &output_path);
    let fallback_run_id = RunId::new();
    let run_id = ctx.map_or(&fallback_run_id, |c| &c.run_id);

    let exporter = create_exporter(output_dir, &filename, format)?;

    let processed_urls: Vec<String> = chunks.iter().map(|c| c.url.clone()).collect();

    // Resume gate BEFORE any export (mirrors `process_single_item`): decide()
    // is read-only over the loaded records + flushed-bytes hash index, so the
    // decisions can be precomputed per chunk without side effects.
    let content_hashes: Vec<String> = chunks
        .iter()
        // Same digest the exporter stamps into the line; computed up front so
        // decide() can prove flush membership even with no record.
        .map(|c| format!("{:x}", Sha256::digest(c.content.as_bytes())))
        .collect();
    let decisions: Vec<ItemDecision> = processed_urls
        .iter()
        .zip(&content_hashes)
        .map(|(url_str, hash)| session.decide(url_str, Some(hash.as_str())))
        .collect();
    let skipped = decisions
        .iter()
        .filter(|d| matches!(d, ItemDecision::AlreadyCommitted))
        .count();
    if skipped > 0 {
        info!(
            skipped,
            total = processed_urls.len(),
            "resume gate: skipping already-flushed AI chunks before batch export"
        );
    }

    // Plan BEFORE the export (#1812): the plan says what will actually be
    // written, so the commit point below can only mark what WAS written.
    // COMMITTED-proven and PromoteFromFlushProof chunks must not re-append
    // (BUG F3-A: exporting before deciding duplicated them on resume).
    let (outcomes, validated_chunks) = plan_ai_chunks(chunks, &decisions, &content_hashes);

    // Use export_batch to avoid per-chunk file open/close (which overwrites in VectorExporter)
    if !validated_chunks.is_empty() {
        exporter.export_batch(&validated_chunks)?;
    }

    commit_ai_outcomes(
        &mut session,
        &processed_urls,
        &content_hashes,
        &decisions,
        &outcomes,
        &output_path,
        run_id,
    );
    session.final_persist();

    let total = processed_urls.len();
    Ok(session.finish(
        processed_urls,
        format!("✅ AI-cleaned export completed: {total} chunks processed"),
    ))
}

#[cfg(all(test, not(miri)))] // tokio_util CancellationToken + time-sensitive drains
mod tests_ai_chunks;
#[cfg(all(test, not(miri)))] // tokio_util CancellationToken + time-sensitive drains
mod tests_ai_drain;
#[cfg(test)]
mod tests_common;
#[cfg(test)]
mod tests_factory;
#[cfg(test)]
mod tests_index;
#[cfg(test)]
mod tests_process_results;
#[cfg(test)]
mod tests_resume;
