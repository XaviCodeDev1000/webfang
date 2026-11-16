//! Resolution-failure coverage for the batch processor (#2823).
//!
//! Extracted from `processor.rs` to keep that grandfathered file from growing:
//! a pure move, no behaviour change.

use super::*;
use crate::application::batch::{BatchJob, BatchProgress};
use crate::domain::CrawlerConfig;
use std::sync::Arc;
use url::Url;

// =====================================================================
// #2823 — a resolution failure is its own population
// =====================================================================

/// A `CrawlError` that carries the `dns_resolution` cause, the shape the
/// fetch stack produces when the resolver never answers.
fn resolution_error() -> CrawlError {
    CrawlError::Network {
        message: "no se pudo resolver el nombre del dominio".to_string(),
        status_code: None,
        source: Some(Box::new(
            crate::domain::error::cause::ClassifiedNetworkError::new(
                crate::domain::error::cause::NetworkCause::DnsResolution,
                Box::new(std::io::Error::other("resolver down")),
            ),
        )),
    }
}

/// A `CrawlError` the SITE produced: the population that must keep its
/// historical behaviour untouched (#2823, AC3).
fn site_refusal_error() -> CrawlError {
    CrawlError::WafChallenge {
        provider: "cloudflare".to_string(),
        url: "https://example.com".to_string(),
        kind: crate::domain::error::WafDetectionKind::BodySignature,
    }
}

/// The streak aborts at the threshold, and a site refusal breaks it.
///
/// The two facts together are the acceptance criterion: a run whose
/// resolver is down stops dispatching, while a run the site is blocking
/// keeps going exactly as before.
#[test]
fn resolution_streak_aborts_at_the_threshold_and_a_refusal_breaks_it() {
    let progress = BatchProgress::new(64);
    let mut errors = Vec::new();
    let root = CorrelationId::new();
    let mut tally = BatchTally::default();

    for i in 1..RESOLUTION_ABORT_THRESHOLD {
        progress.start_one();
        let abort = record_batch_outcome(
            &mut tally,
            &progress,
            &mut errors,
            Ok((format!("https://dead-{i}.test/"), Err(resolution_error()))),
            &root,
        );
        assert!(
            !abort,
            "the run must survive {i} resolution failures (threshold {RESOLUTION_ABORT_THRESHOLD})"
        );
    }
    progress.start_one();
    let abort = record_batch_outcome(
        &mut tally,
        &progress,
        &mut errors,
        Ok((
            "https://dead-last.test/".to_string(),
            Err(resolution_error()),
        )),
        &root,
    );
    assert!(
        abort,
        "the threshold-th consecutive resolution failure must abort the run"
    );
    assert_eq!(tally.resolution_failures, RESOLUTION_ABORT_THRESHOLD);
    assert_eq!(
        tally.block_failures, 0,
        "a resolution failure is not a site refusal"
    );

    // A site refusal in the middle is a DIFFERENT population: it resets
    // the streak instead of counting towards the abort.
    let progress = BatchProgress::new(64);
    let mut errors = Vec::new();
    let mut tally = BatchTally::default();
    for i in 0..RESOLUTION_ABORT_THRESHOLD - 1 {
        progress.start_one();
        record_batch_outcome(
            &mut tally,
            &progress,
            &mut errors,
            Ok((format!("https://dead-{i}.test/"), Err(resolution_error()))),
            &root,
        );
    }
    progress.start_one();
    let abort = record_batch_outcome(
        &mut tally,
        &progress,
        &mut errors,
        Ok((
            "https://blocked.test/".to_string(),
            Err(site_refusal_error()),
        )),
        &root,
    );
    assert!(!abort, "a site refusal must not abort the run");
    assert_eq!(tally.block_failures, 1);
    assert_eq!(tally.resolution_failures, RESOLUTION_ABORT_THRESHOLD - 1);

    for i in 0..RESOLUTION_ABORT_THRESHOLD - 1 {
        progress.start_one();
        let abort = record_batch_outcome(
            &mut tally,
            &progress,
            &mut errors,
            Ok((
                format!("https://dead-again-{i}.test/"),
                Err(resolution_error()),
            )),
            &root,
        );
        assert!(
                !abort,
                "the refusal reset the streak, so {RESOLUTION_ABORT_THRESHOLD} - 1 new failures must not abort"
            );
    }
    assert_eq!(
        tally.resolution_failures,
        2 * (RESOLUTION_ABORT_THRESHOLD - 1),
        "both populations are counted apart"
    );
}

/// A downloader whose every fetch is a resolution failure — the injected
/// resolver the issue asks the test to drive (#2823, AC4).
#[derive(Debug)]
struct DeadResolverDownloader;

impl crate::domain::downloader_port::Downloader for DeadResolverDownloader {
    fn fetch<'a>(
        &'a self,
        _url: &'a Url,
    ) -> futures::future::BoxFuture<
        'a,
        Result<
            crate::domain::downloader_port::FetchedPage,
            crate::domain::downloader_port::DownloadError,
        >,
    > {
        Box::pin(async {
            Err(crate::domain::error::mapping::attach_cause(
                crate::domain::downloader_port::DownloadError::dns("the resolver did not answer"),
                crate::domain::error::cause::NetworkCause::DnsResolution,
            ))
        })
    }

    fn supports_interactions(&self) -> bool {
        false
    }

    fn memory_cost(&self) -> usize {
        0
    }
}

/// Factory that always hands back the same injected downloader.
#[derive(Debug)]
struct FixedDownloaderFactory;

impl crate::domain::downloader_factory::DownloaderFactory for FixedDownloaderFactory {
    fn build(
        &self,
        _spec: &crate::domain::downloader_factory::DownloaderSpec,
        _cookie_bridge: Arc<tokio::sync::RwLock<crate::domain::cookie_bridge::CookieBridge>>,
        _cancel_token: CancellationToken,
    ) -> Result<
        Arc<dyn crate::domain::downloader_port::Downloader>,
        crate::domain::downloader_port::DownloadError,
    > {
        Ok(Arc::new(DeadResolverDownloader))
    }
}

/// End to end: a batch pointed at a resolver that never answers stops
/// dispatching and reports the two populations apart (#2823).
///
/// No network and no real DNS: the resolver is injected through the
/// downloader seam, so the outcome is deterministic.
#[cfg(not(miri))] // wreq/btls-sys FFI is not supported by Miri
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn batch_aborts_when_the_resolver_is_down_and_reports_both_populations() {
    let urls: Vec<String> = (0..40)
        .map(|i| format!("https://dead-resolver-{i}.test/"))
        .collect();
    let seed = Url::parse("https://dead-resolver-0.test/").unwrap();
    let base = CrawlerConfig::builder(seed)
        .max_depth(0)
        .max_pages(1)
        .timeout_secs(1)
        .ignore_robots(true)
        .build();

    let job = BatchJob::new("dns-storm".to_string(), urls, base);
    let processor = BatchProcessor::new(2)
        .unwrap()
        .with_downloader_factory(Arc::new(FixedDownloaderFactory));

    let result = processor.process_batch(job).await.unwrap();
    assert!(
        result.aborted_on_resolution,
        "a resolver that never answers must stop the run"
    );
    // `total` is the JOB size, not what was attempted: the abort is read
    // from how many URLs actually finished.
    assert_eq!(result.total, 40, "the job still describes every URL");
    assert!(
        result.failed < 40,
        "the run must NOT attempt all 40 URLs; it attempted {}",
        result.failed
    );
    assert!(
        result.failed <= RESOLUTION_ABORT_THRESHOLD + 2,
        "the abort must be prompt: attempted {} URLs for a threshold of {}",
        result.failed,
        RESOLUTION_ABORT_THRESHOLD
    );
    assert_eq!(
        result.block_failures, 0,
        "a resolution failure must never be reported as a site refusal"
    );
    assert!(
        result.resolution_failures >= RESOLUTION_ABORT_THRESHOLD,
        "every failure must land in the resolution counter, got {}",
        result.resolution_failures
    );
    assert_eq!(
        result.succeeded, 0,
        "no URL could succeed against a dead resolver"
    );
}
