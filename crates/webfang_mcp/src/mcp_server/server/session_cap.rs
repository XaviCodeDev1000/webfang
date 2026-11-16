//! Session admission control (#1611, F6) — moved verbatim from the `server.rs`
//! hub (PR 2 of the Ola 1 split, #2584). `pub` constants keep their crate path
//! via the hub's `pub use`; the rest is `pub(super)` (visible inside `server`).

use std::collections::VecDeque;
use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::State;
use axum::http::{header, HeaderValue, Method, StatusCode};
use axum::middleware;
use axum::response::IntoResponse;
use axum::response::Response;

use super::rate_limit::retry_after_secs;

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
pub(super) const MCP_SESSION_ID_HEADER: &str = "mcp-session-id";

/// `operation` field of every event this cap emits — the resource it governs.
pub(super) const SESSION_OPERATION: &str = "mcp.session.create";

/// Spanish sentence for the operator to hand to whoever is being shed, carried
/// as the `user_message` field of the rejection event (see
/// `session_cap_middleware` for why it is not an HTTP body).
/// Says what actually happened, with no promise about the wait: the wait is
/// one admission window — 300s by default — so "in a few seconds" would be a
/// lie for the very configuration the crate ships. The machine-readable half
/// of the same answer is the `Retry-After` header on the 429.
pub(super) const SESSION_CAP_USER_MESSAGE: &str =
    "Límite de sesiones MCP alcanzado. La ventana de admisión está completa; reintente cuando expire la sesión más antigua.";

/// Whether `path` addresses the nested MCP endpoint (`/mcp` or `/mcp/…`).
///
/// The cap is mounted on the whole router, so without this a POST to any
/// other path would burn a session slot — and answering a 404 with 429 is how
/// a scanner locks a legitimate operator out of its own server.
pub(super) fn is_mcp_endpoint(path: &str) -> bool {
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
pub(super) fn creates_session(request: &axum::http::Request<axum::body::Body>) -> bool {
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
pub(super) struct SessionCap {
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
    pub(super) fn new(max: NonZeroUsize, window: Duration) -> Self {
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
    pub(super) fn try_admit(&self, now: Instant) -> Result<usize, usize> {
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
    pub(super) fn retry_after_secs(&self, now: Instant) -> Option<u64> {
        let admitted = self.lock();
        let oldest = admitted.front().copied()?;
        let remaining = self
            .window
            .saturating_sub(now.saturating_duration_since(oldest));
        Some(retry_after_secs(remaining))
    }

    /// Slots currently held — test-only view, without pruning.
    #[cfg(test)]
    pub(super) fn held(&self) -> usize {
        self.lock().len()
    }

    /// Recover from a poisoned mutex instead of propagating the panic.
    ///
    /// The critical section only prunes and pushes one timestamp, so a panic
    /// inside it can leave a stale entry but never a corrupted counter —
    /// whereas propagating the poison would turn one panicking request into a
    /// cap that is broken for the life of the process.
    pub(super) fn lock(&self) -> std::sync::MutexGuard<'_, VecDeque<Instant>> {
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
pub(super) async fn session_cap_middleware(
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
pub(super) fn shed_response(retry_after_secs: u64) -> Response {
    let mut response = StatusCode::TOO_MANY_REQUESTS.into_response();
    let seconds = HeaderValue::from(retry_after_secs);
    response.headers_mut().insert(header::RETRY_AFTER, seconds);
    response
}
