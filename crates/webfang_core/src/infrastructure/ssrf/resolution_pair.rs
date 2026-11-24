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
    use super::super::name_source::LookupFuture;
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

    /// A name source that answers at once AND reports when the answer is back.
    ///
    /// #3026: the nightly ignored suite must not race the resolution mark
    /// against the connect deadline. The signal lets the test observe that the
    /// name came back before it reads the marker; `Notify` stores a permit if
    /// `notify_one` wins the race, so the wakeup cannot be lost.
    #[derive(Debug)]
    struct SignalingNameSource {
        addrs: Vec<std::net::SocketAddr>,
        answered: Arc<tokio::sync::Notify>,
    }

    impl NameSource for SignalingNameSource {
        fn lookup(&self, _host: String) -> LookupFuture {
            let addrs = self.addrs.clone();
            let answered = Arc::clone(&self.answered);
            Box::pin(async move {
                answered.notify_one();
                Ok(addrs)
            })
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

    /// A name source that never answers, counting how many times it was asked.
    ///
    /// The count is the observable the retry accounting needs: one lookup per
    /// connection attempt, so it says how many attempts the downloader made
    /// without a clock and without a network (#2823).
    #[derive(Debug)]
    struct CountingHangingNameSource {
        calls: Arc<std::sync::atomic::AtomicUsize>,
    }

    impl NameSource for CountingHangingNameSource {
        fn lookup(&self, _host: String) -> LookupFuture {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Box::pin(std::future::pending())
        }
    }

    /// A broken resolver spends its OWN budget, not the site-rejection ladder.
    ///
    /// This is #2823 in one number: a name that never comes back costs
    /// `1 + DNS_RETRY_BUDGET` lookups, whatever `max_retries` says. The
    /// exponential ladder is what turned a broken DNS into a WAF-looking
    /// slowdown of a whole 50-URL run.
    ///
    /// The client is injected through the `NameSource` seam, so the failure is
    /// deterministic and no network is involved.
    #[cfg_attr(miri, ignore = "btls-sys FFI (wreq Client) not supported by Miri")]
    #[tokio::test]
    async fn a_hung_resolver_spends_its_own_budget_not_the_site_ladder() {
        use crate::domain::downloader_port::Downloader;
        use crate::domain::error::mapping::is_resolution_failure;
        use crate::infrastructure::downloader::wreq_downloader::dns_budget::DNS_RETRY_BUDGET;
        use crate::infrastructure::downloader::wreq_downloader::WreqDownloader;
        use std::sync::atomic::Ordering;

        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let client = wreq::Client::builder()
            .connect_timeout(std::time::Duration::from_millis(300))
            .dns_resolver(resolver_over(Arc::new(CountingHangingNameSource {
                calls: Arc::clone(&calls),
            })))
            .build()
            .expect("the test client must build");

        // Three site retries at 1 ms: if the ladder were charged for the
        // resolution, this would fire four lookups. The DNS budget must win.
        let downloader = WreqDownloader::from_client_with_retry(client, 5, 3, 1, 1);
        let url = url::Url::parse("http://webfang-dns-budget.test/").expect("the test URL parses");

        let err = downloader
            .fetch(&url)
            .await
            .expect_err("a name that never comes back must fail the fetch");

        assert!(
            is_resolution_failure(&err),
            "the failure must be attributed to the resolution: {err}"
        );
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1 + DNS_RETRY_BUDGET as usize,
            "a resolution failure spends its own budget, never the site-rejection ladder"
        );
    }

    /// THE SECOND SIDE: the accept queue is full, so the SYN is dropped.
    ///
    /// The name came BACK (the resolution completed) and the handshake never
    /// completed: this failure is NEVER a DNS failure. The marker is what keeps
    /// the two apart — the resolution left no evidence behind, so DNS cannot
    /// be blamed.
    ///
    /// #3026: `wreq` does not always expose an `io::Error` kind for a dropped
    /// SYN (its chain is sometimes just `client error (Connect)`), so the raw
    /// classification can be `ConnectTimeout` or `Unknown` depending on the
    /// runner. The positive `connect_timeout` verdict is pinned by the
    /// deterministic seam tests in `transport_seam.rs` and `resolution.rs`;
    /// this probe asserts the invariant that survives either chain shape.
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
    async fn a_dropped_syn_is_never_a_dns_failure() {
        let url =
            url::Url::parse("http://webfang-accept-queue.test/").expect("the test URL parses");
        // TEST-NET-1: never routable, so the SYN goes nowhere and the connect
        // deadline fires — with a resolution that DID come back.
        let addrs = vec![std::net::SocketAddr::new(
            std::net::IpAddr::V4(std::net::Ipv4Addr::new(192, 0, 2, 1)),
            80,
        )];
        let answered = Arc::new(tokio::sync::Notify::new());
        let client = wreq::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(2))
            .dns_resolver(resolver_over(Arc::new(SignalingNameSource {
                addrs,
                answered: Arc::clone(&answered),
            })))
            .build()
            .expect("the test client must build");

        let request_started = std::time::Instant::now();
        let request = client.get(url.as_str()).send();
        // Drive the request and the answer signal together: the request future
        // must be polled for the resolver to run at all.
        let ((), outcome) = tokio::join!(
            async {
                tokio::time::timeout(std::time::Duration::from_secs(1), answered.notified())
                    .await
                    .expect("the injected resolver must answer before the connect deadline")
            },
            request
        );
        let err = outcome.expect_err("an address that never answers must fail the request");
        let raw_cause = crate::domain::error::cause::classify(
            &err,
            crate::domain::error::cause::FetchPhase::Connect,
        );
        let marker = crate::domain::error::resolution::resolution_tracker()
            .state("webfang-accept-queue.test", request_started);
        let detail = err.to_string();

        assert_eq!(
            marker,
            crate::domain::error::resolution::ResolutionState::None,
            "the name came back, so the marker must hold no DNS evidence"
        );
        let cause = cause_of(err, &url, request_started);
        assert!(
            !matches!(cause, NetworkCause::DnsResolution),
            "a resolved name must never be blamed on DNS (cause={cause:?}, raw={raw_cause:?}, error={detail})"
        );
    }

    /// The deterministic half of the dropped-SYN probe: a name that DID come
    /// back leaves no marker, so a connect timeout stays a connect timeout
    /// (#3026). It lives here rather than in `resolution.rs`, which is at the
    /// file-size ratchet limit.
    #[test]
    fn a_finished_resolution_keeps_the_connect_timeout() {
        let host = "webfang-finished-resolution.test";
        let request_started = std::time::Instant::now();
        let mut mark = crate::domain::error::resolution::resolution_tracker().begin(host);
        mark.finished(true);
        drop(mark);

        assert_eq!(
            crate::domain::error::resolution::refine_connect_cause(
                NetworkCause::ConnectTimeout,
                host,
                request_started,
                true,
            ),
            NetworkCause::ConnectTimeout
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
