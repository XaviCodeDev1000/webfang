//! MCP Server — Axum router with Streamable HTTP transport
//!
//! Sets up the MCP server using rmcp's StreamableHttpService
//! mounted on an Axum router at /mcp, with a full middleware stack:
//! panic containment, panic hook, timeout, body limit, rate limiting,
//! session admission control and optional auth.

use std::any::Any;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::response::{IntoResponse, Response};
use axum::{middleware, Router};
use rmcp::transport::streamable_http_server::{
    session::local::LocalSessionManager, tower::StreamableHttpService,
};
use tokio_util::sync::CancellationToken;
use tower_http::catch_panic::CatchPanicLayer;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;
use tracing::info;

use webfang_core::application::crawler::engine::TERMINATION_REGISTRATION_REJECTED;
use webfang_core::infrastructure::platform::first_termination_event;

use super::auth::{validate_auth, AuthState};
use super::panic_containment::{self, IdSource, RecoveredRequestId};
use super::panic_hook::setup_panic_hook;
use super::state::McpState;
use super::McpHandler;

/// Default address for the MCP server.
pub const DEFAULT_MCP_ADDR: &str = "127.0.0.1:8080";

mod options;
pub use options::*;

/// Build the Axum router with MCP endpoint and full middleware stack.
///
/// This is the production composition root: it builds the rmcp
/// [`StreamableHttpService`] over [`McpHandler::new`] and hands it to
/// [`build_mcp_router_with_service`], which owns the whole middleware stack.
/// The signature is unchanged for every existing caller (the HTTP binary, the
/// test harness and the integration suites).
///
/// [`StreamableHttpService`]: rmcp::transport::streamable_http_server::tower::StreamableHttpService
pub fn build_mcp_router(state: McpState, options: &ServerOptions) -> Router {
    let service = StreamableHttpService::new(
        move || Ok(McpHandler::new(state.clone())),
        LocalSessionManager::default().into(),
        Default::default(),
    );

    build_mcp_router_with_service(service, options)
}

/// Mount an already-built MCP service under `/mcp` behind the full stack.
///
/// # Why the service is a parameter
///
/// The stack is the load-bearing part of the panic-containment design
/// (#1611, F2) and it must be provable on the REAL composition, not on a
/// hand-rolled replica — a test that mounted a different stack would prove
/// nothing. So the stack lives here and the `/mcp` service is injected. The
/// bounds are exactly what [`Router::nest_service`] requires, deliberately NOT
/// specialized to `StreamableHttpService<McpHandler, _>`: that would force the
/// seam back to a single concrete service, and the panic-to-JSON-RPC mapping
/// (`jsonrpc_panic_response`, module-private) could then only be exercised
/// through a live listener. Generic over the service, `tower::ServiceExt::oneshot`
/// can drive the identical stack in-process with a service that panics, which is
/// what pins the mapping deterministically.
///
/// Production callers keep using [`build_mcp_router`]; the generic parameter
/// exists for tests that must mount a different service (`StreamableHttpService`
/// over a handler carrying a test-only tool) or a deliberately panicking one.
///
/// # Protocol-shape answers we do not own
///
/// The `/mcp` service is rmcp's [`StreamableHttpService`], which answers
/// JSON-RPC envelope problems at the HTTP layer BEFORE any method dispatch, and
/// its status codes are not configurable from here (#1294 P5-1/P5-2):
///
/// | Client mistake | Answer |
/// | :--- | :--- |
/// | `Accept` lacking both `application/json` and `text/event-stream` | `406` |
/// | `Content-Type` not `application/json` | `415` |
/// | body that is not a JSON-RPC 2.0 message (includes a 1.0 body) | `415` |
/// | non-`initialize` request with no `mcp-session-id` | `422` |
///
/// Only after those gates does a request reach dispatch, where an unknown method
/// becomes a JSON-RPC `-32601` over HTTP 200. Each row is pinned by
/// `tests/mcp_transport_contract_test.rs`; changing one upstream breaks that
/// suite instead of silently changing what agents see. Deliberately not handled
/// here: a translating middleware layer, which would fight the framework on every
/// rmcp bump (design decision, #1294 slice A).
///
/// [`StreamableHttpService`]: rmcp::transport::streamable_http_server::tower::StreamableHttpService
pub fn build_mcp_router_with_service<S>(service: S, options: &ServerOptions) -> Router
where
    S: tower::Service<axum::http::Request<axum::body::Body>, Error = std::convert::Infallible>
        + Clone
        + Send
        + Sync
        + 'static,
    S::Response: IntoResponse + 'static,
    S::Future: Send + 'static,
{
    let service = tower::ServiceExt::map_response(service, into_axum_response);

    let auth_state = AuthState {
        expected_token: options.auth_token.clone().map(Arc::from),
        allow_anonymous: options.allow_anonymous,
    };
    let rate_limiter = build_rate_limiter(options, &auth_state);
    let session_cap = Arc::new(SessionCap::new(
        options.max_sessions,
        Duration::from_secs(options.session_cap_window_secs.get()),
    ));

    Router::new()
        .nest_service(MCP_ENDPOINT_PATH, service)
        // Innermost of all, applied FIRST: `.layer()` wraps what is already
        // there, so this is the LAST gate a request meets. It is where the
        // body is finally readable, and a panic raised at or below the nested
        // service is answered here with the CALLER'S OWN JSON-RPC id (#1646)
        // instead of a `null` the client cannot correlate — which is the whole
        // reason it sits here and not above.
        //
        // Inside the other gates is deliberate, and the security property is
        // the same one the session cap below is mounted for: auth, the rate
        // limiter, the timeout and the body limit have all answered by now, so
        // a request rejected 401 (no/invalid token) or shed 429 by the rate
        // limiter never has its body buffered here, and can therefore neither
        // exhaust the session budget of the legitimate operator nor make an
        // unauthenticated flood pay for a body read.
        .layer(middleware::from_fn(
            panic_containment::jsonrpc_panic_containment,
        ))
        // Next innermost, applied second: the session cap. A request rejected
        // 401 (no/invalid token) or shed 429 by the rate limiter can never
        // consume a session slot, so an unauthenticated flood cannot exhaust
        // the session budget of the legitimate operator.
        .layer(middleware::from_fn_with_state(
            session_cap,
            session_cap_middleware,
        ))
        // #1611 F5: the rate limiter is mounted INSIDE authentication — the
        // `.layer(auth)` call below it wraps this one. That ordering is the
        // fix, not a detail: a request with no or a wrong bearer token is
        // answered `401` here, before it can spend a single cell of the
        // legitimate operator's quota, so an unauthenticated flood can no
        // longer lock the operator out of its own server by exhausting the
        // shared budget. What this stack used to do — charge the quota first,
        // then check the token — inverted both halves of that.
        .layer(middleware::from_fn_with_state(
            rate_limiter,
            rate_limit_middleware,
        ))
        // Auth wraps the rate limiter, the session cap and the id-echoing
        // panic layer; see the F5 note above for why the order is this way
        // round.
        .layer(middleware::from_fn_with_state(auth_state, validate_auth))
        .layer(TimeoutLayer::with_status_code(
            axum::http::StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(options.request_timeout_secs),
        ))
        // #1611 G-20, deliberately NOT fixed here: this timeout bounds the
        // REQUEST FUTURE, not a streaming response body. Once rmcp has answered
        // with an SSE stream, the body keeps flowing past this deadline. The
        // other half of G-20 — "SSE channels uncapped" — no longer holds on
        // rmcp 1.8.0: every session/event channel is built with
        // `SessionConfig::channel_capacity`, which defaults to 16
        // (`session/local.rs:451,576,1179`). A body-idle timeout would mean
        // wrapping the response stream, which can cut a legitimate long-running
        // crawl's event stream; that is a design decision, not a patch.
        .layer(RequestBodyLimitLayer::new(options.body_limit_bytes))
        .layer(TraceLayer::new_for_http())
        // Outermost: applied LAST, so it wraps auth, rate limiting, timeout,
        // body limit, tracing and the nested service. It is the backstop for
        // panics raised ABOVE the id-echoing layer, where no body has been
        // read at all and the id is `null` by the documented rule in
        // `panic_containment` — a rule, not a silent fallback. A
        // tool-handler panic is contained one level deeper, in
        // `McpHandler::call_tool` (#1611, F2).
        .layer(CatchPanicLayer::custom(jsonrpc_panic_response))
}

/// Erase the concrete response body type: axum routers and the tower-http
/// layers above are all monomorphic in `Response<Body>`, and `nest_service`
/// only requires the `IntoResponse` contract.
fn into_axum_response<R: IntoResponse>(response: R) -> Response {
    response.into_response()
}

/// Map a panic raised ANYWHERE on the HTTP request path to a JSON-RPC
/// `-32603` error body (HTTP 500, `application/json`).
///
/// Named (not a closure) so the contract is unit-testable: the mapping is the
/// only thing an agent sees when a panic escapes the inner layers, and
/// tower-http's own default handler answers with an EMPTY body, which an agent
/// reads as a transport failure rather than a server error.
///
/// A named `fn` is also what satisfies
/// [`CatchPanicLayer::custom`]: the handler must be `FnMut + Clone`, so a
/// closure would have to be written as an explicitly cloneable wrapper.
///
/// # The id it echoes is `null`, and that is a rule
///
/// This is the OUTERMOST backstop. By the time it runs, auth, rate limiting,
/// the timeout, the body limit and
/// [`panic_containment::jsonrpc_panic_containment`] have all already answered,
/// and a panic that reaches here was raised in one of them — which is to say
/// before (or without) any request body being read. There is no envelope in
/// scope, so the id is `null` **by an explicit documented rule**, not by a
/// silent fallback: the alternative would be buffering a body for every
/// rejected request, and an unauthenticated flood would then be able to make
/// the server read bodies it is about to refuse.
///
/// The recoverable case is handled one layer in, by
/// [`panic_containment::jsonrpc_panic_containment`], which reads the POST
/// body, recovers the id, and answers a contained panic with that id (#1646).
/// Both paths share one response builder, so the shape a client sees never
/// depends on which layer caught the panic.
fn jsonrpc_panic_response(payload: Box<dyn Any + Send + 'static>) -> Response {
    panic_containment::jsonrpc_internal_error_response(
        RecoveredRequestId::unrecoverable(IdSource::NotARequest),
        payload.as_ref(),
    )
}

mod rate_limit;
use rate_limit::{build_rate_limiter, rate_limit_middleware};

mod session_cap;
use session_cap::{session_cap_middleware, shed_response, SessionCap};
pub use session_cap::{
    DEFAULT_MAX_SESSIONS, DEFAULT_SESSION_CAP_WINDOW_SECS, MAX_ALLOWED_SESSIONS_CAP,
    MCP_ENDPOINT_PATH,
};

/// Start the MCP server on the given address with the full middleware stack.
///
/// # Connection Pooling
///
/// For production use, create a shared `Downloader` and inject it via
/// `McpState::with_downloader()`. Long-lived servers MUST use the bounded
/// composition-root helper (#1120), never the legacy unbounded
/// `Downloader::new`:
///
/// ```rust,ignore
/// use std::sync::Arc;
/// use webfang_mcp::mcp_server::{build_shared_downloader, McpState};
///
/// let downloader = Arc::new(build_shared_downloader()?);
/// let state = McpState::new(container).with_downloader(downloader);
/// start_mcp_server(state, addr, ServerOptions::default()).await?;
/// ```
///
/// Without a shared Downloader, each MCP tool call creates a fresh connection pool,
/// defeating keep-alive and TLS session reuse.
///
/// # Errors
///
/// Returns an error if the TCP listener cannot bind to `addr`, or if the
/// server fails while serving.
pub async fn start_mcp_server(
    state: McpState,
    addr: SocketAddr,
    options: ServerOptions,
) -> anyhow::Result<()> {
    setup_panic_hook();

    let app = build_mcp_router(state.clone(), &options);

    // The admission cap is the one piece of configuration whose EFFECT is
    // invisible until it sheds a client, hours into a run. Logging the
    // effective values (not the flag defaults) is what lets an operator tell
    // "the cap is off" from "the cap is on at 3 slots" from a log line alone.
    info!(
        addr = %addr,
        path = MCP_ENDPOINT_PATH,
        max_sessions = options.max_sessions.get(),
        session_cap_window_secs = options.session_cap_window_secs.get(),
        "MCP server starting"
    );

    let listener = tokio::net::TcpListener::bind(addr).await?;
    // Capture the serve result instead of `?`: a serve error must still drain
    // the writer below — the early-return would silently skip the flush of
    // every buffered record (#1143 review). The failure is propagated after
    // the drain completes.
    let serve_result = axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal(state.cancel_token.clone()))
        .await;

    // #1121: drain and join the crawl-result background writer before the
    // runtime goes away, so every write acknowledged by `save` is confirmed
    // persisted and a dead writer is reported instead of silently detached.
    if let Some(repo) = state.container.crawl_result_repository() {
        if let Err(e) = repo.shutdown().await {
            tracing::warn!(error = %e, "crawl-result writer shutdown reported errors");
        }
    }

    serve_result?;

    Ok(())
}

/// Future that resolves when a shutdown is requested (Ctrl+C, SIGTERM or a
/// Windows console close/logoff/shutdown).
///
/// Drives the OS signal handler in the background and cancels the supplied
/// [`CancellationToken`]. Returns a `'static` future (it owns its
/// own clone of the token) so it is suitable for
/// `axum::serve(...).with_graceful_shutdown(...)`.
///
/// The console-event source (#1808 — XP-S-02) only *fires* this: the token
/// stays the server's one shutdown authority. Off Windows it is compiled out
/// and this is exactly the historical Ctrl+C wait.
async fn shutdown_signal(token: CancellationToken) {
    match first_termination_event().await {
        Some(name) => {
            info!("{name} received — MCP server shutting down");
            token.cancel();
        },
        None => {
            // #2193: the SAME shared warning the crawl engine and the CLI
            // shutdown guard emit for a rejected registration. The MCP server
            // is the THIRD shutdown authority, and the rejection is one fact
            // with one wording — three texts for one event is how a reader
            // loses track of what the process just said.
            tracing::warn!(authority = "mcp", "{}", TERMINATION_REGISTRATION_REJECTED);
            // Without a working Ctrl+C handler, wait on the token directly so the
            // server can still be torn down via an explicit cancel().
            token.cancelled().await;
        },
    }
}

#[cfg(test)]
#[path = "server/tests_common.rs"]
mod tests_common;
#[cfg(test)]
#[path = "server/tests_handler_logic.rs"]
mod tests_handler_logic;
#[cfg(test)]
#[path = "server/tests_options.rs"]
mod tests_options;
#[cfg(test)]
#[path = "server/tests_panic.rs"]
mod tests_panic;
#[cfg(test)]
#[path = "server/tests_rate_limit.rs"]
mod tests_rate_limit;
#[cfg(test)]
#[path = "server/tests_session_cap.rs"]
mod tests_session_cap;
