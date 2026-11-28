#![cfg_attr(
    test,
    allow(clippy::unwrap_used, clippy::expect_used, clippy::disallowed_methods)
)]

//! Cookie jar scope `session`: one jar per caller session (T2a-2).
//!
//! The subject is the production seam — [`ScopedHttpClient`] over the hardened
//! [`HttpClient`], which is what `Container::with_cookie_jar_scope` builds and
//! what the MCP server serves from — driven against an ephemeral loopback
//! listener that journals what each arrival carried. Every assertion reads the
//! SERVER's observation, never the client's opinion of itself.
//!
//! | Test | Spec question |
//! |------|---------------|
//! | `session_scope_offers_a_cookie_set_by_a_previous_call_of_the_same_session` | (b) call 1 → call 2 inside one session |
//! | `session_scope_never_offers_another_sessions_cookie` | (a) session A's cookie never reaches session B |
//! | `a_session_scoped_facade_without_a_session_id_uses_the_process_jar` | (c) no `Mcp-Session-Id` → the documented `shared` fallback |
//! | `session_scope_sends_the_operator_seed_next_to_the_site_cookie` | (e) `--cookie` and `Set-Cookie` coexist |
//! | `releasing_the_lease_drops_the_cookie_a_later_binding_gets` | (d) end of session frees the jar |
//!
//! The `Session` scope's BOUNDS (cap, size, expiry) are unit-tested where they
//! live (`application::http_client::session_jars`): they are properties of the
//! store, not of a request.

#[path = "common/wreq_invariants.rs"]
mod wreq_invariants;

use std::sync::Arc;

use webfang_core::application::http_client::{session_jars::SessionJarLease, ScopedHttpClient};
use webfang_core::domain::ports::HttpClientPort;
use webfang_core::domain::JarScope;
use wreq::cookie::Jar;
use wreq_invariants::{
    cookie_pairs, hardened_client, pairs, set_cookie_then_echo, target, TestServer,
};

/// A `session`-scoped facade, in the shape `Container::with_cookie_jar_scope`
/// builds it.
fn session_facade(seed: Option<Arc<Jar>>) -> ScopedHttpClient {
    ScopedHttpClient::new(hardened_client(), JarScope::Session, seed)
}

/// The same facade bound to one session, plus the lease that keeps its jar
/// alive (dropping the lease ends the session).
fn bound_session(seed: Option<Arc<Jar>>, session: &str) -> (ScopedHttpClient, SessionJarLease) {
    let facade = session_facade(seed);
    let store = facade
        .sessions()
        .cloned()
        .expect("a `session`-scoped facade keeps jars");
    let lease = store.lease(session);
    let bound = facade
        .for_session(&lease)
        .expect("a `session`-scoped facade binds to a session");
    (bound, lease)
}

/// (b) Inside one session the jar is shared: a cookie call 1 stored is offered
/// on call 2, which is the whole point of the `session` scope.
#[tokio::test]
async fn session_scope_offers_a_cookie_set_by_a_previous_call_of_the_same_session() {
    let server = TestServer::start().await;
    let (client, _session) = bound_session(None, "session-a");

    set_cookie_then_echo(&client, &server).await;

    assert_eq!(
        cookie_pairs(&server, 0),
        pairs(&[]),
        "the first call carries nothing: its session has no cookies yet"
    );
    assert_eq!(
        cookie_pairs(&server, 1),
        pairs(&["probe=1"]),
        "the second call of the SAME session is offered the cookie the first one stored"
    );
}

/// (a) The negative half of the same rule: two sessions never share a jar, so
/// one agent's cookie cannot travel on another agent's request.
#[tokio::test]
async fn session_scope_never_offers_another_sessions_cookie() {
    let server = TestServer::start().await;
    let (session_a, _lease_a) = bound_session(None, "session-a");
    let (session_b, _lease_b) = bound_session(None, "session-b");

    session_a
        .get(&target(&server, "/set-cookie"))
        .await
        .expect("session A stores a cookie");
    session_b
        .get(&target(&server, "/echo"))
        .await
        .expect("session B's call succeeds");

    assert_eq!(
        cookie_pairs(&server, 0),
        pairs(&[]),
        "session A's first call carries nothing"
    );
    assert_eq!(
        cookie_pairs(&server, 1),
        pairs(&[]),
        "the cookie session A stored must not reach session B"
    );
}

/// (c) A request with no session id is fetched with the process jar — the
/// historical behavior, which is the documented fallback and the only reason
/// this slice is additive.
#[tokio::test]
async fn a_session_scoped_facade_without_a_session_id_uses_the_process_jar() {
    let server = TestServer::start().await;
    // No `for_session`: exactly the facade the container serves from, which is
    // what a stdio run, a stateless transport and the `initialize` call use.
    let client = session_facade(None);

    set_cookie_then_echo(&client, &server).await;

    assert_eq!(
        cookie_pairs(&server, 1),
        pairs(&["probe=1"]),
        "without a session id the client's own process jar applies, exactly as \
         the default `shared` scope does"
    );
}

/// (e) The operator `--cookie` seed and the site's `Set-Cookie` coexist inside a
/// session: the seed rides on the first call and survives next to what the site
/// sets.
#[tokio::test]
async fn session_scope_sends_the_operator_seed_next_to_the_site_cookie() {
    let server = TestServer::start().await;
    let seed = Jar::default();
    seed.add("seed=1", target(&server, "/"));
    let (client, _session) = bound_session(Some(Arc::new(seed)), "session-a");

    set_cookie_then_echo(&client, &server).await;

    assert_eq!(
        cookie_pairs(&server, 0),
        pairs(&["seed=1"]),
        "the session jar starts with the operator seed"
    );
    assert_eq!(
        cookie_pairs(&server, 1),
        pairs(&["seed=1", "probe=1"]),
        "the seed and the site's cookie coexist: the seed is merged, never replaced"
    );
}

/// (d) End of session: dropping the lease frees the jar, so a later session that
/// happens to reuse nothing of the old one starts empty.
#[tokio::test]
async fn releasing_the_lease_drops_the_cookie_a_later_binding_gets() {
    let server = TestServer::start().await;
    let (client, session) = bound_session(None, "session-a");
    client
        .get(&target(&server, "/set-cookie"))
        .await
        .expect("the cookie-setting call succeeds");

    let store = session.store().clone();
    assert_eq!(
        store.cookie_count("session-a"),
        Some(1),
        "the session's jar holds the cookie while the session lives"
    );
    drop(session);
    assert_eq!(
        store.cookie_count("session-a"),
        None,
        "the jar is released with the lease"
    );

    // A request for the SAME session id after the release starts from nothing:
    // the cookie of the ended session is gone.
    let (restarted, _new_session) = bound_session(None, "session-a");
    restarted
        .get(&target(&server, "/echo"))
        .await
        .expect("the restarted session's call succeeds");
    assert_eq!(
        cookie_pairs(&server, 1),
        pairs(&[]),
        "an ended session's cookie never reaches the next one"
    );
}
