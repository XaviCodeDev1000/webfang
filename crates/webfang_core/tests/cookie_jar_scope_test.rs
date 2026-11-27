#![cfg_attr(
    test,
    allow(clippy::unwrap_used, clippy::expect_used, clippy::disallowed_methods)
)]

//! Cookie-jar scope: how far a cookie travels (`--cookie-jar-scope`, T2a-1).
//!
//! Two levels are exercised, each where the behavior actually lives:
//!
//! - the PRODUCTION seam — [`ScopedHttpClient`] over the hardened [`HttpClient`],
//!   which is what `Container::with_cookie_jar_scope` builds — for the scope
//!   rules themselves;
//! - the `wreq` layer for the questions the decorator delegates: a `Set-Cookie`
//!   on a redirect hop, the operator seed inside a `cookie_provider` jar, and a
//!   retry re-sending the same provider.
//!
//! The `wreq`-level cases cannot run through the hardened client: its SSRF
//! redirect policy stops every hop whose target is a loopback LITERAL (the only
//! host an ephemeral local fixture can offer), so the 302 would never be
//! followed. That is the guard doing its job, not a cookie behavior; the
//! decorator's only job on that path is to install the provider, which the
//! facade cases below assert on the wire.
//!
//! Every test drives an ephemeral listener (`tests/common/wreq_invariants.rs`)
//! that journals what each arrival carried: the assertions read the server's
//! observation, never the client's opinion of itself.
//!
//! | Test | Spec question |
//! |------|---------------|
//! | `shared_scope_offers_a_cookie_set_by_a_previous_response` | the default reproduces the historical behavior |
//! | `operation_scope_does_not_survive_the_call` | two operations share nothing |
//! | `operation_scope_sends_the_operator_seed_with_the_site_cookie` | the seed reaches the wire and the site's cookie is accepted next to it |
//! | `a_set_cookie_on_a_302_hop_reaches_the_next_hop_with_the_per_request_provider` | a `Set-Cookie` on a redirect hop reaches the next hop |
//! | `the_operator_seed_and_a_site_cookie_coexist_across_a_302_hop` | the seed and a `Set-Cookie` merge instead of replacing each other |
//! | `a_retry_resends_with_the_same_per_request_provider` | the retry layer re-sends the request, so the provider and its jar survive |

#[path = "common/wreq_invariants.rs"]
mod wreq_invariants;

use std::collections::BTreeSet;
use std::sync::Arc;

use webfang_core::application::http_client::{HttpClient, HttpClientConfig, ScopedHttpClient};
use webfang_core::domain::ports::HttpClientPort;
use webfang_core::domain::JarScope;
use wreq::cookie::Jar;
use wreq_invariants::{counting_client, TestServer};

/// The hardened production client, wired as the container wires it.
fn hardened_client() -> Arc<HttpClient> {
    Arc::new(HttpClient::new(HttpClientConfig::default()).expect("the hardened client builds"))
}

/// The port facade under test, in the shape `Container::with_cookie_jar_scope`
/// builds: one hardened client, a scope, and the optional operator seed.
fn facade(scope: JarScope, seed: Option<Arc<Jar>>) -> ScopedHttpClient {
    ScopedHttpClient::new(hardened_client(), scope, seed)
}

/// A loopback URL for `path`: the tests dial the fixture directly, so no
/// resolver — ours or the system's — answers for it.
fn target(server: &TestServer, path: &str) -> String {
    format!("http://{}{path}", server.addr())
}

/// The operator seed jar, built the way `fresh_scrape_cookie_set` seeds it.
fn seeded_jar(url: &str) -> Arc<Jar> {
    let jar = Jar::default();
    jar.add("seed=1", url);
    Arc::new(jar)
}

/// The cookie pairs of the `Cookie` header an arrival carried, order-independent.
fn cookie_pairs(server: &TestServer, arrival: usize) -> BTreeSet<String> {
    server.requests()[arrival]
        .header("cookie")
        .unwrap_or_default()
        .split("; ")
        .filter(|pair| !pair.is_empty())
        .map(str::to_owned)
        .collect()
}

/// The pairs of one cookie, as the assertion below reads them.
fn pairs(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|name| (*name).to_owned()).collect()
}

/// The default scope reproduces the historical behavior: one jar for the whole
/// process, so a cookie a response sets is offered to the next request.
#[tokio::test]
async fn shared_scope_offers_a_cookie_set_by_a_previous_response() {
    // `Container::new` wires exactly this: `JarScope::default()` and no seed.
    assert_eq!(JarScope::default(), JarScope::Shared);
    let server = TestServer::start().await;
    let client = facade(JarScope::default(), None);

    client
        .get(&target(&server, "/set-cookie"))
        .await
        .expect("the cookie-setting request succeeds");
    client
        .get(&target(&server, "/echo"))
        .await
        .expect("the follow-up request succeeds");

    assert_eq!(
        cookie_pairs(&server, 0),
        pairs(&[]),
        "request 1 carries no cookie: nothing has been stored yet"
    );
    assert_eq!(
        cookie_pairs(&server, 1),
        pairs(&["probe=1"]),
        "the default `shared` scope offers the stored cookie to the next request"
    );
    // Both requests ride the same keep-alive connection and are the GETs the
    // test issued: the cookie above came from the shared jar, not from a
    // re-dial or a different method.
    assert_eq!(
        server.connection_count(),
        1,
        "one keep-alive connection served both requests"
    );
    let observed = server.requests();
    assert!(
        observed.iter().all(|entry| entry.method == "GET"),
        "every arrival is the GET the test issued"
    );
    assert!(
        observed.iter().all(|entry| entry.peer == observed[0].peer),
        "both arrivals share one peer address, so one TCP connection"
    );
}

/// `operation` buys isolation: the jar lives for one call, so two operations
/// share no cookie.
#[tokio::test]
async fn operation_scope_does_not_survive_the_call() {
    let server = TestServer::start().await;
    let client = facade(JarScope::Operation, None);

    let first = client
        .get(&target(&server, "/set-cookie"))
        .await
        .expect("the first operation succeeds");
    let second = client
        .get(&target(&server, "/echo"))
        .await
        .expect("the second operation succeeds");

    assert_eq!(first.status, 200);
    assert_eq!(second.status, 200);
    assert_eq!(
        server.paths(),
        vec!["/set-cookie", "/echo"],
        "two operations, two arrivals: nothing was cached or replayed"
    );
    assert_eq!(
        cookie_pairs(&server, 1),
        pairs(&[]),
        "the cookie of operation 1 must not reach operation 2"
    );
}

/// The seed contract, as the production facade delivers it: the operator
/// `--cookie` seed rides on the operation's request and the site's `Set-Cookie`
/// comes back next to it instead of being suppressed.
#[tokio::test]
async fn operation_scope_sends_the_operator_seed_with_the_site_cookie() {
    let server = TestServer::start().await;
    let client = facade(JarScope::Operation, Some(seeded_jar(&target(&server, "/"))));

    let response = client
        .get(&target(&server, "/set-cookie"))
        .await
        .expect("the seeded operation succeeds");

    assert_eq!(
        cookie_pairs(&server, 0),
        pairs(&["seed=1"]),
        "the operation jar starts with the operator seed"
    );
    assert_eq!(
        response
            .headers
            .get("set-cookie")
            .map(String::as_str)
            .unwrap_or_default(),
        "probe=1; Path=/",
        "the site's cookie is still delivered to the caller: the seed does not \
         shadow it"
    );
}

/// The hop the spec asks to confirm BY EXECUTING: a `Set-Cookie` delivered on a
/// 302 must be offered on the hop that follows, through the per-request
/// `cookie_provider` the decorator installs.
///
/// The redirect policy is set explicitly because `wreq 0.16.0` defaults to
/// `Policy::none()` (`src/client.rs`, `ClientBuilder::build`); the hardened
/// production client gets its 10-hop policy from `ssrf_guard().secure_client`.
#[tokio::test]
async fn a_set_cookie_on_a_302_hop_reaches_the_next_hop_with_the_per_request_provider() {
    let server = TestServer::start().await;
    let (client, _) = counting_client(vec![server.addr()], |builder| {
        builder.redirect(wreq::redirect::Policy::default())
    });

    let response = client
        .get(server.url("/redirect-cookie"))
        .cookie_provider(Jar::default())
        .send()
        .await
        .expect("the redirect is followed");

    assert_eq!(response.status(), wreq::StatusCode::OK);
    assert_eq!(
        server.paths(),
        vec!["/redirect-cookie", "/landing"],
        "the 302 was followed once, inside the same call"
    );
    assert_eq!(
        cookie_pairs(&server, 0),
        pairs(&[]),
        "the first hop starts with nothing stored"
    );
    assert_eq!(
        cookie_pairs(&server, 1),
        pairs(&["hop=1"]),
        "the `Set-Cookie` of the 302 reaches the next hop through the \
         per-request provider"
    );
}

/// The seed merges with what the site sets: a jar seeded with `--cookie` cookies
/// accumulates the site's `Set-Cookie` on top, and neither replaces the other.
#[tokio::test]
async fn the_operator_seed_and_a_site_cookie_coexist_across_a_302_hop() {
    let server = TestServer::start().await;
    let (client, _) = counting_client(vec![server.addr()], |builder| {
        builder.redirect(wreq::redirect::Policy::default())
    });

    let response = client
        .get(server.url("/redirect-cookie"))
        .cookie_provider(seeded_jar(&server.url("/")))
        .send()
        .await
        .expect("the seeded redirect is followed");

    assert_eq!(response.status(), wreq::StatusCode::OK);
    assert_eq!(
        cookie_pairs(&server, 0),
        pairs(&["seed=1"]),
        "hop 1 carries the operator seed"
    );
    assert_eq!(
        cookie_pairs(&server, 1),
        pairs(&["seed=1", "hop=1"]),
        "hop 2 carries the seed AND the site's cookie: the seed is merged, \
         never replaced"
    );
}

/// The retry question (`wreq::retry::Policy`, the only retry layer that re-sends
/// a request carrying a `cookie_provider`): a retry goes out with the SAME
/// provider, so a cookie stored by the attempt that failed is still offered on
/// the attempt that follows.
#[tokio::test]
async fn a_retry_resends_with_the_same_per_request_provider() {
    let server = TestServer::start().await;
    let policy = wreq::retry::Policy::default().classify_fn(|req_rep| match req_rep.status() {
        Some(wreq::StatusCode::SERVICE_UNAVAILABLE) => req_rep.retryable(),
        _ => req_rep.success(),
    });
    let (client, _) = counting_client(vec![server.addr()], |builder| builder.retry(policy));

    let response = client
        .get(server.url("/flaky-cookie"))
        .cookie_provider(Jar::default())
        .send()
        .await
        .expect("the retried request still resolves to a response");

    assert_eq!(response.status(), wreq::StatusCode::OK);
    assert_eq!(
        server.count_path("/flaky-cookie"),
        2,
        "the 503 was classified as retryable and re-sent"
    );
    assert_eq!(
        cookie_pairs(&server, 0),
        pairs(&[]),
        "the first attempt starts with an empty per-request jar"
    );
    assert_eq!(
        cookie_pairs(&server, 1),
        pairs(&["probe=1"]),
        "the retry re-sends with the same provider, so the `Set-Cookie` of the \
         failed attempt is offered on the retried one"
    );
}
