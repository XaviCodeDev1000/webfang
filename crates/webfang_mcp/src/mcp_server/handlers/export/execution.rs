use super::sidecar::{export_success_result, write_provenance_sidecar};
use crate::mcp_server::provenance;
use crate::mcp_server::validation::SanitizedFilename;
use rmcp::{model::CallToolResult, ErrorData as McpError};
use std::path::{Path, PathBuf};
use webfang_core::application::export_factory::process_results;
use webfang_core::domain::entities::ExportFormat;
use webfang_core::domain::ScrapedContent;

/// Run a real export of persisted results to the given format.
///
/// Maps operational [`ExporterError`](webfang_core::domain::exporter::ExporterError)s
/// to an honest `CallToolResult::error` (isError:true, Spanish) and reports the
/// real written path on success.
///
/// On success a provenance sidecar is written next to the export (PI-9) and its
/// path is named in the response, so the boundary travels with the bytes rather
/// than only with this tool result.
///
/// Issue #1814 (slice B, AC4): `process_results` is sync filesystem I/O plus a
/// SHA-256 per item plus resume-gate state writes, so it runs on the blocking
/// pool. The handler already owns `results` (a snapshot copy of the session
/// buffer), so the owned `Vec` moves into the blocking task — no content
/// clone, and the provenance/error mapping is unchanged.
pub(super) async fn export_results(
    results: Vec<ScrapedContent>,
    output_dir: PathBuf,
    format: ExportFormat,
    filename: SanitizedFilename,
) -> Result<CallToolResult, McpError> {
    let count = results.len();
    let name = filename.as_str().to_string();
    let work_dir = output_dir.clone();
    // Re-enter the current span inside the blocking task: `spawn_blocking`
    // does not propagate the tracing context, and spanless export events
    // would fall back to the run seed and split the run's trace_id. The
    // guard lives only inside the synchronous task, never across an
    // `.await` (async-rules compliant).
    let span = tracing::Span::current();
    let outcome = tokio::task::spawn_blocking(move || {
        let _span_guard = span.enter();
        // The skipped_non_utf8 count is the one the export itself computed
        // while indexing the existing JSONL file (#1812) — the core transports
        // it in its outcome, so the MCP never recomputes it (#2272).
        process_results(&results, work_dir, format, &name, None)
    })
    .await;
    match outcome {
        Ok(Ok(exported)) => {
            let skipped_non_utf8 = exported.skipped_non_utf8;
            let path = resolve_export_path(&output_dir, &filename, format);
            write_provenance_sidecar(&path, format, count);
            tracing::info!(documents = count, skipped_non_utf8, path = %path.display(), "export completed");
            Ok(export_success_result(count, &path, skipped_non_utf8))
        },
        Ok(Err(e)) => Ok(provenance::neutralized_error(&format!(
            "error al exportar: {e}"
        ))),
        Err(join) => {
            tracing::warn!(error = %join, "export_spawn_blocking_join_failed");
            Ok(provenance::neutralized_error(&format!(
                "export error: the export task failed in the blocking pool: {join}"
            )))
        },
    }
}

/// Compute the real on-disk path an export wrote to.
///
/// For concrete formats this is `{output_dir}/{filename}.{ext}`. For `Auto`
/// the concrete extension is resolved from what actually exists (jsonl
/// preferred, then json), so the reported path is always truthful.
///
/// `filename` is a [`SanitizedFilename`], so the join can never escape
/// `output_dir` (issue #601).
pub(super) fn resolve_export_path(
    output_dir: &Path,
    filename: &SanitizedFilename,
    format: ExportFormat,
) -> PathBuf {
    let name = filename.as_str();
    match format {
        ExportFormat::Auto => {
            let jsonl = output_dir.join(format!("{name}.jsonl"));
            if jsonl.exists() {
                jsonl
            } else {
                output_dir.join(format!("{name}.json"))
            }
        },
        concrete => output_dir.join(format!("{name}.{}", concrete.extension())),
    }
}
