//! Crawl error categorization for observability (issue #374).
//!
//! Maps the 20+ `CrawlError` variants into a fixed set of operational
//! categories suitable for metrics, structured logging, and dashboards.

use serde::Serialize;

use crate::domain::CrawlError;

/// Operational category for a crawl error.
///
/// Reduces the full `CrawlError` variant space to a fixed set of
/// categories that are meaningful in dashboards and alerts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum CrawlErrorCategory {
    /// WAF challenge detected (Cloudflare, AWS WAF, etc.)
    Waf,
    /// HTTP status errors (4xx/5xx, transient)
    Http,
    /// Request timeout
    Timeout,
    /// Network-level failures (DNS, connection, download)
    Network,
    /// Rate limiting (429 / retry-after)
    RateLimit,
    /// Content extraction / parsing failures
    Extraction,
    /// Internal / catch-all (storage, checkpoint, config, etc.)
    Internal,
    /// Task panicked (JoinError)
    Panic,
}

impl CrawlErrorCategory {
    /// All categories in stable order (for array indexing and iteration).
    pub const ALL: [Self; 8] = [
        Self::Waf,
        Self::Http,
        Self::Timeout,
        Self::Network,
        Self::RateLimit,
        Self::Extraction,
        Self::Internal,
        Self::Panic,
    ];

    /// Index for array-based counting.
    #[inline]
    #[must_use]
    pub const fn index(self) -> usize {
        match self {
            Self::Waf => 0,
            Self::Http => 1,
            Self::Timeout => 2,
            Self::Network => 3,
            Self::RateLimit => 4,
            Self::Extraction => 5,
            Self::Internal => 6,
            Self::Panic => 7,
        }
    }

    /// Tracing field name for this category.
    #[inline]
    #[must_use]
    pub const fn tracing_field(self) -> &'static str {
        match self {
            Self::Waf => "errors_waf",
            Self::Http => "errors_http",
            Self::Timeout => "errors_timeout",
            Self::Network => "errors_network",
            Self::RateLimit => "errors_rate_limit",
            Self::Extraction => "errors_extraction",
            Self::Internal => "errors_internal",
            Self::Panic => "errors_panic",
        }
    }
}

impl std::fmt::Display for CrawlErrorCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let label = match self {
            Self::Waf => "waf",
            Self::Http => "http",
            Self::Timeout => "timeout",
            Self::Network => "network",
            Self::RateLimit => "rate_limit",
            Self::Extraction => "extraction",
            Self::Internal => "internal",
            Self::Panic => "panic",
        };
        f.write_str(label)
    }
}

/// HTTP status codes that signal a WAF-style challenge rather than a generic
/// HTTP error. These are the access-denied / rate-limit family of responses
/// (401 Unauthorized, 403 Forbidden, 429 Too Many Requests) that operators
/// should attribute to WAF blocking in `errors_waf` (issue #603).
#[inline]
#[must_use]
fn is_waf_status(status: u16) -> bool {
    matches!(status, 401 | 403 | 429)
}

impl From<&CrawlError> for CrawlErrorCategory {
    fn from(err: &CrawlError) -> Self {
        match err {
            CrawlError::WafChallenge { .. } => Self::Waf,
            CrawlError::Http { status, .. } | CrawlError::TransientHttp { status, .. } => {
                if is_waf_status(*status) {
                    Self::Waf
                } else {
                    Self::Http
                }
            },
            CrawlError::Timeout => Self::Timeout,
            // The `source` is deliberately NOT matched: a transport failure
            // now carries the classified cause there (#2784), so requiring
            // `source: None` would silently drop the WAF/RateLimit attribution
            // for every real 401/403/429. The status is what decides.
            CrawlError::Network {
                status_code: Some(status),
                ..
            } if matches!(*status, 401 | 403) => Self::Waf,
            CrawlError::Network {
                status_code: Some(429),
                ..
            } => Self::RateLimit,
            // The cause decides the bucket: the vocabulary and this enum are
            // ONE taxonomy, and `NetworkCause::category` is the only place
            // that maps between them (#2784).
            CrawlError::Network { .. }
            | CrawlError::Connection { .. }
            | CrawlError::Download(..) => crate::domain::error::cause::cause_in_chain(err).map_or(
                Self::Network,
                crate::domain::error::cause::NetworkCause::category,
            ),
            CrawlError::RateLimited(..) => Self::RateLimit,
            CrawlError::Parse(..) => Self::Extraction,
            // Every arm below was previously the `_ => Self::Internal`
            // catch-all. Each is now mapped explicitly per the Error
            // Classification Matrix (docs/error-classification-matrix.md);
            // the comment cites the matrix row driving the choice.
            // Row 15 (validation): no dedicated validation category exists,
            // so the Internal catch-all remains the closest match.
            CrawlError::InvalidUrl(_) => Self::Internal,
            // Row 33 (P6-2 slice 2): invalid run description — a
            // validation failure, same Internal catch-all as row 15.
            CrawlError::InvalidSession(_) => Self::Internal,
            // Rows 9/10/11/12 (limits/budget family): budget stops are not
            // operational failures and there is no budget category; the
            // Internal catch-all is the closest existing category.
            CrawlError::MaxDepthExceeded { .. }
            | CrawlError::MaxPagesExceeded { .. }
            | CrawlError::UrlExcluded(_)
            | CrawlError::ResourceExhausted { .. }
            | CrawlError::SitemapDepthExceeded => Self::Internal,
            // Row 18 (domain/content family): a rejected content type is a
            // content-processing failure, so it lands in Extraction next to
            // `Parse`.
            CrawlError::InvalidContentType(_) => Self::Extraction,
            // Rows 21/22 (io by kind): infrastructure failures → Internal.
            CrawlError::Io(_) => Self::Internal,
            CrawlError::Internal(_) => Self::Internal,
            // Row 20 (sitemap discovery family): no dedicated discovery
            // category exists; Internal remains the catch-all.
            CrawlError::SitemapNotFound(_) | CrawlError::Discovery(_) => Self::Internal,
            CrawlError::SitemapEmpty => Self::Internal,
            // Row 23 (persistence/internal infrastructure).
            CrawlError::Storage(_) | CrawlError::Checkpoint(_) => Self::Internal,
            // Row 24 (internal infrastructure: session pool lifecycle).
            CrawlError::SessionPool(_) => Self::Internal,
            // Taxonomy: per-URL retry-budget terminal state; job continues.
            // No dedicated retry category exists → Internal catch-all.
            CrawlError::RetryExhausted { .. } => Self::Internal,
            // Family 1 (HTTP request construction/body-read failure):
            // closest existing category is Http.
            CrawlError::RequestFailed(_) => Self::Http,
            // Row 13 (backpressure configuration bug) → Internal.
            CrawlError::SemaphoreInanition => Self::Internal,
            // Special cell — Cancelled: control signal, not an operational
            // failure category; Internal catch-all if it ever surfaces.
            CrawlError::Cancelled => Self::Internal,
            // #2338: a run that ended early is an engine-level failure; the
            // Internal catch-all is the closest existing category (same cell
            // the stall error occupied as `Internal` before the variant).
            CrawlError::PartialRun { .. } => Self::Internal,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::error::cause::ErrorSource;
    use crate::domain::error::WafDetectionKind;
    use crate::domain::CrawlResult;

    #[test]
    fn categorize_partial_run_lands_in_internal() {
        let err = CrawlError::PartialRun {
            message: "crawl stalled: test".into(),
            partial: CrawlResult::empty(),
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
        let err = CrawlError::Timeout;
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
}
