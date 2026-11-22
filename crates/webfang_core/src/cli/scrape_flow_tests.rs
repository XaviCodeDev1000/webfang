use super::{apply_resume_mode, build_http_client_config, scrape_urls};
use crate::application::crawl_options::CrawlOptions;
use crate::infrastructure::crawler::robots_utils::RobotsFetcher;
use std::num::NonZeroUsize;
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;
use url::Url;
use wiremock::matchers::{method, path};
use wiremock::{Mock, ResponseTemplate};

/// Pre-v2 envelope: one processed URL, no `version` key — what makes it
/// v1. Loading it routes `RecordStore::load` through `migrate_v1`, whose
/// backup-first `fs::copy` is `copy_file_range` (326), which Miri cannot
/// execute (upstream miri#4183). Its two consumers are gated per test.
const V1_ONE_PROCESSED_URL: &str = r#"{"domain":"example.com","processed_urls":["https://example.com/a"],"last_export":null,"total_exported":1}"#;

// ===== scrape-path concurrency derives from the budget model (task 2.5a) =====

fn fixed_cores(n: usize) -> crate::domain::budget::detector::FixedDetector {
    crate::domain::budget::detector::FixedDetector::with_detection(
        NonZeroUsize::new(n).expect("test core counts are non-zero"),
        None,
    )
}

/// The scrape bound must follow the INJECTED detector's crawl tier —
/// never the host's `available_parallelism` and never the raw CLI flag.
#[test]
fn scrape_concurrency_follows_injected_detector_crawl_tier() {
    let opts = CrawlOptions::default();

    // Auto table: 6 cores ⇒ 5, ≥9 cores ⇒ min(cores−1, 8) = 8.
    assert_eq!(super::scrape_concurrency(&opts, &fixed_cores(6)), 5);
    assert_eq!(super::scrape_concurrency(&opts, &fixed_cores(9)), 8);
}

// ===== shutdown tests (#653) =====

#[cfg_attr(miri, ignore)] // btls/wreq FFI (BoringSSL TLS_method) not supported by Miri
#[tokio::test]
async fn a_cancelled_run_scrapes_nothing() {
    // Regression for #653: a shutdown signal must stop new page fetches.
    // The URLs point at a closed port — if any were actually fetched they
    // would land in `failures` instead of being silently skipped.
    let urls: Vec<Url> = (0..4)
        .map(|i| Url::parse(&format!("http://127.0.0.1:1/{i}")).expect("loopback URL must parse"))
        .collect();
    let opts = CrawlOptions {
        crawl: crate::application::crawl_options::CrawlLimits {
            ignore_robots: true,
            ..Default::default()
        },
        ..Default::default()
    };
    let cancel = CancellationToken::new();
    cancel.cancel();

    let (results, failures, blocked) = scrape_urls(
        &urls,
        &crate::domain::config::ScraperConfig::default(),
        &opts,
        &crate::application::progress_observer::NoopObserver,
        None,
        None,
        &crate::domain::CorrelationId::new(),
        &cancel,
        &[],
    )
    .await
    .expect("setup must succeed even when cancelled");

    assert!(results.is_empty(), "no page may be scraped after shutdown");
    assert!(
        failures.is_empty(),
        "skipped URLs are not failures, got: {failures:?}"
    );
    assert_eq!(blocked, 0, "shutdown skips are not robots-blocks");
}

// ===== robots-blocked counting tests (#705) =====

/// A URL disallowed by robots.txt is neither a result nor a failure: it
/// lands in the dedicated blocked counter so the orchestrator can route
/// all-blocked runs to exit 77 (#705).
#[cfg_attr(miri, ignore)] // btls/wreq FFI (BoringSSL TLS_method) not supported by Miri
#[tokio::test]
async fn robots_blocked_urls_are_counted_not_failed() {
    // Entry-guard allowance (F-06 + F-32, #1217): the seed below is a
    // wiremock loopback literal, which production now rejects at entry.
    let _guard = webfang_test_utils::EnvGuard::with(&[(
        crate::domain::ssrf_guard::DISABLE_ENTRY_GUARD_ENV,
        "1",
    )]);
    let server = wiremock::MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/robots.txt"))
        .respond_with(ResponseTemplate::new(200).set_body_string("User-agent: *\nDisallow: /\n"))
        .mount(&server)
        .await;

    let urls = vec![Url::parse(&format!("{}/page", server.uri())).expect("valid URL")];
    let opts = CrawlOptions::default(); // ignore_robots: false
    let cancel = CancellationToken::new();

    let (results, failures, blocked) = scrape_urls(
        &urls,
        &crate::domain::config::ScraperConfig::default(),
        &opts,
        &crate::application::progress_observer::NoopObserver,
        None,
        None,
        &crate::domain::CorrelationId::new(),
        &cancel,
        &[],
    )
    .await
    .expect("setup must succeed");

    assert!(results.is_empty(), "blocked URL must not be scraped");
    assert!(
        failures.is_empty(),
        "robots-blocked URLs are not failures, got: {failures:?}"
    );
    assert_eq!(blocked, 1, "blocked URL must be counted");
}

// ===== build_http_client_config tests =====

#[test]
fn build_http_client_config_uses_opts_timeout_secs() {
    let mut opts = CrawlOptions::default();
    opts.network.timeout_secs = 7;

    let config = build_http_client_config(&opts).unwrap();

    assert_eq!(config.timeout_secs, 7);
    assert_eq!(config.max_retries, opts.network.max_retries);
    assert_eq!(config.backoff_base_ms, opts.network.backoff_base_ms);
    assert_eq!(config.backoff_max_ms, opts.network.backoff_max_ms);
    assert_eq!(config.accept_language, opts.network.accept_language);
}

#[test]
fn build_http_client_config_preserves_default_timeout_when_unset() {
    let opts = CrawlOptions::default();

    let config = build_http_client_config(&opts).unwrap();

    assert_eq!(config.timeout_secs, 30);
}

#[test]
fn build_http_client_config_propagates_ignore_waf() {
    // REQ-WAF-07: the bypass flag flows CrawlOptions -> HttpClientConfig so
    // the HTTP client builds InspectionContext with ignore_waf set.
    let mut opts = CrawlOptions::default();
    opts.crawl.ignore_waf = true;

    let config = build_http_client_config(&opts).unwrap();

    assert!(config.ignore_waf);
}

#[test]
fn build_http_client_config_maps_h2_profile_to_tls_emulation() {
    let mut opts = CrawlOptions::default();
    opts.network.h2_profile = "Chrome131".to_owned();

    let config = build_http_client_config(&opts).unwrap();

    assert_eq!(config.tls_emulation, wreq_util::Profile::Chrome131);
}

#[test]
fn build_http_client_config_rejects_unknown_profile() {
    let mut opts = CrawlOptions::default();
    opts.network.h2_profile = "Firefox".to_owned();

    let err = build_http_client_config(&opts).unwrap_err();

    assert_eq!(err.name, "Firefox");
}

// ===== robots tests =====

#[cfg_attr(miri, ignore)] // btls/wreq FFI (BoringSSL TLS_method) not supported by Miri
#[tokio::test]
async fn robots_cache_allows_public_urls() {
    let fetcher = RobotsFetcher::new(wreq_util::Profile::Chrome145, 30).unwrap();
    // No robots.txt for localhost → fail-open → allowed
    assert!(fetcher
        .is_allowed("http://localhost:18080/page", "localhost")
        .await
        .allows());
}

#[test]
fn ignore_robots_flag_defaults_to_false() {
    let opts = CrawlOptions::default();
    assert!(!opts.crawl.ignore_robots);
}

// ===== apply_resume_mode tests (via PersistenceMode) =====

#[tokio::test]
async fn apply_resume_mode_disabled_returns_all_urls() {
    let root = crate::domain::CorrelationId::new();
    let urls = vec![
        Url::parse("https://example.com/a").unwrap(),
        Url::parse("https://example.com/b").unwrap(),
    ];
    let mode = crate::domain::persistence::PersistenceMode::Disabled;

    let (filtered, state_store) =
        apply_resume_mode(urls.clone(), &mode, "https://example.com", &root)
            .await
            .expect("resume disabled should not fail");

    assert_eq!(filtered.len(), 2);
    assert!(state_store.is_none());
}

#[tokio::test]
async fn apply_resume_mode_checkpoint_returns_all_urls() {
    let root = crate::domain::CorrelationId::new();
    let urls = vec![
        Url::parse("https://example.com/a").unwrap(),
        Url::parse("https://example.com/b").unwrap(),
    ];
    let mode = crate::domain::persistence::PersistenceMode::Checkpoint {
        cfg: crate::domain::persistence::CheckpointCfg {
            dir: std::path::PathBuf::from("/tmp/chk"),
            interval: 100,
        },
    };

    let (filtered, state_store) =
        apply_resume_mode(urls.clone(), &mode, "https://example.com", &root)
            .await
            .expect("checkpoint only should not fail");

    assert_eq!(filtered.len(), 2);
    assert!(state_store.is_none());
}

#[cfg_attr(miri, ignore = "copy_file_range(326) unsupported by Miri")]
#[tokio::test]
async fn apply_resume_mode_skips_previously_scraped_urls() {
    let root = crate::domain::CorrelationId::new();
    let tmp = TempDir::new().unwrap();
    let state_dir = tmp.path().to_path_buf();

    // Pre-populate state with one processed URL
    std::fs::create_dir_all(&state_dir).unwrap();
    std::fs::write(state_dir.join("example.com.json"), V1_ONE_PROCESSED_URL).unwrap();

    let urls = vec![
        Url::parse("https://example.com/a").unwrap(),
        Url::parse("https://example.com/b").unwrap(),
        Url::parse("https://example.com/c").unwrap(),
    ];
    let mode = crate::domain::persistence::PersistenceMode::Resume {
        dir: state_dir.clone(),
    };

    let (filtered, state_store) = apply_resume_mode(urls, &mode, "https://example.com", &root)
        .await
        .expect("valid state dir should not fail");

    // URL "a" was already processed, should be skipped
    assert_eq!(filtered.len(), 2, "should skip 1 already-processed URL");
    assert!(
        !filtered
            .iter()
            .any(|u| u.as_str() == "https://example.com/a"),
        "processed URL should be filtered out"
    );
    assert!(
        state_store.is_some(),
        "should create state store when resume enabled"
    );
}

#[cfg_attr(miri, ignore = "copy_file_range(326) unsupported by Miri")]
#[tokio::test]
async fn apply_resume_mode_full_skips_previously_scraped_urls() {
    let root = crate::domain::CorrelationId::new();
    let tmp = TempDir::new().unwrap();
    let state_dir = tmp.path().to_path_buf();

    std::fs::create_dir_all(&state_dir).unwrap();
    std::fs::write(state_dir.join("example.com.json"), V1_ONE_PROCESSED_URL).unwrap();

    let urls = vec![
        Url::parse("https://example.com/a").unwrap(),
        Url::parse("https://example.com/b").unwrap(),
    ];
    let mode = crate::domain::persistence::PersistenceMode::Full {
        resume_dir: state_dir.clone(),
        checkpoint: crate::domain::persistence::CheckpointCfg {
            dir: state_dir.clone(),
            interval: 50,
        },
    };

    let (filtered, state_store) = apply_resume_mode(urls, &mode, "https://example.com", &root)
        .await
        .expect("Full mode should not fail");

    assert_eq!(filtered.len(), 1);
    assert!(state_store.is_some());
}

#[tokio::test]
async fn apply_resume_mode_with_corrupted_state_returns_all_urls() {
    let root = crate::domain::CorrelationId::new();
    let tmp = TempDir::new().unwrap();
    let state_dir = tmp.path().to_path_buf();

    // Write corrupted state file
    let state_file = state_dir.join("example.com.json");
    std::fs::create_dir_all(&state_dir).unwrap();
    std::fs::write(&state_file, "not valid json!!!").unwrap();

    let urls = vec![
        Url::parse("https://example.com/a").unwrap(),
        Url::parse("https://example.com/b").unwrap(),
    ];
    let mode = crate::domain::persistence::PersistenceMode::Resume {
        dir: state_dir.clone(),
    };

    let (filtered, state_store) =
        apply_resume_mode(urls.clone(), &mode, "https://example.com", &root)
            .await
            .expect("corrupted state file should not prevent store creation");

    // Corrupted state → fallback to all URLs (graceful degradation)
    assert_eq!(
        filtered.len(),
        2,
        "should return all URLs on corrupted state"
    );
    assert!(state_store.is_some());
}

#[tokio::test]
async fn apply_resume_mode_with_custom_state_dir() {
    let root = crate::domain::CorrelationId::new();
    let tmp = TempDir::new().unwrap();
    let state_dir = tmp.path().join("custom_state");
    std::fs::create_dir_all(&state_dir).unwrap();

    let urls = vec![Url::parse("https://example.com/a").unwrap()];
    let mode = crate::domain::persistence::PersistenceMode::Resume {
        dir: state_dir.clone(),
    };

    let (filtered, state_store) = apply_resume_mode(urls, &mode, "https://example.com", &root)
        .await
        .expect("custom state dir should not fail");

    assert_eq!(filtered.len(), 1);
    assert!(
        state_store.is_some(),
        "should create state store with custom dir"
    );
    // Verify state store uses custom dir
    let store = state_store.unwrap();
    let state_path = store.get_state_path();
    assert!(
        state_path.starts_with(&state_dir),
        "state path should be under custom state_dir: {state_path:?}"
    );
}

#[test]
fn resolve_default_state_dir_contains_webfang_state() {
    let dir = super::resolve_default_state_dir();
    // Normalize the OBSERVED separator so the expected constant stays
    // forward-slash and cross-platform (Windows: backslashes).
    let normalized = dir.to_string_lossy().replace('\\', "/");
    assert!(
        normalized.contains("webfang/state"),
        "default state dir should contain webfang/state, got: {dir:?}"
    );
}
