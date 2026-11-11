use super::*;
use crate::domain::error::CrawlError;

#[test]
fn test_is_transient_network_broken_pipe() {
    let err = ScraperError::Network(Box::new(std::io::Error::new(
        std::io::ErrorKind::BrokenPipe,
        "broken pipe",
    )));
    assert_eq!(err.classify(), ErrorClass::TransientRetriable);
}

// ========================================================================
// #537: service-unavailable transport errors must not classify as bugs
// ========================================================================

#[test]
fn test_is_transient_network_connection_refused_io() {
    let err = ScraperError::Download(Box::new(std::io::Error::new(
        std::io::ErrorKind::ConnectionRefused,
        "refused",
    )));
    assert_eq!(
        err.classify(),
        ErrorClass::TransientRetriable,
        "io ConnectionRefused → TransientRetriable (service unavailable, EX_UNAVAILABLE)"
    );
}

#[test]
fn test_is_transient_network_unreachable_io_kinds() {
    for kind in [
        std::io::ErrorKind::HostUnreachable,
        std::io::ErrorKind::NetworkUnreachable,
        std::io::ErrorKind::AddrNotAvailable,
    ] {
        let err = ScraperError::Network(Box::new(std::io::Error::new(kind, "unreachable")));
        assert_eq!(
            err.classify(),
            ErrorClass::TransientRetriable,
            "io {kind:?} → TransientRetriable (#537)"
        );
    }
}

#[test]
fn test_is_transient_network_wreq_connect_wrapper_text() {
    // wreq wraps connect failures as "…: client error (Connect)". The io
    // source is NOT the boxed outer error, so the io downcast misses and
    // the text fallback at `is_transient_network` must catch it (#537).
    let io_src = std::io::Error::new(
        std::io::ErrorKind::ConnectionRefused,
        "error sending request for uri (http://127.0.0.1:1/): client error (Connect)",
    );
    let err = ScraperError::Download(Box::new(io_src));
    assert_eq!(
        err.classify(),
        ErrorClass::TransientRetriable,
        "connect-style io error must classify TransientRetriable (#537)"
    );
}

#[test]
fn test_is_transient_network_walks_source_chain() {
    // Production chain (#537): `ScraperError::Network(Box<DownloadError>)`
    // → `DownloadError::Network`'s `#[source]` → `wreq::Error`. Walking
    // `Error::source()` must reach a transient io link even though the
    // boxed outer error is neither io nor wreq-typed.
    #[derive(Debug)]
    struct Wrapper(&'static str, Option<std::io::Error>);
    impl std::fmt::Display for Wrapper {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(self.0)
        }
    }
    impl std::error::Error for Wrapper {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            self.1.as_ref().map(|e| e as _)
        }
    }

    let io_leaf = std::io::Error::new(std::io::ErrorKind::TimedOut, "request timeout");
    let wrapper = Wrapper("download failed", Some(io_leaf));
    let err = ScraperError::Network(Box::new(wrapper));
    assert_eq!(
        err.classify(),
        ErrorClass::TransientRetriable,
        "source-chain walk must surface the transient io leaf (#537)"
    );

    // Text fallback through the same chain (non-io leaf carrying wreq text).
    #[derive(Debug)]
    struct TextLeaf;
    impl std::fmt::Display for TextLeaf {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("client error (Connect)")
        }
    }
    impl std::error::Error for TextLeaf {}
    #[derive(Debug)]
    struct TextWrapper(&'static str);
    impl std::fmt::Display for TextWrapper {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(self.0)
        }
    }
    impl std::error::Error for TextWrapper {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            Some(&TextLeaf)
        }
    }
    let err = ScraperError::Network(Box::new(TextWrapper("network error")));
    assert_eq!(
        err.classify(),
        ErrorClass::TransientRetriable,
        "source-chain text fallback must catch wreq connect wrapper (#537)"
    );
}

#[test]
fn test_classify_non_network_nonwreq_still_internal() {
    // Safety net stays: a non-io, non-wreq-scoped error keeps falling
    // through to InternalFatal (genuine bug bucket).
    let err = ScraperError::Internal("unreachable state".into());
    assert_eq!(err.classify(), ErrorClass::InternalFatal);
}

// ========================================================================
// #1824: DNS resolution and TLS handshake failures are transport
// failures — the Network variant is unreachable from InternalFatal.
// ========================================================================

#[test]
fn classify_dns_text_is_transient_retriable() {
    // Resolver literals from the platforms this repo ships on (glibc,
    // macOS/BSD, Windows) plus the canonical wrapper text webfang emits
    // itself (`strip_display_marker` → "name resolution failed"; the
    // observed real wreq DNS error on the #1824 red run was
    // "DNS error: name resolution failed").
    let dns_messages = [
        "dns error: name resolution failed",
        "Name or service not known",
        "failed to lookup address information: Name or service not known",
        "Temporary failure in name resolution",
        "name resolution failed",
        "no such host is known.",
        "nodename nor servname provided, or not known",
        "Domain name not found: NXDOMAIN",
        "could not resolve hostname",
        "dns lookup failed",
    ];
    for msg in dns_messages {
        let err = ScraperError::Network(Box::new(std::io::Error::other(msg)));
        assert_eq!(
            err.classify(),
            ErrorClass::TransientRetriable,
            "DNS text {msg:?} must classify TransientRetriable (#1824)"
        );
    }
}

#[test]
fn classify_tls_text_is_transient_retriable() {
    let tls_messages = [
        "tls handshake failed",
        "tlsv1 alert internal error",
        "certificate verify failed",
        "certificate has expired",
        "handshake failure",
    ];
    for msg in tls_messages {
        let err = ScraperError::Network(Box::new(std::io::Error::other(msg)));
        assert_eq!(
            err.classify(),
            ErrorClass::TransientRetriable,
            "TLS text {msg:?} must classify TransientRetriable (#1824)"
        );
    }
}

#[test]
fn classify_dns_tls_text_walks_source_chain() {
    // A wrapper erasing the concrete type must not hide the transport
    // text: the chain walk reaches the DNS/TLS leaf (#1824). Mirrors the
    // wrapper/leaf stubs of `test_is_transient_network_walks_source_chain`.
    #[derive(Debug)]
    struct TextLeaf(&'static str);
    impl std::fmt::Display for TextLeaf {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(self.0)
        }
    }
    impl std::error::Error for TextLeaf {}

    #[derive(Debug)]
    struct TextWrapper(&'static str, Option<TextLeaf>);
    impl std::fmt::Display for TextWrapper {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(self.0)
        }
    }
    impl std::error::Error for TextWrapper {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            self.1.as_ref().map(|e| e as _)
        }
    }

    let err = ScraperError::Network(Box::new(TextWrapper(
        "download failed",
        Some(TextLeaf("Name or service not known")),
    )));
    assert_eq!(
        err.classify(),
        ErrorClass::TransientRetriable,
        "DNS leaf behind a wrapper must classify TransientRetriable (#1824)"
    );

    let err = ScraperError::Network(Box::new(TextWrapper(
        "download failed",
        Some(TextLeaf("certificate verify failed")),
    )));
    assert_eq!(
        err.classify(),
        ErrorClass::TransientRetriable,
        "TLS leaf behind a wrapper must classify TransientRetriable (#1824)"
    );
}

#[test]
fn classify_synthetic_io_other_kind_with_dns_text_is_transient_retriable() {
    // The io-downcast early-return trap (#1824): `is_transient_network`
    // returns false at the FIRST io link when the kind is not transient,
    // so the CrawlError-synthetic path (transport text INSIDE an
    // `ErrorKind::Other` message) must be covered by the explicit DNS/TLS
    // arm instead.
    let err = ScraperError::Network(Box::new(std::io::Error::other(
        "dns error: name resolution failed",
    )));
    assert_eq!(
        err.classify(),
        ErrorClass::TransientRetriable,
        "DNS text inside ErrorKind::Other must classify TransientRetriable (#1824)"
    );

    // End-to-end through the layer conversion: CrawlError::Network's
    // flattened message reaches the synthetic io error, and the class
    // survives `From<CrawlError>`.
    let crawl_err = CrawlError::Network {
        message: "tls handshake failed: certificate verify failed".to_string(),
        status_code: None,
    };
    let scraper_err = ScraperError::from(crawl_err);
    assert_eq!(
        scraper_err.classify(),
        ErrorClass::TransientRetriable,
        "TLS text through From<CrawlError> must classify TransientRetriable (#1824)"
    );
}

#[test]
fn classify_network_variant_never_internal_fatal() {
    // Pin (#1824): the Network variant is unreachable from InternalFatal —
    // exit 69 (EX_UNAVAILABLE) is the whole network family's contract.
    let cases: Vec<ScraperError> = vec![
        // Transient io kinds.
        ScraperError::Network(Box::new(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "request timeout",
        ))),
        ScraperError::Network(Box::new(std::io::Error::new(
            std::io::ErrorKind::ConnectionRefused,
            "refused",
        ))),
        // DNS + TLS flavors.
        ScraperError::Network(Box::new(std::io::Error::other(
            "dns error: name resolution failed",
        ))),
        ScraperError::Network(Box::new(std::io::Error::other("certificate expired"))),
        // Unknown transport text — conservatively unrecognized by the
        // enumerated arm, but still a transport failure
        // (EX_UNAVAILABLE), never an internal bug.
        ScraperError::Network(Box::new(std::io::Error::other(
            "genuinely unknown transport failure",
        ))),
        // The wreq-shaped connect wrapper.
        ScraperError::Network(Box::new(std::io::Error::other(
            "error sending request for uri (https://x.invalid/): client error (Connect)",
        ))),
    ];
    for err in &cases {
        assert_ne!(
            err.classify(),
            ErrorClass::InternalFatal,
            "Network variant must never classify InternalFatal (#1824), got {err:?}"
        );
    }
}
