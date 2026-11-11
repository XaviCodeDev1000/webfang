#![allow(clippy::io_other_error)]
use super::*;

// ========================================================================
// Phase 2: Error Stratification — DomainError → ScraperError From tests
// ========================================================================

#[test]
fn test_domain_error_invalid_url_wraps_to_scraper() {
    let domain_err = crate::domain::error::DomainError::InvalidUrl("bad url".to_string());
    let scraper_err: ScraperError = domain_err.into();
    assert_eq!(
        scraper_err.to_string(),
        "URL inválida: bad url",
        "Spanish Display message must be preserved through From conversion"
    );
}

#[test]
fn test_domain_error_readability_wraps_to_scraper() {
    let domain_err = crate::domain::error::DomainError::Readability("parse failed".to_string());
    let scraper_err: ScraperError = domain_err.into();
    assert!(
        matches!(scraper_err, ScraperError::Readability(s) if s == "parse failed"),
        "DomainError::Readability must convert to ScraperError::Readability"
    );
}

#[test]
fn test_domain_error_extraction_wraps_to_scraper() {
    let domain_err = crate::domain::error::DomainError::Extraction("no content".to_string());
    let scraper_err: ScraperError = domain_err.into();
    assert!(
        matches!(scraper_err, ScraperError::Extraction(s) if s == "no content"),
        "DomainError::Extraction must convert to ScraperError::Extraction"
    );
}

#[test]
fn test_domain_error_extraction_failed_wraps_to_scraper() {
    let domain_err = crate::domain::error::DomainError::ExtractionFailed {
        url: "https://example.com".to_string(),
        reason: "empty body".to_string(),
    };
    let scraper_err: ScraperError = domain_err.into();
    assert!(
        matches!(
            scraper_err,
            ScraperError::ExtractionFailed { url, reason }
            if url == "https://example.com" && reason == "empty body"
        ),
        "DomainError::ExtractionFailed fields must be preserved"
    );
}

#[test]
fn test_domain_error_validation_wraps_to_scraper() {
    let domain_err = crate::domain::error::DomainError::Validation("bad pattern".to_string());
    let scraper_err: ScraperError = domain_err.into();
    assert!(
        matches!(scraper_err, ScraperError::Validation(s) if s == "bad pattern"),
        "DomainError::Validation must convert to ScraperError::Validation"
    );
}

#[test]
fn test_domain_error_feature_gated_wraps_to_scraper() {
    let domain_err = crate::domain::error::DomainError::FeatureGated("AI module".to_string());
    let scraper_err: ScraperError = domain_err.into();
    assert!(
        matches!(scraper_err, ScraperError::FeatureGated(s) if s == "AI module"),
        "DomainError::FeatureGated must convert to ScraperError::FeatureGated"
    );
}

#[test]
fn test_domain_error_conversion_wraps_to_scraper() {
    let domain_err = crate::domain::error::DomainError::Conversion("YAML parse".to_string());
    let scraper_err: ScraperError = domain_err.into();
    assert!(
        matches!(scraper_err, ScraperError::Conversion(s) if s == "YAML parse"),
        "DomainError::Conversion must convert to ScraperError::Conversion"
    );
}

#[test]
fn test_domain_error_question_mark_operator() {
    fn inner() -> std::result::Result<(), crate::domain::error::DomainError> {
        Err(crate::domain::error::DomainError::InvalidUrl(
            "test".to_string(),
        ))
    }

    fn outer() -> std::result::Result<(), ScraperError> {
        inner().map_err(ScraperError::from)?;
        Ok(())
    }

    let err = outer().unwrap_err();
    assert!(
        matches!(err, ScraperError::InvalidUrl(s) if s == "test"),
        "? operator must preserve InvalidUrl variant and message"
    );
}

// ========================================================================
// Phase 2: Error Stratification — InfraError → ScraperError From tests
// ========================================================================

#[test]
fn test_infra_error_http_wraps_to_scraper() {
    let infra_err = crate::infrastructure::error::InfraError::Http {
        status: 500,
        url: "https://example.com".to_string(),
    };
    let scraper_err: ScraperError = infra_err.into();
    assert!(
        matches!(scraper_err, ScraperError::Http { status: 500, .. }),
        "Status code must be preserved"
    );
    assert!(
        matches!(
            scraper_err,
            ScraperError::Http { url, .. }
            if url == "https://example.com"
        ),
        "URL must be preserved"
    );
}

#[test]
fn test_infra_error_network_wraps_to_scraper() {
    let infra_err = crate::infrastructure::error::InfraError::Network(Box::new(
        std::io::Error::new(std::io::ErrorKind::ConnectionRefused, "connection refused"),
    ));
    let scraper_err: ScraperError = infra_err.into();
    assert!(
        matches!(scraper_err, ScraperError::Network(_)),
        "InfraError::Network must convert to ScraperError::Network"
    );
}

#[test]
fn test_infra_error_download_wraps_to_scraper_download_variant() {
    // Regression: download failures must reach `ScraperError::Download`,
    // NOT be silently misrouted into `ScraperError::Network` (arch-remediation).
    let infra_err = crate::infrastructure::error::InfraError::Download(Box::new(
        std::io::Error::new(std::io::ErrorKind::Other, "checksum mismatch"),
    ));
    let scraper_err: ScraperError = infra_err.into();
    assert!(
        matches!(scraper_err, ScraperError::Download(_)),
        "InfraError::Download must map to ScraperError::Download, got: {scraper_err}"
    );
    // Display contract: Download variant must preserve Spanish prefix and inner message
    assert_eq!(
        scraper_err.to_string(),
        "Error de descarga: checksum mismatch"
    );
    // Must NOT be classified as a Network error.
    assert!(!matches!(scraper_err, ScraperError::Network(_)));
}

#[test]
fn test_infra_error_waf_blocked_wraps_to_scraper() {
    let infra_err = crate::infrastructure::error::InfraError::WafBlocked {
        url: "https://example.com".to_string(),
        provider: "Cloudflare".to_string(),
    };
    let scraper_err: ScraperError = infra_err.into();
    assert!(
        matches!(
            scraper_err,
            ScraperError::WafBlocked { url, provider }
            if url == "https://example.com" && provider == "Cloudflare"
        ),
        "WafBlocked fields must be preserved"
    );
}

#[test]
fn test_infra_error_persistence_wraps_to_scraper() {
    let infra_err = crate::infrastructure::error::InfraError::Persistence("disk full".to_string());
    let scraper_err: ScraperError = infra_err.into();
    assert!(
        matches!(scraper_err, ScraperError::Persistence(s) if s == "disk full"),
        "InfraError::Persistence must convert to ScraperError::Persistence"
    );
}

#[test]
fn test_infra_error_ingestion_wraps_to_scraper() {
    let infra_err =
        crate::infrastructure::error::InfraError::Ingestion("pipeline failed".to_string());
    let scraper_err: ScraperError = infra_err.into();
    assert!(
        matches!(scraper_err, ScraperError::Ingestion(s) if s == "pipeline failed"),
        "InfraError::Ingestion must convert to ScraperError::Ingestion"
    );
}

#[test]
fn test_infra_error_global_timeout_wraps_to_scraper() {
    let infra_err = crate::infrastructure::error::InfraError::GlobalTimeout;
    let scraper_err: ScraperError = infra_err.into();
    assert!(
        matches!(scraper_err, ScraperError::GlobalTimeout),
        "InfraError::GlobalTimeout must convert to ScraperError::GlobalTimeout"
    );
}

#[test]
fn test_infra_error_slowloris_timeout_wraps_to_scraper() {
    let infra_err = crate::infrastructure::error::InfraError::SlowlorisTimeout;
    let scraper_err: ScraperError = infra_err.into();
    assert!(
        matches!(scraper_err, ScraperError::SlowlorisTimeout),
        "InfraError::SlowlorisTimeout must convert to ScraperError::SlowlorisTimeout"
    );
}

#[test]
fn test_infra_error_payload_too_large_wraps_to_scraper() {
    let infra_err = crate::infrastructure::error::InfraError::PayloadTooLarge;
    let scraper_err: ScraperError = infra_err.into();
    assert!(
        matches!(scraper_err, ScraperError::PayloadTooLarge),
        "InfraError::PayloadTooLarge must convert to ScraperError::PayloadTooLarge"
    );
}

#[test]
fn test_infra_error_semaphore_inanition_wraps_to_scraper() {
    let infra_err = crate::infrastructure::error::InfraError::SemaphoreInanition;
    let scraper_err: ScraperError = infra_err.into();
    assert!(
        matches!(scraper_err, ScraperError::SemaphoreInanition),
        "InfraError::SemaphoreInanition must convert to ScraperError::SemaphoreInanition"
    );
}

#[test]
fn test_infra_error_h2_config_wraps_to_scraper() {
    let infra_err = crate::infrastructure::error::InfraError::H2Config("ALPN failed".to_string());
    let scraper_err: ScraperError = infra_err.into();
    assert!(
        matches!(scraper_err, ScraperError::H2Config(s) if s == "ALPN failed"),
        "InfraError::H2Config must convert to ScraperError::H2Config"
    );
}

#[test]
fn test_infra_error_url_parse_wraps_to_scraper() {
    let url_err = url::ParseError::EmptyHost;
    let infra_err = crate::infrastructure::error::InfraError::UrlParse(url_err);
    let scraper_err: ScraperError = infra_err.into();
    assert!(
        matches!(scraper_err, ScraperError::UrlParse(_)),
        "InfraError::UrlParse must convert to ScraperError::UrlParse"
    );
}

#[test]
fn test_unknown_profile_error_wraps_to_scraper_config() {
    let profile_err = crate::domain::UnknownProfileError {
        name: "Firefox".to_string(),
    };
    let scraper_err: ScraperError = profile_err.into();
    assert!(
        matches!(&scraper_err, ScraperError::Config(msg) if msg.contains("Perfil TLS desconocido: 'Firefox'")),
        "UnknownProfileError must map to ScraperError::Config preserving the Spanish message, got: {scraper_err}"
    );
}

#[test]
fn test_download_error_wraps_to_scraper_download() {
    let download_err = crate::infrastructure::downloader::DownloadError::Timeout(30);
    let scraper_err: ScraperError = download_err.into();
    assert!(
        matches!(scraper_err, ScraperError::Download(_)),
        "DownloadError must map to ScraperError::Download, got: {scraper_err}"
    );
}
