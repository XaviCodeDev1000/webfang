//! AI export planning: what the `--clean-ai` path will do with each chunk,
//! and the commit that marks only what was actually written.
//!
//! Split out of `export_factory.rs` when the T6 degradation marker (issue
//! #3117) gave the per-chunk outcome a second payload: the file was already at
//! the 500-line ratchet's edge, and the plan + commit pair is the one cohesive
//! unit that can move without touching the D3 protocol around it.

use std::collections::HashSet;

use tracing::info;

use super::commit_session::{CommitSession, ItemDecision};
use super::factory::classify_export_failure;
use crate::application::resume::RunId;
use crate::domain::cleaning_status::DEGRADED_REASON_KEY;
use crate::domain::exporter::ExporterError;

/// What the AI export path will do with one chunk.
///
/// The plan is computed BEFORE the batch export, so the commit point can
/// only mark what was actually written (#1812). The non-AI path
/// ([`process_single_item`]) is the reference: a chunk that cannot be
/// exported is a RECORDED FAILURE, never a silent commit — a committed
/// record that was never written makes the resume gate skip the page
/// forever.
pub(crate) enum AiChunkOutcome {
    /// The resume gate decided otherwise: this chunk is not driven.
    NotDriven,
    /// Validated and queued for the batch export. Carries the content hash
    /// the commit point must record, plus the degradation marker the cleaning
    /// phase stamped on THIS chunk (T6, issue #3117) so the durable record can
    /// state why the page's text is the raw fallback.
    Exported {
        /// Hash of the exported content.
        content_hash: String,
        /// `Some(<variant>)` when the page fell back to raw text.
        degraded_reason: Option<String>,
    },
    /// A within-run duplicate of an earlier chunk: the FIRST occurrence is
    /// the one exported (#1215) — never a failure, never a second append.
    Duplicate,
    /// Validation rejected the chunk: an honest, recorded failure.
    Invalid(String),
}

/// Plan every chunk of the AI export path before anything is written.
///
/// Returns the per-chunk outcome and the chunks that will be exported.
pub(crate) fn plan_ai_chunks(
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
        // The marker the cleaning phase stamped onto this chunk, read once,
        // here, from the SAME chunk that is about to be exported.
        let degraded_reason = chunk.metadata.get(DEGRADED_REASON_KEY).cloned();
        outcomes.push(AiChunkOutcome::Exported {
            content_hash: content_hash.clone(),
            degraded_reason,
        });
        validated_chunks.push(validated);
    }
    (outcomes, validated_chunks)
}

/// Commit the AI export path's per-chunk outcomes.
///
/// Mirrors [`process_single_item`]: only what was actually written is
/// committed, and a chunk that could not be exported is a recorded failure
/// at a non-advanced status (SC6) — never a silent commit (#1812).
pub(crate) fn commit_ai_outcomes(
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
                AiChunkOutcome::Exported {
                    content_hash: hash,
                    degraded_reason,
                } => {
                    session.commit_item(
                        url_str,
                        hash.clone(),
                        output_path.to_path_buf(),
                        run_id,
                        degraded_reason.as_deref(),
                    );
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
