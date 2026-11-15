//! Per-run resume/commit session: the loaded record map, the content-hash
//! index of the target output file, and the D3 commit-point protocol
//! (flush barrier → EXPORTED checkpoint → commit point). Split out of
//! `export_factory.rs` under the ~500-line rule (#2584, Ola 1); mechanical
//! movement only.

use std::collections::HashSet;
use std::path::PathBuf;

use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::application::resume::{canonical_key, load_preserving, RunId};
use crate::domain::page_state::{PageStatus, Stateful};
use crate::domain::persistence::{
    DomainRecords, LastError, RawRecord, RecordStoreError, RecordStorePort,
};
use crate::domain::record_transition;

use super::{build_content_hash_index, ExportOutcome};

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
pub(crate) enum ItemDecision {
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
    pub(crate) hash_index: HashSet<String>,
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

    pub(crate) fn cancelled(&self) -> bool {
        self.ctx
            .and_then(|c| c.cancel)
            .is_some_and(|t| t.is_cancelled())
    }

    /// Consume the session into its export outcome (#2272): the URLs that
    /// were driven plus the count of lines the index skipped. The count
    /// travels BOTH in the structured summary log (the surface the operator
    /// reads) and in the outcome (the surface callers transport).
    pub(crate) fn finish(self, processed_urls: Vec<String>, summary: String) -> ExportOutcome {
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
    pub(crate) fn claim_append(&mut self, url: &str, content_hash: &str) -> bool {
        self.driven
            .insert((url.to_string(), content_hash.to_string()))
    }

    /// The resume-gate decision for one item. With `resume = false` every
    /// item re-drives (fresh-run semantics); with `resume = true` only
    /// `COMMITTED`-proven records skip.
    pub(crate) fn decide(&self, url: &str, fresh_hash: Option<&str>) -> ItemDecision {
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

    pub(crate) fn promote_from_flush_proof(
        &mut self,
        url: &str,
        output_location: String,
        content_hash: &str,
    ) {
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
    pub(crate) fn resolve_flush_proof_record(
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
    pub(crate) fn drive_to_export_flushed(
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
    pub(crate) fn fail_item(&mut self, url: &str, class: crate::error::ErrorClass, message: &str) {
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
    pub(crate) fn merge_persist(&self, ctx: &ResumeContext<'_>) -> Result<(), RecordStoreError> {
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

    pub(crate) fn save_and_notify(
        &self,
        observer: Option<&dyn Fn(PageStatus)>,
        status: PageStatus,
    ) {
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
    pub(crate) fn save_notifying(&self, status: PageStatus) {
        self.save_and_notify(None, status);
    }

    /// Save failure state WITHOUT firing the transition observer (a failed
    /// item did not advance; the observer tracks lifecycle transitions only).
    pub(crate) fn save_quiet(&self) {
        if let Some(ctx) = self.ctx {
            if let Err(e) = self.merge_persist(ctx) {
                tracing::error!(error = %e, "record-store save failed");
            }
        }
    }
}

pub(crate) fn now_millis() -> i64 {
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
pub(crate) fn drive_to_processed(
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
pub(crate) fn fresh_discovered(
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
