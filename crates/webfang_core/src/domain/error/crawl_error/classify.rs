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
