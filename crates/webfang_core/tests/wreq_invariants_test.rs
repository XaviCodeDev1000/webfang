#![cfg_attr(
    test,
    allow(clippy::unwrap_used, clippy::expect_used, clippy::disallowed_methods)
)]

//! `wreq 0.16.0` client invariants, pinned against the REAL client (#3082).
//!
//! `wreq` is the only HTTP engine WebFang may use (AGENTS.md § Fetch guard
//! chain), and five of its behaviours are load-bearing for that chain. Each
//! one is asserted here at the `wreq` level — never through `Container` — so a
//! later cookie-scope default flip (T2b) can neither break nor mask them:
//!
//! | Test | Invariant |
//! |------|-----------|
//! | `three_requests_over_one_client_use_one_resolver_call` | keep-alive: three requests, one dial, one `Resolve` call |
//! | `a_new_client_resolves_for_itself` | reuse belongs to the `Client`, not to the process |
//! | `connections_go_to_the_resolved_socket_addr` | the dial targets the addr the resolver answered with |
//! | `the_client_cookie_jar_is_shared_between_requests` | one `Set-Cookie` serves later requests with no per-request provider |
//! | `retry_layer_resends_against_real_wreq` | a `Retryable` classification reaches the server twice; the caller still gets a response |
//!
//! Every test owns ephemeral loopback listeners and a resolver that answers
//! from the test itself: no name is ever looked up, no real DNS, no packet
//! leaves the host, and ordering is driven by awaiting the server's observed
//! request — never by `sleep`.
//!
//! Sources, read in the pinned registry copy `wreq-0.16.0` (the version this
//! tree compiles), not from memory of another release:
//!
//! - `ClientBuilder::dns_resolver` (`src/client.rs`): the custom-resolver hook
//!   accepting any `IntoResolve`; per-name overrides configured with
//!   `resolve` / `resolve_to_addrs` still apply on top of it.
//! - `Resolve` (`src/dns/resolve.rs`): the resolver is assumed ALWAYS READY to
//!   poll and takes `&self`, so the counter here is an atomic — never a lock
//!   held across an `.await`.
//! - `ClientBuilder::cookie_provider` (`src/client.rs`) contrasts with
//!   `RequestBuilder::cookie_provider` (`src/client/request.rs`): a
//!   request-level store REPLACES the client jar for that request.
//! - `ClientBuilder::retry` (`src/client.rs`) with `retry::Policy`'s
//!   `classify_fn`, whose `Classify` returns `Action::{Success, Retryable}`
//!   (`src/client/layer/retry/classify.rs`). The public surface is the builder
//!   method: `RetryPolicy`/`RetryLayer` under `src/client/layer/retry.rs` are
//!   wired internally and are not nameable from here.
//!
//! Noted, deliberately NOT asserted: in 0.16.0 the TCP timeout is shared
//! across the attempts of one dial (Happy Eyeballs race), so a slow first
//! attempt can consume the budget of the second. Nothing here depends on it.

#[path = "common/wreq_invariants.rs"]
mod wreq_invariants;

use std::sync::atomic::Ordering;

use wreq_invariants::{counting_client, TestServer};

// ---------------------------------------------------------------------------
// Keep-alive reuse: three sequential requests over ONE client = one dial.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn three_requests_over_one_client_use_one_resolver_call() {
    let server = TestServer::start().await;
    let (client, calls) = counting_client(vec![server.addr()], |builder| builder);

    for path in ["/p1", "/p2", "/p3"] {
        let body = client
            .get(server.url(path))
            .send()
            .await
            .expect("the loopback request succeeds")
            .text()
            .await
            .expect("the loopback body decodes");
        assert_eq!(body, "ok", "the fixture body must reach the caller");
    }

    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "three requests on one keep-alive connection resolve once: the pool \
         reuses the dial, it does not re-resolve per request"
    );
    assert_eq!(
        server.connection_count(),
        1,
        "one connection served all three requests"
    );
    let observed = server.requests();
    assert_eq!(server.paths(), vec!["/p1", "/p2", "/p3"]);
    assert!(
        observed.iter().all(|entry| entry.method == "GET"),
        "every arrival is the GET the test issued"
    );
    assert!(
        observed.iter().all(|entry| entry.peer == observed[0].peer),
        "all three requests share one peer address, so they share one TCP \
         connection and not three sequential dials"
    );
}

// ---------------------------------------------------------------------------
// Reuse is a property of the Client, not of the process.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_new_client_resolves_for_itself() {
    let server = TestServer::start().await;
    let (first_client, first_calls) = counting_client(vec![server.addr()], |builder| builder);
    let (second_client, second_calls) = counting_client(vec![server.addr()], |builder| builder);

    for (client, path) in [
        (&first_client, "/from-first"),
        (&second_client, "/from-second"),
    ] {
        let _ = client
            .get(server.url(path))
            .send()
            .await
            .expect("the loopback request succeeds")
            .text()
            .await
            .expect("the loopback body decodes");
    }

    assert_eq!(
        first_calls.load(Ordering::SeqCst),
        1,
        "the first client resolved its own dial"
    );
    assert_eq!(
        second_calls.load(Ordering::SeqCst),
        1,
        "a NEW client resolves for itself instead of inheriting the first \
         client's dial"
    );
    assert_eq!(
        server.connection_count(),
        2,
        "two clients, two connections: nothing is shared across clients"
    );
    assert_eq!(server.paths(), vec!["/from-first", "/from-second"]);
}

// ---------------------------------------------------------------------------
// The dial goes to the SocketAddr the resolver returned.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn connections_go_to_the_resolved_socket_addr() {
    let first = TestServer::start().await;
    let second = TestServer::start().await;
    // The resolver answers `first` on its first call and `second` on its
    // second. A re-resolution — or a dial that ignored the answer — becomes
    // visible as an arrival on `second`.
    let (client, calls) = counting_client(vec![first.addr(), second.addr()], |builder| builder);

    let response = client
        .get(first.url("/where"))
        .send()
        .await
        .expect("the loopback request succeeds");
    assert_eq!(
        response.remote_addr(),
        Some(first.addr()),
        "the client reports the peer it dialed"
    );
    let _ = response.text().await.expect("the loopback body decodes");

    assert_eq!(first.paths(), vec!["/where"]);
    assert_eq!(
        second.connection_count(),
        0,
        "the request landed on the addr the resolver returned, not on the \
         next answer it would have produced"
    );

    // A second request on the same client reuses the pooled connection, so
    // there is still no second resolution and still no arrival on the
    // alternate answer.
    let _ = client
        .get(first.url("/where-again"))
        .send()
        .await
        .expect("the second loopback request succeeds")
        .text()
        .await
        .expect("the loopback body decodes");

    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "one dial, one resolve call"
    );
    assert_eq!(second.connection_count(), 0, "still no re-resolution");
    assert_eq!(first.paths(), vec!["/where", "/where-again"]);
}

// ---------------------------------------------------------------------------
// The Client cookie jar is shared by the requests it issues.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_client_cookie_jar_is_shared_between_requests() {
    let server = TestServer::start().await;
    let (client, _) = counting_client(vec![server.addr()], |builder| {
        builder.cookie_provider(wreq::cookie::Jar::default())
    });

    let first = client
        .get(server.url("/set-cookie"))
        .send()
        .await
        .expect("the cookie-setting request succeeds");
    assert_eq!(first.status(), wreq::StatusCode::OK);
    let _ = first.text().await.expect("the loopback body decodes");

    // No per-request provider: whatever request 2 carries came from the
    // client-level jar.
    let second = client
        .get(server.url("/echo"))
        .send()
        .await
        .expect("the follow-up request succeeds");
    assert_eq!(second.status(), wreq::StatusCode::OK);
    let _ = second.text().await.expect("the loopback body decodes");

    let observed = server.requests();
    assert_eq!(observed.len(), 2, "two arrivals");
    assert_eq!(
        observed[0].header("cookie"),
        None,
        "request 1 carries no cookie: nothing has been stored yet"
    );
    assert_eq!(
        observed[1].header("cookie"),
        Some("probe=1"),
        "the Set-Cookie of response 1 is sent on request 2 by the shared jar"
    );
}

// ---------------------------------------------------------------------------
// RetryLayer against real wreq: classification reaches the server twice.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn retry_layer_resends_against_real_wreq() {
    let server = TestServer::start().await;
    let policy = wreq::retry::Policy::default().classify_fn(|req_rep| match req_rep.status() {
        Some(wreq::StatusCode::SERVICE_UNAVAILABLE) => req_rep.retryable(),
        _ => req_rep.success(),
    });
    let (client, _) = counting_client(vec![server.addr()], |builder| builder.retry(policy));

    let response = client
        .get(server.url("/flaky"))
        .send()
        .await
        .expect("a retried request still resolves to a response");
    assert_eq!(
        response.status(),
        wreq::StatusCode::OK,
        "the caller receives the retried response, not the refused one"
    );
    let _ = response.text().await.expect("the loopback body decodes");

    assert_eq!(
        server.count_path("/flaky"),
        2,
        "a `Retryable` classification resends: the server saw 503 then 200"
    );

    // Control: the same 503 with the default policy (no `Retryable`
    // classification) reaches the caller on the first attempt, so the arrival
    // count above is the retry and not something the fixture does on its own.
    let control = TestServer::start().await;
    let (plain_client, _) = counting_client(vec![control.addr()], |builder| builder);
    let response = plain_client
        .get(control.url("/flaky"))
        .send()
        .await
        .expect("the first attempt returns the 503 response");
    assert_eq!(response.status(), wreq::StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        control.count_path("/flaky"),
        1,
        "no retry without a retryable classification"
    );
}
