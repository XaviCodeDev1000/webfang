//! Tests extracted from the `server.rs` hub by topic (PR 1 of #2584, pure move).
//! Bodies are verbatim from the original inline test module.
//!
//! The mapping is a fixed JSON-RPC `-32603` document: HTTP 500,
//! `application/json`, and NOT the empty body tower-http answers by
//! default (which an agent reads as a dead transport, not a server error).

use super::{build_mcp_router_with_service, jsonrpc_panic_response, ServerOptions};
use crate::mcp_server::panic_containment::{JSONRPC_INTERNAL_ERROR, PANIC_HTTP_ERROR};
use crate::mcp_server::render_panic_payload;
use axum::http::{header, Method, StatusCode};
use axum::response::Response;
/// The contained-panic mapping contract is asserted above; this test makes
/// the whole stack panic-safe in-process: a service that panics on the HTTP
/// request path, driven through the REAL stack with `oneshot` (no
/// listener, no sleep, no network).
///
/// A POST carrying a real envelope, because that is the case #1646 is
/// about: the contained panic must answer with the CALLER'S id. A GET
/// with an empty body would have proved only the `null` rule, which the
/// direct-handler test below already pins.
#[tokio::test]
async fn panic_on_the_http_path_is_mapped_by_the_real_stack() {
    use tower::ServiceExt;

    async fn panicking_route(
        _req: axum::http::Request<axum::body::Body>,
    ) -> Result<Response, std::convert::Infallible> {
        panic!("probe: panic on the HTTP request path");
    }

    let app = build_mcp_router_with_service(
        tower::service_fn(panicking_route),
        &ServerOptions {
            // #1611 G-18: the stack is fail-closed by default, so a test
            // that mounts a service directly has to say it wants anonymous
            // operation — same opt-in production gets from --allow-anonymous.
            allow_anonymous: true,
            ..Default::default()
        },
    );
    let request = axum::http::Request::builder()
        .method(Method::POST)
        .uri("/mcp")
        .body(axum::body::Body::from(
            r#"{"jsonrpc":"2.0","id":"pc4-probe-7","method":"tools/list"}"#,
        ))
        .expect("valid request");

    let response = app
        .oneshot(request)
        .await
        .expect("CatchPanicLayer turns the panic into a response");

    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body reads");
    let text = String::from_utf8_lossy(&body);
    let parsed: serde_json::Value = serde_json::from_slice(&body).expect("mapped body is JSON");
    assert_eq!(parsed["error"]["code"], JSONRPC_INTERNAL_ERROR);
    assert_eq!(
        parsed["id"], "pc4-probe-7",
        "a panic at or below the service must echo the caller's id: {parsed}"
    );
    assert!(
        !text.contains("HTTP request path"),
        "the panic payload must not leak through the stack: {text}"
    );
}

///
/// The payload is a secret-shaped string on purpose: it must not appear
/// anywhere in the answer. The trace owns the payload, the client owns a
/// sentence.
///
/// This is the OUTER backstop, driven directly, so it pins the documented
/// `null` id rule (no body was read, so there is no envelope to echo) —
/// the recoverable case belongs to
/// `panic_containment::jsonrpc_panic_containment` and is pinned through
/// the real stack above and in the integration suite.
#[tokio::test]
async fn panic_mapping_answers_jsonrpc_internal_error_without_payload() {
    let response = jsonrpc_panic_response(Box::new(String::from("probe payload: sk-do-not-leak")));

    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(
        response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok()),
        Some("application/json"),
        "the contained-panic body must be JSON, not an empty/text fallback"
    );

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("mapping body reads");
    let text = String::from_utf8_lossy(&body);
    let parsed: serde_json::Value = serde_json::from_slice(&body).expect("mapping body is JSON");
    assert_eq!(parsed["jsonrpc"], "2.0");
    assert!(
        parsed["id"].is_null(),
        "the OUTER backstop reads no body, so the id is null by rule: {parsed}"
    );
    assert_eq!(parsed["error"]["code"], JSONRPC_INTERNAL_ERROR);
    assert_eq!(parsed["error"]["message"], PANIC_HTTP_ERROR);
    assert!(
        !text.contains("sk-do-not-leak"),
        "the panic payload must never reach the client: {text}"
    );
}

/// The user-facing text is Spanish, per the project convention for
/// user-facing errors (tracing fields and code comments stay English).
///
/// Pinned on markers an English sentence cannot contain (the article `La`
/// and a Spanish-specific character — `ñ`, or an accented vowel): an "is
/// it non-empty / is it lowercase" check would pass on the English
/// translation of the same sentence, so it would pin nothing.
#[test]
fn panic_messages_are_spanish() {
    for (label, message) in [
        ("http", PANIC_HTTP_ERROR),
        ("tool", crate::mcp_server::PANIC_CONTAINED_TOOL_ERROR),
    ] {
        let has_spanish_char = message
            .chars()
            .any(|c| matches!(c, 'ñ' | 'á' | 'é' | 'í' | 'ó' | 'ú'));
        assert!(
            message.contains("La ") && has_spanish_char,
            "the {label} message must be Spanish, got: {message}"
        );
    }
}

/// `render_panic_payload` must be total: it never panics, never returns
/// more than its bound, and says so plainly for a non-string payload
/// (`panic_any` with a custom type is legal).
#[test]
fn render_panic_payload_is_bounded_and_total() {
    assert_eq!(render_panic_payload(&String::from("boom")), "boom");
    assert_eq!(render_panic_payload(&"boom"), "boom");
    assert_eq!(render_panic_payload(&42_u32), "<non-string panic payload>");

    let long = "x".repeat(5_000);
    let rendered = render_panic_payload(&long);
    assert!(
        rendered.chars().count() <= 201,
        "the rendered payload must stay bounded, got {} chars",
        rendered.chars().count()
    );
}

/// The bound is a CHARACTER bound on a multibyte payload (#1626, PC-2).
///
/// The truncation index has to land on a char boundary, or the slice would
/// not be valid UTF-8 and the renderer would panic — while reporting a
/// panic. A megabyte of multibyte characters is also what makes the gap
/// between the old full `chars().count()` walk and the bounded one enormous
/// in bytes while identical in output, which is why the input is
/// megabyte-scale on purpose.
///
/// What this does NOT pin: that the walk is bounded. Complexity is not
/// observable from a unit test without a timing assertion, and a timing
/// assertion on a panic path is exactly the flake that hides a real
/// regression. The bounded walk is justified by construction
/// (`char_indices().nth(MAX_CHARS)`); this test pins the *semantics* it has
/// to preserve.
#[test]
fn render_panic_payload_truncates_multibyte_at_a_char_boundary() {
    let payload = "é".repeat(1_000_000); // 2 MB, 1M chars
    let rendered = render_panic_payload(&payload);

    assert_eq!(
        rendered.chars().count(),
        201,
        "expected MAX_CHARS plus the ellipsis, split on a char boundary"
    );
    assert!(
        rendered.ends_with('…'),
        "truncation must be marked: {rendered:?}"
    );
    assert!(
        rendered.starts_with(&"é".repeat(200)),
        "the kept prefix must be verbatim, not re-encoded"
    );
}
