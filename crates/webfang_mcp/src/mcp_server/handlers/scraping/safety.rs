use super::*;

/// robots.txt gate for single-URL direct-fetch tools (#749).
///
/// Delegates to the single policy source
/// (`McpState::robots_denied_for` → `scraper_service::enforce_robots_policy`,
/// fail-open when no fetcher is wired). On denial, records an
/// `Outcome::Error` scrape identity for `tool` — the same shape as each
/// handler's HTTP-error branch — and returns the tool error envelope carrying
/// the denial text (`WafBlocked(url, "robots.txt")` for genuine rules
/// denials; `Network` with the guard's cause for policy refusals, #1301).
pub(super) async fn robots_denied_response(
    handler: &McpHandler,
    tool: &'static str,
    url: &url::Url,
    raw_url: &str,
    start: Instant,
    correlation: &webfang_core::domain::CorrelationId,
) -> Option<CallToolResult> {
    let err = handler.state.robots_denied_for(url).await?;
    handler.state.record_scrape_identity(
        tool,
        domain_of(raw_url),
        Outcome::Error,
        0,
        start,
        correlation,
    );
    Some(provenance::neutralized_error(&err.to_string()))
}

/// Keep only seed-host-internal URLs in the discovery output (REQ-01, SSRF
/// defense-in-depth).
///
/// The engine's internal-link gating already applies on the BFS crawl path,
/// but sitemap discovery returns every `<loc>` host-agnostic. This post-hoc
/// filter is the authoritative gate that ensures no external-domain or
/// forbidden-literal-IP URL ever reaches an MCP response, complementing the
/// entry-level SSRF validator. Each exclusion emits a structured `warn` with
/// the `url` field (REQ-01/REQ-07).
///
/// Returns the number of excluded URLs. `total_pages` / crawl counts are NOT
/// recomputed — they keep engine semantics (pages crawled, not links emitted).
pub(super) fn filter_ssrf_safe(
    urls: &[webfang_core::domain::DiscoveredUrl],
    seed: &url::Url,
    out: &mut Vec<String>,
) -> usize {
    let seed_host = seed.host_str().unwrap_or(""); // mirrors crawl_task.rs internal-link anchor
    let mut excluded = 0usize;
    for d in urls {
        let internal =
            webfang_core::domain::url_validation::is_internal_link(d.url.as_str(), seed_host);
        if internal {
            // Hostnames compared here are non-literal hosts; literal IPs on a
            // non-seed host fall through to the exclusion branch below.
            out.push(d.url.to_string());
            continue;
        }
        excluded += 1;
        let reason = if webfang_core::domain::ssrf_guard::is_forbidden_literal_host(
            d.url.host_str().unwrap_or(""),
        ) {
            "forbidden_literal_ip"
        } else {
            "external_domain"
        };
        tracing::warn!(
            url = %d.url,
            reason = %reason,
            "discovered URL excluded from MCP response (SSRF defense-in-depth)"
        );
    }
    excluded
}
