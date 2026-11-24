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
fn test_download_error_display() {
    let err = DownloadError::Http {
        status: 403,
        message: "forbidden".into(),
    };
    assert!(err.to_string().contains("403"));
}

#[test]
fn test_download_error_classify() {
    use crate::domain::error::ErrorClass;
    let dns = DownloadError::dns("NXDOMAIN");
    assert_eq!(dns.classify(), ErrorClass::PermanentFatal);
    let timeout = DownloadError::timeout(5);
    assert_eq!(timeout.classify(), ErrorClass::TransientBackoff);
}

/// #2824: a copy keeps the class AND the cause.
///
/// The class is what the retry rule reads — never the cause — so a copy
/// must never move it. The cause, on the other hand, IS the diagnosis: a
/// copy that dropped it would turn a classified refusal into an anonymous
/// network error at the application boundary (#2784 classified it, #2824
/// keeps it across the copy).
#[test]
fn a_clone_keeps_the_class_and_the_cause() {
    use crate::domain::error::cause::{cause_in_chain, ClassifiedNetworkError, NetworkCause};
    use crate::domain::error::ErrorClass;

    let carrier = ClassifiedNetworkError::new(
        NetworkCause::ConnectRefused,
        Box::new(std::io::Error::other("connection refused")),
    );
    let original = DownloadError::Network(Box::new(carrier));
    assert_eq!(original.classify(), ErrorClass::InternalFatal);

    let copy = original.clone();
    assert_eq!(
        copy.classify(),
        ErrorClass::InternalFatal,
        "the class must not move: the retry rule reads it"
    );
    assert!(
        copy.to_string()
            .contains("the server refused the connection"),
        "the message must survive the copy: {copy}"
    );
    assert_eq!(
        cause_in_chain(&copy),
        Some(NetworkCause::ConnectRefused),
        "the cause must survive the copy: {copy}"
    );
}

/// #2824 S1: the copy keeps the cause in EVERY transport variant.
///
/// The five variants are the ones `attach_cause` classifies: `Network`,
/// `Io`, `Dns`, `Tls` and `Timeout`. A copy that kept the cause in four of
/// them and dropped it in the fifth would still lose the diagnosis on some
/// failures — the retry clones the error, so the copy IS the error the
/// application sees.
#[test]
fn a_copy_keeps_the_cause_in_every_transport_variant() {
    use crate::domain::error::cause::{cause_in_chain, NetworkCause};
    use crate::domain::error::mapping::attach_cause;

    let cases = [
        DownloadError::Network(Box::new(std::io::Error::other("boom"))),
        DownloadError::Io(std::io::Error::new(
            std::io::ErrorKind::ConnectionRefused,
            "connection refused",
        )),
        DownloadError::dns("name resolution failed"),
        DownloadError::tls("handshake failed"),
        DownloadError::timeout(30),
    ];

    for error in cases {
        let classified = attach_cause(error, NetworkCause::ConnectRefused);
        let copy = classified.clone();
        assert_eq!(
            cause_in_chain(&copy),
            Some(NetworkCause::ConnectRefused),
            "the cause must survive the copy of {classified}"
        );
    }
}

/// #2824 S2: the plain `source()` walk from `Io` reaches the typed cause.
///
/// `std::io::Error` does NOT publish its payload as a source: `source()`
/// returns the payload's OWN source. A consumer that walks the chain
/// without knowing that quirk (the trace, `CrawlErrorCategory`, an
/// external consumer) must still reach the typed cause.
#[test]
fn the_source_walk_from_io_reaches_the_typed_cause() {
    use crate::domain::error::cause::{ClassifiedNetworkError, NetworkCause};
    use crate::domain::error::mapping::attach_cause;

    let classified = attach_cause(
        DownloadError::Io(std::io::Error::new(
            std::io::ErrorKind::ConnectionRefused,
            "connection refused",
        )),
        NetworkCause::ConnectRefused,
    );

    let mut current = std::error::Error::source(&classified);
    let mut found = None;
    while let Some(error) = current {
        if let Some(carrier) = error.downcast_ref::<ClassifiedNetworkError>() {
            found = Some(carrier.cause());
            break;
        }
        current = error.source();
    }
    assert_eq!(
        found,
        Some(NetworkCause::ConnectRefused),
        "the plain source() walk must reach the typed cause: {classified}"
    );
}

/// #2824 S4: the `ErrorKind` is the retry rule's input — it must not move.
///
/// Neither the classification nor the copy may touch the kind: the rule
/// reads the kind, and the kind is what makes a failure retriable.
#[test]
fn the_io_kind_survives_the_classification_and_the_copy() {
    use crate::domain::error::cause::NetworkCause;
    use crate::domain::error::mapping::attach_cause;

    let classified = attach_cause(
        DownloadError::Io(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "unexpected eof",
        )),
        NetworkCause::ConnectionClosed,
    );

    let DownloadError::Io(original) = &classified else {
        panic!("the variant must not move: {classified}");
    };
    assert_eq!(original.kind(), std::io::ErrorKind::UnexpectedEof);

    let copy = classified.clone();
    let DownloadError::Io(copied) = &copy else {
        panic!("the variant must not move: {copy}");
    };
    assert_eq!(
        copied.kind(),
        std::io::ErrorKind::UnexpectedEof,
        "the kind must survive the copy: {copy}"
    );
}

#[test]
fn test_downloader_trait_is_dyn_compatible() {
    // Compile-time proof that `dyn Downloader` is object-safe via BoxFuture
    #[allow(dead_code)]
    fn assert_dyn(_: &dyn Downloader) {}
    // If this compiles, the trait is dyn-compatible
    assert_eq!(1, 1);
}
