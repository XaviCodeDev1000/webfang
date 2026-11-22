//! Sitemap discovery — sitemap-based URL discovery use cases.
//!
//! Hosts the sitemap crawling orchestration (explicit/auto-discovered sitemap
//! fetching and the #1382 SSRF entry guards) plus the URL-building/filter
//! helpers. Extracted from `discovery.rs` (issue #442) to separate the sitemap
//! infrastructure concern from DOM-link discovery. Ola 3 B5 moved three seams
//! to sibling modules: the `SitemapParserPort` boundary
//! (`sitemap_discovery_parser.rs`), the robots.txt/fallback/sub-path URL
//! probing chain (`sitemap_discovery_probe.rs`) and the sub-path crawl
//! fallback for sectioned sites (`sitemap_discovery_subpath.rs`).

use crate::application::url_filter::is_allowed;
use crate::domain::crawler_port::sitemap::SitemapUrl;
use crate::domain::http_config::HttpClientConfig;
use crate::domain::site::SitemapConfig;
use crate::domain::ValidUrl;
use crate::domain::{CorrelationId, CrawlError, CrawlerConfig, DiscoveredUrl};
use crate::error::ScraperError;
use tracing::{info, instrument};
use url::Url;

#[path = "sitemap_discovery_parser.rs"]
mod sitemap_discovery_parser;
#[path = "sitemap_discovery_probe.rs"]
mod sitemap_discovery_probe;
#[path = "sitemap_discovery_subpath.rs"]
mod sitemap_discovery_subpath;

use self::sitemap_discovery_parser::{build_sitemap_parser, parse_sitemap};
use self::sitemap_discovery_probe::discover_sitemap_url;
use self::sitemap_discovery_subpath::crawl_with_subpath_sitemaps;

/// Crawl site using sitemap (legacy string entry).
///
/// Kept for the frozen `discover_sitemap_wiremock` contract (that suite is
/// outside this slice's surfaces, so its call sites cannot be repointed
/// here). It routes through the same `SitemapConfig` boundary as the live
/// path: an explicit URL is validated EARLY (Spanish, typed) instead of
/// failing late at fetch/parse time, and `None` keeps the historical
/// auto-discovery semantics.
///
/// New code resolves `SitemapConfig` from the config and calls
/// [`crawl_with_sitemap_resolved`] with the resolved value.
///
/// Following **own-borrow-over-clone**: Accepts `&str` not `&String`.
///
/// # Arguments
///
/// * `base_url` - Base URL of the website
/// * `sitemap_url` - Optional explicit sitemap URL (auto-discovers if None)
/// * `config` - Crawler configuration
/// * `correlation` - Caller run-root correlation (spans derive children from it)
///
/// # Returns
///
/// * `Ok(Vec<DiscoveredUrl>)` - URLs discovered from sitemap
/// * `Err(CrawlError)` - Error during sitemap fetch or parse
///
/// # Examples
///
/// ```no_run
/// use webfang_core::application::crawl_with_sitemap;
/// use webfang_core::domain::{CorrelationId, CrawlerConfig};
/// use url::Url;
///
/// # #[tokio::main]
/// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let seed = Url::parse("https://example.com")?;
/// let config = CrawlerConfig::new(seed);
///
/// let urls = crawl_with_sitemap("https://example.com", None, &config, &CorrelationId::new()).await?;
/// println!("Found {} URLs from sitemap", urls.len());
/// # Ok(())
/// # }
/// ```
#[instrument(
    name = "crawl_with_sitemap",
    skip(config),
    fields(
        base_url,
        sitemap_url = ?sitemap_url
    )
)]
pub async fn crawl_with_sitemap(
    base_url: &str,
    sitemap_url: Option<&str>,
    config: &CrawlerConfig,
    correlation: &CorrelationId,
) -> Result<Vec<DiscoveredUrl>, CrawlError> {
    // Through the boundary even on the legacy path: `resolve` is the single
    // home of the `Some(url) implies intent` coercion, and it parses NOW, so
    // `true + Some("not-a-url")` fails here instead of travelling unchecked
    // into `resolve_sitemap_url` (#1190). `resolve` only fails with
    // `InvalidUrl`; the second arm is defensive.
    let explicit = match sitemap_url {
        Some(raw) => {
            match SitemapConfig::resolve(true, Some(raw)).map_err(|e| match e {
                ScraperError::InvalidUrl(msg) => CrawlError::InvalidUrl(msg),
                other => CrawlError::Parse(other.to_string()),
            })? {
                SitemapConfig::Enabled { url } => url,
                SitemapConfig::Disabled => None,
            }
        },
        None => None,
    };
    crawl_with_sitemap_resolved(base_url, explicit.as_ref(), config, correlation).await
}

/// Crawl site using a boundary-resolved sitemap URL (#1190).
///
/// This is the live entry: callers match on the `SitemapConfig` resolved
/// from the config and pass the validated value. The parameter is
/// `Option<&ValidUrl>` rather than `&SitemapConfig` on purpose — the mode
/// enum is `pub(crate)` (the MCP adapter cannot name it) while [`ValidUrl`]
/// is the public resolved value, already scheme-hardened with credentials
/// stripped, so no second parse happens downstream.
///
/// * `Some(url)` - fetch exactly this sitemap (already validated).
/// * `None` - auto-discover the sitemap (robots.txt `Sitemap:` directive,
///   fallback paths, then sub-path probes).
///
/// # Arguments
///
/// * `base_url` - Base URL of the website
/// * `sitemap` - Resolved explicit sitemap URL (auto-discovers if None)
/// * `config` - Crawler configuration (timeouts, TLS profile, depth gate)
///
/// # Returns
///
/// * `Ok(Vec<DiscoveredUrl>)` - URLs discovered from sitemap
/// * `Err(CrawlError)` - Error during sitemap fetch or parse
#[instrument(
    name = "crawl_with_sitemap_resolved",
    skip(config),
    fields(
        base_url,
        sitemap = ?sitemap
    )
)]
pub async fn crawl_with_sitemap_resolved(
    base_url: &str,
    sitemap: Option<&ValidUrl>,
    config: &CrawlerConfig,
    correlation: &CorrelationId,
) -> Result<Vec<DiscoveredUrl>, CrawlError> {
    crawl_with_sitemap_resolved_boxed(base_url, sitemap, config, correlation).await
}

// This manual boxing cuts the `dyn Future + Send` coercion across both the
// `crawl_with_sitemap` and `discover_sitemap` `#[tool]` boundaries. Without
// this boundary, the compiler can hit the recursion limit while evaluating
// `CoerceUnsized` through the deeply nested sitemap future (rust-lang/rust#159228).
// If downstream depth reintroduces this warning, add the same boxed boundary at
// the new convergence point rather than suppressing the recursion diagnostic.
fn crawl_with_sitemap_resolved_boxed<'a>(
    base_url: &'a str,
    sitemap: Option<&'a ValidUrl>,
    config: &'a CrawlerConfig,
    correlation: &'a CorrelationId,
) -> std::pin::Pin<
    std::boxed::Box<
        dyn std::future::Future<Output = Result<Vec<DiscoveredUrl>, CrawlError>> + Send + 'a,
    >,
> {
    std::boxed::Box::pin(async move {
        crawl_with_sitemap_internal(base_url, sitemap, config, correlation).await
    })
}

/// Crawl with sitemap (internal version with progress tracking)
///
/// This is the internal implementation that supports optional progress tracking.
/// The public `crawl_with_sitemap_resolved` function calls this one.
///
/// Following **own-borrow-over-clone**: Accepts `&ValidUrl` (borrowed), not an owned URL.
/// #1382: layer-1 entry guard for the sitemap path — the seed host is a
/// forbidden IP literal (loopback/RFC1918/...), the exact target every
/// discovery probe would dial. Refusing here means no robots.txt GET, no
/// HEAD/GET probes, no parser fetch ever leave: pre-socket by construction.
/// Reads the documented entry hatch on every call, so test harnesses
/// driving wiremock through this path disarm only this layer (#1217).
fn reject_sitemap_entry_target(raw_url: &str) -> Result<(), CrawlError> {
    let parsed =
        Url::parse(raw_url).map_err(|e| CrawlError::InvalidUrl(format!("URL inválida: {e}")))?;
    if let Err(rejection) = crate::domain::ssrf_guard::reject_forbidden_literal_url(&parsed) {
        tracing::warn!(
            url = raw_url,
            "SSRF literal-IP target rejected at sitemap entry (no socket opened)"
        );
        return Err(CrawlError::InvalidUrl(rejection.to_string()));
    }
    Ok(())
}

#[allow(unused_variables)]
async fn crawl_with_sitemap_internal(
    base_url: &str,
    sitemap: Option<&ValidUrl>,
    config: &CrawlerConfig,
    correlation: &CorrelationId,
) -> Result<Vec<DiscoveredUrl>, CrawlError> {
    info!("Crawling with sitemap for {}", base_url);

    // #1382: entry guard BEFORE the discovery client exists (layer 1 of
    // the AGENTS.md guard-chain order — see `reject_sitemap_entry_target`).
    reject_sitemap_entry_target(base_url)?;

    // Build the discovery client through the shared factory so sitemap probes
    // carry the same Chrome Client Hints, pooled user-agent, pool tuning, and
    // gzip/brotli as the DOM path (#298).
    let discovery_client = build_discovery_client(config)?;

    // Use default batch size (10,000) - the parser `SitemapConfig` handles pagination
    const DEFAULT_BATCH_SIZE: usize = 10_000;

    // Auto-discover sitemap URL if not provided
    let sitemap_url = resolve_sitemap_url(base_url, sitemap, &discovery_client).await?;

    tracing::info!("Using sitemap: {}", sitemap_url);

    // Create sitemap parser with config (including pagination settings).
    // #323: thread the configured TLS/H2 profile so sitemap XML fetches honor
    // the user's --h2-profile selection instead of a hardcoded Chrome145.
    let parser = build_sitemap_parser(config, DEFAULT_BATCH_SIZE)?;

    // Parse sitemap
    let urls = parse_sitemap(parser.as_ref(), &sitemap_url, correlation).await?;

    // Validate sitemap relevance: check if any URLs share a path prefix
    // with the target URL. This handles cases where robots.txt points to
    // an unrelated sitemap (e.g. blog sitemap for a docs site).
    let base = Url::parse(base_url).map_err(|e| CrawlError::InvalidUrl(e.to_string()))?;
    let target_path = base.path().to_string();
    let relevant_urls = filter_relevant_urls(urls, &target_path);

    // If no relevant URLs found, try sub-path sitemaps as fallback
    if relevant_urls.is_empty() {
        tracing::warn!(
            sitemap = %sitemap_url,
            target_path = %target_path,
            "sitemap has no URLs matching the target path; trying sub-path sitemaps"
        );
        return crawl_with_subpath_sitemaps(
            base_url,
            &base,
            parser.as_ref(),
            3,
            0,
            config.max_depth,
            &discovery_client,
            correlation,
        )
        .await;
    }

    build_discovered_urls_or_limit_error(relevant_urls, &base, config)
}

/// Apply the depth gate to discovered sitemap URLs and surface a crawl-limit
/// error when the configuration makes the result empty.
///
/// `max_depth 0` means "only the seed page": a sitemap that lists the seed
/// itself still yields it (depth 0), but a sitemap listing only deeper URLs
/// yields nothing — that is a configuration limit, not an empty sitemap, so it
/// maps to exit 69 via `MaxDepthExceeded` instead of exit 2
/// (stabilization-sitemap-regression, scenario 8).
fn build_discovered_urls_or_limit_error(
    relevant_urls: Vec<SitemapUrl>,
    base: &Url,
    config: &CrawlerConfig,
) -> Result<Vec<DiscoveredUrl>, CrawlError> {
    let discovered = build_discovered_urls(relevant_urls, base, config);
    if discovered.is_empty() && config.max_depth == 0 {
        return Err(CrawlError::MaxDepthExceeded { current: 1, max: 0 });
    }
    Ok(discovered)
}

/// Build the shared discovery HTTP client with the #281/#312 timeout and TLS
/// profile policy.
fn build_discovery_client(config: &CrawlerConfig) -> Result<wreq::Client, CrawlError> {
    // Timeouts replicate the #281 policy: request timeout as configured,
    // connect timeout capped at 10s.
    let http_config = HttpClientConfig {
        timeout_secs: config.timeout_secs,
        connect_timeout_secs: config.timeout_secs.min(10), // #281 policy
        tls_emulation: config.tls_emulation,               // #312 honor configured profile
        ..Default::default()
    };
    super::super::create_http_client_with_config(&http_config)
        // LCOV_EXCL_LINE defensive: wreq-client-build — client construction fails only on invalid TLS profile, an invariant
        .map_err(|e| CrawlError::Internal(format!("failed to build discovery client: {e}")))
}

/// Resolve the sitemap URL: use the provided (already validated) one, or auto-discover it.
///
/// #1382: BOTH branches pay the entry guard — an explicit sitemap URL whose
/// host is a forbidden literal (validated for scheme/credentials by
/// `ValidUrl`, never for target address) and a robots.txt directive pointing
/// at one are cut HERE, pre-socket, before the parser fetch dials anything.
async fn resolve_sitemap_url(
    base_url: &str,
    sitemap: Option<&ValidUrl>,
    client: &wreq::Client,
) -> Result<String, CrawlError> {
    let resolved = match sitemap {
        Some(url) => {
            tracing::info!("Sitemap URL provided: {}", url.as_str());
            url.as_str().to_string()
        },
        None => discover_sitemap_url_for(base_url, client).await?,
    };

    // Entry guard over the resolved sitemap target (#1382): the seed may be a
    // hostname, but the sitemap this chain is about to fetch must not be a
    // forbidden literal. This also covers the robots.txt `Sitemap:` directive
    // case — the directive is attacker-controllable text and its target is
    // exactly what the parser would dial next.
    reject_sitemap_entry_target(&resolved)?;

    Ok(resolved)
}

/// Auto-discover a sitemap URL, logging the discovery outcome.
async fn discover_sitemap_url_for(
    base_url: &str,
    client: &wreq::Client,
) -> Result<String, CrawlError> {
    tracing::info!("Auto-discovering sitemap URL for {}", base_url);
    match discover_sitemap_url(base_url, client).await {
        Ok(url) => {
            tracing::info!("Discovered sitemap URL: {}", url);
            Ok(url)
        },
        Err(CrawlError::SitemapNotFound(url)) => Err(CrawlError::SitemapNotFound(url)),
        Err(e) => Err(e),
    }
}

/// Keep only sitemap URLs whose path shares the target's path prefix.
///
/// Paths are compared using the domain's [`canonical_path`] so that a seed of
/// `/docs/` still matches a sitemap URL `/docs` (and vice-versa). Without this,
/// `"/docs".starts_with("/docs/")` is `false` and the section index is silently
/// dropped.
///
/// [`canonical_path`]: crate::domain::url_validation::canonical_path
fn filter_relevant_urls(urls: Vec<SitemapUrl>, target_path: &str) -> Vec<SitemapUrl> {
    let target = crate::domain::url_validation::canonical_path(target_path);
    urls.into_iter()
        .filter(|url| {
            crate::domain::url_validation::canonical_path(url.url.path()).starts_with(target)
        })
        .collect()
}

/// Map relevant sitemap URLs to discovered URLs, applying include/exclude
/// patterns and the depth gate.
///
/// Depth assignment: the seed itself is depth 0; every other sitemap URL is one
/// hop from the seed, hence depth 1. Filtering by `depth <= max_depth` enforces
/// the CLI contract "0 = only seed URL" here, because the CLI scrape flow scrapes
/// whatever discovery returns verbatim — there is no later depth gate (the Engine's
/// `run_crawl_task` check is a separate, non-CLI code path).
fn build_discovered_urls(
    relevant_urls: Vec<SitemapUrl>,
    base: &Url,
    config: &CrawlerConfig,
) -> Vec<DiscoveredUrl> {
    let max_depth = config.max_depth;
    relevant_urls
        .into_iter()
        .filter(|url| is_allowed(url.url.as_str(), config))
        .filter_map(|url| {
            let depth = if url.url == *base { 0 } else { 1 };
            (depth <= max_depth).then(|| DiscoveredUrl::html(url.url, depth, base.clone()))
        })
        .collect()
}

// All tests in this module build a wreq::Client (test_client()), which
// depends on btls-sys (BoringSSL → TLS_method FFI). Miri cannot execute
// C FFI — gate the whole module instead of patching test by test, keeping
// Miri focused on UB in pure Rust logic (same pattern as readability). The
// gate is spelled as the stacked equivalent (cfg attributes AND together)
// so scripts/split_tests.sh can own the extraction: semantics identical.
#[cfg(test)]
#[cfg(not(miri))]
#[path = "sitemap_discovery_tests.rs"]
mod tests;

// Same btls-sys Miri gate as `mod tests` above, spelled as the stacked
// equivalent (cfg attributes AND together) so scripts/split_tests.sh can
// own the extraction: semantics identical, no test gains or loses a gate.
#[cfg(test)]
#[cfg(not(miri))]
#[path = "sitemap_discovery_entry_tests.rs"]
mod entry_tests;

// Same btls-sys Miri gate as `mod tests` above, spelled as the stacked
// equivalent (cfg attributes AND together) so scripts/split_tests.sh can
// own the extraction: semantics identical, no test gains or loses a gate.
#[cfg(test)]
#[cfg(not(miri))]
#[path = "sitemap_discovery_ssrf_tests.rs"]
mod ssrf_tests;
