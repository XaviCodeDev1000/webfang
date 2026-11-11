use super::*;

// ========================================================================
// Phase 2: Error Stratification — CrawlError → ScraperError From tests
// (Issue #508: structured→Internal flattening variants)
// ========================================================================

#[test]
fn test_crawl_error_parse_flattens_to_internal_preserving_msg() {
    let crawl_err = crate::domain::error::CrawlError::Parse("html5 failed".to_string());
    let scraper_err: ScraperError = crawl_err.into();
    assert!(
        matches!(&scraper_err, ScraperError::Internal(m) if m == "parse: html5 failed"),
        "Parse must flatten to ScraperError::Internal(\"parse: <msg>\"), got: {scraper_err}"
    );
    let rendered = scraper_err.to_string();
    assert!(
        rendered.contains("parse"),
        "Display must keep the parse category, got: {rendered}"
    );
    assert!(
        rendered.contains("html5 failed"),
        "Display must preserve the original message, got: {rendered}"
    );
}

#[test]
fn test_crawl_error_storage_flattens_to_internal_preserving_msg() {
    let crawl_err = crate::domain::error::CrawlError::Storage("disk full".to_string());
    let scraper_err: ScraperError = crawl_err.into();
    assert!(
        matches!(&scraper_err, ScraperError::Internal(m) if m == "storage: disk full"),
        "Storage must flatten to ScraperError::Internal(\"storage: <msg>\"), got: {scraper_err}"
    );
    let rendered = scraper_err.to_string();
    assert!(rendered.contains("storage"), "missing category: {rendered}");
    assert!(rendered.contains("disk full"), "missing msg: {rendered}");
}

#[test]
fn test_crawl_error_checkpoint_flattens_to_internal_preserving_msg() {
    let crawl_err = crate::domain::error::CrawlError::Checkpoint("CRC mismatch".to_string());
    let scraper_err: ScraperError = crawl_err.into();
    assert!(
        matches!(&scraper_err, ScraperError::Internal(m) if m == "checkpoint: CRC mismatch"),
        "Checkpoint must flatten to ScraperError::Internal(\"checkpoint: <msg>\"), got: {scraper_err}"
    );
    let rendered = scraper_err.to_string();
    assert!(
        rendered.contains("checkpoint"),
        "missing category: {rendered}"
    );
    assert!(rendered.contains("CRC mismatch"), "missing msg: {rendered}");
}

#[test]
fn test_crawl_error_session_pool_flattens_to_internal_preserving_msg() {
    let crawl_err =
        crate::domain::error::CrawlError::SessionPool("no sessions available".to_string());
    let scraper_err: ScraperError = crawl_err.into();
    assert!(
        matches!(&scraper_err, ScraperError::Internal(m) if m == "session pool: no sessions available"),
        "SessionPool must flatten to ScraperError::Internal(\"session pool: <msg>\"), got: {scraper_err}"
    );
    let rendered = scraper_err.to_string();
    assert!(
        rendered.contains("session pool"),
        "missing category: {rendered}"
    );
    assert!(
        rendered.contains("no sessions available"),
        "missing msg: {rendered}"
    );
}

#[test]
fn test_crawl_error_discovery_flattens_to_internal_preserving_msg() {
    let crawl_err = crate::domain::error::CrawlError::Discovery("robots.txt 403".to_string());
    let scraper_err: ScraperError = crawl_err.into();
    assert!(
        matches!(&scraper_err, ScraperError::Internal(m) if m == "discovery: robots.txt 403"),
        "Discovery must flatten to ScraperError::Internal(\"discovery: <msg>\"), got: {scraper_err}"
    );
    let rendered = scraper_err.to_string();
    assert!(
        rendered.contains("discovery"),
        "missing category: {rendered}"
    );
    assert!(
        rendered.contains("robots.txt 403"),
        "missing msg: {rendered}"
    );
}

#[test]
fn test_crawl_error_retry_exhausted_flattens_to_internal_preserving_fields() {
    let crawl_err = crate::domain::error::CrawlError::RetryExhausted {
        url: "https://x.com".to_string(),
        attempts: 3,
    };
    let scraper_err: ScraperError = crawl_err.into();
    assert!(
        matches!(
            &scraper_err,
            ScraperError::Internal(m) if m == "retry exhausted for https://x.com after 3 attempts"
        ),
        "RetryExhausted must flatten with url + attempts in the message, got: {scraper_err}"
    );
    let rendered = scraper_err.to_string();
    assert!(
        rendered.contains("https://x.com"),
        "missing url: {rendered}"
    );
    assert!(rendered.contains('3'), "missing attempts count: {rendered}");
    assert!(rendered.contains("retry"), "missing category: {rendered}");
}

#[test]
fn test_crawl_error_url_excluded_flattens_to_internal_preserving_url() {
    let crawl_err = crate::domain::error::CrawlError::UrlExcluded("https://spam.com".to_string());
    let scraper_err: ScraperError = crawl_err.into();
    assert!(
        matches!(&scraper_err, ScraperError::Internal(m) if m == "URL excluded: https://spam.com"),
        "UrlExcluded must flatten to ScraperError::Internal(\"URL excluded: <url>\"), got: {scraper_err}"
    );
    let rendered = scraper_err.to_string();
    assert!(
        rendered.contains("excluded"),
        "missing category: {rendered}"
    );
    assert!(
        rendered.contains("https://spam.com"),
        "missing url: {rendered}"
    );
}

#[test]
fn test_crawl_error_invalid_content_type_flattens_to_internal_preserving_ct() {
    let crawl_err =
        crate::domain::error::CrawlError::InvalidContentType("application/x-binary".to_string());
    let scraper_err: ScraperError = crawl_err.into();
    assert!(
        matches!(&scraper_err, ScraperError::Internal(m) if m == "invalid content type: application/x-binary"),
        "InvalidContentType must flatten to ScraperError::Internal(\"invalid content type: <ct>\"), got: {scraper_err}"
    );
    let rendered = scraper_err.to_string();
    assert!(
        rendered.contains("content type"),
        "missing category: {rendered}"
    );
    assert!(
        rendered.contains("application/x-binary"),
        "missing content type: {rendered}"
    );
}

#[test]
fn test_crawl_error_request_failed_passes_through_to_internal_unchanged() {
    // RequestFailed(msg) is documented as "already Internal" — verify the
    // From impl does NOT double-prefix it (regression guard for #508).
    let crawl_err =
        crate::domain::error::CrawlError::RequestFailed("upstream returned 502".to_string());
    let scraper_err: ScraperError = crawl_err.into();
    assert!(
        matches!(&scraper_err, ScraperError::Internal(m) if m == "upstream returned 502"),
        "RequestFailed must pass through verbatim, got: {scraper_err}"
    );
    let rendered = scraper_err.to_string();
    assert!(
        !rendered.contains("request failed:"),
        "must NOT prefix the message, got: {rendered}"
    );
}

#[test]
fn test_crawl_error_resource_exhausted_passes_through_preserving_fields() {
    let crawl_err = crate::domain::error::CrawlError::ResourceExhausted {
        resource: crate::domain::error::ResourceKind::RamBudget,
        limit: 512,
        actual: 1024,
    };
    let scraper_err: ScraperError = crawl_err.into();
    assert!(
        matches!(
            &scraper_err,
            ScraperError::ResourceExhausted {
                resource: crate::domain::error::ResourceKind::RamBudget,
                limit: 512,
                actual: 1024,
            }
        ),
        "ResourceExhausted must keep its typed kind + budget fields, got: {scraper_err}"
    );
    let rendered = scraper_err.to_string();
    assert!(
        rendered.contains("resource exhausted"),
        "missing category: {rendered}"
    );
    assert!(rendered.contains("512"), "missing limit: {rendered}");
    assert!(rendered.contains("1024"), "missing actual: {rendered}");
}
