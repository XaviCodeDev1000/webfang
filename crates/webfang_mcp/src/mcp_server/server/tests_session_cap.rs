//! Tests extraídos del hub `server.rs` por tema (PR 1 de #2584, movimiento puro).
//! Los cuerpos son verbatim del módulo de tests inline original.

use super::session_cap::{
    creates_session, SessionCap, MCP_SESSION_ID_HEADER, SESSION_CAP_USER_MESSAGE,
};
use super::{
    build_mcp_router_with_service, ServerOptions, DEFAULT_MAX_SESSIONS,
    DEFAULT_SESSION_CAP_WINDOW_SECS,
};
use axum::http::{header, StatusCode};
use std::num::NonZeroUsize;
use std::time::{Duration, Instant};
#[test]
fn session_cap_defaults_bound_a_real_fleet() {
    assert_eq!(DEFAULT_MAX_SESSIONS, 64);
    // 300 is rmcp's `SessionConfig::keep_alive` default, which is the whole
    // justification for the number (see the constant's doc comment).
    assert_eq!(DEFAULT_SESSION_CAP_WINDOW_SECS, 300);
}

fn cap(max: usize, window_secs: u64) -> SessionCap {
    SessionCap::new(
        NonZeroUsize::new(max).expect("test max is non-zero"),
        Duration::from_secs(window_secs),
    )
}

/// The cap admits exactly `max` and then refuses, and the refusal carries
/// the count that produced it.
#[test]
fn try_admit_refuses_past_the_cap_and_reports_the_live_count() {
    let cap = cap(3, 300);
    let t0 = Instant::now();
    assert_eq!(cap.try_admit(t0), Ok(1));
    assert_eq!(cap.try_admit(t0), Ok(2));
    assert_eq!(cap.try_admit(t0), Ok(3));
    assert_eq!(cap.try_admit(t0), Err(3));
    assert_eq!(cap.try_admit(t0), Err(3));
    assert_eq!(cap.held(), 3, "a refused request must not consume a slot");
}

/// A slot is released when ITS window slides, not when the counter as a
/// whole rolls over: a fixed-window counter would let one client admit
/// `2 × max` by straddling a boundary. Two admissions 1s apart in a 1s
/// window therefore shed the second and admit the first's replacement.
///
/// Time is injected (`try_admit` takes the instant), so this is exact at
/// the eviction boundary instead of approximate after a sleep.
#[test]
fn a_slot_is_released_when_its_own_window_slides() {
    let cap = cap(1, 1);
    let t0 = Instant::now();
    assert_eq!(cap.try_admit(t0), Ok(1));
    assert_eq!(cap.try_admit(t0 + Duration::from_millis(999)), Err(1));
    assert_eq!(
        cap.try_admit(t0 + Duration::from_secs(1)),
        Ok(1),
        "an admission exactly one window old is expired"
    );
}

/// Pruning releases only the AGED-OUT admissions: a fresh one survives, so
/// the cap measures a sliding window and not "everything since the last
/// full reset". (Clearing the deque on prune would pass the test above and
/// fail here.)
#[test]
fn pruning_releases_only_the_aged_out_admissions() {
    let cap = cap(2, 10);
    let t0 = Instant::now();
    assert_eq!(cap.try_admit(t0), Ok(1));
    assert_eq!(cap.try_admit(t0 + Duration::from_secs(6)), Ok(2));

    // t=12s: the first admission (12s old) expired, the second (6s old)
    // did not — so exactly one slot came back.
    assert_eq!(cap.try_admit(t0 + Duration::from_secs(12)), Ok(2));
    assert_eq!(cap.try_admit(t0 + Duration::from_secs(12)), Err(2));
}

/// The predicate's contract, stated exactly: charge every request that
/// carries no USABLE `mcp-session-id` to a POST on the endpoint, and
/// nothing else.
///
/// "Usable" is rmcp's word, not a nicety — it means `to_str()` succeeds
/// (`tower.rs:1059-1060`), which is the condition under which rmcp skips
/// `create_session()`. Note what this does NOT claim: that the charged set
/// equals the set of requests that allocate. rmcp's own earlier gates
/// (DNS-rebinding check, 406, 415) answer before `create_session` is
/// called, so the predicate deliberately OVER-charges them. Each row below
/// is a real traffic class, so a change here changes who pays.
#[test]
fn creates_session_charges_every_request_without_a_usable_session_id() {
    let build = |method: &str, path: &str, session: bool| {
        let mut builder = axum::http::Request::builder().method(method).uri(path);
        if session {
            builder = builder.header(MCP_SESSION_ID_HEADER, "s-1");
        }
        builder
            .body(axum::body::Body::empty())
            .expect("valid request")
    };

    // The allocation trigger itself.
    assert!(creates_session(&build("POST", "/mcp", false)));
    assert!(creates_session(&build("POST", "/mcp/", false)));
    // Established-session traffic: rides a session, allocates nothing.
    assert!(!creates_session(&build("POST", "/mcp", true)));
    // GET opens the SSE stream of an existing session; DELETE closes one.
    assert!(!creates_session(&build("GET", "/mcp", false)));
    assert!(!creates_session(&build("DELETE", "/mcp", false)));
    // Off-endpoint POSTs must not burn a slot (a scanner could otherwise
    // lock the operator out with requests that allocate nothing).
    assert!(!creates_session(&build("POST", "/", false)));
    assert!(!creates_session(&build("POST", "/mcp-not", false)));
    assert!(!creates_session(&build("POST", "/other", false)));
}

/// The regression row for the bypass this predicate shape exists to close:
/// a PRESENT but non-UTF-8 `mcp-session-id` is read by rmcp as "no session
/// id" (`to_str()` fails on obs-text), so rmcp allocates a session for it.
/// A `contains_key`-shaped predicate charged it nothing — unbounded
/// sessions, never a 429.
///
/// obs-text bytes (0x80-0xFF) are LEGAL `HeaderValue` bytes, so this is a
/// request any client can send; that is exactly why it must be charged.
/// Built in-process: no listener, no network, no sleep.
#[test]
fn creates_session_charges_a_present_but_non_utf8_session_id() {
    let build_with_session_bytes = |bytes: &[u8]| {
        axum::http::Request::builder()
            .method("POST")
            .uri("/mcp")
            .header(
                MCP_SESSION_ID_HEADER,
                axum::http::HeaderValue::from_bytes(bytes)
                    .expect("obs-text is a legal HeaderValue byte"),
            )
            .body(axum::body::Body::empty())
            .expect("valid request")
    };

    assert!(
        creates_session(&build_with_session_bytes(b"\x80")),
        "a session id rmcp cannot read as a string IS a session-creating request"
    );
    assert!(
        creates_session(&build_with_session_bytes(b"\xff\xfe")),
        "same for any other invalid-UTF-8 value"
    );
    assert!(
        !creates_session(&build_with_session_bytes(b"s-1")),
        "a readable session id still rides its session"
    );
}

/// The 429 must say when to come back, and the cap already knows: the
/// oldest live admission is what frees the first slot. Injected clock, so
/// the numbers are exact rather than "about 300".
#[test]
fn retry_after_counts_down_from_the_oldest_live_admission() {
    let cap = cap(2, 300);
    let t0 = Instant::now();
    assert_eq!(
        cap.retry_after_secs(t0),
        None,
        "an empty cap has nothing to wait for"
    );

    assert_eq!(cap.try_admit(t0), Ok(1));
    assert_eq!(cap.retry_after_secs(t0), Some(300));
    assert_eq!(cap.try_admit(t0 + Duration::from_secs(1)), Ok(2));
    // The OLDEST admission decides, not the newest: the second one still
    // has 299s to run, but the first frees a slot in 299s.
    assert_eq!(cap.retry_after_secs(t0 + Duration::from_secs(1)), Some(299));

    // Rounds UP, never down: 0.5s of window left must not be advertised as
    // "0", which would invite the client back into a cap that refuses it.
    assert_eq!(
        cap.retry_after_secs(t0 + Duration::from_millis(1_500)),
        Some(299)
    );
}

/// A sub-second remainder is still one whole second to a client, and
/// `Retry-After: 0` is a promise the cap cannot keep.
#[test]
fn retry_after_never_advertises_zero_seconds() {
    let cap = cap(1, 1);
    let t0 = Instant::now();
    assert_eq!(cap.try_admit(t0), Ok(1));
    assert_eq!(
        cap.retry_after_secs(t0 + Duration::from_millis(999)),
        Some(1)
    );
    assert_eq!(cap.retry_after_secs(t0 + Duration::from_secs(1)), Some(1));
}

/// The shed, end to end through the REAL stack: a 429 carrying
/// `Retry-After` in the configured window, and an empty body (the property
/// the bare-answer test pins from the outside).
///
/// Driven with `oneshot` against a stub service, so it is the middleware —
/// not a listener, not rmcp — that is under test, and the assertions are
/// about the header we synthesize.
#[tokio::test]
async fn the_shed_carries_retry_after_for_the_configured_window() {
    use tower::ServiceExt;

    let app = build_mcp_router_with_service(
        tower::service_fn(|_req: axum::http::Request<axum::body::Body>| async {
            Ok::<_, std::convert::Infallible>(StatusCode::OK)
        }),
        &ServerOptions {
            max_sessions: NonZeroUsize::new(1).expect("one is non-zero"),
            // #1611 G-18: token-less by default means fail-closed, so a
            // test that mounts a service directly states the opt-in.
            allow_anonymous: true,
            ..Default::default()
        },
    );
    let post = || {
        axum::http::Request::builder()
            .method("POST")
            .uri("/mcp")
            .body(axum::body::Body::empty())
            .expect("valid request")
    };

    let admitted = app
        .clone()
        .oneshot(post())
        .await
        .expect("in-process response");
    assert_eq!(admitted.status(), StatusCode::OK, "the first fits the cap");

    let shed = app.oneshot(post()).await.expect("in-process response");
    assert_eq!(shed.status(), StatusCode::TOO_MANY_REQUESTS);

    let retry_after = shed
        .headers()
        .get(header::RETRY_AFTER)
        .expect("a shed must tell the client when to come back")
        .to_str()
        .expect("Retry-After is ASCII")
        .parse::<u64>()
        .expect("Retry-After is an integer number of seconds");
    assert!(
        (1..=DEFAULT_SESSION_CAP_WINDOW_SECS).contains(&retry_after),
        "Retry-After must fit the configured window, got {retry_after}"
    );

    let body = axum::body::to_bytes(shed.into_body(), usize::MAX)
        .await
        .expect("body reads");
    assert!(
        body.is_empty(),
        "the shed body stays empty; the sentence lives in the trace: {body:?}"
    );
}

/// The user-facing sentence is Spanish, like every other message a human
/// reads out of this server (`panic_messages_are_spanish` pins the same
/// convention for the contained-panic bodies), and it makes no promise the
/// cap cannot keep: it must not claim a wait in SECONDS, because the wait
/// is one window — 300s by default.
#[test]
fn session_cap_user_message_is_spanish_and_promises_no_wait() {
    let has_spanish_char = SESSION_CAP_USER_MESSAGE
        .chars()
        .any(|c| matches!(c, 'ñ' | 'á' | 'é' | 'í' | 'ó' | 'ú'));
    assert!(
        SESSION_CAP_USER_MESSAGE.contains("Límite") && has_spanish_char,
        "the operator-facing sentence must be Spanish, got: {SESSION_CAP_USER_MESSAGE}"
    );
    assert!(
        !SESSION_CAP_USER_MESSAGE.contains("segundos"),
        "the wait is one admission window (300s by default), not seconds: \
         {SESSION_CAP_USER_MESSAGE}"
    );
}
