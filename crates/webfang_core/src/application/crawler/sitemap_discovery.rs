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
// Miri focused on UB in pure Rust logic (same pattern as readability).
#[cfg(all(test, not(miri)))]
mod tests {
    use super::*;
    use crate::domain::error::WafDetectionKind;
    use crate::infrastructure::observability::test_log_capture::LogBuffer;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    // Install the real WAF inspector on first test access (#996). Idempotent.
    fn ensure_waf_inspector() {
        use crate::domain::waf::set_waf_inspector;
        use crate::infrastructure::http::waf_engine::WafInspector;
        use std::sync::{Arc, OnceLock};
        static INIT: OnceLock<()> = OnceLock::new();
        INIT.get_or_init(|| {
            set_waf_inspector(
                Arc::new(WafInspector) as Arc<dyn crate::domain::waf::WafInspectorPort>
            );
        });
    }

    fn test_client() -> wreq::Client {
        wreq::Client::new()
    }

    #[cfg_attr(
        miri,
        ignore = "test_client() builds a real wreq::Client (btls-sys TLS_method FFI at build time); unsupported by Miri (exhaustive survey for #1563)"
    )]
    #[tokio::test]
    async fn discovers_via_robots_absolute_directive() {
        let mock = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/robots.txt"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string("User-agent: *\nSitemap: https://example.com/sitemap.xml\n"),
            )
            .mount(&mock)
            .await;

        let result = discover_sitemap_url(mock.uri().as_str(), &test_client()).await;
        assert_eq!(result.unwrap(), "https://example.com/sitemap.xml");
    }

    #[cfg_attr(
        miri,
        ignore = "test_client() builds a real wreq::Client (btls-sys TLS_method FFI at build time); unsupported by Miri (exhaustive survey for #1563)"
    )]
    #[tokio::test]
    async fn discovers_via_robots_relative_directive() {
        let mock = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/robots.txt"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string("User-agent: *\nSitemap: /sitemap.xml\n"),
            )
            .mount(&mock)
            .await;

        let result = discover_sitemap_url(mock.uri().as_str(), &test_client()).await;
        assert_eq!(result.unwrap(), format!("{}/sitemap.xml", mock.uri()));
    }

    #[cfg_attr(
        miri,
        ignore = "test_client() builds a real wreq::Client (btls-sys TLS_method FFI at build time); unsupported by Miri (exhaustive survey for #1563)"
    )]
    #[tokio::test]
    async fn discovers_via_fallback_location() {
        let mock = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/robots.txt"))
            .respond_with(ResponseTemplate::new(200).set_body_string("User-agent: *\n"))
            .mount(&mock)
            .await;
        Mock::given(method("HEAD"))
            .and(path("/sitemap.xml"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&mock)
            .await;

        let result = discover_sitemap_url(mock.uri().as_str(), &test_client()).await;
        assert_eq!(result.unwrap(), format!("{}/sitemap.xml", mock.uri()));
    }

    #[cfg_attr(
        miri,
        ignore = "test_client() builds a real wreq::Client (btls-sys TLS_method FFI at build time); unsupported by Miri (exhaustive survey for #1563)"
    )]
    #[tokio::test]
    async fn discovers_via_subpath_sitemap() {
        let mock = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/robots.txt"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&mock)
            .await;
        Mock::given(method("HEAD"))
            .and(path("/sitemap.xml"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&mock)
            .await;
        Mock::given(method("HEAD"))
            .and(path("/docs/sitemap.xml"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&mock)
            .await;

        let base = format!("{}/docs", mock.uri());
        let result = discover_sitemap_url(&base, &test_client()).await;
        assert_eq!(result.unwrap(), format!("{}/docs/sitemap.xml", mock.uri()));
    }

    // ── Issue #879 (Option A): WAF inspection over sitemap-chain bodies ──

    /// Cloudflare Turnstile widget marker — Challenge-tier (T1), blocks even
    /// in degraded mode per REQ-WAF-05/09.
    const CHALLENGE_HTML: &str = r#"<html><body>Just a moment...</body><div id="cf-turnstile" data-sitekey="abc"></div></html>"#;

    /// A WAF challenge served where robots.txt should be must abort discovery
    /// with the typed `CrawlError::WafChallenge` (carrying host context and
    /// evidence chain) and emit a structured trace event — not silently fall
    /// through to `SitemapNotFound` (issue #879).
    #[cfg_attr(
        miri,
        ignore = "test_client() builds a real wreq::Client (btls-sys TLS_method FFI at build time); unsupported by Miri (exhaustive survey for #1563)"
    )]
    #[test]
    fn robots_txt_waf_challenge_yields_typed_error_and_trace_event() {
        ensure_waf_inspector();
        let buf = LogBuffer::run(|| {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("test runtime");
            rt.block_on(async {
                let mock = MockServer::start().await;
                Mock::given(method("GET"))
                    .and(path("/robots.txt"))
                    .respond_with(ResponseTemplate::new(200).set_body_string(CHALLENGE_HTML))
                    .mount(&mock)
                    .await;

                let result = discover_sitemap_url(mock.uri().as_str(), &test_client()).await;

                match result {
                    Err(CrawlError::WafChallenge {
                        provider,
                        kind,
                        url,
                    }) => {
                        assert_eq!(kind, WafDetectionKind::BodySignature);
                        assert!(
                            provider.contains("Cloudflare"),
                            "evidence chain expected in provider, got: {provider}"
                        );
                        assert_eq!(url, format!("{}/robots.txt", mock.uri()));
                    },
                    other => panic!("expected CrawlError::WafChallenge, got: {other:?}"),
                }
            });
        });

        let out = buf.text();
        assert!(
            out.contains("challenge"),
            "WAF trace event expected on the discovery path, got: {out}"
        );
    }

    /// A benign robots.txt that merely mentions a WAF vendor at status 200 is
    /// Fingerprint-tier evidence, which never blocks without a correlated WAF
    /// status (REQ-WAF-09): discovery must proceed to fallbacks as before.
    #[cfg_attr(
        miri,
        ignore = "test_client() builds a real wreq::Client (btls-sys TLS_method FFI at build time); unsupported by Miri (exhaustive survey for #1563)"
    )]
    #[tokio::test]
    async fn benign_robots_txt_mentioning_vendor_does_not_trigger_waf_error() {
        let mock = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/robots.txt"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string("User-agent: *\n# site hosted behind cloudflare\n"),
            )
            .mount(&mock)
            .await;

        let result = discover_sitemap_url(mock.uri().as_str(), &test_client()).await;
        assert!(
            !matches!(result, Err(CrawlError::WafChallenge { .. })),
            "benign vendor mention must not raise WafChallenge, got: {result:?}"
        );
    }

    #[cfg_attr(
        miri,
        ignore = "test_client() builds a real wreq::Client (btls-sys TLS_method FFI at build time); unsupported by Miri (exhaustive survey for #1563)"
    )]
    #[tokio::test]
    async fn errors_when_no_sitemap_found() {
        let mock = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/robots.txt"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&mock)
            .await;
        Mock::given(method("HEAD"))
            .and(path("/sitemap.xml"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&mock)
            .await;

        let result = discover_sitemap_url(mock.uri().as_str(), &test_client()).await;
        assert!(result.is_err());
    }

    // ── Issue #1162/#1190: rejection end-to-end via discovery ──
    // These replace the isolated-enum `issue_1162_*` tests (which passed
    // while production still failed late): each drives a real discovery
    // entry, so a regression reintroduces the late failure — not a unit
    // miss (systemic-issue-triage: trust the reproduction over the
    // mechanism).

    /// `Some("not-a-url")` is rejected by the legacy entry BEFORE any
    /// client build or fetch (Spanish, typed) — no mock needed because
    /// the network is never touched.
    #[tokio::test]
    async fn issue_1162_invalid_url_rejected_before_fetch() {
        let seed = Url::parse("https://example.com").expect("valid seed");
        let config = CrawlerConfig::new(seed);
        let err = crawl_with_sitemap(
            "https://example.com",
            Some("not-a-url"),
            &config,
            &CorrelationId::new(),
        )
        .await
        .expect_err("invalid sitemap URL must fail at the boundary");
        match err {
            CrawlError::InvalidUrl(msg) => assert!(
                msg.contains("sitemap") && msg.contains("inválida"),
                "rejection must name the sitemap URL in Spanish, got: {msg}"
            ),
            other => panic!("expected InvalidUrl, got: {other:?}"),
        }
    }

    /// Non-http(s) schemes are rejected at the same edge (`ValidUrl`
    /// hardening applies at the boundary, not just URL shape).
    #[tokio::test]
    async fn issue_1162_non_http_sitemap_scheme_rejected() {
        let seed = Url::parse("https://example.com").expect("valid seed");
        let config = CrawlerConfig::new(seed);
        let err = crawl_with_sitemap(
            "https://example.com",
            Some("ftp://example.com/sitemap.xml"),
            &config,
            &CorrelationId::new(),
        )
        .await
        .expect_err("non-http(s) sitemap URL must fail at the boundary");
        assert!(
            matches!(err, CrawlError::InvalidUrl(_)),
            "expected InvalidUrl, got: {err:?}"
        );
    }

    /// An explicit VALID URL flows through the boundary untouched: served
    /// by wiremock, the resolved entry returns exactly its `<loc>` entries.
    #[tokio::test]
    async fn issue_1162_explicit_valid_url_used_verbatim() {
        // Loopback mock seed: the entry guard (#1382) now cuts literal-IP
        // targets exactly as production should — these tests exercise the
        // verbatim-URL contract, not the guard, so the entry hatch is the
        // documented one-layer disarmer (same posture as the scrape-flow
        // robots tests, #1369).
        let _entry_off = webfang_test_utils::EnvGuard::entry_guard_off();
        let mock = MockServer::start().await;
        let page1 = format!("{}/page1", mock.uri());
        let page2 = format!("{}/page2", mock.uri());
        let xml = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
    <url><loc>{page1}</loc></url>
    <url><loc>{page2}</loc></url>
</urlset>"#
        );
        Mock::given(method("GET"))
            .and(path("/sitemap.xml"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(xml)
                    .insert_header("Content-Type", "application/xml"),
            )
            .mount(&mock)
            .await;

        let base_url = mock.uri();
        let seed = Url::parse(&base_url).expect("valid mock URL");
        let config = CrawlerConfig::new(seed);
        let sitemap_url = format!("{base_url}/sitemap.xml");
        let explicit = ValidUrl::parse(&sitemap_url).expect("explicit test URL is valid");

        let urls =
            crawl_with_sitemap_resolved(&base_url, Some(&explicit), &config, &CorrelationId::new())
                .await
                .expect("explicit sitemap must parse");

        assert_eq!(urls.len(), 2, "expected both <loc> entries");
        let found: Vec<String> = urls.into_iter().map(|d| d.url.to_string()).collect();
        assert!(found.contains(&page1), "should contain page1");
        assert!(found.contains(&page2), "should contain page2");
    }

    /// No explicit URL auto-discovers through the resolved entry
    /// (robots.txt `Sitemap:` directive → sitemap fetch → URLs).
    #[tokio::test]
    async fn issue_1162_auto_discovery_without_url() {
        // Loopback mock seed — same entry-hatch posture as the verbatim test
        // above (#1382/#1369): the contract under test is the discovery
        // chain, not the guard.
        let _entry_off = webfang_test_utils::EnvGuard::entry_guard_off();
        let mock = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/robots.txt"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string("User-agent: *\nDisallow:\nSitemap: /sitemap.xml\n"),
            )
            .mount(&mock)
            .await;
        let page1 = format!("{}/page1", mock.uri());
        let page2 = format!("{}/page2", mock.uri());
        let xml = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
    <url><loc>{page1}</loc></url>
    <url><loc>{page2}</loc></url>
</urlset>"#
        );
        Mock::given(method("GET"))
            .and(path("/sitemap.xml"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(xml)
                    .insert_header("Content-Type", "application/xml"),
            )
            .mount(&mock)
            .await;

        let base_url = mock.uri();
        let seed = Url::parse(&base_url).expect("valid mock URL");
        let config = CrawlerConfig::new(seed);

        let urls = crawl_with_sitemap_resolved(&base_url, None, &config, &CorrelationId::new())
            .await
            .expect("auto-discovery should succeed");

        assert_eq!(urls.len(), 2, "expected both auto-discovered URLs");
    }

    // ── #1382: sitemap discovery SSRF entry guard (pre-socket) ──
    //
    // `build_discovery_client` installs SSRF layers 3 (validating resolver)
    // and 4 (redirect policy) but the entry layer (`reject_forbidden_literal_url`)
    // was never called on this path, so a loopback *seed* opened real sockets
    // (measured: 18 requests with every guard armed, #1376/#1382). These pins
    // restore the AGENTS.md guard-chain order: entry validation BEFORE the
    // discovery client touches the network.
    //
    // Every pin below needs layer 1 ARMED, which makes them *observers* of the
    // process-global environment rather than mutators of it. Taking no lock is
    // not neutrality here: `ENV_LOCK` serializes only the threads that take it,
    // so a sibling test inside an `entry_guard_off()` window would leave these
    // asserting against a DISARMED guard — the RFC1918 literal would pass
    // through and the downloader would dial it, failing with `Http { Connect }`
    // where the contract says `CrawlError::InvalidUrl`. That is #1788, and it
    // reproduces under libtest (one process, N threads — what the `Coverage`
    // job runs, no `--nextest`) rather than under nextest (one process per
    // test, so the window never coincides). Hence `EnvGuard::entry_guard_on()`:
    // it excludes the disarm window for its whole lifetime and mutates nothing.

    /// A loopback seed is rejected at entry, pre-socket: the run fails with
    /// the typed `CrawlError::InvalidUrl` carrying the Spanish SSRF message,
    /// and the mock — the only listener for the seed's host — receives zero
    /// requests of any kind (no robots.txt GET, no probes, nothing).
    #[tokio::test]
    async fn sitemap_discovery_rejects_loopback_seed_pre_socket() {
        // Layer 1 is the SUBJECT: it must be armed, and observing that
        // requires excluding every sibling's disarm window (#1788).
        let _entry_on = webfang_test_utils::EnvGuard::entry_guard_on();
        let mock = MockServer::start().await;
        // Tripwire: any request that escapes the guard lands here and fails
        // the zero-request assertion.
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string("<html></html>"))
            .mount(&mock)
            .await;

        let base_url = mock.uri();
        let seed = Url::parse(&base_url).expect("mock URL must parse");
        let config = CrawlerConfig::new(seed);

        let err = crawl_with_sitemap_resolved(&base_url, None, &config, &CorrelationId::new())
            .await
            .expect_err("loopback seed must be rejected at the entry guard");

        match err {
            CrawlError::InvalidUrl(msg) => assert!(
                msg.contains("SSRF detectado"),
                "rejection must carry the Spanish SSRF message, got: {msg}"
            ),
            other => panic!("expected CrawlError::InvalidUrl, got: {other:?}"),
        }
        let seen = mock.received_requests().await.unwrap_or_default();
        assert!(
            seen.is_empty(),
            "rejection is pre-socket: the mock must observe zero requests, got {seen:?}"
        );
    }

    /// An explicit sitemap URL whose host is a forbidden literal is rejected
    /// at entry even when the seed host itself is fetchable text: the guard
    /// applies to every URL this path would fetch, not only the seed. The
    /// seed is never fetched by sitemap discovery, so a hostname seed keeps
    /// this test focused on the sitemap target alone.
    #[tokio::test]
    async fn sitemap_discovery_rejects_forbidden_literal_sitemap_url() {
        let _entry_on = webfang_test_utils::EnvGuard::entry_guard_on();
        // Hostname seed: sitemap discovery never fetches the seed itself, and
        // the entry guard lets hostnames through (that is layer-3 territory).
        let seed = Url::parse("https://example.com").expect("hostname seed");
        let config = CrawlerConfig::new(seed);
        // Forbidden literal, RFC1918. Nothing listens there and nothing may
        // ever dial it — the assertion is the typed rejection; the address is
        // unroutable in the test environment, so a followed fetch could never
        // succeed anyway.
        let sitemap_url =
            ValidUrl::parse("http://192.168.1.5:59999/sitemap.xml").expect("literal parses");

        let err = crawl_with_sitemap_resolved(
            "https://example.com",
            Some(&sitemap_url),
            &config,
            &CorrelationId::new(),
        )
        .await
        .expect_err("forbidden-literal sitemap URL must be rejected at entry");

        match err {
            CrawlError::InvalidUrl(msg) => assert!(
                msg.contains("SSRF detectado") && msg.contains("192.168.1.5"),
                "rejection must name the offending literal in Spanish, got: {msg}"
            ),
            other => panic!("expected CrawlError::InvalidUrl, got: {other:?}"),
        }
    }

    /// A robots.txt `Sitemap:` directive pointing at a forbidden literal is
    /// rejected before the sitemap fetch: robots.txt itself is fetched (one
    /// request, the discovery chain's legitimate first hop), but the pointed
    /// sitemap URL is never dialed. The seed uses the `localhost` hostname —
    /// the entry layer correctly lets hostnames through (#1217 layer
    /// separation) — with the resolver hatch set so the loopback answer is
    /// dialable by the harness. Layer 1 (the subject under test) stays ARMED:
    /// the mock's one-request journal plus the typed rejection pin that the
    /// directive target was cut before its socket existed.
    #[tokio::test]
    async fn sitemap_discovery_rejects_robots_directive_pointing_at_literal() {
        // Resolver-only hatch: `localhost` resolves to 127.0.0.1, a forbidden
        // answer for layer 3. Layer 1 (literal guard) stays armed — it is the
        // layer under test. ONE guard, not two: `ENV_LOCK` is not reentrant,
        // so an armed-guard observer stacked on an `EnvGuard::with` mutator
        // would self-deadlock (#1788, #1224). The combined constructor keeps a
        // single lock holder and leaves the hatch names in this one module.
        let _entry_on = webfang_test_utils::EnvGuard::entry_guard_on_resolver_off();
        // `localhost` + the mock's port: the same wiremock listener, spelled
        // as a hostname so the entry layer is not the thing that fires on the
        // seed itself.
        let mock = MockServer::start().await;
        let base_url = format!("http://localhost:{}/", mock.address().port());

        Mock::given(method("GET"))
            .and(path("/robots.txt"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string("User-agent: *\nSitemap: http://10.9.8.7:9/sitemap.xml\n"),
            )
            .mount(&mock)
            .await;

        let seed = Url::parse(&base_url).expect("localhost seed must parse");
        let config = CrawlerConfig::new(seed);

        let err = crawl_with_sitemap_resolved(&base_url, None, &config, &CorrelationId::new())
            .await
            .expect_err("robots directive at a forbidden literal must be rejected");

        match err {
            CrawlError::InvalidUrl(msg) => assert!(
                msg.contains("SSRF detectado") && msg.contains("10.9.8.7"),
                "rejection must name the directive's forbidden literal, got: {msg}"
            ),
            other => panic!("expected CrawlError::InvalidUrl, got: {other:?}"),
        }
        let seen = mock.received_requests().await.unwrap_or_default();
        assert_eq!(
            seen.len(),
            1,
            "only the robots.txt fetch may reach the mock (the legitimate \
             first hop); the directive target must never be dialed, got {seen:?}"
        );
    }

    /// A sitemap index whose `<sitemap>` child points at a forbidden literal
    /// is stopped by the parser's per-URL entry guard: the child URL is
    /// never dialed (pre-socket) and the typed SSRF rejection surfaces
    /// instead of a fetch error. The seed uses the `localhost` hostname with
    /// the resolver-only hatch so the index itself is fetchable while the
    /// child literal — an unroutable RFC1918 address — is cut by the armed
    /// entry layer before its socket exists.
    #[tokio::test]
    async fn sitemap_discovery_rejects_index_child_pointing_at_literal() {
        // Same single-holder constraint as the robots-directive pin above: the
        // entry guard must stay ARMED (it is the layer under test) and the
        // resolver hatch must be lifted for the loopback index, so both facts
        // are established by ONE non-reentrant-lock holder (#1788).
        let _entry_on = webfang_test_utils::EnvGuard::entry_guard_on_resolver_off();
        let mock = MockServer::start().await;
        let base_url = format!("http://localhost:{}/", mock.address().port());

        Mock::given(method("GET"))
            .and(path("/robots.txt"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string("User-agent: *\nSitemap: /sitemap.xml\n"),
            )
            .mount(&mock)
            .await;
        let index_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<sitemapindex xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
    <sitemap><loc>http://192.168.7.7:9/child-sitemap.xml</loc></sitemap>
</sitemapindex>"#;
        Mock::given(method("GET"))
            .and(path("/sitemap.xml"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(index_xml)
                    .insert_header("Content-Type", "application/xml"),
            )
            .mount(&mock)
            .await;

        let seed = Url::parse(&base_url).expect("localhost seed must parse");
        let config = CrawlerConfig::new(seed);

        let err = crawl_with_sitemap_resolved(&base_url, None, &config, &CorrelationId::new())
            .await
            .expect_err("index child at a forbidden literal must be rejected");

        match err {
            CrawlError::InvalidUrl(msg) => assert!(
                msg.contains("SSRF detectado") && msg.contains("192.168.7.7"),
                "rejection must name the child's forbidden literal, got: {msg}"
            ),
            other => panic!("expected CrawlError::InvalidUrl, got: {other:?}"),
        }
    }
}
