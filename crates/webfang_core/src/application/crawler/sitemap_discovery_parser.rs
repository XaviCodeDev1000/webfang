//! Sitemap parser boundary — config shaping and fetch/parse error mapping.
//!
//! Extracted from `sitemap_discovery.rs` (Ola 3 B5): the composition-root
//! wrapper that turns a `CrawlerConfig` into an erased [`SitemapParserPort`]
//! plus the fetch-and-parse step that maps `SitemapError` onto `CrawlError`
//! with its observability contract (#1318). Pure move — no behavior change.

use crate::domain::crawler_port::sitemap::{SitemapError, SitemapParserPort, SitemapUrl};
use crate::domain::error::WafDetectionKind;
use crate::domain::{CorrelationId, CrawlError, CrawlerConfig};
use crate::infrastructure::observability::log_scrape_error;
use std::sync::Arc;

/// Build the sitemap parser with pagination settings and the configured TLS
/// profile.
///
/// Config-shaping wrapper over the composition-root seam
/// [`crate::application::container::build_sitemap_parser`] (ADR-0012-B): the
/// concrete `SitemapParser` is named only there; this layer receives it
/// erased to the domain [`SitemapParserPort`].
///
/// #2490: the parser's fetch client inherits the deadline only when the
/// OPERATOR configured one (`--timeout-secs` / `WEBFANG_TIMEOUT_SECS`). The
/// spec default (30) is not an operator decision — threading it unconditionally
/// would silently replace the parser's own 10 s default on the no-flag path,
/// and an explicit `--timeout-secs 30` must stay distinguishable from the
/// default (that distinction only exists at the argv boundary, hence the
/// `*_explicit` bit riding `CrawlerConfig`).
pub(super) fn build_sitemap_parser(
    config: &CrawlerConfig,
    batch_size: usize,
) -> Result<Arc<dyn SitemapParserPort>, CrawlError> {
    let mut sitemap = crate::domain::crawler_port::SitemapConfig::builder()
        .gzip_enabled(true)
        .max_depth(3)
        .concurrency(5)
        .batch_size(batch_size)
        .pagination_enabled(true);
    if config.timeout_secs_explicit {
        sitemap = sitemap.timeout(std::time::Duration::from_secs(config.timeout_secs));
    }
    crate::application::container::build_sitemap_parser(sitemap.build(), config.tls_emulation)
}

/// Parse a sitemap URL, mapping parse failures to a [`CrawlError`].
///
/// The terminal parse failure is reported through [`log_scrape_error`] with
/// `stage = "sitemap.discovery"` (#1318). The correlation lives inside the
/// parser (the port's boxed future owns the root it mints), so this
/// application-level summary logs with an optional correlation, exactly as
/// the observability contract permits.
pub(super) async fn parse_sitemap(
    parser: &dyn SitemapParserPort,
    sitemap_url: &str,
    correlation: &CorrelationId,
) -> Result<Vec<SitemapUrl>, CrawlError> {
    let urls = parser
        .parse_from_url(sitemap_url, correlation)
        .await
        .map_err(|e| {
            log_scrape_error(
                &e,
                sitemap_url,
                "sitemap.discovery",
                None,
                "Failed to parse sitemap",
            );
            // Preserve the specific error type for proper exit code mapping
            match e {
                SitemapError::HttpError { status, message } => CrawlError::Http {
                    status,
                    url: message,
                },
                // ADR-0012-B: the XmlError payload is now a String carrying the
                // bare `quick_xml::Error` Display text (the prefix lives on the
                // enum's own Display). Re-adding the prefix here keeps the
                // user-visible message byte-identical to the pre-port behavior
                // (pinned by behavioral__malformed_xml_stderr.snap) and the
                // exit-code mapping unchanged (Parse → exit 69).
                SitemapError::XmlError(e) => CrawlError::Parse(format!("XML parsing failed: {e}")),
                SitemapError::InvalidContentType(ct) => CrawlError::InvalidContentType(ct),
                SitemapError::SitemapNotFound(url) => CrawlError::SitemapNotFound(url),
                SitemapError::MaxDepthExceeded => CrawlError::SitemapDepthExceeded,
                SitemapError::NoUrlsFound => CrawlError::SitemapEmpty,
                SitemapError::DecompressionError(e) => {
                    CrawlError::Parse(format!("decompression failed: {e}"))
                },
                // #1822: the aggregate index budget is a typed resource stop.
                // Wired like its closest siblings ResponseTooLarge /
                // DecompressedTooLarge (parse-level limit → CrawlError::Parse →
                // exit 69 via Internal); the variant stays matchable here if
                // the exit-code contract ever needs its own row.
                SitemapError::MemoryLimitExceeded(mb) => {
                    CrawlError::Parse(format!("sitemap memory limit exceeded: {mb} MB"))
                },
                // All children FAILED to fetch/parse (HTTP errors) — this is an
                // infrastructure failure (exit 69 via Parse→Internal), NOT a
                // fully-empty sitemap (which is NoUrlsFound→SitemapEmpty→exit 2).
                // Keep this mapping.
                SitemapError::AllChildrenFailed(count, details) => {
                    CrawlError::Parse(format!("all {count} child sitemaps failed: {details}"))
                },
                // Issue #879: a challenge detected in the sitemap chain keeps its
                // typed identity through the CrawlError layer (PermanentFatal per
                // classify(), Spanish evidence chain via ScraperError::WafBlocked).
                SitemapError::WafChallenge { url, provider } => CrawlError::WafChallenge {
                    provider,
                    kind: WafDetectionKind::BodySignature,
                    url,
                },
                // #1382: a forbidden-literal sitemap target (initial or index
                // child) rejected pre-socket by the parser's entry guard maps to
                // the same typed InvalidUrl the CLI/MCP entry points produce, so
                // the exit-code mapping (69) and the Spanish SSRF copy are
                // byte-identical to every other fetch surface.
                SitemapError::SsrfLiteralRejected(msg) => CrawlError::InvalidUrl(msg),
                other => CrawlError::Parse(other.to_string()),
            }
        })?;
    tracing::info!("Parsed {} total URLs from sitemap", urls.len());
    Ok(urls)
}
