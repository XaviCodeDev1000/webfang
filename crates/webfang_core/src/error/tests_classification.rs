#![allow(clippy::io_other_error)]
use super::*;
use crate::domain::error::CrawlError;

// ========================================================================
// ErrorMap V2: ErrorClass classify() tests
// ========================================================================

#[test]
fn test_classify_transient_retriable_connection_reset() {
    let err = ScraperError::Network(Box::new(std::io::Error::new(
        std::io::ErrorKind::ConnectionReset,
        "reset",
    )));
    assert_eq!(err.classify(), ErrorClass::TransientRetriable);
}

#[test]
fn test_classify_transient_retriable_5xx() {
    let err = ScraperError::http(503, "https://example.com");
    assert_eq!(err.classify(), ErrorClass::TransientRetriable);
}

#[test]
fn test_classify_transient_backoff_timeout() {
    let err = ScraperError::GlobalTimeout;
    assert_eq!(err.classify(), ErrorClass::TransientBackoff);
}

#[test]
fn test_classify_transient_backoff_slowloris() {
    let err = ScraperError::SlowlorisTimeout;
    assert_eq!(err.classify(), ErrorClass::TransientBackoff);
}

#[test]
fn test_classify_transient_backoff_429() {
    let err = ScraperError::http(429, "https://example.com");
    assert_eq!(err.classify(), ErrorClass::TransientBackoff);
}

#[test]
fn test_classify_permanent_fatal_invalid_url() {
    let err = ScraperError::invalid_url("bad url");
    assert_eq!(err.classify(), ErrorClass::PermanentFatal);
}

#[test]
fn test_classify_permanent_fatal_4xx() {
    let err = ScraperError::http(404, "https://example.com");
    assert_eq!(err.classify(), ErrorClass::PermanentFatal);
}

#[test]
fn test_classify_permanent_fatal_waf() {
    let err = ScraperError::waf_blocked("https://example.com", "Cloudflare");
    assert_eq!(err.classify(), ErrorClass::PermanentFatal);
}

// Matrix row 20: SitemapEmpty / SitemapNotFound are DomainRecoverable
// (empty/missing sitemap is a domain-content condition, not a bug),
// matching CrawlError::classify. The typed exit-2 override in
// cli/error.rs (`empty_discovery_exit_for`) matches variants directly
// and is unaffected by this class.
#[test]
fn test_classify_domain_recoverable_sitemap_empty() {
    let err = ScraperError::SitemapEmpty;
    assert_eq!(err.classify(), ErrorClass::DomainRecoverable);
}

#[test]
fn test_classify_domain_recoverable_sitemap_not_found() {
    let err = ScraperError::SitemapNotFound("https://example.com/sitemap.xml".to_string());
    assert_eq!(err.classify(), ErrorClass::DomainRecoverable);
}

// Triangulation: row 20's DomainRecoverable must survive the
// `From<CrawlError>` conversion — both layers classify the same class,
// while neighboring row-15 variants keep PermanentFatal.
#[test]
fn test_classify_sitemap_class_survives_crawl_error_conversion() {
    assert_eq!(
        ScraperError::from(CrawlError::SitemapEmpty).classify(),
        ErrorClass::DomainRecoverable
    );
    assert_eq!(
        ScraperError::from(CrawlError::SitemapNotFound("https://example.com".into())).classify(),
        ErrorClass::DomainRecoverable
    );
    assert_eq!(
        ScraperError::from(CrawlError::InvalidUrl("bad".into())).classify(),
        ErrorClass::PermanentFatal
    );
}

// Matrix rows 9/11 (#957): crawl-budget limits — max-depth, max-pages and
// sitemap-depth — are DomainRecoverable. `CrawlLimit` is the shared
// pass-through target of `From<CrawlError>` for all three, so classifying
// it `PermanentFatal` inverted the class for every sibling.
#[test]
fn test_classify_domain_recoverable_crawl_limit() {
    let err = ScraperError::CrawlLimit("maximum pages 100 exceeded".to_string());
    assert_eq!(err.classify(), ErrorClass::DomainRecoverable);
}

// Matrix rows 11/12 (#957): each `ResourceExhausted` kind keeps its own
// class across the layer boundary — sitemap budgets are by-design stops
// (row 11), RamBudget backpressure resolves itself when memory frees
// (row 12). Flattening every kind to `Internal` made both InternalFatal.
#[test]
fn test_classify_resource_exhausted_class_survives_crawl_error_conversion() {
    assert_eq!(
        ScraperError::from(CrawlError::ResourceExhausted {
            resource: crate::domain::error::ResourceKind::RamBudget,
            limit: 1024,
            actual: 2048,
        })
        .classify(),
        ErrorClass::TransientBackoff
    );
    assert_eq!(
        ScraperError::from(CrawlError::ResourceExhausted {
            resource: crate::domain::error::ResourceKind::SitemapDepth,
            limit: 10,
            actual: 11,
        })
        .classify(),
        ErrorClass::DomainRecoverable
    );
}

// Triangulation: the row-9/11/12 budget siblings survive `From<CrawlError>`
// with the matrix class, while the row-13 backpressure bug and the row-15
// bad input keep their fatal classes (no blanket widening).
#[test]
fn test_classify_budget_class_survives_crawl_error_conversion() {
    assert_eq!(
        ScraperError::from(CrawlError::SitemapDepthExceeded).classify(),
        ErrorClass::DomainRecoverable
    );
    assert_eq!(
        ScraperError::from(CrawlError::MaxDepthExceeded { current: 5, max: 3 }).classify(),
        ErrorClass::DomainRecoverable
    );
    assert_eq!(
        ScraperError::from(CrawlError::MaxPagesExceeded { max: 100 }).classify(),
        ErrorClass::DomainRecoverable
    );
    assert_eq!(
        ScraperError::from(CrawlError::SemaphoreInanition).classify(),
        ErrorClass::InternalFatal
    );
    assert_eq!(
        ScraperError::from(CrawlError::InvalidUrl("bad".into())).classify(),
        ErrorClass::PermanentFatal
    );
}

#[test]
fn test_classify_internal_fatal_semaphore() {
    let err = ScraperError::SemaphoreInanition;
    assert_eq!(err.classify(), ErrorClass::InternalFatal);
}

// Matrix row 22: non-transient io kinds (incl. `Other`) are PermanentFatal,
// not InternalFatal — the CLI maps them to the typed exit-74 override.
#[test]
fn test_classify_permanent_io_other_kind() {
    let err: ScraperError = std::io::Error::new(std::io::ErrorKind::Other, "test").into();
    assert_eq!(err.classify(), ErrorClass::PermanentFatal);
}

#[test]
fn test_classify_internal_fatal_persistence() {
    let err = ScraperError::persistence("disk full");
    assert_eq!(err.classify(), ErrorClass::InternalFatal);
}

// ========================================================================
// B-05: CrawlError → ScraperError → classify() integration tests
// ========================================================================

#[test]
fn test_crawl_rate_limited_classifies_as_transient_backoff() {
    let crawl_err = crate::domain::error::CrawlError::RateLimited(60);
    let scraper_err: ScraperError = crawl_err.into();
    assert_eq!(
        scraper_err.classify(),
        ErrorClass::TransientBackoff,
        "RateLimited maps to ScraperError::RateLimited → TransientBackoff"
    );
}

#[test]
fn test_crawl_timeout_classifies_as_transient_retriable() {
    let crawl_err = crate::domain::error::CrawlError::Timeout;
    let scraper_err: ScraperError = crawl_err.into();
    assert_eq!(
        scraper_err.classify(),
        ErrorClass::TransientRetriable,
        "Timeout maps to ScraperError::Network(TimedOut io::Error) → TransientRetriable"
    );
}

#[test]
fn test_crawl_connection_classifies_as_transient_retriable() {
    let crawl_err = crate::domain::error::CrawlError::Connection("refused".into());
    let scraper_err: ScraperError = crawl_err.into();
    assert_eq!(
        scraper_err.classify(),
        ErrorClass::TransientRetriable,
        "Connection maps to ScraperError::Network(ConnectionReset io::Error) → TransientRetriable"
    );
}

#[test]
fn test_scraper_rate_limited_classifies_as_transient_backoff() {
    // EC-RATE-LIMITED: the typed variant classifies as backoff-retriable.
    let err = ScraperError::RateLimited(30);
    assert_eq!(err.classify(), ErrorClass::TransientBackoff);
}

#[test]
fn test_rate_limited_payload_survives_conversion() {
    // EC-RATE-LIMITED: retry_after must survive From<CrawlError> intact.
    let crawl_err = crate::domain::error::CrawlError::RateLimited(120);
    let scraper_err: ScraperError = crawl_err.into();
    assert!(
        matches!(&scraper_err, ScraperError::RateLimited(120)),
        "retry_after payload must survive conversion, got: {scraper_err}"
    );
    // Display contract: byte-identical to CrawlError::RateLimited and the
    // former Internal message — keeps the integration "rate limited" guard
    // (tests/scraper_service_test.rs::test_mock_rate_limited_error) green.
    assert_eq!(scraper_err.to_string(), "rate limited, retry after 120s");
}

#[test]
fn test_timeout_maps_to_transient_io_source() {
    // EC-TIMEOUT: the Network source MUST be a synthetic io::Error with
    // ErrorKind::TimedOut so is_transient_network's downcast recognizes it.
    // A bare message would fall through to InternalFatal and defeat the fix.
    let crawl_err = crate::domain::error::CrawlError::Timeout;
    let scraper_err: ScraperError = crawl_err.into();
    match &scraper_err {
        ScraperError::Network(source) => {
            let io_err = source
                .downcast_ref::<std::io::Error>()
                .expect("Network source must downcast to io::Error for is_transient_network");
            assert_eq!(io_err.kind(), std::io::ErrorKind::TimedOut);
        },
        other => panic!("Timeout must map to ScraperError::Network, got: {other}"),
    }
}

#[test]
fn test_connection_preserves_message_and_transient() {
    // EC-CONNECTION: original message preserved, classifies TransientRetriable.
    let crawl_err = crate::domain::error::CrawlError::Connection("peer reset".into());
    let scraper_err: ScraperError = crawl_err.into();
    assert_eq!(scraper_err.classify(), ErrorClass::TransientRetriable);
    match &scraper_err {
        ScraperError::Network(source) => {
            let io_err = source
                .downcast_ref::<std::io::Error>()
                .expect("Network source must downcast to io::Error");
            assert_eq!(io_err.kind(), std::io::ErrorKind::ConnectionReset);
            assert!(
                io_err.to_string().contains("peer reset"),
                "original message must be preserved, got: {io_err}"
            );
        },
        other => panic!("Connection must map to ScraperError::Network, got: {other}"),
    }
}

#[test]
fn test_request_failed_classifies_as_internal_fatal() {
    // EC-REQUEST-FAILED: typed variant, classification unchanged (InternalFatal).
    let crawl_err = crate::domain::error::CrawlError::RequestFailed("build failed".to_string());
    let scraper_err: ScraperError = crawl_err.into();
    assert!(
        matches!(&scraper_err, ScraperError::Internal(m) if m == "build failed"),
        "RequestFailed must map to ScraperError::Internal, got: {scraper_err}"
    );
    assert_eq!(scraper_err.classify(), ErrorClass::InternalFatal);
}

#[test]
fn test_classification_matrix_unchanged() {
    // EC-REGRESSION-GUARD: pre-existing classifications must not drift.
    assert_eq!(
        ScraperError::http(429, "u").classify(),
        ErrorClass::TransientBackoff,
        "Http{{429}} → TransientBackoff"
    );
    assert_eq!(
        ScraperError::http(500, "u").classify(),
        ErrorClass::TransientRetriable,
        "Http{{>=500}} → TransientRetriable"
    );
    assert_eq!(
        ScraperError::http(503, "u").classify(),
        ErrorClass::TransientRetriable,
        "Http{{>=500}} → TransientRetriable"
    );
    assert_eq!(
        ScraperError::http(404, "u").classify(),
        ErrorClass::PermanentFatal,
        "Http{{other 4xx}} → PermanentFatal"
    );
    assert_eq!(
        ScraperError::http(403, "u").classify(),
        ErrorClass::PermanentFatal,
        "Http{{other 4xx}} → PermanentFatal"
    );
    assert_eq!(
        ScraperError::invalid_url("bad").classify(),
        ErrorClass::PermanentFatal,
        "InvalidUrl → PermanentFatal"
    );
    assert_eq!(
        ScraperError::waf_blocked("u", "Cloudflare").classify(),
        ErrorClass::PermanentFatal,
        "WafBlocked → PermanentFatal"
    );
    assert_eq!(
        ScraperError::Internal("x".into()).classify(),
        ErrorClass::InternalFatal,
        "Internal → InternalFatal"
    );
}

#[test]
fn test_crawl_http_403_classifies_as_permanent_fatal() {
    let crawl_err = crate::domain::error::CrawlError::Http {
        status: 403,
        url: "https://example.com".to_string(),
    };
    let scraper_err: ScraperError = crawl_err.into();
    assert_eq!(
        scraper_err.classify(),
        ErrorClass::PermanentFatal,
        "HTTP 403 should be PermanentFatal"
    );
}

#[test]
fn test_crawl_http_503_classifies_as_transient_retriable() {
    let crawl_err = crate::domain::error::CrawlError::Http {
        status: 503,
        url: "https://example.com".to_string(),
    };
    let scraper_err: ScraperError = crawl_err.into();
    assert_eq!(
        scraper_err.classify(),
        ErrorClass::TransientRetriable,
        "HTTP 503 should be TransientRetriable"
    );
}

#[test]
fn test_crawl_http_429_classifies_as_transient_backoff() {
    let crawl_err = crate::domain::error::CrawlError::Http {
        status: 429,
        url: "https://example.com".to_string(),
    };
    let scraper_err: ScraperError = crawl_err.into();
    assert_eq!(
        scraper_err.classify(),
        ErrorClass::TransientBackoff,
        "HTTP 429 should be TransientBackoff"
    );
}
