use super::*;

#[test]
fn a_refused_connection_is_classified_by_its_io_kind() {
    let err = std::io::Error::new(io::ErrorKind::ConnectionRefused, "refused");
    assert_eq!(
        classify(&err, FetchPhase::Connect),
        NetworkCause::ConnectRefused
    );
}

#[test]
fn a_reset_or_an_eof_is_a_closed_connection() {
    for kind in [
        io::ErrorKind::ConnectionReset,
        io::ErrorKind::UnexpectedEof,
        io::ErrorKind::BrokenPipe,
    ] {
        let err = std::io::Error::new(kind, "closed");
        assert_eq!(
            classify(&err, FetchPhase::Connect),
            NetworkCause::ConnectionClosed,
            "kind {kind:?} must classify as a closed connection"
        );
    }
}

#[test]
fn the_phase_decides_a_timeout() {
    let err = std::io::Error::new(io::ErrorKind::TimedOut, "timed out");
    assert_eq!(
        classify(&err, FetchPhase::Connect),
        NetworkCause::ConnectTimeout
    );
    assert_eq!(classify(&err, FetchPhase::Body), NetworkCause::ReadTimeout);
}

#[test]
fn the_resolvers_own_failure_decides_the_dns_class() {
    let err = DnsResolutionError::new(Box::new(std::io::Error::other("no such host")));
    assert_eq!(
        classify(&err, FetchPhase::Connect),
        NetworkCause::DnsResolution
    );
}

#[test]
fn an_unreadable_chain_is_unknown() {
    let err = std::io::Error::other("something else entirely");
    assert_eq!(classify(&err, FetchPhase::Connect), NetworkCause::Unknown);
}

#[test]
fn a_plain_error_without_io_is_unknown() {
    #[derive(Debug)]
    struct Plain;
    impl fmt::Display for Plain {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("plain")
        }
    }
    impl Error for Plain {}

    assert_eq!(classify(&Plain, FetchPhase::Connect), NetworkCause::Unknown);
}
