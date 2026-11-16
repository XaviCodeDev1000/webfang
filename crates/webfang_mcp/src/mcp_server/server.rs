//! MCP Server — Axum router with Streamable HTTP transport
//!
//! Sets up the MCP server using rmcp's StreamableHttpService
//! mounted on an Axum router at /mcp, with a full middleware stack:
//! panic containment, panic hook, timeout, body limit, rate limiting,
//! session admission control and optional auth.

use std::any::Any;
use std::collections::VecDeque;
use std::net::SocketAddr;
use std::num::{NonZeroU32, NonZeroUsize};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::State;
use axum::http::{header, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{middleware, Router};
use governor::{
    clock::Clock, clock::DefaultClock, state::keyed::HashMapStateStore, Quota,
    RateLimiter as GovernorLimiter,
};
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

/// Build a `governor` rate limiter from [`ServerOptions`].
///
/// # Keyed, not direct (#1611, F5)
///
/// The finding was a `NotKeyed` limiter: ONE quota shared by every caller,
/// with no way to tell a noisy client from a well-behaved one. `Keyed` gives
/// each identity its own token bucket, so the quota is per caller.
///
/// `auth_enabled` decides whether a request's own `Authorization` header is
/// allowed to choose the key (see [`rate_key`]): with auth configured, only
/// the exact expected token can reach this limiter at all, so the key space is
/// one entry; with auth disabled, every request shares the anonymous entry and
/// a client cannot mint new buckets by inventing headers. That is what keeps
/// the keyed map from reintroducing the unbounded-map shape F6 just closed,
/// and it is asserted by a test rather than assumed.
fn build_rate_limiter(options: &ServerOptions, auth_state: &AuthState) -> KeyedRateLimiter {
    let per_second = NonZeroU32::new(options.rate_per_second).unwrap_or(NonZeroU32::MIN);
    let burst = NonZeroU32::new(options.rate_burst).unwrap_or(NonZeroU32::MIN);
    let quota = Quota::per_second(per_second).allow_burst(burst);
    KeyedRateLimiter {
        limiter: Arc::new(GovernorLimiter::<
            RateKey,
            HashMapStateStore<RateKey>,
            DefaultClock,
        >::hashmap(quota)),
        auth_enabled: auth_state.expected_token.is_some(),
        // One clock instance for the whole router: measuring how long a shed
        // client must wait needs a "now" in the limiter's own time type, and
        // building one per request would cost more than the check it serves.
        clock: Arc::new(DefaultClock::default()),
    }
}

/// The keyed limiter this stack mounts: one token bucket per [`rate_key`].
type RateLimiter = GovernorLimiter<RateKey, HashMapStateStore<RateKey>, DefaultClock>;

/// The rate limiter plus the one fact about the request context `governor`
/// cannot hold for itself: whether authentication is configured.
///
/// Carried here rather than inside the limiter because governor's type is
/// foreign — a wrapper is what lets the middleware ask the question the
/// [`rate_key`] rule depends on without re-reading `ServerOptions`.
#[derive(Clone)]
struct KeyedRateLimiter {
    limiter: Arc<RateLimiter>,
    auth_enabled: bool,
    clock: Arc<DefaultClock>,
}

impl KeyedRateLimiter {
    /// Charge `key` one cell, or report how long until one frees.
    fn check(&self, key: &RateKey) -> Result<(), Duration> {
        self.limiter
            .check_key(key)
            .map_err(|not_until| not_until.wait_time_from(self.clock.now()))
    }

    /// How many distinct buckets exist — the number whose bound must be
    /// asserted, not assumed (see [`rate_key`]).
    #[cfg(test)]
    fn tracked_keys(&self) -> usize {
        self.limiter.len()
    }
}

/// Key a request is charged to: a 64-bit fingerprint of the credential that
/// authenticated it.
///
/// Never the token itself. `InMemoryState` entries live for the life of the
/// process, so a map keyed by bearer tokens would keep every credential the
/// server ever accepted in memory; a fingerprint keeps the bucket identity and
/// drops the secret. A fingerprint collision would merge two clients' buckets,
/// which costs fairness and nothing else — and with a single configured token
/// (`AuthState::expected_token` matches exactly one value) there is only one
/// key in practice anyway.
type RateKey = u64;

/// Key every token-less request shares when authentication is disabled.
///
/// A named constant rather than the fingerprint of the empty string so the
/// anonymous bucket is recognisable in a debugger and cannot collide with a
/// real token's fingerprint by accident.
const ANONYMOUS_RATE_KEY: RateKey = 0;

/// Which bucket this request is charged to.
///
/// `auth_enabled` mirrors [`AuthState::expected_token`]: with auth configured,
/// a request that reaches the rate limiter has already presented THE expected
/// token (auth is mounted outside it), so keying on the presented value is
/// safe AND the key space collapses to that one token. With auth disabled, the
/// presented header is attacker-controlled and is deliberately ignored —
/// otherwise a client could mint an unbounded number of buckets and get a
/// fresh full quota with every invented header, turning the bound into a
/// no-op (the same "unbounded map" shape F6 closed for sessions).
fn rate_key(auth_enabled: bool, auth_header: Option<&str>) -> RateKey {
    if !auth_enabled {
        return ANONYMOUS_RATE_KEY;
    }
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    auth_header.unwrap_or_default().hash(&mut hasher);
    hasher.finish()
}

/// `operation` field of every event the rate limiter emits — the resource it
/// governs (same convention as the session cap's `SESSION_OPERATION`).
const RATE_LIMIT_OPERATION: &str = "mcp.request.rate_limit";

/// Round a wait up to whole seconds for a `Retry-After` header, never below 1.
///
/// Shared with the session cap so both shed responses state the same rule:
/// `Retry-After` is an integer number of seconds, and rounding a sub-second
/// remainder DOWN would invite the client back while the limiter still refuses
/// it, turning an honest retry into a second 429.
fn retry_after_secs(wait: Duration) -> u64 {
    let rounded_up = u64::from(wait.subsec_nanos() > 0) + wait.as_secs();
    rounded_up.max(1)
}

/// Rate limiting middleware — rejects requests exceeding the quota with 429
/// plus a `Retry-After` (#1611, F5).
///
/// The header is the one thing a shed client is told, and it is computable
/// exactly: `governor` returns the instant the next cell frees. The body stays
/// empty for the same reason the session cap's does — load shedding is an
/// HTTP-layer answer, and a JSON-RPC envelope here would only invite an agent
/// to parse it as a protocol error.
///
/// The middleware now answers `Response` rather than `Result<_, StatusCode>`
/// because the shed answer needs a header the status alone cannot carry.
async fn rate_limit_middleware(
    State(limiter): State<KeyedRateLimiter>,
    request: axum::http::Request<axum::body::Body>,
    next: middleware::Next,
) -> Response {
    // `build_rate_limiter` captured whether auth is configured at compose
    // time; the limiter itself carries no view of the request, so the same
    // rule is applied here.
    let auth_enabled = limiter.auth_enabled;
    let auth_header = request
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok());
    let key = rate_key(auth_enabled, auth_header);

    match limiter.check(&key) {
        Ok(()) => next.run(request).await,
        Err(wait) => {
            let retry_after = retry_after_secs(wait);
            tracing::warn!(
                operation = RATE_LIMIT_OPERATION,
                remote = %request.uri().path(),
                wait_ms = wait.as_millis() as u64,
                retry_after_secs = retry_after,
                "rate limit exceeded — rejecting with 429"
            );
            shed_response(retry_after)
        },
    }
}

// ---------------------------------------------------------------------------
// Session admission control (#1611, F6)
// ---------------------------------------------------------------------------

/// Path the MCP service is nested at (see `build_mcp_router_with_service`).
pub const MCP_ENDPOINT_PATH: &str = "/mcp";

/// Default number of session-creating requests admitted per window (see
/// [`DEFAULT_SESSION_CAP_WINDOW_SECS`]).
///
/// 64 is roughly an order of magnitude above the fleet a single-instance MCP
/// server actually serves (one session per agent host, plus reconnects), while
/// still bounding what an unbounded session map can cost: each session holds a
/// spawned worker, two 16-slot channels and a cloned `McpHandler`. Above the
/// rate limiter's default (10 rps, burst 20) it takes seconds of sustained
/// `initialize` traffic to fill, so the cap sheds abuse instead of throttling
/// honest reconnects. Operators with a larger fleet raise it
/// (`--max-sessions` / `WEBFANG_MCP_MAX_SESSIONS`).
pub const DEFAULT_MAX_SESSIONS: usize = 64;

/// Default admission window, in seconds.
///
/// 300 is rmcp's own `SessionConfig::keep_alive` default, and that is exactly
/// why: a slot is released one keep-alive period after the request that took
/// it, which is the longest the session it accounts for can still be alive.
/// A longer window would hold slots for sessions rmcp has already reaped; a
/// shorter one would free slots belonging to live sessions.
pub const DEFAULT_SESSION_CAP_WINDOW_SECS: u64 = 300;

/// Ceiling accepted for the `--max-sessions` /
/// `WEBFANG_MCP_MAX_SESSIONS` override (enforced at the argv boundary, in the
/// `webfang-mcp` binary).
///
/// `NonZeroUsize` says the value is not zero; it says nothing about MAGNITUDE,
/// and this cap is not only a number the limiter compares against: the deque
/// behind it (`SessionCap`) is pre-reserved to the accepted maximum, and an
/// `Instant` is 16 bytes, so
/// `--max-sessions 1000000000` reserves ~16 GB at startup and aborts the
/// process with a raw allocator failure — before the fail-fast checks next to
/// it could say anything in Spanish. 100 000 is three orders of magnitude
/// above any real fleet (the default is 64) while capping the reservation at
/// ~1.6 MB, so a fat-fingered magnitude is an error message instead of a dead
/// process.
pub const MAX_ALLOWED_SESSIONS_CAP: usize = 100_000;

/// Header rmcp uses to bind a request to an existing session. The literal is
/// mirrored here because rmcp keeps `HEADER_SESSION_ID` crate-private
/// (`transport/common/http_header.rs`); it is the same literal the transport
/// emits and the test harness sends.
const MCP_SESSION_ID_HEADER: &str = "mcp-session-id";

/// `operation` field of every event this cap emits — the resource it governs.
const SESSION_OPERATION: &str = "mcp.session.create";

/// Spanish sentence for the operator to hand to whoever is being shed, carried
/// as the `user_message` field of the rejection event (see
/// `session_cap_middleware` for why it is not an HTTP body).
/// Says what actually happened, with no promise about the wait: the wait is
/// one admission window — 300s by default — so "in a few seconds" would be a
/// lie for the very configuration the crate ships. The machine-readable half
/// of the same answer is the `Retry-After` header on the 429.
const SESSION_CAP_USER_MESSAGE: &str =
    "Límite de sesiones MCP alcanzado. La ventana de admisión está completa; reintente cuando expire la sesión más antigua.";

/// Whether `path` addresses the nested MCP endpoint (`/mcp` or `/mcp/…`).
///
/// The cap is mounted on the whole router, so without this a POST to any
/// other path would burn a session slot — and answering a 404 with 429 is how
/// a scanner locks a legitimate operator out of its own server.
fn is_mcp_endpoint(path: &str) -> bool {
    path == MCP_ENDPOINT_PATH
        || path
            .strip_prefix(MCP_ENDPOINT_PATH)
            .is_some_and(|rest| rest.starts_with('/'))
}

/// Whether this request is one rmcp will turn into a new session.
///
/// Deliberately NOT "the body says `initialize`". The predicate mirrors
/// rmcp 1.8.0's own allocation trigger, and mirrors it EXACTLY — including the
/// string conversion. `handle_post` reads the header as
///
/// ```text
/// let session_id = part.headers.get(HEADER_SESSION_ID).and_then(|v| v.to_str().ok());
/// ```
///
/// (`transport/streamable_http_server/tower.rs:1059-1060`) and calls
/// `session_manager.create_session()` in exactly one place (`tower.rs:1129`):
/// the `else` arm of "do we have a session id?". The charged condition is
/// therefore "carries no USABLE `mcp-session-id`", and a `to_str()` failure
/// counts as absent.
///
/// That distinction is the whole security value of the predicate, not a
/// detail. `HeaderValue` accepts obs-text bytes (0x80-0xFF) as perfectly legal
/// bytes, so a client that sends `mcp-session-id: \x80` is read by rmcp as
/// session-less and gets a session allocated — while a `contains_key`-shaped
/// predicate charged it nothing: unbounded sessions, and never a 429. Using
/// `to_str()` here is what keeps the two in step; changing either side alone
/// reopens that hole.
///
/// Everything else (a session-bearing POST, GET, DELETE) rides an existing
/// session or allocates nothing.
fn creates_session(request: &axum::http::Request<axum::body::Body>) -> bool {
    request.method() == Method::POST
        && request
            .headers()
            .get(MCP_SESSION_ID_HEADER)
            .and_then(|value| value.to_str().ok())
            .is_none()
        && is_mcp_endpoint(request.uri().path())
}

/// Admission control for Streamable HTTP sessions (#1611, F6).
///
/// # The finding
///
/// `build_mcp_router` composes rmcp's `LocalSessionManager::default()`
/// (`session/local.rs:30-34`), whose only knobs are `SessionConfig`'s timeouts
/// and channel capacity: `keep_alive` bounds the life of ONE session, never how
/// many exist, and neither `SessionManager` nor `StreamableHttpServerConfig`
/// carries a count anywhere in the crate. A client can therefore open sessions
/// without bound.
///
/// # Why a middleware and not a `SessionManager` wrapper
///
/// The semantically correct shape is a newtype implementing `SessionManager`,
/// delegating to `LocalSessionManager` and refusing `create_session` past the
/// cap by reading its public `sessions` map: it would count REAL sessions and a
/// DELETE would free its slot on the spot. It is deliberately not this slice,
/// because rmcp maps a `create_session` error to **HTTP 500 with a plain-text
/// body** (`internal_error_response`, `transport/common/server_side_http.rs`)
/// — a load-shedding condition answered as a server fault, with no JSON-RPC
/// envelope to explain it. A middleware answers 429 at the HTTP layer, next to
/// the rate limiter that already rejects with a bare `StatusCode`. Exact
/// per-session accounting is the follow-up slice.
///
/// # What is counted, and why
///
/// The counter is charged by `creates_session`, i.e. by rmcp's own allocation
/// trigger (see that function). Counting `initialize` *bodies* instead would
/// leave the resource uncapped through a path this crate does not own: rmcp
/// allocates the session at `tower.rs:1129` and only THEN checks that the
/// message is an initialize request (`tower.rs:1146`), so a session-less POST
/// of anything else is answered `422` and still leaves an entry in the session
/// map — whose worker gives up on `init_timeout` without the manager ever
/// removing it. The predicate therefore charges the requests that allocate,
/// and over-charges the ones rmcp rejects at its OWN gates (its DNS-rebinding
/// / 406 / 415 checks all answer before `create_session` is ever called) —
/// over-counting sheds load, never admits it.
///
/// # What this cap does NOT know (all three accepted, none hidden)
///
/// 1. **It counts requests, not live sessions.** For a well-behaved client the
///    two coincide — every re-`initialize` allocates a second session and
///    never reuses the first — so a client that re-initializes is charged for
///    work it really did. The divergence is only in the safe direction:
///    malformed traffic is charged for sessions that were never created.
/// 2. **A DELETE does not free its slot.** rmcp closes the session at once; the
///    slot is released when the window slides. Bounded by construction: a slot
///    outlives its request by at most the window, which defaults to the
///    session's own maximum lifetime, so the only over-hold is a client that
///    closed a session early and keeps its slot for the rest of the window.
///    Freeing it exactly would need the session id of each admission — the
///    `SessionManager` follow-up's job.
/// 3. **A client that re-initializes periodically does spend budget.** Steady
///    state is (re-initializations per second × window) slots, so the defaults
///    admit one reconnect every ~4.7s indefinitely; a client that reconnects
///    faster than that is doing exactly what the cap exists to bound. Raise
///    `max_sessions` for a larger fleet.
///
/// # Two honest limitations
///
/// Neither of them is a race: the bound itself is EXACT. The prune, the
/// `held >= max` test and the `push_back` all run inside the one `Mutex`
/// critical section of [`SessionCap::try_admit`], so any number of concurrent
/// session-creating requests can admit at most `max` slots — there is no
/// check-then-act window to slip through.
///
/// 1. **The predicate is a proxy for rmcp's decision, not rmcp's decision.**
///    It re-implements the shape of the transport's "do we have a session
///    id?" branch; it does not call it. A refactor upstream that widened or
///    narrowed that branch would desynchronize the two, and the only thing
///    that notices is the regression row in the predicate test plus
///    `tests/mcp_transport_contract_test.rs`. The divergence is one-directional
///    by design (over-charging, see above), but a one-directional bug is still
///    a bug.
/// 2. **Accounting is per REQUEST, not per confirmed session.** rmcp decides
///    to allocate here, this crate decides to charge here, and the two are the
///    same decision reached twice — a session that is allocated and then
///    reaped a microsecond later still spent a slot, and a request charged
///    here is not proof that a session exists. Reading the real live count
///    would mean wrapping `SessionManager`, which is the agreed follow-up
///    (see "Why a middleware and not a `SessionManager` wrapper").
struct SessionCap {
    /// Slots available inside one window.
    max: NonZeroUsize,
    /// Lifetime of a single slot, measured from the admission that took it.
    window: Duration,
    /// Admission instants currently holding a slot, oldest first. Bounded by
    /// `max`: a rejected request never pushes, so the deque cannot grow past
    /// the cap it enforces.
    admitted: Mutex<VecDeque<Instant>>,
}

impl SessionCap {
    /// Build a cap of `max` slots, each living `window`.
    ///
    /// The deque is pre-reserved to `max` so `try_admit`'s `push_back` can
    /// never reallocate: that allocation would happen with the lock held, on
    /// the request path, and the deque is bounded by `max` anyway (a refused
    /// request never pushes), so reserving the maximum is exact rather than a
    /// guess. The property is load-bearing — it is why the lock section is
    /// prune + test + push and nothing else — and it is only affordable
    /// because the magnitude is bounded at the argv boundary by
    /// [`MAX_ALLOWED_SESSIONS_CAP`]. `NonZeroUsize` alone would not stop
    /// `--max-sessions 1000000000` from asking for ~16 GB here.
    fn new(max: NonZeroUsize, window: Duration) -> Self {
        Self {
            max,
            window,
            admitted: Mutex::new(VecDeque::with_capacity(max.get())),
        }
    }

    /// Charge one slot at `now`, or report how many are held.
    ///
    /// `Ok(held)` is the count AFTER the admission; `Err(held)` is the count
    /// that refused it. The whole decision happens inside one short
    /// synchronous section, so no lock is ever held across an `.await`.
    ///
    /// `now` is a parameter rather than `Instant::now()` so the eviction
    /// boundary is testable exactly instead of approximately after a sleep —
    /// there is one production caller and it passes the wall clock.
    fn try_admit(&self, now: Instant) -> Result<usize, usize> {
        let mut admitted = self.lock();
        while admitted
            .front()
            .is_some_and(|at| now.duration_since(*at) >= self.window)
        {
            admitted.pop_front();
        }
        let held = admitted.len();
        if held >= self.max.get() {
            return Err(held);
        }
        admitted.push_back(now);
        Ok(held + 1)
    }

    /// Seconds until the OLDEST live admission ages out — the earliest instant
    /// a shed client can be admitted again, and therefore the `Retry-After` a
    /// 429 must carry. `None` when nothing is held.
    ///
    /// The result is rounded UP, and never below 1s: `Retry-After` is an
    /// integer number of seconds, and rounding a sub-second remainder down
    /// would invite the client back while the cap still refuses it, which
    /// turns an honest retry into a second 429.
    ///
    /// `now` is injected for the same reason as in [`Self::try_admit`], so the
    /// advertised wait is testable exactly instead of after a sleep.
    fn retry_after_secs(&self, now: Instant) -> Option<u64> {
        let admitted = self.lock();
        let oldest = admitted.front().copied()?;
        let remaining = self
            .window
            .saturating_sub(now.saturating_duration_since(oldest));
        Some(retry_after_secs(remaining))
    }

    /// Slots currently held — test-only view, without pruning.
    #[cfg(test)]
    fn held(&self) -> usize {
        self.lock().len()
    }

    /// Recover from a poisoned mutex instead of propagating the panic.
    ///
    /// The critical section only prunes and pushes one timestamp, so a panic
    /// inside it can leave a stale entry but never a corrupted counter —
    /// whereas propagating the poison would turn one panicking request into a
    /// cap that is broken for the life of the process.
    fn lock(&self) -> std::sync::MutexGuard<'_, VecDeque<Instant>> {
        self.admitted
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// Session admission middleware — sheds session-creating requests past the cap
/// with 429, and charges nothing for traffic on an established session.
///
/// Rejection is a 429 with an EMPTY body, exactly like
/// `rate_limit_middleware`: load shedding is an HTTP-layer answer, and the repo
/// does not answer it with a JSON-RPC envelope. The reason a client was shed
/// therefore reaches the operator and not the agent — as the Spanish
/// `user_message` field of the event below (tracing field names and event
/// messages stay English; the sentence a human hands to a user is Spanish).
///
/// The one thing the client IS told is WHEN to come back: a 429 without
/// `Retry-After` tells an agent nothing, and the honest answer is computable —
/// the cap already holds every live admission, and the oldest one is what frees
/// the first slot ([`SessionCap::retry_after_secs`]). A body would carry the
/// same information, so the header is the only addition: `Retry-After` is
/// exactly the standard's answer to "shed me", and it survives clients that
/// read headers instead of bodies.
///
/// `log_scrape_error` is deliberately NOT used: its shape is an error VALUE
/// plus a URL, a stage and a correlation id for a scrape failure, and a shed
/// admission is neither — there is no error, no URL and no scrape to correlate.
/// A structured event at the decision point, carrying the limit, the live count
/// and the operation, is the honest shape.
async fn session_cap_middleware(
    State(cap): State<Arc<SessionCap>>,
    request: axum::http::Request<axum::body::Body>,
    next: middleware::Next,
) -> Response {
    if !creates_session(&request) {
        return next.run(request).await;
    }

    match cap.try_admit(Instant::now()) {
        Ok(held) => {
            tracing::debug!(
                operation = SESSION_OPERATION,
                limit = cap.max.get(),
                admitted = held,
                window_secs = cap.window.as_secs(),
                "session admitted within the admission cap"
            );
            next.run(request).await
        },
        Err(held) => {
            let retry_after_secs = cap.retry_after_secs(Instant::now()).unwrap_or(1);
            tracing::warn!(
                operation = SESSION_OPERATION,
                limit = cap.max.get(),
                admitted = held,
                window_secs = cap.window.as_secs(),
                retry_after_secs,
                user_message = SESSION_CAP_USER_MESSAGE,
                "session admission cap exceeded — rejecting the request with 429"
            );
            shed_response(retry_after_secs)
        },
    }
}

/// The 429 itself: status, `Retry-After`, empty body.
///
/// A named function so the header is unit-testable without a listener, and
/// built by hand rather than through a tuple so the body is provably empty:
/// `StatusCode::into_response()` is an empty body, and adding a sentence to it
/// would turn load shedding into a body an agent has to parse (or, worse, one
/// that looks like a JSON-RPC error).
fn shed_response(retry_after_secs: u64) -> Response {
    let mut response = StatusCode::TOO_MANY_REQUESTS.into_response();
    let seconds = HeaderValue::from(retry_after_secs);
    response.headers_mut().insert(header::RETRY_AFTER, seconds);
    response
}

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
