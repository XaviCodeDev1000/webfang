//! Downloader abstraction — shim re-exporting domain::downloader_port (ADR-0010).
//!
//! The `Downloader` trait and DTOs now live in `crate::domain::downloader_port`
//! so `application::*` can depend on `domain::*` without
//! `application→infrastructure`. The concrete `wreq`-based impls stay here
//! and implement the domain trait; the `wreq::Error → DownloadError` mapping
//! and `strip_display_marker` helper remain infrastructure-owned (they would be
//! an outward `domain→wreq` dependency if they lived in `domain`).

mod cause;
/// Chrome profile directory helper for unique user-data-dir per launch.
#[cfg(feature = "chromium")]
pub mod chrome_profile;
pub mod chromiumoxide_downloader;
pub mod fetch_router;
pub mod hybrid_router;
pub mod obscura_downloader;
pub mod resource_governor;
pub mod spa_detector;
/// Domain-port impl for system RAM-usage reads — see
/// `crate::domain::ram_probe_port` and ADR-0012 sub-slice 3.B-1c.
pub mod system_ram_probe;
pub mod wreq_downloader;

// Domain-owned port — re-exported so `crate::infrastructure::downloader::Downloader`
// still resolves (shim for `webfang_core::infrastructure::downloader` path).
pub use crate::domain::downloader_port::{Cookie, DownloadError, Downloader, FetchedPage};

impl From<wreq::Error> for DownloadError {
    /// Walk the transport error's cause chain to recover a typed variant.
    fn from(e: wreq::Error) -> Self {
        use std::error::Error as _;

        // FIX-1 (#1236 F-09): builder-class errors mean the REQUEST ITSELF is
        // invalid (unsupported scheme, malformed URL). They never open a
        // socket, so retrying cannot help: classify them as PermanentFatal via
        // the dedicated InvalidUrl variant instead of Network (InternalFatal,
        // which the old mapping produced and the retry loop re-attempted up to
        // `max_retries` times with full exponential backoff).
        if e.is_builder() {
            return DownloadError::InvalidUrl(e.to_string());
        }

        // The resolver's OWN typed failure decides the DNS class (#2784). It
        // sits BELOW the transport's "dns error" text in the chain, so it has
        // to be looked for BEFORE any text match narrows the chain away.
        if let Some(detail) = typed_dns_failure(&e) {
            return DownloadError::Dns {
                message: detail,
                source: Some(Box::new(e)),
            };
        }

        let narrowed = {
            let mut source = e.source();
            let mut narrowed = None;
            while let Some(s) = source {
                if let Some(io_err) = s.downcast_ref::<std::io::Error>() {
                    narrowed = Some(Narrowed::Io(std::io::Error::new(
                        io_err.kind(),
                        io_err.to_string(),
                    )));
                    break;
                }
                let msg = s.to_string().to_lowercase();
                if msg.contains("dns") || msg.contains("resolve") {
                    narrowed = Some(Narrowed::Dns(strip_display_marker(
                        &s.to_string(),
                        "dns error",
                    )));
                    break;
                }
                if msg.contains("tls") || msg.contains("certificate") || msg.contains("ssl") {
                    narrowed = Some(Narrowed::Tls(strip_display_marker(
                        &s.to_string(),
                        "tls error",
                    )));
                    break;
                }
                source = s.source();
            }
            narrowed
        };

        match narrowed {
            Some(Narrowed::Dns(message)) => DownloadError::Dns {
                message,
                source: Some(Box::new(e)),
            },
            Some(Narrowed::Tls(message)) => DownloadError::Tls {
                message,
                source: Some(Box::new(e)),
            },
            Some(Narrowed::Io(io_err)) => DownloadError::Io(io_err),
            None => DownloadError::Network(Box::new(e)),
        }
    }
}

/// The transport failure, narrowed to the variant that decides its class.
enum Narrowed {
    /// A name resolution failure, with the detail behind it.
    Dns(String),
    /// A TLS failure, with the detail behind it.
    Tls(String),
    /// An I/O failure, with its kind preserved.
    Io(std::io::Error),
}

/// The detail behind the resolver's typed failure, if the chain carries one.
///
/// The typed failure is the resolver's own: it exists only because a
/// resolution was attempted and did not succeed, and it decides the DNS class
/// by TYPE. It sits BELOW the transport's "dns error" text, so the text match
/// would narrow the chain away before the walk ever reached it (#2784).
fn typed_dns_failure(error: &(dyn std::error::Error + 'static)) -> Option<String> {
    use crate::domain::error::cause::DnsResolutionError;
    let mut current = error.source();
    while let Some(err) = current {
        if err.downcast_ref::<DnsResolutionError>().is_some() {
            // The detail behind the typed failure, without the cause's own
            // words: the cause is composed by the mapping that knows it.
            return Some(
                err.source()
                    .map_or_else(|| err.to_string(), |inner| inner.to_string()),
            );
        }
        current = err.source();
    }
    None
}

/// Strip a leading marker that the variant's `Display` already prints.
fn strip_display_marker(payload: &str, marker: &str) -> String {
    let stripped = if payload.to_lowercase().starts_with(marker) {
        payload[marker.len()..]
            .trim_start_matches([':', ' '])
            .to_string()
    } else {
        payload.to_string()
    };
    if stripped.is_empty() {
        if marker.starts_with("dns") {
            "name resolution failed".to_string()
        } else {
            "handshake failed".to_string()
        }
    } else {
        stripped
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    #[test]
    fn test_fetched_page_clone() {
        let page = FetchedPage {
            url: "https://example.com".parse().unwrap(),
            html: "<html></html>".to_string(),
            status: 200,
            headers: HashMap::new(),
            cookies: vec![],
        };
        let cloned = page.clone();
        assert_eq!(page.url, cloned.url);
        assert_eq!(page.html, cloned.html);
    }

    #[test]
    fn test_cookie_struct() {
        let cookie = Cookie {
            name: "session".into(),
            value: "abc123".into(),
            domain: ".example.com".into(),
            path: "/".into(),
            http_only: true,
            secure: true,
        };
        assert!(cookie.http_only);
        assert!(cookie.secure);
    }

    #[test]
    fn test_download_error_display() {
        let err = DownloadError::Network(Box::new(std::io::Error::new(
            std::io::ErrorKind::ConnectionRefused,
            "connection refused",
        )));
        assert!(err.to_string().contains("connection refused"));

        let err = DownloadError::Http {
            status: 403,
            message: "forbidden".into(),
        };
        assert!(err.to_string().contains("403"));

        let err = DownloadError::WafChallenge("Cloudflare".into());
        assert!(err.to_string().contains("Cloudflare"));

        let err = DownloadError::SpaDetected("React SPA".into());
        assert!(err.to_string().contains("React SPA"));

        let err = DownloadError::timeout(30);
        assert!(err.to_string().contains("30"));
    }

    #[test]
    fn test_strip_display_marker_dedupes_redundant_prefix() {
        assert_eq!(
            super::strip_display_marker("dns error", "dns error"),
            "name resolution failed"
        );
        assert_eq!(
            super::strip_display_marker("dns error: NXDOMAIN", "dns error"),
            "NXDOMAIN"
        );
        assert_eq!(
            super::strip_display_marker("failed to lookup address", "dns error"),
            "failed to lookup address"
        );
        assert_eq!(
            super::strip_display_marker("tls error", "tls error"),
            "handshake failed"
        );
    }

    #[test]
    fn test_download_error_classify_variants() {
        use crate::error::ErrorClass;

        let cases: &[(DownloadError, ErrorClass)] = &[
            (DownloadError::dns("NXDOMAIN"), ErrorClass::PermanentFatal),
            (
                DownloadError::tls("certificate expired"),
                ErrorClass::PermanentFatal,
            ),
            (
                DownloadError::Io(std::io::Error::new(
                    std::io::ErrorKind::ConnectionReset,
                    "peer",
                )),
                ErrorClass::TransientRetriable,
            ),
            (
                DownloadError::Io(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "trunc",
                )),
                ErrorClass::TransientRetriable,
            ),
            (
                DownloadError::Io(std::io::Error::other("bug")),
                ErrorClass::InternalFatal,
            ),
            (DownloadError::timeout(1), ErrorClass::TransientBackoff),
            (
                DownloadError::Http {
                    status: 503,
                    message: "x".into(),
                },
                ErrorClass::TransientRetriable,
            ),
            (
                DownloadError::Http {
                    status: 429,
                    message: "x".into(),
                },
                ErrorClass::TransientBackoff,
            ),
            (
                DownloadError::Http {
                    status: 404,
                    message: "x".into(),
                },
                ErrorClass::PermanentFatal,
            ),
        ];
        for (err, expected) in cases {
            assert_eq!(err.classify(), *expected, "variant classification mismatch");
        }
    }

    #[test]
    fn test_download_error_into_crawl_error() {
        let err = DownloadError::Network(Box::new(std::io::Error::new(
            std::io::ErrorKind::ConnectionReset,
            "reset",
        )));
        let crawl_err: crate::domain::CrawlError = err.into();
        assert!(crawl_err.to_string().contains("download error"));
        assert!(crawl_err.to_string().contains("reset"));
    }

    #[test]
    fn test_download_error_feature_gated_clone_and_classify() {
        let original = DownloadError::FeatureGated("chromium disabled".to_string());
        let cloned = original.clone();
        assert!(
            matches!(cloned, DownloadError::FeatureGated(ref s) if s == "chromium disabled"),
            "FeatureGated clone must preserve String, got: {cloned:?}"
        );
        assert_eq!(cloned.classify(), crate::error::ErrorClass::PermanentFatal,);
        assert!(cloned.to_string().contains("chromium disabled"));
        assert!(
            cloned.to_string().contains("funcionalidad no disponible"),
            "FeatureGated Display must be Spanish, got: {cloned}"
        );
    }
}
