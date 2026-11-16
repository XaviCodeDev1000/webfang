//! Tests of the `CrawlErrorCategory` mapping (moved out of the
//! grandfathered-size parent: a pure move, no behaviour change).

use super::*;
use crate::domain::error::cause::ErrorSource;
use crate::domain::error::WafDetectionKind;
use crate::domain::CrawlResult;

#[test]
fn categorize_partial_run_lands_in_internal() {
    let err = CrawlError::PartialRun {
        message: "crawl stalled: test".into(),
        partial: Box::new(CrawlResult::empty()),
    };
    assert_eq!(CrawlErrorCategory::from(&err), CrawlErrorCategory::Internal);
}

#[test]
fn categorize_waf() {
    let err = CrawlError::WafChallenge {
        provider: "Cloudflare".into(),
        kind: WafDetectionKind::BodySignature,
        url: "https://example.com".into(),
    };
    assert_eq!(CrawlErrorCategory::from(&err), CrawlErrorCategory::Waf);
}

#[test]
fn categorize_http() {
    let err = CrawlError::Http {
        status: 404,
        url: "https://example.com".into(),
    };
    assert_eq!(CrawlErrorCategory::from(&err), CrawlErrorCategory::Http);

    let err = CrawlError::TransientHttp {
        status: 503,
        url: "https://example.com".into(),
    };
    assert_eq!(CrawlErrorCategory::from(&err), CrawlErrorCategory::Http);
}

#[test]
fn categorize_waf_status_as_waf() {
    // Issue #603: 401/403/429 are WAF challenges, not generic HTTP errors.
    for status in [401u16, 403, 429] {
        let err = CrawlError::Http {
            status,
            url: "https://example.com".into(),
        };
        assert_eq!(
            CrawlErrorCategory::from(&err),
            CrawlErrorCategory::Waf,
            "status {status} must be classified as waf"
        );

        let err = CrawlError::TransientHttp {
            status,
            url: "https://example.com".into(),
        };
        assert_eq!(
            CrawlErrorCategory::from(&err),
            CrawlErrorCategory::Waf,
            "transient status {status} must be classified as waf"
        );
    }
}

#[test]
fn categorize_other_http_status_as_http() {
    for status in [400u16, 404, 500, 503] {
        let err = CrawlError::Http {
            status,
            url: "https://example.com".into(),
        };
        assert_eq!(
            CrawlErrorCategory::from(&err),
            CrawlErrorCategory::Http,
            "status {status} is a generic HTTP error, not waf"
        );
    }
}

#[test]
fn categorize_timeout() {
    let err = CrawlError::timeout();
    assert_eq!(CrawlErrorCategory::from(&err), CrawlErrorCategory::Timeout);
}

/// #2841 (S9): the category of a timeout is cause-blind — a timeout whose
/// `source` carries a classified connect-cause must still land in the
/// `Timeout` row, never leak into `Network` because of what it carries.
#[test]
fn categorize_timeout_is_cause_blind() {
    let err = CrawlError::timeout_with_cause(Box::new(
        crate::domain::error::cause::ClassifiedNetworkError::new(
            crate::domain::error::cause::NetworkCause::ConnectTimeout,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "connect deadline",
            )),
        ),
    ));
    assert_eq!(CrawlErrorCategory::from(&err), CrawlErrorCategory::Timeout);
}

#[test]
fn categorize_network() {
    let err = CrawlError::Network {
        message: "dns failure".into(),
        status_code: None,
        source: None,
    };
    assert_eq!(CrawlErrorCategory::from(&err), CrawlErrorCategory::Network);

    let err = CrawlError::Connection {
        message: "refused".into(),
        source: None,
    };
    assert_eq!(CrawlErrorCategory::from(&err), CrawlErrorCategory::Network);

    let err = CrawlError::Download(Box::new(std::io::Error::new(
        std::io::ErrorKind::ConnectionReset,
        "reset",
    )));
    assert_eq!(CrawlErrorCategory::from(&err), CrawlErrorCategory::Network);
}

/// The two taxonomies are ONE (#2784): every cause folds into the bucket
/// it declares, and an error that CARRIES a cause categorizes as that bucket.
#[test]
fn every_cause_folds_into_the_category_it_declares() {
    use crate::domain::error::cause::{ClassifiedNetworkError, NetworkCause};
    for cause in NetworkCause::ALL {
        assert_eq!(cause.category(), CrawlErrorCategory::Network, "{cause:?}");
        let err = CrawlError::Connection {
            message: cause.message().to_string(),
            source: Some(Box::new(ClassifiedNetworkError::new(
                cause,
                Box::new(std::io::Error::other(cause.message())),
            ))),
        };
        assert_eq!(
            CrawlErrorCategory::from(&err),
            cause.category(),
            "{cause:?}"
        );
    }
}

/// A transport `Network` failure with the given status, optionally carrying
/// the classified cause #2784 puts in its `source`.
fn network_with(status: u16, source: Option<ErrorSource>) -> CrawlError {
    CrawlError::Network {
        message: format!("HTTP error: {status}"),
        status_code: Some(status),
        source,
    }
}

#[test]
fn categorize_network_with_waf_status_as_waf() {
    // Issue #629: CrawlError::Network carrying a 401/403 status code (from
    // http_client non-success responses) must promote to Waf, not Network.
    // #2784: with or without the cause in the source — every real
    // transport failure carries one now, and the status still decides.
    for status in [401u16, 403] {
        for source in [
            None,
            Some(Box::new(std::io::Error::other("classified")) as ErrorSource),
        ] {
            let has_source = source.is_some();
            let err = network_with(status, source);
            assert_eq!(
                CrawlErrorCategory::from(&err),
                CrawlErrorCategory::Waf,
                "Network with status {status} must classify as waf, with source: {has_source}"
            );
        }
    }
}

#[test]
fn categorize_network_with_429_as_rate_limit() {
    // Issue #629: CrawlError::Network carrying a 429 must promote to RateLimit.
    for source in [
        None,
        Some(Box::new(std::io::Error::other("classified")) as ErrorSource),
    ] {
        let has_source = source.is_some();
        let err = network_with(429, source);
        assert_eq!(
            CrawlErrorCategory::from(&err),
            CrawlErrorCategory::RateLimit,
            "Network with status 429 must classify as rate_limit, with source: {has_source}"
        );
    }
}

#[test]
fn categorize_network_with_generic_http_status_as_network() {
    // Non-WAF status codes in Network variant stay as Network.
    for status in [500u16, 502, 503] {
        let err = CrawlError::Network {
            message: format!("HTTP error: {status}"),
            status_code: Some(status),
            source: None,
        };
        assert_eq!(
            CrawlErrorCategory::from(&err),
            CrawlErrorCategory::Network,
            "Network with status {status} must stay as network"
        );
    }
}

#[test]
fn categorize_rate_limit() {
    let err = CrawlError::RateLimited(60);
    assert_eq!(
        CrawlErrorCategory::from(&err),
        CrawlErrorCategory::RateLimit
    );
}

#[test]
fn categorize_parse_as_extraction() {
    let err = CrawlError::Parse("malformed html".into());
    assert_eq!(
        CrawlErrorCategory::from(&err),
        CrawlErrorCategory::Extraction
    );
}

#[test]
fn categorize_internal_fallback() {
    let err = CrawlError::Internal("unknown".into());
    assert_eq!(CrawlErrorCategory::from(&err), CrawlErrorCategory::Internal);

    let err = CrawlError::Storage("corrupt".into());
    assert_eq!(CrawlErrorCategory::from(&err), CrawlErrorCategory::Internal);

    let err = CrawlError::Checkpoint("decode failed".into());
    assert_eq!(CrawlErrorCategory::from(&err), CrawlErrorCategory::Internal);

    let err = CrawlError::InvalidUrl("bad".into());
    assert_eq!(CrawlErrorCategory::from(&err), CrawlErrorCategory::Internal);
}

#[test]
fn every_former_catch_all_variant_is_mapped_explicitly() {
    use crate::domain::error::ResourceKind;

    // Triangulation for the removed `_ => Internal` arm: variants that
    // keep the Internal catch-all keep it by explicit matrix-row
    // decision, and the two family-matched ones land in their family
    // category.
    let internal = [
        CrawlError::MaxDepthExceeded { current: 5, max: 3 },
        CrawlError::MaxPagesExceeded { max: 10 },
        CrawlError::UrlExcluded("https://spam.com".into()),
        CrawlError::ResourceExhausted {
            resource: ResourceKind::RamBudget,
            limit: 1,
            actual: 2,
        },
        CrawlError::SitemapDepthExceeded,
        CrawlError::Io(std::io::Error::other("io")),
        CrawlError::SitemapNotFound("https://example.com".into()),
        CrawlError::Discovery("robots.txt unreachable".into()),
        CrawlError::SitemapEmpty,
        CrawlError::SessionPool("pool exhausted".into()),
        CrawlError::RetryExhausted {
            url: "https://example.com".into(),
            attempts: 3,
        },
        CrawlError::SemaphoreInanition,
        CrawlError::Cancelled,
    ];
    for err in &internal {
        assert_eq!(
            CrawlErrorCategory::from(err),
            CrawlErrorCategory::Internal,
            "expected Internal for {err:?}"
        );
    }

    let err = CrawlError::InvalidContentType("image/png".into());
    assert_eq!(
        CrawlErrorCategory::from(&err),
        CrawlErrorCategory::Extraction
    );

    let err = CrawlError::RequestFailed("body read failed".into());
    assert_eq!(CrawlErrorCategory::from(&err), CrawlErrorCategory::Http);
}

#[test]
fn all_categories_have_unique_index() {
    let mut seen = [false; 8];
    for cat in CrawlErrorCategory::ALL {
        let idx = cat.index();
        assert!(!seen[idx], "duplicate index {idx} for {cat}");
        seen[idx] = true;
    }
    assert!(seen.iter().all(|&s| s), "all indices must be covered");
}

#[test]
fn display_matches_serde_rename() {
    for cat in CrawlErrorCategory::ALL {
        let display = cat.to_string();
        let tracing = cat.tracing_field();
        assert!(
            tracing.starts_with("errors_"),
            "tracing_field must start with errors_: {tracing}"
        );
        assert!(
            tracing.contains(&display),
            "tracing_field {tracing} must contain display {display}"
        );
    }
}
