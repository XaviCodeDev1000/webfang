//! Sub-path sitemap crawl fallback — sectioned sites.
//!
//! Extracted from `sitemap_discovery.rs` (Ola 3 B5): when the discovered
//! sitemap has no URLs relevant to the target path, the crawl falls back to
//! probing `/<section>/sitemap.xml` candidates and returning their parsed
//! URLs at depth 1. Pure move — no behavior change.

use crate::domain::crawler_port::sitemap::{SitemapParserPort, SitemapUrl};
use crate::domain::{CorrelationId, CrawlError, DiscoveredUrl};
use url::Url;

/// Try sub-path sitemaps when the discovered sitemap has no relevant URLs
///
/// For nested sites like `https://example.com/docs/en/`, this tries
/// `/docs/sitemap.xml`, `/docs/en/sitemap.xml`, etc.
/// Follows nested sitemaps recursively up to `sitemap_max_depth` levels.
///
/// `crawl_max_depth` is the crawl's configured max depth (CLI `--max-depth`),
/// distinct from the sitemap-index recursion depth above. Sub-path sitemap URLs
/// are one hop from the seed (depth 1), so when `crawl_max_depth` is 0 ("only
/// the seed URL") none of them qualify and an empty list is returned — mirroring
/// the `depth <= max_depth` gate in `crawl_with_sitemap_internal`.
///
/// Following **own-borrow-over-clone**: Accepts `&Url` not `&String`.
/// Following **err-no-unwrap-prod**: Proper error handling throughout.
// Depth bounds + client + run-root correlation (#1386) exceed the arity
// lint; bundling would only move the same wiring one level up.
#[allow(clippy::too_many_arguments)]
pub(super) async fn crawl_with_subpath_sitemaps(
    base_url: &str,
    base: &Url,
    parser: &dyn SitemapParserPort,
    sitemap_max_depth: usize,
    sitemap_current_depth: usize,
    crawl_max_depth: u8,
    client: &wreq::Client,
    correlation: &CorrelationId,
) -> Result<Vec<DiscoveredUrl>, CrawlError> {
    if sitemap_current_depth >= sitemap_max_depth {
        tracing::warn!(
            depth = sitemap_current_depth,
            max_depth = sitemap_max_depth,
            "sitemap recursion depth reached max; stopping"
        );
        return Ok(Vec::new());
    }

    // Sub-path sitemap URLs are depth 1; with a crawl max_depth of 0 none pass
    // the gate, so short-circuit before probing the network.
    if crawl_max_depth == 0 {
        tracing::info!(
            "crawl max_depth is 0, skipping sub-path sitemap URLs for {}",
            base_url
        );
        return Ok(Vec::new());
    }

    let all_urls = probe_subpath_sitemaps_for_crawl(base, client, parser, correlation).await;

    if all_urls.is_empty() {
        tracing::warn!(base_url = %base_url, "no sub-path sitemaps found");
        Ok(Vec::new())
    } else {
        // Sub-path sitemap URLs are at depth 1 (one hop from seed)
        Ok(all_urls
            .into_iter()
            .map(|sitemap_url| DiscoveredUrl::html(sitemap_url.url, 1, base.clone()))
            .collect())
    }
}

/// Probe up to 3 sub-path sitemap levels (`/docs`, `/docs/en`, ...) for both
/// `sitemap.xml` and `sitemap_index.xml`, returning every parsed URL.
async fn probe_subpath_sitemaps_for_crawl(
    base: &Url,
    client: &wreq::Client,
    parser: &dyn SitemapParserPort,
    correlation: &CorrelationId,
) -> Vec<SitemapUrl> {
    let segments: Vec<_> = base.path().split('/').filter(|s| !s.is_empty()).collect();
    let mut all_urls = Vec::new();

    // Try up to 3 path levels: /docs, /docs/en, /docs/en/quickstart
    for i in 1..=segments.len().min(3) {
        let sub_path = segments[..i].join("/");
        for sitemap_name in &["sitemap.xml", "sitemap_index.xml"] {
            let candidate = format!("/{sub_path}/{sitemap_name}");
            if let Some(urls) =
                try_subpath_sitemap(base, client, parser, &candidate, correlation).await
            {
                all_urls.extend(urls);
            }
        }
    }
    all_urls
}

/// Probe a single sub-path sitemap candidate; returns its parsed URLs when the
/// server answers 2xx, otherwise `None`.
async fn try_subpath_sitemap(
    base: &Url,
    client: &wreq::Client,
    parser: &dyn SitemapParserPort,
    candidate: &str,
    correlation: &CorrelationId,
) -> Option<Vec<SitemapUrl>> {
    let sitemap_url = base.join(candidate).ok()?;
    let sitemap_str = sitemap_url.as_str();
    tracing::debug!("Trying sub-path sitemap: {}", sitemap_str);
    let response = client.head(sitemap_str).send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    tracing::info!("Found sub-path sitemap: {}", sitemap_str);
    parse_subpath_sitemap(parser, sitemap_str, correlation).await
}

/// Parse a discovered sub-path sitemap, logging the URL count on success.
async fn parse_subpath_sitemap(
    parser: &dyn SitemapParserPort,
    sitemap_str: &str,
    correlation: &CorrelationId,
) -> Option<Vec<SitemapUrl>> {
    match parser.parse_from_url(sitemap_str, correlation).await {
        Ok(urls) => {
            tracing::info!(
                "Parsed {} URLs from sub-path sitemap {}",
                urls.len(),
                sitemap_str
            );
            Some(urls)
        },
        Err(_) => None,
    }
}
