//! Scraping flow logic extracted from orchestrator.

use std::collections::HashMap;

use futures::stream::StreamExt;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use url::Url;

use crate::application::container;
use crate::application::container::Container;
use crate::application::crawl_options::CrawlOptions;
use crate::application::crawler::content_sink::CapturedPage;
use crate::application::progress_observer::ProgressObserver;
use crate::application::rate_limiter::{PacingContext, RateLimiterConfig, SharedRateLimiter};
use crate::application::scrape_single_url;
use crate::domain::config::ScraperConfig;
use crate::domain::crawler_port::{RobotsDecision, RobotsPort};
use crate::domain::entities::progress::{ScrapeError, ScrapeStatus};
use crate::domain::{CorrelationId, ScrapedContent};
use crate::infrastructure::downloader::Downloader;
use crate::infrastructure::observability::log_scrape_error_with_cause;
use crate::HttpClientConfig;

use crate::application::adaptive_engine::AdaptiveSelectorEngine;

#[path = "scrape_fingerprint.rs"]
mod scrape_fingerprint;

use self::scrape_fingerprint::{build_fingerprint_repo, record_extraction_fingerprint};
#[path = "scrape_resume.rs"]
mod scrape_resume;

pub use scrape_resume::{apply_resume_mode, resolve_default_state_dir};
pub(crate) use scrape_resume::{record_store_bridge, warn_unreadable_state};

/// Scrape all URLs, reporting progress via the provided observer.
///
/// Returns `(results, failures, blocked)` where `blocked` counts URLs skipped
/// because robots.txt disallowed them (#705) — distinct from both successful
/// results and failures, and from shutdown-skipped URLs.
///
/// Correlation contract (#501): `root_correlation` is the run-root identity
/// owned by the orchestrator; each page derives `.child()` from it — one
/// shared `trace_id` for the whole run, a fresh `span_id` per page.
///
/// The observer handles quiet/channel logic internally — callers pass
/// `&NoopObserver` for dry-run or `&LiveProgressObserver` for live output.
///
/// `captured` carries discovery-captured bodies (F-05, #1229): URLs with a
/// cached body skip the HTTP fetch and extract from the capture instead —
/// one request per page. Misses fall back to a normal fetch. Pass `&[]`
/// when discovery ran without a sink (dry-run, sitemap, single-page).
///
/// # Errors
///
/// Returns [`crate::error::ScraperError`] if the configured H2/TLS profile name
/// (`opts.network.h2_profile`) is not recognized, or if the fetch router's HTTP
/// client cannot be built. Both are setup failures that abort the whole batch
/// before any URL is scraped.
// The parameter list is the scrape phase's full dependency set (config,
// observer, downloader, adaptive engine, correlation root, shutdown token).
// Bundling them into a struct would only move the same wiring one level up.
#[allow(clippy::too_many_arguments)]
pub async fn scrape_urls(
    urls: &[Url],
    scraper_config: &ScraperConfig,
    opts: &CrawlOptions,
    observer: &dyn ProgressObserver,
    downloader: Option<&dyn crate::domain::ports::AssetDownloaderPort>,
    engine: Option<&AdaptiveSelectorEngine>,
    root_correlation: &CorrelationId,
    cancel: &CancellationToken,
    captured: &[CapturedPage],
) -> Result<
    (
        Vec<ScrapedContent>,
        Vec<(String, crate::error::ScraperError)>,
        usize,
    ),
    crate::error::ScraperError,
> {
    // Single wiring graph (#1149): the fetch downloader, cookie set, and
    // robots fetcher are built through the `Container` ephemeral factories —
    // the same production factory the crawl Engine uses. Wire values are
    // unchanged: UA #503, headers/lang/cookies/jar #890, obscura binary #787,
    // shutdown token #653, robots-per-batch TLS fingerprint #337.
    let http_config = build_http_client_config(opts)?;
    let (cookie_bridge, initial_cookie_jar) = Container::fresh_scrape_cookie_set(opts, urls);
    let spec = Container::scrape_downloader_spec(opts, &http_config, initial_cookie_jar);
    let router = Container::build_scrape_downloader(&spec, cookie_bridge, cancel.clone())?;

    let _total_urls = urls.len();

    // Robots.txt fetcher — shares the batch's TLS fingerprint so the robots.txt
    // request is indistinguishable from a page fetch (#337). Shared across all
    // URLs in this batch. Erased to the domain seam; the concrete is the same
    // `RobotsFetcher` as before, now named only inside the Container.
    let robots_fetcher =
        container::build_robots_fetcher(http_config.tls_emulation, http_config.timeout_secs)?;

    // F-05 (#1229 slice 2): index discovery-captured bodies by URL so
    // cache hits skip the HTTP fetch — one request per page. First
    // occurrence wins; the engine dedups, so dupes only arise from
    // overlapping runs sharing a sink.
    let mut captured_bodies: HashMap<String, String> = HashMap::with_capacity(captured.len());
    for page in captured {
        captured_bodies
            .entry(page.url.clone())
            .or_insert_with(|| page.html.clone());
    }

    // Apply max_pages limit if configured
    let urls_to_process = apply_max_pages_limit(urls, scraper_config);

    let processing_count = urls_to_process.len();
    let mut results = Vec::with_capacity(processing_count);
    let mut failures: Vec<(String, crate::error::ScraperError)> = Vec::new();

    let ctx = ScrapeContext {
        router: router.as_ref(),
        scraper_config,
        downloader,
        engine,
        robots_fetcher: robots_fetcher.as_ref(),
        fingerprint_repo: build_fingerprint_repo(opts).await,
        captured_bodies: &captured_bodies,
        rate_limiter: build_scrape_rate_limiter(opts),
    };

    // Concurrency bound (#653): the previous sequential loop made concurrency a
    // no-op on the default scrape path. `buffer_unordered` keeps at most
    // `concurrency` fetches in flight; the enumerated index restores the
    // original URL order afterwards so output stays deterministic. The bound
    // derives from the budget model's Operation.crawl tier (task 2.5a) — the
    // NonZero tier type guarantees ≥ 1, so no `.max(1)` guard is needed.
    let concurrency = scrape_concurrency(opts, &crate::domain::budget::detector::SystemDetector);
    info!(
        concurrency,
        urls = processing_count,
        "scraping with bounded concurrency"
    );

    let mut ordered: Vec<(usize, Option<ScrapeOutcome>)> =
        futures::stream::iter(urls_to_process.into_iter().enumerate())
            .map(|(index, url)| {
                let ctx = &ctx;
                async move {
                    // Shutdown (#653): stop starting new pages, but let the ones
                    // already in flight finish so their content still reaches
                    // the export phase.
                    if cancel.is_cancelled() {
                        return (index, None);
                    }
                    // #P4-4: take a token BEFORE any socket opens for this
                    // URL. Governor consumes the permit at grant time, so
                    // waiting after the fetch would space nothing. A wait
                    // abandoned by shutdown is a skip, not a failure (#509).
                    //
                    // #1610 (OBS-H2): the per-page identity is minted BEFORE
                    // the wait, so the pacing event carries the same
                    // correlation as the page it delayed instead of floating
                    // free between the batch and the first page span.
                    let page_correlation = root_correlation.child();
                    if let Some(limiter) = ctx.rate_limiter.as_ref() {
                        let wait_ctx = PacingContext::bare("cli_scrape")
                            .with_url(url.as_str())
                            .with_correlation(&page_correlation);
                        if limiter
                            .until_ready_or_cancel_observed(&wait_ctx, cancel)
                            .await
                            .is_err()
                        {
                            return (index, None);
                        }
                    }
                    let outcome = scrape_one_url(
                        &url,
                        ctx,
                        opts,
                        observer,
                        &page_correlation,
                        root_correlation,
                    )
                    .await;
                    (index, Some((url, outcome)))
                }
            })
            .buffer_unordered(concurrency)
            .collect()
            .await;

    ordered.sort_by_key(|(index, _)| *index);

    let mut skipped = 0usize;
    let mut blocked = 0usize;
    for (_, slot) in ordered {
        let Some((url, outcome)) = slot else {
            skipped += 1;
            continue;
        };
        match outcome {
            Ok(Some(content)) => results.push(content),
            // Robots.txt blocked — skipped, not a failure. Counted separately
            // (#705) so an all-blocked run can exit 77 instead of a misleading
            // "no pages scraped" network error.
            Ok(None) => blocked += 1,
            Err(e) => failures.push((url.as_str().to_string(), e)),
        }
    }

    if skipped > 0 {
        warn!(skipped, "shutdown requested — URLs left unscraped");
    }

    let total_successful = results.len();
    let total_failed = failures.len();
    observer
        .on_finished(processing_count, total_successful, total_failed)
        .await;

    Ok((results, failures, blocked))
}

/// Result of one page scrape: the URL plus its outcome (content, robots-skip,
/// or failure). Kept as an alias so the concurrent pipeline's element type
/// stays readable.
type ScrapeOutcome = (
    Url,
    Result<Option<ScrapedContent>, crate::error::ScraperError>,
);

/// Shared per-URL dependencies for a single scrape, bundled to keep the
/// per-page helper's signature small.
struct ScrapeContext<'a> {
    router: &'a dyn Downloader,
    scraper_config: &'a ScraperConfig,
    downloader: Option<&'a dyn crate::domain::ports::AssetDownloaderPort>,
    engine: Option<&'a AdaptiveSelectorEngine>,
    robots_fetcher: &'a dyn RobotsPort,
    /// Discovery-captured bodies by URL (F-05, #1229): hits skip the
    /// fetch and extract from the capture instead.
    captured_bodies: &'a HashMap<String, String>,
    /// Extraction failure fingerprint sink (#792). `None` when
    /// `--extraction-fingerprint` is off — recording is opt-in.
    fingerprint_repo:
        Option<std::sync::Arc<dyn crate::domain::fingerprint_repository::FingerprintRepository>>,
    /// Token bucket gating every scrape-phase fetch (#P4-4).
    ///
    /// `None` means "no `--delay-ms` was asked for": no bucket is built and
    /// the per-URL path performs no await at all, so the unthrottled run
    /// keeps its exact pre-fix cost and cadence.
    rate_limiter: Option<SharedRateLimiter>,
}

/// Apply the `max_pages` cap to the URL list when configured.
/// Scrape-path `buffer_unordered` bound.
///
/// Derives from the budget model's Operation.crawl tier built from the run's
/// operator overrides plus the given hardware detector (task 2.5a); the
/// enforcement mechanism (`buffer_unordered`) is unchanged.
fn scrape_concurrency(
    opts: &CrawlOptions,
    detector: &dyn crate::domain::budget::detector::HardwareDetector,
) -> usize {
    // Single budget point (#1149): the CLI scrape bound reads the same
    // Operation.crawl tier the Engine tiers derive from.
    Container::scrape_concurrency(opts, detector)
}

/// Build the token bucket that gates the scrape phase (#P4-4).
///
/// `--delay-ms` reached the crawl Engine's discovery limiter but never the
/// scrape path, so a direct scrape ignored it entirely and a crawl's
/// scrape-phase re-fetches ran free. The bucket is built from the SAME two
/// inputs `Engine::run` uses — `delay_ms` as the refill period and the
/// budget model's independent burst tier (`rate_limiter_config`,
/// engine.rs:152) — so discovery and scrape share one cadence policy.
///
/// `delay_ms == 0` AND `jitter_ms == 0` returns `None`: no bucket is
/// allocated and the per-URL path performs no await, which keeps an
/// unthrottled run identical to the pre-fix behavior (zero overhead, zero
/// drift in the existing suites). A jitter on its own IS a pacing request,
/// so it wires the bucket even with a zero delay (#2610).
///
/// A construction failure degrades to `None` with a WARN, mirroring the
/// `Container` precedent (`container.rs:608`). It is unreachable in
/// practice: `SharedRateLimiter::new` rejects only a zero period (clamped
/// to 1 ms) or zero burst (`BurstPermits` is `NonZeroU32`).
fn build_scrape_rate_limiter(opts: &CrawlOptions) -> Option<SharedRateLimiter> {
    // A zero delay means "no pacing" — but only when the operator asked for
    // no pacing. A jitter on its own IS a pacing request, so the limiter is
    // built even with a zero base delay (#2610): the gate then spaces the
    // requests by `U[0, jitter_ms]` over the clamped 1 ms floor, exactly
    // like the crawl path's `delay_ms.max(1)`. Without this, the same
    // `--delay-ms 0 --jitter-ms N` would mean "no pacing" here and "paced"
    // on the crawl path.
    if opts.network.delay_ms == 0 && opts.network.jitter_ms == 0 {
        return None;
    }
    let budget = crate::domain::budget::BudgetModel::build(
        opts.budget_overrides,
        &crate::domain::budget::detector::SystemDetector,
    );
    let burst = budget.burst().get();
    match SharedRateLimiter::new(
        &RateLimiterConfig::new(opts.network.delay_ms, burst).with_jitter(opts.network.jitter_ms),
    ) {
        Ok(limiter) => {
            info!(
                delay_ms = opts.network.delay_ms,
                jitter_ms = opts.network.jitter_ms,
                burst,
                "scrape rate limiter wired"
            );
            Some(limiter)
        },
        Err(e) => {
            warn!(
            error = %e,
            delay_ms = opts.network.delay_ms,
            "scrape rate limiter unavailable — continuing unthrottled"
            );
            None
        },
    }
}

fn apply_max_pages_limit(urls: &[Url], scraper_config: &ScraperConfig) -> Vec<Url> {
    if let Some(max_pages) = scraper_config.max_pages {
        let limited: Vec<_> = urls.iter().take(max_pages).cloned().collect();
        if limited.len() < urls.len() {
            tracing::info!(
                "Limiting to {} pages (max_pages={}), skipping {} URLs",
                limited.len(),
                max_pages,
                urls.len() - limited.len()
            );
        }
        limited
    } else {
        urls.to_vec()
    }
}

/// Scrape a single URL, reporting progress and enforcing robots.txt.
///
/// Returns `Ok(Some(content))` on success, `Ok(None)` when the URL is blocked
/// by robots.txt (skipped, not a failure), and `Err(e)` on a scrape failure.
async fn scrape_one_url(
    url: &Url,
    ctx: &ScrapeContext<'_>,
    opts: &CrawlOptions,
    observer: &dyn ProgressObserver,
    page_correlation: &CorrelationId,
    root_correlation: &CorrelationId,
) -> Result<Option<ScrapedContent>, crate::error::ScraperError> {
    // SSRF entry guard (F-06 + F-32, #1217): reject literal-IP seeds with a
    // typed error BEFORE robots.txt or page fetches open any
    // socket. Same shared choke-point check the downloader and MCP use.
    if let Err(rejection) = crate::domain::ssrf_guard::reject_forbidden_literal_url(url) {
        return Err(crate::error::ScraperError::invalid_url(
            rejection.to_string(),
        ));
    }
    let url_str = url.as_str();
    let _url_host = url.host_str().unwrap_or("unknown").to_string();

    observer.on_page_started(url_str).await;

    // Robots.txt enforcement — skip disallowed URLs unless --ignore-robots.
    // #1329: the typed verdict names the cause; the entry guard above already
    // rejected literal-IP seeds with the CLI's `invalid_url` error, so a
    // `PolicyRefused` here is defensive — surface the guard's real cause
    // instead of a phantom robots skip.
    if !opts.crawl.ignore_robots {
        let domain = url.host_str().unwrap_or("unknown");
        match ctx.robots_fetcher.is_allowed(url_str, domain).await {
            RobotsDecision::Allowed => {},
            RobotsDecision::RulesDenied => {
                info!("Blocked by robots.txt: {}", url_str);
                observer.on_robots_blocked(url_str).await;
                return Ok(None);
            },
            RobotsDecision::PolicyRefused(forbidden) => {
                return Err(crate::error::ScraperError::Network(Box::new(forbidden)));
            },
        }
    }

    observer
        .on_status_changed(url_str, ScrapeStatus::Fetching)
        .await;

    // F-05 (#1229 slice 2): reuse the discovery-captured body when
    // present — one HTTP request per page. SSRF and robots above still
    // apply; a miss (cap tripped, uncaptured URL) falls back to a normal
    // fetch through the same post-processing below. No `PageSource` seam:
    // the branch is a plain cache lookup at the call site.
    let outcome = if let Some(html) = ctx.captured_bodies.get(url.as_str()) {
        crate::application::crawler::extract_content(
            html,
            url,
            ctx.scraper_config,
            ctx.downloader,
            ctx.engine,
            page_correlation,
        )
        .await
    } else {
        scrape_single_url(
            ctx.router,
            url,
            ctx.scraper_config,
            ctx.downloader,
            ctx.engine,
            None,
            page_correlation,
            root_correlation,
        )
        .await
    };
    match outcome {
        Ok(mut content) => {
            observer
                .on_status_changed(url_str, ScrapeStatus::Extracting)
                .await;
            // Extraction failure fingerprinting (#792 Slice B): a low-quality
            // extraction that produced an honest hint is recorded against its
            // site/selector pair, and the accumulated failure count is attached
            // back to the hint. Recording failures never fail the scrape —
            // persistence is best-effort observability, not a data path.
            record_extraction_fingerprint(
                url,
                ctx.fingerprint_repo.as_deref(),
                &ctx.scraper_config.selector,
                &mut content,
            )
            .await;
            let chars = content.content.chars().count();
            observer.on_page_completed(url_str, chars).await;
            Ok(Some(content))
        },
        Err(e) => {
            let url_str = url.as_str().to_string();
            log_scrape_error_with_cause(
                &e,
                &url_str,
                "scrape",
                Some(page_correlation),
                "page scrape failed",
            );
            // ScraperError doesn't impl Clone, so we format for the observer
            // and keep the original for the failures vec (needed for error chain display).
            let scrape_err = ScrapeError::Other(format!("{e}"));
            observer.on_page_failed(&url_str, &scrape_err).await;
            Err(e)
        },
    }
}

fn build_http_client_config(
    opts: &CrawlOptions,
) -> Result<HttpClientConfig, crate::domain::UnknownProfileError> {
    Ok(HttpClientConfig {
        max_retries: opts.network.max_retries,
        backoff_base_ms: opts.network.backoff_base_ms,
        backoff_max_ms: opts.network.backoff_max_ms,
        accept_language: opts.network.accept_language.clone(),
        user_agent: opts.network.user_agent.clone(),
        timeout_secs: opts.network.timeout_secs,
        tls_emulation: HttpClientConfig::profile_from_name(&opts.network.h2_profile)?,
        ignore_waf: opts.crawl.ignore_waf,
        custom_headers: opts.network.custom_headers.clone(),
        ..HttpClientConfig::default()
    })
}

#[cfg(test)]
#[path = "scrape_flow_tests.rs"]
mod tests;

// ===== extraction fingerprint wiring tests (#792 Slice B) =====

#[cfg(test)]
#[path = "scrape_flow_fingerprint_tests.rs"]
mod fingerprint_wiring;

#[cfg(test)]
#[path = "scrape_flow_rate_limit_tests.rs"]
mod rate_limit_tests;
