//! Session health pool — Per-domain session tracking with exponential backoff.
//!
//! Sealed trait pattern: only `DomainSessionPool` can implement `SessionManager`.
//! Uses DashMap for concurrent access without holding locks across `.await`.
//!
//! # Design Decisions
//!
//! - **DashMap<String, DomainEntry>** — concurrent per-domain state, same pattern as UrlDeduplicator
//! - **Exponential backoff** — `base_delay * 2^min(failures, max_exp)`, capped at `max_delay`
//! - **TTL eviction** — stale sessions reset on `acquire()`; idle domains removed
//!   outright by `evict_stale()`, no background thread
//! - **Bounded domain map (#1130)** — `acquire()` enforces the shared
//!   `MAX_TRACKED_DOMAINS` cap by evicting the least-recently-seen domain,
//!   so a long-lived MCP server cannot grow the map linearly with domain
//!   cardinality
//! - **Frozen cooldown (#1134)** — the ban deadline (backoff + per-slot
//!   recovery stagger) is computed once in `apply_failure`; the read path is
//!   a pure `now >= deadline` comparison, so a frozen state always yields the
//!   same verdict
//! - **Balanced slot pick (#1134)** — `find_available` starts its scan at
//!   `hash(domain, now)` instead of always returning slot 0, spreading load
//!   across healthy slots with zero extra state
//! - **Zero-cost abstraction** — `impl SessionManager` not `Box<dyn SessionManager>`
//!
//! # D6 lock-across-await audit (task 2.3, change stabilization-concurrency-budget)
//!
//! Functions rewired by commit de54342a (budget-derived pool size):
//!
//! | Function | `.await` points | Guard discipline | Verdict |
//! |---|---|---|---|
//! | `SessionPoolConfig::default` | none (sync fn) | pure value construction, no locks | PASS |
//! | `DomainSessionPool::acquire` | none (sync `SessionManager` method) | DashMap `RefMut` from `entry().or_insert_with(..)` is explicitly `drop(sessions)`-ped BEFORE any re-entrant map access (the RefMut-dropped-BEFORE-gauge ordering invariant, see comment above the drop in `acquire`) | PASS |
//!
//! Cross-check: the crate's only `tokio::sync::Mutex` guards live in
//! `infrastructure/crawler/url_queue.rs` inside documented sync-only sections
//! (invariant AL-2); this module never spans an await while holding a guard.
//!
//! Enforcement: `#![deny(clippy::await_holding_lock)]` below fails the build if
//! a future edit ever holds a `std` lock guard across an `.await` in this module.

#![deny(clippy::await_holding_lock)]

use std::fmt;
use std::sync::Arc;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use tracing::{debug, instrument, warn};

use crate::domain::budget::DomainSlots;
use crate::domain::clock::{Clock, SystemClock};
use crate::domain::session_port::SessionId;

use pick::pick_start;

/// Health status of a session for a given domain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionStatus {
    /// Session is healthy and available for requests.
    Healthy,
    /// Session is temporarily banned due to consecutive failures.
    Banned,
    /// Session is being retired after exceeding max failures.
    Retiring,
}

/// Per-domain session state tracked by the pool.
#[derive(Debug, Clone)]
struct SessionState {
    status: SessionStatus,
    consecutive_failures: u32,
    last_failure_time: Option<Instant>,
    next_retry_time: Option<Instant>,
}

impl SessionState {
    fn healthy() -> Self {
        Self {
            status: SessionStatus::Healthy,
            consecutive_failures: 0,
            last_failure_time: None,
            next_retry_time: None,
        }
    }
}

/// Per-domain tracked entry: session slots + last-activity stamp.
///
/// #1130: `last_seen` powers both eviction paths — `evict_stale()` removes
/// domains idle past the TTL outright, and `acquire()` evicts the
/// least-recently-seen domain once the map crosses `MAX_TRACKED_DOMAINS`.
#[derive(Debug, Clone)]
struct DomainEntry {
    states: Vec<SessionState>,
    last_seen: Instant,
}

/// Configuration for the session pool.
#[derive(Debug, Clone)]
pub struct SessionPoolConfig {
    /// Number of session slots per domain — the budget model's `Domain`
    /// tier (task 2.2c). A plain `usize` would let zero or unclamped values
    /// in; the [`DomainSlots`] newtype makes an invalid slot count
    /// unrepresentable (design D4).
    pub pool_size: DomainSlots,
    /// Base delay for exponential backoff.
    pub base_delay: Duration,
    /// Maximum delay cap for backoff.
    pub max_delay: Duration,
    /// Maximum exponent for backoff calculation.
    pub max_exp: u32,
    /// TTL for idle sessions before eviction.
    pub ttl_duration: Duration,
}

impl Default for SessionPoolConfig {
    fn default() -> Self {
        Self {
            // LCOV_EXCL_LINE defensive: domain-slot-default — the constant is non-zero by definition
            pool_size: DomainSlots::new(crate::domain::budget::DOMAIN_SLOTS_DEFAULT)
                .unwrap_or_else(|_| unreachable!("domain slot default is non-zero")),
            base_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(60),
            max_exp: 6,
            ttl_duration: Duration::from_secs(300),
        }
    }
}

/// Sealed trait for session manager implementations.
///
/// Only implementors within this module can implement this trait.
pub trait SessionManager: sealed::Sealed {
    /// Acquire an available session for the given domain.
    ///
    /// Returns `None` if all sessions are banned or in cooldown.
    fn acquire(&self, domain: &str) -> Option<SessionId>;

    /// Report a successful request for the given domain's session.
    fn report_success(&self, domain: &str, session_id: SessionId);

    /// Report a failed request with the HTTP status code.
    ///
    /// Status codes 429, 503, and 403 trigger ban logic.
    fn report_failure(&self, domain: &str, session_id: SessionId, status_code: u16);

    /// Remove sessions that have been idle beyond the TTL.
    fn evict_stale(&self);

    /// Return the current pool size for a domain (for diagnostics).
    fn domain_count(&self, domain: &str) -> usize;

    /// Return total tracked domains (for diagnostics).
    fn total_domains(&self) -> usize;
}

/// Per-domain session pool with health tracking and exponential backoff.
#[derive(Clone)]
pub struct DomainSessionPool {
    /// Per-domain session states. Key = domain string, Value = entry with the
    /// slot Vec plus the `last_seen` stamp backing bounded eviction (#1130).
    sessions: DashMap<String, DomainEntry>,
    config: SessionPoolConfig,
    /// Injected clock for deterministic time in tests.
    clock: Arc<dyn Clock>,
}

impl DomainSessionPool {
    /// Create a new session pool with the given configuration and clock.
    #[must_use]
    pub fn new(config: SessionPoolConfig, clock: Arc<dyn Clock>) -> Self {
        Self {
            sessions: DashMap::new(),
            config,
            clock,
        }
    }

    /// Create a pool with default configuration using the system clock.
    #[must_use]
    pub fn default_pool() -> Self {
        Self::new(SessionPoolConfig::default(), Arc::new(SystemClock))
    }

    /// Build the pool from the domain `SessionPoolConfig` DTO.
    ///
    /// ADR-0012-B sub-slice 3.F (#1075): the composition-root helper
    /// `application::container::build_crawl_session_pool` is the only
    /// production caller — it lets the application layer hand over the
    /// domain config DTO without ever naming this module's infrastructure
    /// config type. The mapping is field-for-field: the domain DTO mirrors
    /// [`SessionPoolConfig`] exactly (same five fields, same defaults).
    #[must_use]
    pub fn from_domain_config(
        config: crate::domain::session_port::SessionPoolConfig,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self::new(
            SessionPoolConfig {
                pool_size: config.pool_size,
                base_delay: config.base_delay,
                max_delay: config.max_delay,
                max_exp: config.max_exp,
                ttl_duration: config.ttl_duration,
            },
            clock,
        )
    }

    /// Calculate exponential backoff delay for a given failure count.
    ///
    /// Applies ±20% jitter to prevent thundering herd on retry.
    fn backoff_delay(&self, consecutive_failures: u32) -> Duration {
        use rand::Rng;
        let exponent = consecutive_failures.min(self.config.max_exp);
        let base_ms = self.config.base_delay.as_millis();
        let max_ms = self.config.max_delay.as_millis();
        let delay_ms = base_ms.saturating_mul(2u128.pow(exponent));
        let capped = delay_ms.min(max_ms).max(1);
        // ±20% jitter: random factor between 0.8 and 1.2
        let jitter_factor = rand::rng().random_range(0.8_f64..=1.2);
        let jittered = (capped as f64 * jitter_factor).round() as u64;
        Duration::from_millis(jittered.max(1))
    }

    /// Pick an available session with a hash-rotated scan (#1134).
    ///
    /// The scan starts at the slot derived from `hash(domain, now)` and wraps
    /// around, so successive acquires at different instants spread across
    /// slots instead of stampeding on `SessionId(0)`. Zero new state: the
    /// start is recomputed per call, and the same `(domain, now)` pair always
    /// resolves to the same slot — the pick stays reproducible under an
    /// injected clock.
    fn find_available(
        &self,
        domain: &str,
        sessions: &[SessionState],
        now: Instant,
    ) -> Option<SessionId> {
        let len = sessions.len();
        let start = pick_start(domain, now, len);
        for k in 0..len {
            let idx = (start + k) % len;
            let state = &sessions[idx];
            match state.status {
                SessionStatus::Healthy => {
                    debug!(domain, session_id = idx, "acquired healthy session");
                    return Some(SessionId(idx));
                },
                SessionStatus::Banned => {
                    if Self::banned_session_ready(state, now) {
                        debug!(domain, session_id = idx, "acquired session after cooldown");
                        return Some(SessionId(idx));
                    }
                },
                SessionStatus::Retiring => continue,
            }
        }
        warn!(domain, "no available sessions for domain");
        None
    }

    /// Whether a banned session's cooldown has elapsed.
    ///
    /// #1134: pure comparison — the deadline (backoff + per-slot recovery
    /// stagger) was frozen at ban time in [`Self::apply_failure`], so the
    /// read path never draws rand and a frozen state always yields the same
    /// verdict.
    fn banned_session_ready(state: &SessionState, now: Instant) -> bool {
        state
            .next_retry_time
            .is_some_and(|deadline| now >= deadline)
    }

    /// Apply a failure to a session state, banning or retiring as appropriate.
    fn apply_failure(
        &self,
        state: &mut SessionState,
        domain: &str,
        session_id: SessionId,
        status_code: u16,
        should_ban: bool,
    ) {
        let now = self.clock.now();
        state.consecutive_failures += 1;
        state.last_failure_time = Some(now);

        if should_ban {
            // #1134: the cooldown deadline is frozen HERE — one rand draw for
            // the exponential backoff plus a deterministic per-slot recovery
            // stagger (0–20% of the delay, spread by slot index). Reads never
            // re-draw: `banned_session_ready` is a pure `now >= deadline`
            // comparison, so the same frozen state always yields the same
            // verdict while different slots still recover at different times.
            let delay = self.backoff_delay(state.consecutive_failures);
            let stagger_window_ms = (delay.as_millis() as u64 / 5).max(1);
            let slot = u64::try_from(session_id.0).unwrap_or(u64::MAX);
            let stagger_ms = slot.wrapping_mul(37) % stagger_window_ms;
            state.next_retry_time = Some(now + delay + Duration::from_millis(stagger_ms));
            state.status = SessionStatus::Banned;
            warn!(
                domain,
                session_id = session_id.0,
                status_code,
                failures = state.consecutive_failures,
                backoff_secs = delay.as_secs(),
                stagger_ms,
                "session banned with exponential backoff (cooldown frozen)"
            );
        } else {
            // Non-ban failure: mark retiring after threshold
            if state.consecutive_failures >= 3 {
                state.status = SessionStatus::Retiring;
                warn!(
                    domain,
                    session_id = session_id.0,
                    failures = state.consecutive_failures,
                    "session retiring after repeated failures"
                );
            }
        }
    }
}

impl fmt::Debug for DomainSessionPool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DomainSessionPool")
            .field("domains", &self.sessions.len())
            .field("config", &self.config)
            .finish()
    }
}

impl sealed::Sealed for DomainSessionPool {}

impl SessionManager for DomainSessionPool {
    #[instrument(skip(self), fields(domain = %domain))]
    fn acquire(&self, domain: &str) -> Option<SessionId> {
        let now = self.clock.now();
        let mut entry = self
            .sessions
            .entry(domain.to_string())
            .or_insert_with(|| DomainEntry {
                states: vec![SessionState::healthy(); self.config.pool_size.get()],
                last_seen: now,
            });

        // Evict stale sessions first
        self.evict_expired(domain, &mut entry.states, now);
        entry.last_seen = now;

        // Find first healthy or recoverable session
        let result = self.find_available(domain, &entry.states, now);

        // Drop the DashMap RefMut before refreshing the gauge to avoid deadlock:
        // any map-wide scan (gauge refresh, cap eviction) calls self.sessions.iter()
        // which needs read access to the same shard that `entry` holds a write
        // lock on (the RefMut-dropped-BEFORE-scan ordering invariant).
        drop(entry);

        // #1130: enforce the shared domain cap AFTER the shard guard is gone —
        // keeps the map bounded in long-lived processes (MCP server).
        self.enforce_domain_cap();

        result
    }

    #[instrument(skip(self), fields(domain = %domain, session_id = %session_id.0))]
    fn report_success(&self, domain: &str, session_id: SessionId) {
        if let Some(mut entry) = self.sessions.get_mut(domain) {
            // Activity touch: keeps the domain ahead of LRU cap eviction (#1130).
            entry.last_seen = self.clock.now();
            if let Some(state) = entry.states.get_mut(session_id.0) {
                state.status = SessionStatus::Healthy;
                state.consecutive_failures = 0;
                state.last_failure_time = None;
                state.next_retry_time = None;
                debug!(domain, session_id = session_id.0, "session marked healthy");
            }
        }
    }

    #[instrument(skip(self), fields(domain = %domain, session_id = %session_id.0, status_code))]
    fn report_failure(&self, domain: &str, session_id: SessionId, status_code: u16) {
        // Only ban on signals that indicate domain-level blocking
        let should_ban = matches!(status_code, 429 | 503 | 403);

        if let Some(mut entry) = self.sessions.get_mut(domain) {
            // Activity touch: keeps the domain ahead of LRU cap eviction (#1130).
            entry.last_seen = self.clock.now();
            if let Some(state) = entry.states.get_mut(session_id.0) {
                self.apply_failure(state, domain, session_id, status_code, should_ban);
            }
        }
    }

    #[instrument(skip(self))]
    fn evict_stale(&self) {
        let now = self.clock.now();
        for mut entry in self.sessions.iter_mut() {
            let domain = entry.key().clone();
            let sessions = &mut entry.value_mut().states;
            let mut evicted = 0;
            for state in sessions.iter_mut() {
                if let Some(last_failure) = state.last_failure_time {
                    if now.duration_since(last_failure) > self.config.ttl_duration
                        && state.status != SessionStatus::Healthy
                    {
                        *state = SessionState::healthy();
                        evicted += 1;
                    }
                }
            }
            if evicted > 0 {
                debug!(domain = %domain, evicted, "evicted stale sessions");
            }
        }

        // #1130: real removal — domains idle past the TTL lose their entry
        // entirely (the reset above only heals sessions inside the Vec).
        let ttl = self.config.ttl_duration;
        let mut removed = 0usize;
        self.sessions.retain(|domain, entry| {
            let idle = now.duration_since(entry.last_seen) > ttl;
            if idle {
                removed += 1;
                debug!(domain = %domain, "evicted idle session-pool domain (TTL)");
            }
            !idle
        });
        if removed > 0 {
            debug!(
                removed,
                tracked = self.sessions.len(),
                "idle session-pool domains evicted"
            );
        }
    }

    fn domain_count(&self, domain: &str) -> usize {
        self.sessions
            .get(domain)
            .map(|e| e.states.len())
            .unwrap_or(0)
    }

    fn total_domains(&self) -> usize {
        self.sessions.len()
    }
}

impl crate::domain::session_port::SessionPort for DomainSessionPool {
    fn acquire(&self, domain: &str) -> Option<SessionId> {
        SessionManager::acquire(self, domain)
    }

    fn report_success(&self, domain: &str, session: SessionId) {
        SessionManager::report_success(self, domain, session)
    }

    fn report_failure(&self, domain: &str, session: SessionId, status: u16) {
        SessionManager::report_failure(self, domain, session, status)
    }
}

/// Sealed trait internals — prevents external implementations.
mod sealed {
    pub trait Sealed {}
}

#[path = "session_pool_eviction.rs"]
mod eviction;

#[path = "session_pool_pick.rs"]
mod pick;

#[cfg(test)]
#[path = "session_pool_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "session_pool_config_tests.rs"]
mod config_tests;
