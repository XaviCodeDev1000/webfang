use super::*;

#[test]
fn every_value_has_a_stable_wire_name() {
    for cause in NetworkCause::ALL {
        assert_eq!(
            NetworkCause::from_value(cause.as_str()),
            Some(cause),
            "the wire name of {cause:?} must round-trip"
        );
    }
}

#[test]
fn the_wire_names_are_the_documented_ones() {
    let names: Vec<&str> = NetworkCause::ALL.iter().map(|c| c.as_str()).collect();
    assert_eq!(
        names,
        vec![
            "dns_resolution",
            "connect_refused",
            "connect_timeout",
            "read_timeout",
            "connection_closed",
            "tls_handshake",
            "unknown",
        ],
        "the taxonomy's wire names are a public contract"
    );
}

#[test]
fn the_user_facing_messages_are_spanish() {
    let messages: Vec<&str> = NetworkCause::ALL
        .iter()
        .map(|cause| cause.message())
        .collect();
    assert!(messages.iter().all(|message| !message.is_empty()));
    // The issue's first acceptance criterion, pinned here: two causes must
    // never produce the same sentence, or the message stops being evidence.
    let unique: std::collections::BTreeSet<&str> = messages.iter().copied().collect();
    assert_eq!(
        unique.len(),
        messages.len(),
        "two causes must never share a message: {messages:?}"
    );
    assert_eq!(
        NetworkCause::ConnectRefused.message(),
        "el servidor rechazó la conexión"
    );
}

/// #2784: a carrier travelling in an I/O error's PAYLOAD is still found.
///
/// An `io::Error` does not expose its payload as a source: `source()`
/// returns the payload's own source. The carrier is therefore invisible to
/// a plain chain walk, and the walk has to look into `get_ref()`. The
/// `Io` variant relies on this to keep the error's `kind` — the retry
/// rule's input — so the mechanism is pinned here.
#[test]
fn a_carrier_in_an_io_errors_payload_is_still_found() {
    let carrier = ClassifiedNetworkError::new(
        NetworkCause::ConnectRefused,
        Box::new(io::Error::other("connection refused")),
    );
    let error = crate::domain::downloader_port::DownloadError::Io(io::Error::new(
        io::ErrorKind::ConnectionRefused,
        carrier,
    ));

    assert_eq!(
        cause_in_chain(&error),
        Some(NetworkCause::ConnectRefused),
        "the carrier travels in the I/O error's payload"
    );
    assert!(
        error
            .to_string()
            .contains("el servidor rechazó la conexión"),
        "the message must name the cause: {error}"
    );
    assert_eq!(
        error.classify(),
        crate::domain::error::ErrorClass::InternalFatal,
        "the kind survives the carrier: a refused connection is not retried"
    );
}

#[test]
fn the_classifier_reads_the_cause_out_of_the_chain() {
    let cause = NetworkCause::DnsResolution;
    let source = std::io::Error::other("name resolution failed");
    let classified = ClassifiedNetworkError::new(cause, Box::new(source));
    assert_eq!(cause_in_chain(&classified), Some(cause));
}

#[test]
fn an_unclassified_chain_has_no_cause() {
    let plain = std::io::Error::other("something else");
    assert_eq!(cause_in_chain(&plain), None);
}
