//! Batch capture spool acquisition — observe → decide → act on the same data.
//!
//! Extracted from `cli/batch_flow.rs` (Ola 3 B4, issue #2966): the acquisition
//! path the code itself names "Slice 2 (WU1)" — the spool path constant, the
//! sidecar/spool observation, the orphan quarantine, the disk-backed sink
//! construction and the best-effort discard. The flush policy itself stays in
//! [`crate::cli::batch_flush`]; acquisition only records what was decided so
//! the flush can act on it.

use tracing::error;

use crate::application::crawl_options::CrawlOptions;
use crate::application::crawler::BoundedFileSink;
use crate::cli::error::CliExit;
use crate::cli::orchestrator::resolve_persistence_root;

/// File name of the batch capture spool, under the run's persistence root.
pub(crate) const BATCH_SPOOL_FILE: &str = ".webfang-batch-capture.jsonl";

/// Spanish disclosure for a spool quarantined as an orphan (slice 2, #2611).
///
/// The data is NOT deleted — it is renamed aside and the operator is pointed
/// at `webfang recover`, which can turn it back into `.md` files.
const ORPHAN_SPOOL_MESSAGE: &str = "Se encontró un archivo temporal de captura de una ejecución anterior que no se puede atribuir a esta corrida (sin sidecar utilizable o con identidad que no coincide). NO se eliminó: se movió a la ruta indicada. Recuperá las páginas con: webfang recover --spool <ruta> -o <dir>";

/// Observe the sidecar + spool pair at the given root (the pure decision's
/// inputs) without mutating anything.
async fn observe_spool_pair(
    root: &std::path::Path,
) -> (
    crate::application::batch::progress::SidecarState,
    crate::application::batch::progress::SpoolFileState,
) {
    use crate::application::batch::progress::{
        spool_first_line_sha256, BatchProgressV1, SidecarState, BATCH_PROGRESS_FILE,
    };

    let spool_path = root.join(BATCH_SPOOL_FILE);
    let sidecar_path = root.join(BATCH_PROGRESS_FILE);

    let sidecar = match tokio::fs::read(&sidecar_path).await {
        Err(_) => SidecarState::Absent,
        Ok(bytes) => match serde_json::from_slice::<BatchProgressV1>(&bytes) {
            Ok(parsed) => SidecarState::Parsed(Box::new(parsed)),
            Err(_) => SidecarState::Unreadable,
        },
    };

    let spool = match tokio::fs::read(&spool_path).await {
        Err(_) => crate::application::batch::progress::SpoolFileState {
            len_bytes: 0,
            first_line_sha256: None,
        },
        Ok(bytes) => crate::application::batch::progress::SpoolFileState {
            len_bytes: bytes.len() as u64,
            first_line_sha256: spool_first_line_sha256(&bytes),
        },
    };

    (sidecar, spool)
}

/// Quarantine a leftover spool whose data cannot be attributed (L5: a sidecar
/// that does not describe the spool must not choose its destination).
///
/// Renames the spool to a timestamped sibling — NEVER deletes — and discloses
/// the path, size and the recover remedy in Spanish. Returns where the orphan
/// went so the acquisition outcome can carry it.
async fn quarantine_orphan_spool(
    spool_path: &std::path::Path,
) -> Option<(std::path::PathBuf, u64)> {
    use crate::application::batch::progress::orphan_spool_path;

    let metadata = match tokio::fs::metadata(spool_path).await {
        Ok(m) => m,
        // Nothing on disk: no orphan to quarantine.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => {
            tracing::warn!(
                error = %e,
                spool = %spool_path.display(),
                "orphan spool could not be inspected before quarantine"
            );
            return None;
        },
    };
    let orphan_bytes = metadata.len();
    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let orphan_path = orphan_spool_path(spool_path, now_secs);

    match tokio::fs::rename(spool_path, &orphan_path).await {
        Ok(()) => {
            tracing::warn!(
                spool = %spool_path.display(),
                orphan = %orphan_path.display(),
                orphan_bytes,
                user_message = ORPHAN_SPOOL_MESSAGE,
                "quarantined an unattributable batch capture spool"
            );
            Some((orphan_path, orphan_bytes))
        },
        Err(e) => {
            tracing::warn!(
                error = %e,
                spool = %spool_path.display(),
                user_message = ORPHAN_SPOOL_MESSAGE,
                "an unattributable batch capture spool could not be moved aside; it stays in place"
            );
            None
        },
    }
}

/// Create the disk-backed capture sink for a batch run through the UNIFIED
/// acquisition path (slice 2, WU1): observe → decide → act on the SAME data
/// the pure table saw, THEN hand a fresh sink its spool path.
///
/// Order pinned by the design (L7): spool identity and the sidecar are read
/// BEFORE anything truncates; the decision is the pure [`decide_sweep`]; an
/// orphan is renamed aside (never deleted, never salvage-able by this run)
/// and the new run gets a fresh spool at the original path. Salvage itself
/// belongs to the flush seam (WU3) — acquisition only records what was
/// decided so the flush can act on it.
pub(super) async fn build_batch_sink(opts: &CrawlOptions) -> Result<BoundedFileSink, CliExit> {
    use crate::application::batch::progress::decide_sweep;

    let root = resolve_persistence_root(opts);
    let spool_path = root.join(BATCH_SPOOL_FILE);

    let (sidecar_state, spool_state) = observe_spool_pair(&root).await;
    match decide_sweep(&sidecar_state, &spool_state) {
        crate::application::batch::progress::SweepDecision::OrphanSpool => {
            // Data without a usable description: quarantine and start fresh.
            let quarantined = quarantine_orphan_spool(&spool_path).await;
            if quarantined.is_none() && spool_state.len_bytes > 0 {
                // The rename failed and the residue still sits in the spool
                // path. This run must NOT truncate it (that would destroy
                // unattributable data): fail the run loudly instead.
                return Err(CliExit::IoError(format!(
                    "No se pudo apartar el spool huérfano en {}. Bórralo o recupéralo con \
                     webfang recover antes de relanzar el batch.",
                    spool_path.display()
                )));
            }
        },
        crate::application::batch::progress::SweepDecision::SalvageFromOffset { .. } => {
            // WU3 wires the flush-side salvage; acquisition records the
            // decision in the structured log so it is queryable now.
            tracing::info!(
                spool = %spool_path.display(),
                "salvageable spool detected — flush-side salvage pending WU3"
            );
        },
        crate::application::batch::progress::SweepDecision::Nothing => {},
    }

    // One buffered page per concurrent crawl, plus headroom, keeps the writer
    // from becoming the bottleneck without unbounding memory. The bound derives
    // from the budget model's Operation.batch tier (task 2.5c).
    let budget = crate::domain::budget::BudgetModel::build(
        opts.budget_overrides,
        &crate::domain::budget::detector::SystemDetector,
    );
    let buffer = budget
        .batch()
        .get()
        .saturating_mul(2)
        .max(crate::application::crawler::bounded_sink::DEFAULT_SINK_BUFFER);
    BoundedFileSink::new(spool_path, buffer).await.map_err(|e| {
        error!(error = %e, "batch content spool could not be created");
        CliExit::IoError(format!(
            "No se pudo crear el archivo temporal de captura: {e}"
        ))
    })
}

/// Remove the batch capture spool once its pages have been extracted.
///
/// Best-effort: a leftover spool is noise, not a failure of the run. If THIS
/// removal fails, the residue is not invisible either — the next batch run's
/// acquisition path discloses or quarantines any survivor, which is the same
/// remedy the crash case gets.
pub(super) async fn discard_batch_spool(sink: &BoundedFileSink) {
    if let Err(e) = tokio::fs::remove_file(sink.spool_path()).await {
        tracing::debug!(
            error = %e,
            spool = %sink.spool_path().display(),
            "batch capture spool could not be removed"
        );
    }
}

// The tests of this module live in a child file (`#[path]`, the pattern
// batch_flush.rs uses), re-parented through scripts/split_tests.sh.
#[cfg(test)]
#[path = "batch_spool_tests.rs"]
mod spool_tests;
