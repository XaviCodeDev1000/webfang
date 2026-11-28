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

use std::path::PathBuf;

use tracing::{info, warn};

use crate::application::resume::RunId;
use crate::domain::crawler_port::filename::confine_filename_component;
use crate::domain::{entities::ExportFormat, exporter::ExporterError};
/// Per-run commit session and record-lifecycle re-drive helpers (D3 protocol).
mod ai_plan;
mod commit_session;
mod factory;
mod jsonl_index;
mod record_redrive;
pub(crate) use ai_plan::{commit_ai_outcomes, plan_ai_chunks};
pub use commit_session::ResumeContext;
pub(crate) use commit_session::{CommitSession, ItemDecision};
pub(crate) use factory::classify_export_failure;
pub use factory::create_exporter;
pub use jsonl_index::{
    jsonl_index_report, read_jsonl_checksum_index, JsonlChecksumIndex, JsonlIndexReport,
};

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
                // The standard path never cleans: it exports the extracted text
                // as-is, so no page can be "degraded" here.
                None,
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

#[cfg(test)]
mod tests_ai_chunks;
#[cfg(test)]
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
