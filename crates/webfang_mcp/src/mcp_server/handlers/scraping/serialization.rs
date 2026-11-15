/// Render scrape results in the shared CLI/MCP record shape (P6-1): one
/// `WebfangMetadata` JSON object per line (checksum + timestamp + word_count
/// + metadata_version), wrapped in the `RemoteFetch` provenance envelope.
///
/// `scrape_url` and `scrape_with_options` serve this exact contract; one
/// function keeps the two copies from drifting apart.
use super::*;

pub(super) fn scraped_results_to_provenance_jsonl(
    results: &[webfang_core::domain::ScrapedContent],
    url: &url::Url,
) -> Result<CallToolResult, McpError> {
    let jsonl_lines: Result<Vec<String>, McpError> = results
        .iter()
        .map(|scraped| {
            let chunk = webfang_core::domain::DocumentChunk::from_scraped_content(scraped)
                .validate()
                .map_err(|e| {
                    McpError::internal_error(format!("chunk validation failed: {e}"), None)
                })?;
            let metadata =
                webfang_core::infrastructure::export::jsonl_exporter::WebfangMetadata::from_chunk(
                    &chunk,
                );
            serde_json::to_string(&metadata).map_err(|e| {
                McpError::internal_error(format!("failed to serialize metadata: {e}"), None)
            })
        })
        .collect();
    let content = jsonl_lines?.join("\n");
    Ok(provenance::untrusted_text(
        &provenance::Origin::RemoteFetch {
            url: url.to_string(),
        },
        &content,
    ))
}

/// Serialize a batch outcome as the shared RC-1 record shape (slice 2).
///
/// One JSONL line per input URL, preserving the #591 per-URL failure
/// contract and the spec's record cardinality (records = input URLs):
///
/// - each successful scrape becomes a `WebfangMetadata` record through the
///   ONLY conversion path (`from_scraped_content` → `validate` →
///   `from_chunk`), identical to `scrape_url` / `scrape_with_options` and to
///   the CLI JSONL export;
/// - each failed URL becomes its own record `{failed_url, error, category}`
///   straight from `ScrapeFailed` — deliberately NOT a degraded
///   `WebfangMetadata` (the CLI field-set binding covers success records
///   only; see the slice-2 amendment in
///   `openspec/changes/rc1-unification/spec.md`).
///
/// Failures come last so a client splitting on success-record fields never
/// sees a failure line in the success prefix. Conversion/validation or
/// serialization failures are `McpError::internal_error` — never a panic,
/// never a silent fallback text (slice-1 error discipline).
pub(super) fn batch_outcome_to_jsonl(
    outcome: &webfang_core::application::scraper_service::ScrapeBatchOutcome,
) -> Result<String, McpError> {
    let mut lines = Vec::with_capacity(outcome.results.len() + outcome.failed.len());
    for scraped in &outcome.results {
        let chunk = webfang_core::domain::DocumentChunk::from_scraped_content(scraped)
            .validate()
            .map_err(|e| McpError::internal_error(format!("chunk validation failed: {e}"), None))?;
        let metadata =
            webfang_core::infrastructure::export::jsonl_exporter::WebfangMetadata::from_chunk(
                &chunk,
            );
        lines.push(serde_json::to_string(&metadata).map_err(|e| {
            McpError::internal_error(format!("failed to serialize metadata: {e}"), None)
        })?);
    }
    for failed in &outcome.failed {
        // `serde_json::Value::to_string` is total (Display), so this path
        // cannot panic or fail.
        lines.push(
            serde_json::json!({
                "failed_url": failed.url,
                "error": failed.error,
                "category": failed.category,
            })
            .to_string(),
        );
    }
    Ok(lines.join("\n"))
}

/// Map a serialized discovery payload to its tool result (#1613, EC-06).
///
/// Shared by `discover_urls` and `discover_sitemap`, which reported a
/// serialization failure as a SUCCESS whose entire body was the literal
/// `"failed to serialize"` — a lost URL list indistinguishable from a real one.
/// The failure is routed to [`provenance::neutralized_error`]: the same channel
/// as the adjacent `Err` arm of both handlers, and the same mapping
/// `render_metrics` already uses in `security.rs` (REQ-10). Deliberately NOT
/// `McpError::internal_error` (`-32603`) — the surrounding `match` returns a
/// `CallToolResult` directly, and `?`-propagating would move these functions'
/// failures to a second channel.
pub(super) fn discovered_urls_tool_result(
    url: &str,
    json: Result<String, serde_json::Error>,
) -> CallToolResult {
    match json {
        Ok(content) => provenance::untrusted_text(
            &provenance::Origin::RemoteFetch {
                url: url.to_string(),
            },
            &content,
        ),
        Err(e) => {
            provenance::neutralized_error(&format!("no se pudo serializar la lista de URLs: {e}"))
        },
    }
}

/// Record per-domain scrape metrics for a completed batch (#696).
///
/// Groups successes and failures by their REAL host so multi-domain batches
/// attribute pages correctly in the metrics snapshot. Every event shares the
/// run-root `correlation` (#501/#698): one operation identity, one event per
/// domain group. `BTreeMap` keeps emission order deterministic (DD-6).
pub(super) fn record_batch_metrics_by_domain(
    state: &crate::mcp_server::McpState,
    outcome: &webfang_core::application::scraper_service::ScrapeBatchOutcome,
    start: Instant,
    correlation: &webfang_core::domain::CorrelationId,
) {
    // (successes, failures) per host.
    let mut groups: std::collections::BTreeMap<String, (usize, usize)> =
        std::collections::BTreeMap::new();
    for result in &outcome.results {
        let host = result
            .url
            .host_str()
            .map(str::to_string)
            .unwrap_or_else(|| "unknown".to_string());
        groups.entry(host).or_default().0 += 1;
    }
    for failed in &outcome.failed {
        let host = url::Url::parse(&failed.url)
            .ok()
            .and_then(|u| u.host_str().map(str::to_string))
            .unwrap_or_else(|| "unknown".to_string());
        groups.entry(host).or_default().1 += 1;
    }
    for (host, (ok, failed)) in groups {
        let outcome_type = if failed == 0 {
            Outcome::Success
        } else if ok == 0 {
            Outcome::Error
        } else {
            Outcome::Partial
        };
        state.record_scrape_identity(
            "scrape_batch",
            host,
            outcome_type,
            ok + failed,
            start,
            correlation,
        );
    }
}
