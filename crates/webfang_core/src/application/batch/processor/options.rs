//! Per-URL configuration and execution for the batch processor.
//!
//! Builds the seed-only [`CrawlerConfig`], the run-scoped rate limiter and the
//! shared [`EngineOptions`], then runs one URL through the explicit engine
//! entry.
//!
//! Extracted from `processor.rs` to keep that grandfathered file from growing:
//! a pure move, no behaviour change.

use std::sync::Arc;

use tracing::{info, warn};

use crate::application::crawler::content_sink::CrawlContentSink;
use crate::application::crawler::engine::EngineOptions;
use crate::application::rate_limiter::{RateLimiterConfig, SharedRateLimiter};
use crate::domain::budget::detector::SystemDetector;
use crate::domain::budget::BudgetModel;
use crate::domain::downloader_factory::DownloaderFactory;
use crate::domain::error::cause::{ClassifiedNetworkError, NetworkCause};
use crate::domain::{CorrelationId, CrawlError, CrawlErrorCategory, CrawlerConfig, JsStrategy};
use crate::ValidUrl;

/// Build the per-URL [`CrawlerConfig`] used by [`process_single_url`].
///
/// #1215 (F-38): batch SCRAPES each input URL — exactly one page per URL.
/// The base config carries the CLI crawl budget (`max_depth`/`max_pages`),
/// which must NOT expand batch seeds: feeding it through verbatim turned
/// every seed into a BFS crawl and duplicated export records (F-02). Depth 0
/// fetches the seed only (link extraction is skipped once
/// `url_depth >= max_depth`); `max_pages 1` is the backstop so the seed
/// itself can never be budget-starved. Every other knob (patterns, robots,
/// TLS profile, timeouts, delay, budget overrides) still rides along.
pub(crate) fn build_per_url_config(
    url: &str,
    base_config: &CrawlerConfig,
) -> Result<CrawlerConfig, CrawlError> {
    let parsed_url =
        url::Url::parse(url).map_err(|e| CrawlError::InvalidUrl(format!("{url}: {e}")))?;
    // Apply URL hardening policy (scheme allow-list + credential strip) to batch-file/stdin URLs.
    // This closes audit finding F-R3-1 (batch entry parity with #1240/#1260).
    let valid_url = ValidUrl::try_from_url(parsed_url)
        .map_err(|e| CrawlError::InvalidUrl(format!("{url}: {e}")))?;

    Ok(CrawlerConfig::builder(valid_url.as_url().clone())
        .max_depth(0)
        .max_pages(1)
        .concurrency(base_config.concurrency)
        .delay_ms(base_config.delay_ms)
        .jitter_ms(base_config.jitter_ms)
        .timeout_secs(base_config.timeout_secs)
        .ignore_robots(base_config.ignore_robots)
        .tls_emulation(base_config.tls_emulation)
        .exclude_patterns(base_config.exclude_patterns.clone())
        .include_patterns(base_config.include_patterns.clone())
        // Bug R2-1: operator budget overrides staged on the base config
        // must survive the per-URL rebuild or the Engine re-derives auto
        // tiers, silently ignoring --concurrency / --rate-limit-burst.
        .budget_overrides(base_config.budget_overrides)
        .build())
}

/// Build the RUN-scoped token bucket shared by every URL of one batch run (#1428).
///
/// Each batch URL runs its own engine (`process_single_url` →
/// `crawl_site_with_options`), and every engine mints a fresh limiter with
/// a full burst — so without this shared bucket `--delay-ms` is inert on
/// the batch path: each URL starts with a full burst and cross-URL spacing
/// is structurally impossible. The bucket is built from the SAME two inputs
/// `Engine::run` uses — `delay_ms` as the refill period and the budget
/// model's independent burst tier — so batch pacing shares the crawl
/// cadence policy (AGENTS.md fetch guard-chain stage 2: pacing happens
/// BEFORE any network is touched).
///
/// `delay_ms == 0` AND `jitter_ms == 0` returns `None`: no bucket is
/// allocated and each engine keeps its historical per-engine limiter, which
/// keeps an unthrottled run identical to the pre-fix behavior (zero
/// overhead, zero drift) — the same contract as
/// `build_scrape_rate_limiter` (`scrape_flow.rs`). A jitter on its own IS a
/// pacing request, so it wires the bucket even with a zero delay (#2610).
///
/// A construction failure degrades to `None` with a WARN, mirroring the
/// scrape-path precedent. It is unreachable in practice:
/// `SharedRateLimiter::new` rejects only a zero period (clamped to 1 ms)
/// or a zero burst (the budget tier is non-zero by construction).
pub(crate) fn build_batch_rate_limiter(base_config: &CrawlerConfig) -> Option<SharedRateLimiter> {
    // A zero delay means "no pacing" — but only when the operator asked for
    // no pacing. A jitter on its own IS a pacing request, so the run-scoped
    // limiter is built even with a zero base delay (#2610): the gate then
    // spaces the requests over the clamped 1 ms floor, exactly like the
    // crawl path's `delay_ms.max(1)`.
    if base_config.delay_ms == 0 && base_config.jitter_ms == 0 {
        return None;
    }
    let burst = BudgetModel::build(base_config.budget_overrides, &SystemDetector)
        .burst()
        .get();
    match SharedRateLimiter::new(
        &RateLimiterConfig::new(base_config.delay_ms, burst).with_jitter(base_config.jitter_ms),
    ) {
        Ok(limiter) => {
            info!(
                delay_ms = base_config.delay_ms,
                jitter_ms = base_config.jitter_ms,
                burst,
                "batch rate limiter wired (run scope)"
            );
            Some(limiter)
        },
        Err(e) => {
            warn!(
            error = %e,
            delay_ms = base_config.delay_ms,
            "batch rate limiter unavailable — continuing with per-engine limiters"
            );
            None
        },
    }
}

/// Build the run-wide [`EngineOptions`] shared by every URL of one batch run.
///
/// #1369: the batch entry used to route through the knobless `crawl_site` /
/// `crawl_site_capturing` shims, which hard-wired the engine transport policy
/// inside the engine. The explicit seam makes each knob's source visible:
///
/// - `ignore_robots`: the batch's only robots source is the base
///   [`CrawlerConfig`] — `build_per_url_config` copies the same value into
///   every per-URL config — so the run-wide option mirrors it. Identical to
///   what `crawl_site_inner` derived per call before.
/// - `content_sink`: the shared sink attached via
///   [`BatchProcessor::with_content_sink`] (#631); `None` keeps the batch
///   metadata-only (bodies discarded — the historical zero-files gotcha).
/// - `checkpoint_path` / `session_pool_enabled` / `js_strategy` /
///   `downloader_factory`: the batch has no operator source for these, so
///   they are set **explicitly** to the values the old entry hardcoded
///   (off / off / static / `None`) — never silently via a drifted default.
///   The remaining transport scalars ride `EngineOptions::default()`, which
///   is field-for-field the old `crawl_site_inner` policy (Chrome145, 3
///   retries, 1s/10s backoff, 100-page checkpoint interval).
///
/// Built once per run and cloned per URL task.
///
/// NOTE: the run-scoped pacer lives OUTSIDE this bag — see
/// [`build_batch_rate_limiter`], wired onto the built options by
/// `process_batch_cancellable`. The split keeps this constructor a pure
/// DTO build (its seam test pins every knob source) while the limiter
/// documents its own budget-derived source separately.
pub(crate) fn build_batch_engine_options(
    base_config: &CrawlerConfig,
    content_sink: Option<Arc<dyn CrawlContentSink>>,
    downloader_factory: Option<Arc<dyn DownloaderFactory>>,
) -> EngineOptions {
    EngineOptions {
        checkpoint_path: None,
        session_pool_enabled: false,
        ignore_robots: base_config.ignore_robots,
        js_strategy: JsStrategy::Static,
        // `None` in production: the batch keeps the static `fetch_url` path.
        downloader_factory,
        content_sink,
        ..Default::default()
    }
}

/// Process a single URL by creating a CrawlerConfig and running the explicit
/// engine entry with the run-wide options and the job's run-root
///
/// Creates a new seed-only `CrawlerConfig` for the given URL (#1215: one
/// page per URL, never a BFS expansion) and captures the fetched body into
/// the shared sink when one is attached (carried on `options.content_sink`,
/// #1369). `correlation` (#1439) is the job's ONE run-root, adopted by the
/// crawl engine so every URL of the batch shares the operation's identity.
///
/// Returns `Err(CrawlError)` if the crawl result has any errors (e.g., timeouts),
/// ensuring the batch processor correctly counts failed URLs.
pub(crate) async fn process_single_url(
    url: &str,
    base_config: CrawlerConfig,
    options: EngineOptions,
    correlation: &CorrelationId,
) -> Result<crate::domain::CrawlResult, CrawlError> {
    let config = build_per_url_config(url, &base_config)?;

    let result =
        crate::application::crawler::engine::crawl_site_with_options(config, options, correlation)
            .await?;

    // Treat any crawl errors (timeouts, etc.) as failures for batch processing,
    // but preserve severity (#537): the engine already partitioned them into
    // `error_breakdown` (issue #374). A genuinely-internal category (storage,
    // checkpoint, parse, panic) is a real bug and must stay `Internal` so the
    // run exits 3 (issue #537, phase 1). A purely transient/external failure
    // set (timeout, network, http, rate-limit, waf) must surface as a transient
    // `CrawlError` so it classifies `TransientRetriable`/`Backoff`/`Permanent`
    // and exits 69 — NOT as a bug (exit 3).
    if result.errors > 0 {
        let breakdown = &result.error_breakdown;

        // Defensive: errors reported without a category breakdown are treated
        // as internal failures (fail-safe → exit 3), matching the classify()
        // safety net for genuinely unknown errors.
        if breakdown.is_empty() {
            return Err(CrawlError::Internal(format!(
                "crawl completed with {} error(s)",
                result.errors
            )));
        }

        // A genuinely-internal category means a real bug. Mixed batches are
        // dominated by the worst severity, so any such category escalates to
        // exit 3 (matches `scraper_failure_for_internal_fatal` in the CLI).
        let has_internal_bug = [
            CrawlErrorCategory::Internal,
            CrawlErrorCategory::Extraction,
            CrawlErrorCategory::Panic,
        ]
        .iter()
        .any(|c| breakdown.get(c).copied().unwrap_or(0) > 0);
        if has_internal_bug {
            return Err(CrawlError::Internal(format!(
                "crawl completed with {} error(s)",
                result.errors
            )));
        }

        // #2823: a resolution failure is OUR resolver, not the site refusing
        // us. Surface it as its own typed failure so the batch can tell the two
        // populations apart and stop dispatching, instead of running the whole
        // list as if every URL were blocked. The cause rides in the source so
        // the classification stays by TYPE (#2784), never by message text.
        if result.resolution_failures > 0 {
            return Err(CrawlError::Network {
                message: format!(
                    "{}: {} URL(s) failed to resolve",
                    NetworkCause::DnsResolution.message(),
                    result.resolution_failures
                ),
                status_code: None,
                source: Some(Box::new(ClassifiedNetworkError::new(
                    NetworkCause::DnsResolution,
                    Box::new(std::io::Error::other(
                        "the resolver did not answer".to_string(),
                    )),
                ))),
            });
        }

        // Purely transient/external: surface as a transient error → exit 69.
        // A timeout is the most common batch failure here.
        if breakdown
            .get(&CrawlErrorCategory::Timeout)
            .copied()
            .unwrap_or(0)
            > 0
        {
            return Err(CrawlError::Timeout);
        }
        return Err(CrawlError::Connection {
            message: format!("crawl completed with {} transient error(s)", result.errors),
            source: None,
        });
    }

    Ok(result)
}
