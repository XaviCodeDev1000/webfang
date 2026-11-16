//! Rate limiting (keyed token bucket + middleware) — moved verbatim from the
//! `server.rs` hub (PR 2 of the Ola 1 split, #2584). Items consumed by the hub
//! or by sibling test modules are `pub(super)`: visible inside `server`, no wider.

use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::middleware;
use axum::response::Response;
use governor::{
    clock::Clock, clock::DefaultClock, state::keyed::HashMapStateStore, Quota,
    RateLimiter as GovernorLimiter,
};

use super::options::ServerOptions;
use super::shed_response;
use crate::mcp_server::auth::AuthState;

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
pub(super) fn build_rate_limiter(
    options: &ServerOptions,
    auth_state: &AuthState,
) -> KeyedRateLimiter {
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
pub(super) type RateLimiter = GovernorLimiter<RateKey, HashMapStateStore<RateKey>, DefaultClock>;

/// The rate limiter plus the one fact about the request context `governor`
/// cannot hold for itself: whether authentication is configured.
///
/// Carried here rather than inside the limiter because governor's type is
/// foreign — a wrapper is what lets the middleware ask the question the
/// [`rate_key`] rule depends on without re-reading `ServerOptions`.
#[derive(Clone)]
pub(super) struct KeyedRateLimiter {
    limiter: Arc<RateLimiter>,
    auth_enabled: bool,
    clock: Arc<DefaultClock>,
}

impl KeyedRateLimiter {
    /// Charge `key` one cell, or report how long until one frees.
    pub(super) fn check(&self, key: &RateKey) -> Result<(), Duration> {
        self.limiter
            .check_key(key)
            .map_err(|not_until| not_until.wait_time_from(self.clock.now()))
    }

    /// How many distinct buckets exist — the number whose bound must be
    /// asserted, not assumed (see [`rate_key`]).
    #[cfg(test)]
    pub(super) fn tracked_keys(&self) -> usize {
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
pub(super) type RateKey = u64;

/// Key every token-less request shares when authentication is disabled.
///
/// A named constant rather than the fingerprint of the empty string so the
/// anonymous bucket is recognisable in a debugger and cannot collide with a
/// real token's fingerprint by accident.
pub(super) const ANONYMOUS_RATE_KEY: RateKey = 0;

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
pub(super) fn rate_key(auth_enabled: bool, auth_header: Option<&str>) -> RateKey {
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
pub(super) const RATE_LIMIT_OPERATION: &str = "mcp.request.rate_limit";

/// Round a wait up to whole seconds for a `Retry-After` header, never below 1.
///
/// Shared with the session cap so both shed responses state the same rule:
/// `Retry-After` is an integer number of seconds, and rounding a sub-second
/// remainder DOWN would invite the client back while the limiter still refuses
/// it, turning an honest retry into a second 429.
pub(super) fn retry_after_secs(wait: Duration) -> u64 {
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
pub(super) async fn rate_limit_middleware(
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
