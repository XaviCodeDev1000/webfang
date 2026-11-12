//! Operational classification of [`ScraperError`].
//!
//! The `classify` entry point used by retry logic and observability systems,
//! plus the network transport heuristics it walks.

use super::{ErrorClass, ResourceKind, ScraperError};

impl ScraperError {
    /// Classify this error by operational severity.
    ///
    /// Used by retry logic and observability systems to decide retry/alert
    /// behavior without matching on specific variants or string-matching on
    /// display text.
    pub fn classify(&self) -> ErrorClass {
        match self {
            // Network/Download: transient if the wrapped source is a known-retryable
            // io::Error kind (timeout, connection reset, etc.)
            // Download: delegate to the typed `DownloadError::classify` when the
            // boxed cause is one (#649), so DNS/TLS/io-kind precision survives the
            // erasure. Falls back to the source-chain heuristic otherwise.
            Self::Download(e) => {
                if let Some(dl_err) =
                    e.downcast_ref::<crate::infrastructure::downloader::DownloadError>()
                {
                    dl_err.classify()
                } else if is_transient_network(e.as_ref()) {
                    ErrorClass::TransientRetriable
                } else {
                    ErrorClass::InternalFatal
                }
            },
            Self::Network(e) if is_transient_network(e.as_ref()) => ErrorClass::TransientRetriable,
            // DNS resolution and TLS handshake failures are transport failures,
            // not code bugs (#1824). The `Io` arm below gets its distinction
            // from `io::ErrorKind`; std has no DNS/TLS kind, so the explicit
            // DNS/TLS arm reads the transport text instead — an enumerated,
            // documented signal, mirroring the Io arm's kind-based approach
            // rather than an accident of the fallback.
            Self::Network(e) if is_dns_or_tls_transport_failure(e.as_ref()) => {
                ErrorClass::TransientRetriable
            },
            Self::Http { status, .. } if *status >= 500 => ErrorClass::TransientRetriable,
            Self::Http { status, .. } if *status == 429 => ErrorClass::TransientBackoff,
            Self::GlobalTimeout => ErrorClass::TransientBackoff,
            Self::SlowlorisTimeout => ErrorClass::TransientBackoff,
            Self::RateLimited(_) => ErrorClass::TransientBackoff,
            Self::InvalidUrl(_) => ErrorClass::PermanentFatal,
            Self::WafBlocked { .. } => ErrorClass::PermanentFatal,
            Self::Readability(_) => ErrorClass::PermanentFatal,
            Self::Extraction(_) => ErrorClass::PermanentFatal,
            Self::ExtractionFailed { .. } => ErrorClass::PermanentFatal,
            Self::Validation(_) => ErrorClass::PermanentFatal,
            Self::FeatureGated(_) => ErrorClass::PermanentFatal,
            Self::Config(_) => ErrorClass::PermanentFatal,
            Self::PayloadTooLarge => ErrorClass::PermanentFatal,
            Self::Http { .. } => ErrorClass::PermanentFatal,
            Self::SemaphoreInanition => ErrorClass::InternalFatal,
            // Rows 21/22: classify by `io::ErrorKind`, mirroring
            // `CrawlError::classify` — transient kinds (`Interrupted`,
            // `WouldBlock`, `TimedOut`) are retriable; the rest (permanent:
            // `NotFound`, `PermissionDenied`, ...) are PermanentFatal, which
            // the CLI boundary maps to the typed exit-74 override via
            // `permanent_io_error_exit_for`.
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
            Self::Serialization(_) => ErrorClass::InternalFatal,
            Self::UrlParse(_) => ErrorClass::InternalFatal,
            Self::Persistence(_) => ErrorClass::InternalFatal,
            Self::Ingestion(_) => ErrorClass::InternalFatal,
            Self::Middleware(_) => ErrorClass::InternalFatal,
            Self::H2Config(_) => ErrorClass::InternalFatal,
            Self::Export(_) => ErrorClass::InternalFatal,
            Self::ExportBatch(_) => ErrorClass::InternalFatal,
            Self::Conversion(_) => ErrorClass::InternalFatal,
            Self::Semantic(inner) => inner.classify(),
            Self::Internal(_) => ErrorClass::InternalFatal,
            // Rows 9/11 (#957): budget exhaustion by design is the crawler
            // doing its job, not a failure. `CrawlLimit` is the shared
            // pass-through target of `From<CrawlError>` for max-depth,
            // max-pages and sitemap-depth, so this arm must agree with
            // `CrawlError::classify` or it inverts all three siblings.
            Self::CrawlLimit(_) => ErrorClass::DomainRecoverable,
            // Rows 11/12 (#957): same split as `CrawlError::classify` —
            // RamBudget backpressure resolves itself when memory frees,
            // sitemap url/depth budgets are by-design stops. Enumerating
            // every kind (no wildcard) keeps a new `ResourceKind` from
            // silently inheriting a class.
            Self::ResourceExhausted {
                resource: ResourceKind::RamBudget,
                ..
            } => ErrorClass::TransientBackoff,
            Self::ResourceExhausted {
                resource: ResourceKind::SitemapUrls | ResourceKind::SitemapDepth,
                ..
            } => ErrorClass::DomainRecoverable,
            // Row 20: empty/missing sitemap is a domain-content condition,
            // not a code bug — same class as CrawlError::classify so the
            // From<CrawlError> pass-through does not invert it. The typed
            // exit-2 override (`empty_discovery_exit_for`) matches variants
            // directly and is unaffected.
            Self::SitemapEmpty | Self::SitemapNotFound(_) => ErrorClass::DomainRecoverable,
            // #1824: any other Network error is still a transport failure —
            // the remote service was unavailable (EX_UNAVAILABLE, 69), never a
            // webfang bug. InternalFatal is unreachable from the Network
            // variant.
            Self::Network(_) => ErrorClass::TransientRetriable,
        }
    }
}

/// Check if a boxed error represents a transient network failure.
///
/// This is a heuristic — `io::ErrorKind` is the primary signal,
/// but some wreq errors may also be transient.
///
/// #537: the io-kind list and the text fallback were extended so common
/// "service unavailable" network failures (`ConnectionRefused`,
/// `HostUnreachable`, `NetworkUnreachable`, `AddrNotAvailable`, wreq
/// connect/DNS wrappers) classify as [`ErrorClass::TransientRetriable`]
/// instead of falling through to `InternalFatal` and masquerading as a bug.
///
/// The heuristic walks the FULL `Error::source()` chain: infrastructure
/// layers wrap transport errors several deep (e.g. `DownloadError::Network`
/// boxing a `wreq::Error` boxing an `io::Error`), so inspecting only the
/// top-level error misses the typed kind and the transport text.
fn is_transient_network(e: &(dyn std::error::Error + 'static)) -> bool {
    let mut current: Option<&(dyn std::error::Error + 'static)> = Some(e);
    while let Some(err) = current {
        if let Some(io_err) = err.downcast_ref::<std::io::Error>() {
            return matches!(
                io_err.kind(),
                std::io::ErrorKind::ConnectionReset
                    | std::io::ErrorKind::ConnectionAborted
                    | std::io::ErrorKind::ConnectionRefused
                    | std::io::ErrorKind::HostUnreachable
                    | std::io::ErrorKind::NetworkUnreachable
                    | std::io::ErrorKind::AddrNotAvailable
                    | std::io::ErrorKind::TimedOut
                    | std::io::ErrorKind::Interrupted
                    | std::io::ErrorKind::BrokenPipe
            );
        }
        // Text fallback runs for every link, not just `WreqError`: wrappers
        // (`DownloadError::Network`, `CrawlError::Network`'s flattened text)
        // commonly erase the concrete type before reaching this function.
        let msg = err.to_string().to_ascii_lowercase();
        let matched = msg.contains("timeout")
            || msg.contains("timed out")
            || msg.contains("connection refused")
            || msg.contains("connection reset")
            || msg.contains("broken pipe")
            || msg.contains("connection aborted")
            || msg.contains("client error (connect)")
            || msg.contains("dns error")
            || msg.contains("dns lookup failed")
            || msg.contains("could not resolve")
            || msg.contains("failed to lookup address");
        if matched {
            tracing::debug!(
                error = %err,
                "is_transient_network: classified transient via source-chain text heuristic"
            );
            return true;
        }
        current = err.source();
    }
    false
}

/// Check if a boxed error is a DNS resolution or TLS handshake transport
/// failure (#1824).
///
/// [`ScraperError::classify`]'s `Io` arm gets its transient/permanent
/// distinction from `io::ErrorKind`; std has no DNS/TLS kind, so transport
/// TEXT is the signal. The patterns are conservative and enumerated — the
/// DNS list covers the resolver literals emitted by glibc, macOS/BSD and
/// Windows stacks plus the internal wrappers that reword them
/// (`DownloadError::Dns` via `strip_display_marker`, the SSRF validating
/// resolver); the TLS list covers the certificate/handshake vocabulary the
/// TLS stacks report. The FULL `Error::source()` chain is walked —
/// deliberately WITHOUT the io-downcast early return of
/// `is_transient_network`, because the synthetic-io path
/// (`io_error_for_network_message`) carries the transport text INSIDE an
/// `ErrorKind::Other` io error's message, where the early return would hide it.
fn is_dns_or_tls_transport_failure(e: &(dyn std::error::Error + 'static)) -> bool {
    /// DNS resolution failure fragments, lowercased.
    const DNS_PATTERNS: [&str; 11] = [
        "name or service not known",
        "temporary failure in name resolution",
        "name resolution failed",
        "no such host",
        "nodename nor servname",
        "nxdomain",
        "domain not found",
        "failed to lookup",
        "could not resolve",
        "dns error",
        "dns lookup",
    ];
    /// TLS handshake failure fragments, lowercased.
    const TLS_PATTERNS: [&str; 3] = ["tls", "handshake", "certificate"];

    let mut current: Option<&(dyn std::error::Error + 'static)> = Some(e);
    while let Some(err) = current {
        // Typed io kind first: a synthetic `ErrorKind::AddrNotAvailable`
        // source (the DNS mapping of `io_error_for_network_message`) is DNS
        // by construction even when the message text drifts.
        if let Some(io_err) = err.downcast_ref::<std::io::Error>() {
            if io_err.kind() == std::io::ErrorKind::AddrNotAvailable {
                return true;
            }
        }
        let msg = err.to_string().to_ascii_lowercase();
        if DNS_PATTERNS.iter().any(|p| msg.contains(p))
            || TLS_PATTERNS.iter().any(|p| msg.contains(p))
        {
            tracing::debug!(
                error = %err,
                "is_dns_or_tls_transport_failure: classified DNS/TLS via source-chain text"
            );
            return true;
        }
        current = err.source();
    }
    false
}
