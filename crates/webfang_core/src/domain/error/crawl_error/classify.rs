//! The classification of a crawl error per the Error Classification Matrix.
//!
//! Extracted from `crawl_error.rs` so the grandfathered file stops growing:
//! a pure move, no behaviour change.

use super::{CrawlError, ResourceKind};

impl CrawlError {
    /// Classify this error per the Error Classification Matrix.
    ///
    /// Contract: `docs/error-classification-matrix.md` (closed, ID
    /// `261bdb66-197e-420f-a73b-66c0e889102d`). The match is flat with ZERO
    /// wildcard arms: adding a variant without a classification fails
    /// compilation (matrix "Exhaustiveness enforcement" #1). Comments cite the
    /// matrix row each arm implements.
    #[must_use]
    pub fn classify(&self) -> crate::domain::error::ErrorClass {
        use crate::domain::error::ErrorClass;

        match self {
            // Rows 1/8: connection-reset-style and generic indeterminate
            // network errors are overwhelmingly transient (matrix rationale
            // for #8). Typed DNS/TLS precision (row 6) is decided upstream,
            // before the infrastructure layer erases the cause into this
            // variant.
            Self::Network { .. } => ErrorClass::TransientRetriable,
            // Row 2: HTTP 5xx.
            Self::Http { status, .. } if *status >= 500 => ErrorClass::TransientRetriable,
            // Row 3: 429 honors Retry-After.
            Self::Http { status, .. } if *status == 429 => ErrorClass::TransientBackoff,
            // Row 5: HTTP 4xx except 429.
            Self::Http { .. } => ErrorClass::PermanentFatal,
            // Row 15.
            Self::InvalidUrl(_) => ErrorClass::PermanentFatal,
            // Row 16.
            Self::Parse(_) => ErrorClass::DomainRecoverable,
            // Row 9: budget exhaustion by design.
            Self::MaxDepthExceeded { .. } | Self::MaxPagesExceeded { .. } => {
                ErrorClass::DomainRecoverable
            },
            // Row 10.
            Self::UrlExcluded(_) => ErrorClass::DomainRecoverable,
            // Row 18.
            Self::InvalidContentType(_) => ErrorClass::DomainRecoverable,
            // Rows 21/22: classify by `io::ErrorKind` per the matrix cell —
            // transient (`Interrupted`, `WouldBlock`, `TimedOut`) vs permanent
            // (`NotFound`, `PermissionDenied`, rest).
            Self::Io(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::Interrupted
                        | std::io::ErrorKind::WouldBlock
                        | std::io::ErrorKind::TimedOut
                ) =>
            {
                ErrorClass::TransientRetriable
            },
            // Row 22 (rest of the io kinds).
            Self::Io(_) => ErrorClass::PermanentFatal,
            // Matrix taxonomy: unspecified internal error = bug indicator
            // (InternalFatal family; cf. row 23).
            Self::Internal(_) => ErrorClass::InternalFatal,
            // #2338: a run that ended early is a terminal failure of the run
            // as a whole — it preserves the InternalFatal class the stall
            // carried as `Internal` before the variant existed. The partial
            // results do not soften the class: resuming is a separate
            // mechanism (checkpoint), never a retry of this result.
            Self::PartialRun { .. } => ErrorClass::InternalFatal,
            // Row 33 (P6-2 slice 2): an invalid run description is caller
            // error — permanent, surfaced as exit 78 through
            // `ScraperError::Config` (same contract family as row 31).
            Self::InvalidSession(_) => ErrorClass::PermanentFatal,
            // Row 20.
            Self::SitemapNotFound(_) => ErrorClass::DomainRecoverable,
            // Row 23: data-integrity errors are NEVER retried (Gate 2).
            Self::Storage(_) | Self::Checkpoint(_) => ErrorClass::InternalFatal,
            // Row 24.
            Self::SessionPool(_) => ErrorClass::TransientBackoff,
            // Row 20 family: robots.txt/sitemap auto-discovery failure is a
            // single-site recoverable failure; the job continues without that
            // site (same semantics as SitemapNotFound).
            Self::Discovery(_) => ErrorClass::DomainRecoverable,
            // Rows 1/8: the boxed cause is type-erased at the domain boundary
            // (infrastructure boxes a `DownloadError` here); indeterminate
            // transport failures are transient. Typed precision survives the
            // upward path: `ScraperError::classify` re-downcasts the same box.
            Self::Download(_) => ErrorClass::TransientRetriable,
            // Row 7.
            Self::WafChallenge { .. } => ErrorClass::PermanentFatal,
            // Taxonomy definition of DomainRecoverable: single-item failure;
            // the job continues without that URL. The retry budget is spent,
            // so no class offering another retry applies.
            Self::RetryExhausted { .. } => ErrorClass::DomainRecoverable,
            // Row 2: the variant is defined as transient 5xx.
            Self::TransientHttp { .. } => ErrorClass::TransientRetriable,
            // Row 3.
            Self::RateLimited(_) => ErrorClass::TransientBackoff,
            // Row 4.
            Self::Timeout { .. } => ErrorClass::TransientBackoff,
            // #2841: an unclassified transport failure is still a transport
            // failure — the remote was unavailable, never a webfang bug. This
            // arm must AGREE with `ScraperError::classify`'s `Network` arm
            // (TransientRetriable, #1824), or the `From<CrawlError>` conversion
            // would invert the class at the boundary.
            Self::Transport { .. } => ErrorClass::TransientRetriable,
            // Row 1.
            Self::Connection { .. } => ErrorClass::TransientRetriable,
            // Variant contract (variant doc): non-transient request
            // construction/body-read failure maps to InternalFatal upstream.
            Self::RequestFailed(_) => ErrorClass::InternalFatal,
            // Rows 11/12: sitemap budget kinds are by-design stops;
            // RamBudget backpressure resolves itself when memory frees.
            Self::ResourceExhausted {
                resource: ResourceKind::SitemapUrls | ResourceKind::SitemapDepth,
                ..
            } => ErrorClass::DomainRecoverable,
            Self::ResourceExhausted {
                resource: ResourceKind::RamBudget,
                ..
            } => ErrorClass::TransientBackoff,
            // Row 20.
            Self::SitemapEmpty => ErrorClass::DomainRecoverable,
            // Row 11.
            Self::SitemapDepthExceeded => ErrorClass::DomainRecoverable,
            // Row 13: backpressure configuration bug.
            Self::SemaphoreInanition => ErrorClass::InternalFatal,
            // Special cell — Cancelled: cooperative control signal intercepted
            // at the CLI boundary; this defensive fallback ensures an escaped
            // signal can never be retried or silently swallowed during teardown.
            Self::Cancelled => ErrorClass::InternalFatal,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::error::WafDetectionKind;
    use crate::error::ErrorClass;

    // ====================================================================
    // Error Classification Matrix (DoD #2/#3)
    //
    // Contract: docs/error-classification-matrix.md (closed, ID
    // 261bdb66-197e-420f-a73b-66c0e889102d). One test per CrawlError
    // variant; every variant must be classified to its matrix row.
    // ====================================================================

    fn assert_class(err: CrawlError, expected: ErrorClass) {
        assert_eq!(err.classify(), expected, "wrong matrix class for {err:?}");
    }

    // Family 1 — Network / HTTP / Timeout

    #[test]
    fn network_generic_is_transient_retriable() {
        // Matrix rows 1/8: connection-reset-style and indeterminate
        // network errors are overwhelmingly transient.
        assert_class(
            CrawlError::network("connection reset", None),
            ErrorClass::TransientRetriable,
        );
    }

    #[test]
    fn http_5xx_is_transient_retriable() {
        // Matrix row 2.
        assert_class(
            CrawlError::Http {
                status: 503,
                url: "https://example.com".to_string(),
            },
            ErrorClass::TransientRetriable,
        );
    }

    #[test]
    fn http_429_is_transient_backoff() {
        // Matrix row 3: honors Retry-After.
        assert_class(
            CrawlError::Http {
                status: 429,
                url: "https://example.com".to_string(),
            },
            ErrorClass::TransientBackoff,
        );
    }

    #[test]
    fn http_4xx_is_permanent_fatal() {
        // Matrix row 5: 4xx except 429 never succeeds on retry.
        assert_class(
            CrawlError::Http {
                status: 404,
                url: "https://example.com".to_string(),
            },
            ErrorClass::PermanentFatal,
        );
    }

    #[test]
    fn transient_http_is_transient_retriable() {
        // Matrix row 2: the variant is defined as 5xx retryable.
        assert_class(
            CrawlError::TransientHttp {
                status: 500,
                url: "https://example.com".to_string(),
            },
            ErrorClass::TransientRetriable,
        );
    }

    #[test]
    fn rate_limited_is_transient_backoff() {
        // Matrix row 3.
        assert_class(CrawlError::RateLimited(60), ErrorClass::TransientBackoff);
    }

    #[test]
    fn timeout_is_transient_backoff() {
        // Matrix row 4.
        assert_class(CrawlError::timeout(), ErrorClass::TransientBackoff);
    }

    #[test]
    fn connection_is_transient_retriable() {
        // Matrix row 1.
        assert_class(
            CrawlError::Connection {
                message: "refused".to_string(),
                source: None,
            },
            ErrorClass::TransientRetriable,
        );
    }

    #[test]
    fn waf_challenge_is_permanent_fatal() {
        // Matrix row 7.
        assert_class(
            CrawlError::WafChallenge {
                provider: "Cloudflare".to_string(),
                kind: WafDetectionKind::BodySignature,
                url: "https://example.com".to_string(),
            },
            ErrorClass::PermanentFatal,
        );
    }

    #[test]
    fn download_is_transient_retriable() {
        // Matrix rows 1/8: the boxed cause is type-erased at the domain
        // boundary; indeterminate transport failures are transient.
        assert_class(
            CrawlError::Download(Box::new(std::io::Error::new(
                std::io::ErrorKind::ConnectionReset,
                "reset",
            ))),
            ErrorClass::TransientRetriable,
        );
    }

    // Family 2 — Limits / Budgets

    #[test]
    fn max_depth_exceeded_is_domain_recoverable() {
        // Matrix row 9: budget exhaustion by design.
        assert_class(
            CrawlError::MaxDepthExceeded { current: 5, max: 3 },
            ErrorClass::DomainRecoverable,
        );
    }

    #[test]
    fn max_pages_exceeded_is_domain_recoverable() {
        // Matrix row 9.
        assert_class(
            CrawlError::MaxPagesExceeded { max: 100 },
            ErrorClass::DomainRecoverable,
        );
    }

    #[test]
    fn url_excluded_is_domain_recoverable() {
        // Matrix row 10.
        assert_class(
            CrawlError::UrlExcluded("https://spam.com".to_string()),
            ErrorClass::DomainRecoverable,
        );
    }

    #[test]
    fn resource_exhausted_sitemap_kinds_are_domain_recoverable() {
        // Matrix row 11.
        assert_class(
            CrawlError::ResourceExhausted {
                resource: ResourceKind::SitemapUrls,
                limit: 100,
                actual: 101,
            },
            ErrorClass::DomainRecoverable,
        );
        assert_class(
            CrawlError::ResourceExhausted {
                resource: ResourceKind::SitemapDepth,
                limit: 10,
                actual: 11,
            },
            ErrorClass::DomainRecoverable,
        );
    }

    #[test]
    fn resource_exhausted_ram_budget_is_transient_backoff() {
        // Matrix row 12: backpressure resolves itself when memory frees.
        assert_class(
            CrawlError::ResourceExhausted {
                resource: ResourceKind::RamBudget,
                limit: 1024,
                actual: 2048,
            },
            ErrorClass::TransientBackoff,
        );
    }

    #[test]
    fn semaphore_inanition_is_internal_fatal() {
        // Matrix row 13: backpressure configuration bug.
        assert_class(CrawlError::SemaphoreInanition, ErrorClass::InternalFatal);
    }

    // Family 3 — Domain / Content

    #[test]
    fn invalid_url_is_permanent_fatal() {
        // Matrix row 15.
        assert_class(
            CrawlError::InvalidUrl("bad-url".to_string()),
            ErrorClass::PermanentFatal,
        );
    }

    #[test]
    fn parse_is_domain_recoverable() {
        // Matrix row 16.
        assert_class(
            CrawlError::Parse("html parse failed".to_string()),
            ErrorClass::DomainRecoverable,
        );
    }

    #[test]
    fn invalid_content_type_is_domain_recoverable() {
        // Matrix row 18.
        assert_class(
            CrawlError::InvalidContentType("image/png".to_string()),
            ErrorClass::DomainRecoverable,
        );
    }

    #[test]
    fn sitemap_empty_is_domain_recoverable() {
        // Matrix row 20.
        assert_class(CrawlError::SitemapEmpty, ErrorClass::DomainRecoverable);
    }

    #[test]
    fn sitemap_not_found_is_domain_recoverable() {
        // Matrix row 20.
        assert_class(
            CrawlError::SitemapNotFound("https://example.com".to_string()),
            ErrorClass::DomainRecoverable,
        );
    }

    #[test]
    fn sitemap_depth_exceeded_is_domain_recoverable() {
        // Matrix row 11.
        assert_class(
            CrawlError::SitemapDepthExceeded,
            ErrorClass::DomainRecoverable,
        );
    }

    // Family 4 — Persistence / Internal infrastructure / AI

    #[test]
    fn io_transient_kinds_are_transient_retriable() {
        // Matrix row 21: Interrupted / WouldBlock / TimedOut.
        for kind in [
            std::io::ErrorKind::Interrupted,
            std::io::ErrorKind::WouldBlock,
            std::io::ErrorKind::TimedOut,
        ] {
            assert_class(
                CrawlError::Io(std::io::Error::new(kind, "io hiccup")),
                ErrorClass::TransientRetriable,
            );
        }
    }

    #[test]
    fn io_permanent_kinds_are_permanent_fatal() {
        // Matrix row 22: NotFound / PermissionDenied / rest.
        for kind in [
            std::io::ErrorKind::NotFound,
            std::io::ErrorKind::PermissionDenied,
            std::io::ErrorKind::Other,
        ] {
            assert_class(
                CrawlError::Io(std::io::Error::new(kind, "io failure")),
                ErrorClass::PermanentFatal,
            );
        }
    }

    #[test]
    fn storage_is_internal_fatal() {
        // Matrix row 23: data-integrity errors are NEVER retried.
        assert_class(
            CrawlError::Storage("append-log corrupt".to_string()),
            ErrorClass::InternalFatal,
        );
    }

    #[test]
    fn checkpoint_is_internal_fatal() {
        // Matrix row 23.
        assert_class(
            CrawlError::Checkpoint("json decode failed".to_string()),
            ErrorClass::InternalFatal,
        );
    }

    #[test]
    fn session_pool_is_transient_backoff() {
        // Matrix row 24.
        assert_class(
            CrawlError::SessionPool("pool exhausted".to_string()),
            ErrorClass::TransientBackoff,
        );
    }

    #[test]
    fn internal_is_internal_fatal() {
        // Matrix taxonomy: unspecified internal error = bug indicator.
        assert_class(
            CrawlError::Internal("unreachable state".to_string()),
            ErrorClass::InternalFatal,
        );
    }

    #[test]
    fn request_failed_is_internal_fatal() {
        // Variant contract (crawl_error.rs doc): non-transient request
        // construction/body-read failure maps to InternalFatal.
        assert_class(
            CrawlError::RequestFailed("body read failed".to_string()),
            ErrorClass::InternalFatal,
        );
    }

    // Variants resolved by matrix family semantics (not named verbatim
    // in the matrix); see classify() comments for the row references.

    #[test]
    fn discovery_is_domain_recoverable() {
        // Row 20 family: single-site discovery failure; job continues.
        assert_class(
            CrawlError::Discovery("robots.txt unreachable".to_string()),
            ErrorClass::DomainRecoverable,
        );
    }

    #[test]
    fn retry_exhausted_is_domain_recoverable() {
        // Taxonomy: single-item failure; job continues without it.
        assert_class(
            CrawlError::RetryExhausted {
                url: "https://example.com".to_string(),
                attempts: 3,
            },
            ErrorClass::DomainRecoverable,
        );
    }

    // Special cell — Cancelled

    #[test]
    fn cancelled_is_internal_fatal_defensive_fallback() {
        // Matrix special cell: cooperative control signal intercepted at
        // the CLI boundary; if it ever escapes, classify() must return
        // InternalFatal so it can never be retried or swallowed.
        assert_class(CrawlError::Cancelled, ErrorClass::InternalFatal);
    }
}
