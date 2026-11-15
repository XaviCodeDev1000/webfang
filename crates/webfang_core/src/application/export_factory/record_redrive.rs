//! Record-lifecycle re-drive helpers for the commit session: walking a raw
//! record forward to `PROCESSED` along its legal typestate chain, and
//! starting a fresh `DISCOVERED` lifecycle when the persisted state is
//! impossible (#876). Split out of `commit_session.rs` under the ~500-line
//! rule (#2584, Ola 1); mechanical movement only.

use crate::application::resume::RunId;
use crate::domain::page_state::{PageStatus, Stateful};
use crate::domain::persistence::RawRecord;

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
