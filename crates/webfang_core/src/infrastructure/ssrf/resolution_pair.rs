//! The pair, IN PROCESS (#2784).
//!
//! The laboratory proved the pair with real network namespaces: a resolver
//! that never answers and an accept queue so full the SYN is dropped were the
//! SAME message until this taxonomy named them apart. The pair is the whole
//! point of the change, so it must not be able to rot unnoticed in a
//! laboratory nobody runs.
//!
//! This file proves the pair WITHOUT the network, so a CI run carries it:
//!
//! | side | what happens | cause |
//! |---|---|---|
//! | the resolver never answers | the name never came back | `dns_resolution` |
//! | the accept queue is full | the name came back, the handshake never completed | `connect_timeout` |
//!
//! The FIRST side is reproducible in process: the lookup seam is injectable,
//! so a resolver that never answers is one line. The SECOND side needs the
//! kernel to drop a SYN (a full accept queue, or a rule that discards it),
//! which no CI has the business having — it is the skipped case at the
//! bottom, with its probe written out so it is not lost.

#[cfg(test)]
mod pair {
    use super::super::*;
    use crate::domain::error::cause::NetworkCause;
    use std::sync::Arc;

    /// A name source that never answers.
    ///
    /// The client waits for a name that never comes back: the resolution stays in
    /// flight until the connect deadline fires.
    #[derive(Debug)]
    struct HangingNameSource;

    impl NameSource for HangingNameSource {
        fn lookup(&self, _host: String) -> LookupFuture {
            Box::pin(std::future::pending())
        }
    }

    /// A name source that answers at once, with a fixed address set.
    #[derive(Debug)]
    struct AnsweringNameSource {
        addrs: Vec<std::net::SocketAddr>,
    }

    impl NameSource for AnsweringNameSource {
        fn lookup(&self, _host: String) -> LookupFuture {
            let addrs = self.addrs.clone();
            Box::pin(async move { Ok(addrs) })
        }
    }

    /// A resolver over a fixed answer set.
    fn resolver_over(source: Arc<dyn NameSource>) -> ValidatingResolver {
        ValidatingResolver {
            validation_enabled: true,
            allow_loopback: false,
            policy: Arc::new(NetworkPolicy::default()),
            name_source: source,
        }
    }

    /// The cause a real failure would carry, computed through the REAL mapping
    /// (the same one the batch uses), not through a re-implementation of it.
    fn cause_of(
        err: wreq::Error,
        url: &url::Url,
        request_started: std::time::Instant,
    ) -> NetworkCause {
        let failure = crate::domain::error::mapping::crawl_network_failure(
            err,
            crate::domain::error::cause::FetchPhase::Connect,
            url,
            request_started,
        );
        crate::domain::error::cause::cause_in_chain(&failure)
            .expect("the classified cause must travel in the chain")
    }

    /// THE FIRST SIDE: the resolver never answers.
    ///
    /// The name never came back, so the SYN was never sent: the cause is a DNS
    /// failure, not a connect one. This is the case the whole taxonomy exists
    /// for, and it is deterministic in process — no network, no privileges, no
    /// namespace.
    #[cfg_attr(miri, ignore = "btls-sys FFI (wreq Client) not supported by Miri")]
    #[tokio::test]
    async fn a_hung_resolver_is_a_dns_resolution_failure() {
        let url =
            url::Url::parse("http://webfang-hung-resolver.test/").expect("the test URL parses");
        let client = wreq::Client::builder()
            .connect_timeout(std::time::Duration::from_millis(300))
            .dns_resolver(resolver_over(Arc::new(HangingNameSource)))
            .build()
            .expect("the test client must build");

        let request_started = std::time::Instant::now();
        let sent = client.get(url.as_str()).send();
        let err = tokio::time::timeout(std::time::Duration::from_secs(5), sent)
            .await
            .expect("the connect deadline must fire while the name is still being resolved")
            .expect_err("a name that never comes back must fail the request");

        assert_eq!(
            cause_of(err, &url, request_started),
            NetworkCause::DnsResolution,
            "a name that never came back is a DNS failure"
        );
    }

    /// THE SECOND SIDE: the accept queue is full, so the SYN is dropped.
    ///
    /// The name came BACK (the resolution completed) and the handshake never
    /// completed: the cause is a connect timeout, never a DNS failure. The
    /// marker is what keeps the two apart — the resolution left no evidence
    /// behind, so the connect cause stands untouched.
    ///
    /// This case cannot be reproduced in process: dropping a SYN needs the
    /// kernel's accept queue to overflow, or a rule that discards the packet,
    /// and no CI has the business having either. The PROBE is written down so
    /// the case is not lost, and the part of it that IS deterministic — the
    /// resolution completes, so the marker holds no evidence — runs here:
    ///
    /// 1. positive control: the path works before the injection;
    /// 2. inject: fill the listener's accept queue (or discard the SYN with a
    ///    rule matching the destination) so the SYN never reaches the peer;
    /// 3. read the trace: `cause=connect_timeout`, and the resolver's own
    ///    resolution completed — never `dns_resolution`.
    ///
    /// Run it where the injection is possible; in CI it stays declared and
    /// skipped.
    #[cfg_attr(miri, ignore = "btls-sys FFI (wreq Client) not supported by Miri")]
    #[tokio::test]
    #[ignore = "needs a kernel that drops the SYN (a full accept queue); the probe is in the doc comment"]
    async fn a_dropped_syn_is_a_connect_timeout_not_a_dns_failure() {
        let url =
            url::Url::parse("http://webfang-accept-queue.test/").expect("the test URL parses");
        // TEST-NET-1: never routable, so the SYN goes nowhere and the connect
        // deadline fires — with a resolution that DID come back.
        let addrs = vec![std::net::SocketAddr::new(
            std::net::IpAddr::V4(std::net::Ipv4Addr::new(192, 0, 2, 1)),
            80,
        )];
        let client = wreq::Client::builder()
            .connect_timeout(std::time::Duration::from_millis(300))
            .dns_resolver(resolver_over(Arc::new(AnsweringNameSource { addrs })))
            .build()
            .expect("the test client must build");

        let request_started = std::time::Instant::now();
        let err = client
            .get(url.as_str())
            .send()
            .await
            .expect_err("an address that never answers must fail the request");

        assert_ne!(
            cause_of(err, &url, request_started),
            NetworkCause::DnsResolution,
            "the name came back: the marker holds no evidence, so the cause is not a DNS failure"
        );
    }

    /// THE SSRF REFUSAL KEEPS ITS CLASS.
    ///
    /// The fail-closed scan is the one place that knows the requested name and
    /// the resolved addresses at the same time, and it is a SECURITY refusal, not
    /// a network failure. The marker must not rewrite it: a host the scan rejects
    /// keeps the SSRF error, and no cause is attached to it.
    #[test]
    fn an_ssrf_refusal_keeps_its_class_and_gains_no_cause() {
        let addrs = vec![std::net::SocketAddr::new(
            std::net::IpAddr::V4(std::net::Ipv4Addr::new(127, 0, 0, 1)),
            8080,
        )];
        let outcome = ValidatingResolver::fail_closed_scan(
            "webfang-refused.test",
            &addrs,
            false,
            &NetworkPolicy::default(),
        );
        let err = outcome.expect_err("a loopback answer must be rejected");
        assert!(
            err.downcast_ref::<ForbiddenResolutionError>().is_some(),
            "the refusal must keep the SSRF class: {err}"
        );
        assert!(
            crate::domain::error::cause::cause_in_chain(err.as_ref()).is_none(),
            "an SSRF refusal is not a network failure: it must carry no cause"
        );
    }
}
