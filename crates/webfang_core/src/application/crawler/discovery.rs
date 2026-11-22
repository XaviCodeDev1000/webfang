//! Discovery module — URL discovery and single-URL scraping
//!
//! Functions for discovering URLs from websites (DOM link extraction) and the
//! CLI single-URL scraping use case — the reusable entry point for one-off
//! scrapes: discover → scrape. Sitemap crawling lives in `sitemap_discovery.rs`
//! and sitemap XML parsing in the infrastructure layer (issue #442); both are
//! re-exported here so existing import paths keep resolving.

use tracing::{debug, info, instrument, warn};
use url::Url;

use crate::application::url_filter::is_allowed;
use crate::domain::config::ScraperConfig;
use crate::domain::crawler_port::derive_filename_from_content_disposition;
use crate::domain::downloader_port::{DownloadError, Downloader};
use crate::domain::http_config::HttpClientConfig;
use crate::domain::site::SitemapConfig;
use crate::domain::url_validation::{
    is_internal_link, normalize_url, NormalizeConfig, RemoveQueryParameters,
};
use crate::domain::waf::{waf_inspector, InspectionContext};
use crate::domain::{CorrelationId, CrawlerConfig, ScrapedContent, ValidUrl};
use crate::error::{Result as ScraperResult, ScraperError};
use crate::infrastructure::observability::log_scrape_error;

use crate::application::adaptive_engine::AdaptiveSelectorEngine;

// Sitemap discovery was extracted to `sitemap_discovery.rs` (and sitemap XML
// parsing stays in the infrastructure layer per #442 / ADR-0012-B). The
// `parse_sitemap` re-export was dismantled in favor of direct consumer
// repoints — no application symbol may reach `infrastructure::crawler` through
// a shim (ADR-0012-B).
pub use crate::application::crawler::sitemap_discovery::crawl_with_sitemap;
use crate::application::crawler::sitemap_discovery::crawl_with_sitemap_resolved;
pub use crate::application::extraction::extract_content;

// ============================================================================
// Discover/Scrape Use Cases — CLI one-off scraping entry points
// ============================================================================

/// Discover URLs from a website without downloading content
///
/// This is the first step of the single-URL discovery path:
/// 1. Discover all URLs from sitemap or DOM scraping
/// 2. Return `Vec<Url>` for the caller to filter or select
/// 3. Scrape only the URLs the caller chose
///
/// Following **own-borrow-over-clone**: Accepts `&str` not `&String`.
///
/// # Arguments
///
/// * `base_url` - Base URL to discover from
/// * `config` - Crawler configuration
/// * `correlation` - Caller run-root correlation (spans derive children from it)
///
/// # Returns
///
/// * `Ok(Vec<Url>)` - Discovered URLs (owned)
/// * `Err(ScraperError)` - Error during discovery
///
/// # Examples
///
/// ```no_run
/// use webfang_core::{application::discover_urls_single_fetch, domain::{CorrelationId, CrawlerConfig}};
/// use url::Url;
///
/// # #[tokio::main]
/// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let seed = Url::parse("https://example.com")?;
/// let config = CrawlerConfig::new(seed);
///
/// let urls = discover_urls_single_fetch("https://example.com", &config, &CorrelationId::new()).await?;
/// println!("Found {} URLs", urls.len());
/// # Ok(())
/// # }
/// ```
#[instrument(
    name = "discover_urls_single_fetch",
    skip(config),
    fields(
        base_url,
        use_sitemap = tracing::field::Empty
    )
)]
pub async fn discover_urls_single_fetch(
    base_url: &str,
    config: &CrawlerConfig,
    correlation: &CorrelationId,
) -> ScraperResult<Vec<Url>> {
    info!("Discovering URLs from {}", base_url);

    // Resolve the sitemap mode ONCE at the boundary (#1190): an explicit but
    // invalid URL fails here (Spanish, typed) instead of travelling unchecked
    // into sitemap fetch/parse. Past this point no productive reader touches
    // the raw `use_sitemap` + `sitemap_url` pair.
    let sitemap = config.sitemap_config();
    tracing::Span::current().record("use_sitemap", sitemap.is_enabled());

    // If sitemap enabled, use sitemap (preferred)
    if let SitemapConfig::Enabled { url } = sitemap {
        let discovered =
            crawl_with_sitemap_resolved(base_url, url.as_ref(), config, correlation).await?;
        let urls: Vec<Url> = discovered.into_iter().map(|d| d.url).collect();

        Ok(urls)
    } else if config.max_depth == 0 {
        // Depth 0 means "only seed URL" — skip link extraction entirely.
        // `plan_urls` injects the seed URL in the DOM branch, so returning empty
        // here yields exactly one page crawled (issue #583).
        info!("max_depth is 0 — skipping DOM link extraction, crawling seed only");
        Ok(Vec::new())
    } else {
        // DOM scraping - extract links from single page.
        // Honor the configured request timeout (#289). The connect timeout is
        // capped at 10s, replicating the #281 policy used by the sitemap branch.
        // user_agent stays None (Default) so this path keeps rotating random pool
        // agents instead of injecting config.user_agent.
        let http_config = HttpClientConfig {
            timeout_secs: config.timeout_secs,
            connect_timeout_secs: config.timeout_secs.min(10), // #281 policy, replicated at call site
            tls_emulation: config.tls_emulation,               // #312 honor configured profile
            ..Default::default()
        };
        let client = super::super::create_http_client_with_config(&http_config)?;

        info!("Fetching {} for link extraction", base_url);
        // SSRF layer 2 (#1615, G-3): this branch built its OWN client and went
        // straight to `send()`, so it applied timeout and TLS config but never
        // the literal-IP entry guard. The seed arrives from `--url` or from
        // sitemap discovery, and in the sitemap case a third party supplies the
        // host -- so an entry the operator never typed could be dialled here
        // with no pre-socket refusal. The JS and sitemap arms are guarded; this
        // one was the gap.
        //
        // It sits at the same position as everywhere else (see the guard chain
        // in `AGENTS.md`), not as a new stage: the check runs before `send()`,
        // and building a client opens no socket.
        let seed = Url::parse(base_url)?;
        if let Err(rejection) = crate::domain::ssrf_guard::reject_forbidden_literal_url(&seed) {
            // The typed error the caller already handles for a bad seed, so this
            // refusal rides the same reporting path as a parse failure instead
            // of inventing a new one. `InvalidUrl` is the variant the
            // static-fetch port uses too (F11), so the two entry points
            // classify one refusal identically.
            return Err(ScraperError::InvalidUrl(rejection.to_string()));
        }

        let response = client.get(base_url).send().await?;

        let status = response.status();
        let content_type = response
            .headers()
            .get("content-type")
            .map(|v| v.to_str().unwrap_or("unknown"))
            .unwrap_or("unknown");
        let content_length = response
            .headers()
            .get("content-length")
            .map(|v| v.to_str().unwrap_or("0"))
            .unwrap_or("0");

        debug!(
            "Response: status={}, content-type={}, content-length={}",
            status, content_type, content_length
        );

        let html = response.text().await?;

        debug!("Received HTML: {} bytes", html.len());

        let base = Url::parse(base_url)?;

        // Extract links — through the composition-root seam (ADR-0012-B
        // unit 6): the scraper-backed concrete stays in infrastructure.
        let links =
            crate::application::container::build_link_extractor().extract_links(&html, base_url)?;

        // Filter and normalize URLs
        let mut urls = Vec::new();
        for link in links {
            // Canonical (query-stripped) form for the dedup-style internal/allowed
            // checks, but parse and push the ORIGINAL link so the fetched URL
            // keeps its query string (#651).
            let canonical = normalize_url(
                &link,
                &NormalizeConfig {
                    strip_www: true,
                    query_policy: RemoveQueryParameters::All,
                },
            );
            if let Ok(parsed_url) = Url::parse(&link) {
                // Check if internal link
                if let Some(seed_domain) = base.host_str() {
                    if is_internal_link(&canonical, seed_domain) {
                        // Check if allowed by filters
                        if is_allowed(&canonical, config) {
                            urls.push(parsed_url);
                        }
                    }
                }
            }
        }

        info!("Discovered {} URLs from {}", urls.len(), base_url);

        Ok(urls)
    }
}

/// Scrape a single URL
///
/// Following **own-borrow-over-clone**: Accepts `&Url` not `&String`.
///
/// # Arguments
///
/// * `downloader` - Downloader (fetch router in production, mock in tests)
/// * `url` - URL to scrape
/// * `config` - Scraper configuration
/// * `correlation` - Per-page correlation identity, derived by the caller from
///   the run-root identity (`root.child()` — same `trace_id`, fresh
///   `span_id`), so every page of a run stays reconstructable under one trace
///   (#501)
///
/// # Returns
///
/// * `Ok(ScrapedContent)` - Scraped content from the URL
/// * `Err(ScraperError)` - Error during scraping
#[instrument(
    name = "scrape_single_url",
    skip(downloader, config, asset_downloader, engine, binary_writer, correlation),
    fields(url = %url)
)]
// Correlation contract (#501/#1386): the run-root travels beside the
// per-page correlation so the span declares the shared trace_id at
// creation. Bundling would only move the same wiring one level up.
#[allow(clippy::too_many_arguments)]
pub async fn scrape_single_url(
    downloader: &dyn Downloader,
    url: &Url,
    config: &ScraperConfig,
    asset_downloader: Option<&dyn crate::domain::ports::AssetDownloaderPort>,
    #[allow(unused_variables)] engine: Option<&AdaptiveSelectorEngine>,
    binary_writer: Option<&dyn crate::domain::ports::BinaryWriterPort>,
    correlation: &CorrelationId,
    root_correlation: &CorrelationId,
) -> ScraperResult<ScrapedContent> {
    scrape_single_url_inner(
        downloader,
        url,
        config,
        asset_downloader,
        engine,
        binary_writer,
        correlation.clone(),
        root_correlation,
    )
    .await
}

/// Inner implementation of [`scrape_single_url`].
///
/// The `#[instrument]` span declares the per-page identity (`correlation_id`,
/// `trace_id`) AT CREATION time (#501). Anything only known at the END of the
/// operation is declared `tracing::field::Empty` and recorded with
/// `tracing::Span::record`, which FileTraceLayer merges into the same snapshot
/// (`on_record`, #1610) — the earlier claim that late records never reach the
/// `--trace-file` JSONL was true until that hook landed, and is false now. The
/// instrumented span lifecycle is also async-safe — no `enter()` guard crosses
/// an `.await` (#501 follow-up).
#[instrument(
    level = "debug",
    name = "scrape_single",
    skip(downloader, config, asset_downloader, engine, binary_writer, correlation),
    fields(
        url = %url,
        correlation_id = %correlation,
        trace_id = %root_correlation.trace_id()
    )
)]
// The crash-injection pin for POST_EXTRACTION_PRE_PIPELINE pushes this
// function past clippy's 100-line budget; the span body is cohesive and
// splitting it would obscure the pipeline order the harness depends on.
#[allow(clippy::too_many_lines)]
// Same correlation contract as the outer fn: run-root beside per-page.
#[allow(clippy::too_many_arguments)]
async fn scrape_single_url_inner(
    downloader: &dyn Downloader,
    url: &Url,
    config: &ScraperConfig,
    asset_downloader: Option<&dyn crate::domain::ports::AssetDownloaderPort>,
    #[allow(unused_variables)] engine: Option<&AdaptiveSelectorEngine>,
    binary_writer: Option<&dyn crate::domain::ports::BinaryWriterPort>,
    correlation: CorrelationId,
    root_correlation: &CorrelationId,
) -> ScraperResult<ScrapedContent> {
    debug!("Scraping: {}", url);

    // Fetch the page through the injected downloader (a `FetchRouter` in
    // production, a mock in tests). Download-level failures are mapped to their
    // `ScraperError` equivalents so WAF/HTTP semantics survive the conversion.
    let page = downloader.fetch(url).await.map_err(|e| match e {
        DownloadError::WafChallenge(provider) => {
            // Observability parity (F-11): a challenge caught on the non-2xx path
            // surfaces here as a mapped error and never reaches the 2xx inspection
            // site below, so without this call a WAF block that the operator can see
            // in the CLI would be absent from the trace — the exact asymmetry the
            // audit measured as `stage: fetch` on 2xx and nothing on 403/503.
            log_scrape_error(
                &provider,
                url.as_str(),
                "fetch",
                Some(&correlation),
                "WAF challenge detected",
            );
            ScraperError::WafBlocked {
                url: url.to_string(),
                provider,
            }
        },
        DownloadError::Http { status, .. } => ScraperError::http(status, url.as_str()),
        other => ScraperError::Network(Box::new(other)),
    })?;

    if !(200..300).contains(&page.status) {
        return Err(ScraperError::http(page.status, url.as_str()));
    }

    // Crash-injection: response received, nothing extracted yet.
    crate::cli::crash_points::hit(crate::cli::crash_points::MID_FETCH);

    // Check content-type before reading body to handle binary content (PDFs, etc.)
    let content_type = page
        .headers
        .get("content-type")
        .cloned()
        .unwrap_or_default();

    let is_binary = content_type.contains("application/pdf")
        || content_type.contains("application/octet-stream")
        || content_type.contains("application/zip")
        || content_type.contains("application/x-")
        || content_type.contains("image/")
        || content_type.contains("audio/")
        || content_type.contains("video/");

    if is_binary {
        debug!("Binary content type detected: {} for {}", content_type, url);

        // Save binary file when download_documents is enabled. Filesystem I/O is
        // routed through the injected BinaryWriterPort (falling back to the
        // composition-root default writer) so the application layer never
        // touches std::fs directly (#442 layer-violation fix). Observable
        // behavior is unchanged: the same bytes land in the same file under
        // `config.output_dir`.
        let saved_path = if config.download_documents {
            let filename = derive_filename_from_content_disposition(
                page.headers.get("content-disposition").map(String::as_str),
                url,
                &content_type,
            );
            let output_path = config.output_dir.join(&filename);

            let bytes = page.html.as_bytes();
            let fallback_writer = crate::application::container::build_binary_writer();
            let writer: &dyn crate::domain::ports::BinaryWriterPort =
                binary_writer.unwrap_or(&fallback_writer);
            match writer.write_bytes(&output_path, bytes) {
                Ok(()) => info!(
                    "Saved binary file: {} ({} bytes)",
                    output_path.display(),
                    bytes.len()
                ),
                Err(e) => warn!(
                    path = %output_path.display(),
                    error = %e,
                    "failed to save binary file"
                ),
            }
            Some(output_path)
        } else {
            None
        };

        let assets = crate::application::scraper_service::download_assets_if_enabled(
            "",
            url,
            config,
            asset_downloader,
        )
        .await?;

        let content = if let Some(ref path) = saved_path {
            format!("[Binary file saved: {}] {}", path.display(), url.as_str())
        } else {
            format!("[Binary content: {content_type}] {}", url.as_str())
        };

        return Ok(ScrapedContent {
            title: url
                .host_str()
                .ok_or_else(|| ScraperError::invalid_url(format!("URL missing host: {url}")))?
                .to_string(),
            content,
            url: ValidUrl::try_from_url(url.clone())?,
            excerpt: None,
            author: None,
            date: None,
            html: None,
            assets,
            correlation_id: Some(correlation),
            quality_hint: None,
        });
    }

    let html = page.html;

    // Detect WAF/CAPTCHA challenges disguised as HTTP 200 (H3 fix, REQ-WAF-05).
    // Context-aware inspection keeps the silent-challenge intent: a 200+HTML
    // script-dense body is still caught via the entropy rule (REQ-WAF-06), while
    // bare vendor names at 200 no longer block. A block carries the full Spanish
    // evidence chain (REQ-WAF-08). `config.ignore_waf` short-circuits to a clean
    // verdict (REQ-WAF-07).
    let ctx =
        InspectionContext::from_lowercase_headers(page.status, &page.headers, config.ignore_waf);
    let verdict = waf_inspector().inspect(&html, &ctx);
    if verdict.is_blocked {
        let chain = verdict.evidence_chain();
        log_scrape_error(
            &chain,
            url.as_str(),
            "fetch",
            Some(&correlation),
            "WAF challenge detected",
        );
        return Err(ScraperError::waf_blocked(url.to_string(), chain));
    }

    // Crash-injection: fetched + WAF-checked; extraction not yet run.
    crate::cli::crash_points::hit(crate::cli::crash_points::POST_FETCH_PRE_EXTRACT);
    let content =
        extract_content(&html, url, config, asset_downloader, engine, &correlation).await?;
    // Crash-injection: extraction returned ScrapedContent; nothing persisted yet.
    crate::cli::crash_points::hit(crate::cli::crash_points::POST_EXTRACTION_PRE_PIPELINE);
    Ok(content)
}

#[cfg(test)]
#[path = "discovery_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "discovery_scrape_tests.rs"]
mod scrape_tests;
