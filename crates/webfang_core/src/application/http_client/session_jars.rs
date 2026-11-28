//! Session-scoped cookie jars — one jar per caller session, bounded.
//!
//! [`JarScope::Session`](crate::domain::JarScope::Session) needs a jar that
//! outlives a single request: the MCP server is a long-lived process serving
//! many client sessions, and a cookie a site sets during one `tools/call` must
//! be offered again on the next call of the SAME session while never reaching
//! another session's requests.
//!
//! That lifetime is what this module exists for. A process-wide jar is
//! unbounded in practice (it lives as long as the server) and a per-request jar
//! forgets everything, so the store keeps one jar per session with three
//! bounds that mirror how the MCP transport itself reaps sessions:
//!
//! - **Live jars.** [`SessionJarStore`] never holds more than
//!   `limits.max_jars` jars: inserting past the cap evicts the
//!   least-recently-used one. The default equals the MCP server's own admission
//!   default (`DEFAULT_MAX_SESSIONS`), so a jar can never outlive the session
//!   budget that admits its session.
//! - **Jar size.** Trimming keeps the NEWEST cookies and drops the oldest ones
//!   until both `max_cookies_per_jar` and `max_cookie_bytes_per_jar` hold. A
//!   site that answers with an unbounded `Set-Cookie` stream therefore cannot
//!   grow the process without bound.
//! - **Idle expiry.** A jar untouched for `idle_ttl` is dropped. The default is
//!   rmcp's own session keep-alive (`SessionConfig::DEFAULT_KEEP_ALIVE`, 300 s),
//!   the same window `DEFAULT_SESSION_CAP_WINDOW_SECS` mirrors: a session that
//!   long without a request is already gone on the transport side, so keeping
//!   its cookies would keep state for a caller that no longer exists.
//!
//! Release is explicit as well: [`SessionJarLease`] frees its session's jar when
//! it is dropped, which is how the MCP handler ties the jar's life to the
//! session worker's.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use wreq::cookie::Jar;

use crate::domain::clock::{Clock, SystemClock};

/// Maximum number of live session jars.
///
/// Mirrors the MCP server's own `DEFAULT_MAX_SESSIONS`: one jar per admitted
/// session is the ceiling, so a server that sheds the Nth client cannot be
/// holding cookies for it.
pub const DEFAULT_MAX_SESSION_JARS: usize = 64;

/// Maximum number of cookies one session jar keeps.
///
/// 128 is comfortably above what a site needs to carry consent, a
/// load-balancing cookie, a WAF clearance cookie and a login session, and far
/// below anything that would matter for memory.
pub const DEFAULT_MAX_COOKIES_PER_JAR: usize = 128;

/// Maximum total `name + value` bytes one session jar keeps.
///
/// 64 KiB: the point is a bound on a single caller's influence, so the value is
/// deliberately small next to the process's own page cap (50 MiB) and large
/// next to a normal cookie (< 4 KiB).
pub const DEFAULT_MAX_COOKIE_BYTES_PER_JAR: usize = 64 * 1024;

/// Idle lifetime of one session jar, in seconds.
///
/// 300 is rmcp's `SessionConfig::DEFAULT_KEEP_ALIVE`: a session with no traffic
/// for that long is already closed on the transport side, so its jar is state
/// for a caller that no longer exists.
pub const DEFAULT_SESSION_JAR_IDLE_TTL_SECS: u64 = 300;

/// The bounds one [`SessionJarStore`] enforces.
#[derive(Debug, Clone, Copy)]
pub struct SessionJarLimits {
    /// Maximum number of live jars.
    pub max_jars: usize,
    /// Maximum number of cookies kept in one jar.
    pub max_cookies_per_jar: usize,
    /// Maximum total `name + value` bytes kept in one jar.
    pub max_cookie_bytes_per_jar: usize,
    /// How long a jar may stay untouched before it is dropped.
    pub idle_ttl: Duration,
}

impl Default for SessionJarLimits {
    fn default() -> Self {
        Self {
            max_jars: DEFAULT_MAX_SESSION_JARS,
            max_cookies_per_jar: DEFAULT_MAX_COOKIES_PER_JAR,
            max_cookie_bytes_per_jar: DEFAULT_MAX_COOKIE_BYTES_PER_JAR,
            idle_ttl: Duration::from_secs(DEFAULT_SESSION_JAR_IDLE_TTL_SECS),
        }
    }
}

/// One live jar plus the last time a request used it.
struct Entry {
    jar: Arc<Jar>,
    last_used: Instant,
    /// Hosts this session has requested, newest last. Trimming needs a host to
    /// name the exact RFC 6265 scope of a cookie it wants to drop, and a
    /// host-only cookie snapshot does not carry one.
    hosts: Vec<String>,
}

#[derive(Default)]
struct State {
    entries: HashMap<String, Entry>,
}

/// One jar per session, bounded by count, size and idle time.
pub struct SessionJarStore {
    state: Mutex<State>,
    limits: SessionJarLimits,
    clock: Arc<dyn Clock>,
}

impl SessionJarStore {
    /// Build a store with production time and the default bounds.
    #[must_use]
    pub fn with_defaults() -> Self {
        Self::with_limits(SessionJarLimits::default())
    }

    /// Build a store with production time and the given bounds.
    #[must_use]
    pub fn with_limits(limits: SessionJarLimits) -> Self {
        Self::with_clock(limits, Arc::new(SystemClock))
    }

    /// Build a store with an injected clock, for deterministic expiry tests.
    #[must_use]
    pub fn with_clock(limits: SessionJarLimits, clock: Arc<dyn Clock>) -> Self {
        Self {
            state: Mutex::new(State::default()),
            limits,
            clock,
        }
    }

    /// The bounds this store enforces.
    #[must_use]
    pub fn limits(&self) -> SessionJarLimits {
        self.limits
    }

    /// The jar of `session`, creating it from the operator `seed` on first use.
    ///
    /// `url` is the request about to be sent: it seeds a new jar (filtered by the
    /// RFC domain and path matching that applies to it) and is remembered as one
    /// of the hosts this session talks to, which trimming needs to name the exact
    /// scope of a cookie it drops.
    ///
    /// The seed is offered only once, at creation: re-seeding on every request
    /// would let an operator cookie silently overwrite a value the site has since
    /// replaced.
    pub fn jar_for(&self, session: &str, seed: Option<&Jar>, url: &str) -> Arc<Jar> {
        let now = self.clock.now();
        let mut state = self.lock();
        state.drop_expired(now, self.limits.idle_ttl);
        let entry = state.entries.entry(session.to_owned()).or_insert_with(|| {
            let jar = Jar::default();
            if let Some(seed) = seed {
                seed_into(&jar, seed, url);
            }
            Entry {
                jar: Arc::new(jar),
                last_used: now,
                hosts: Vec::new(),
            }
        });
        entry.last_used = now;
        if let Some(host) = host_of(url) {
            if !entry.hosts.iter().any(|known| known == &host) {
                entry.hosts.push(host);
            }
        }
        let jar = Arc::clone(&entry.jar);
        state.evict_to_cap(self.limits.max_jars);
        jar
    }

    /// Drop `session`'s jar. Returns whether one was live.
    pub fn release(&self, session: &str) -> bool {
        self.lock().entries.remove(session).is_some()
    }

    /// Number of live jars, after dropping the ones that expired.
    pub fn live_jars(&self) -> usize {
        let now = self.clock.now();
        let mut state = self.lock();
        state.drop_expired(now, self.limits.idle_ttl);
        state.entries.len()
    }

    /// Number of cookies `session`'s jar holds, or `None` when it has none.
    pub fn cookie_count(&self, session: &str) -> Option<usize> {
        self.lock()
            .entries
            .get(session)
            .map(|entry| entry.jar.len())
    }

    /// Trim `session`'s jar to the size bounds, keeping the newest cookies.
    ///
    /// Called after a request, because that is when the jar has grown: the
    /// cookies a response set are only visible once `wreq` has stored them.
    pub fn trim(&self, session: &str) {
        let mut state = self.lock();
        let Some(entry) = state.entries.get_mut(session) else {
            return;
        };
        let cookies: Vec<wreq::cookie::Cookie<'static>> = entry.jar.get_all().collect();
        let drop = how_many_to_drop(&cookies, self.limits);
        if drop == 0 {
            return;
        }
        for cookie in &cookies[..drop] {
            // `Jar::remove` scopes by host + path, which a host-only snapshot
            // does not carry: try every host this session has talked to, plus
            // the cookie's own domain when it has one.
            for host in entry
                .hosts
                .iter()
                .map(String::as_str)
                .chain(cookie.domain())
            {
                let path = cookie.path().unwrap_or("/");
                entry
                    .jar
                    .remove(cookie.clone(), format!("https://{host}{path}"));
            }
        }
        tracing::debug!(
            session,
            dropped = drop,
            kept = cookies.len().saturating_sub(drop),
            "session_cookie_jar_trimmed"
        );
    }

    /// A lease that releases `session`'s jar when it is dropped.
    pub fn lease(self: &Arc<Self>, session: &str) -> SessionJarLease {
        SessionJarLease {
            store: Arc::clone(self),
            session: session.to_owned(),
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        // A poisoned lock is never fatal: the store's invariant is per-entry
        // bookkeeping, so recovering the guard is strictly better than dropping
        // every session's cookies on the floor.
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl State {
    fn drop_expired(&mut self, now: Instant, idle_ttl: Duration) {
        self.entries
            .retain(|_, entry| now.duration_since(entry.last_used) < idle_ttl);
    }

    /// Evict least-recently-used jars until at most `max_jars` entries remain.
    fn evict_to_cap(&mut self, max_jars: usize) {
        while self.entries.len() > max_jars {
            let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.last_used)
                .map(|(session, _)| session.clone())
            else {
                return;
            };
            self.entries.remove(&oldest);
            tracing::debug!(session = %oldest, "session_cookie_jar_evicted_lru");
        }
    }
}

/// The host of a request URL, as the RFC 6265 scope names it.
///
/// Trimming removes a cookie by `host + path + name`, so the host must be the
/// authority without a scheme, port or trailing path.
fn host_of(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url).ok()?;
    parsed.host_str().map(str::to_owned)
}

/// Copy the seed cookies that apply to `url` into `jar`.
///
/// The seed is an operator `--cookie` jar; a seed for one host must not leak to
/// another, so each cookie is filtered through the RFC domain and path matching
/// that applies to `url`.
fn seed_into(jar: &Jar, seed: &Jar, url: &str) {
    for cookie in seed.matches(url) {
        jar.add(cookie, url);
    }
}

/// How many of the OLDEST cookies to drop so both size bounds hold.
///
/// Keeps from the newest backwards: trimming a jar must never discard the cookie
/// a site just set in favor of one it set earlier.
fn how_many_to_drop(cookies: &[wreq::cookie::Cookie<'static>], limits: SessionJarLimits) -> usize {
    let mut bytes = 0usize;
    let mut drop = cookies.len();
    for (kept, cookie) in cookies.iter().rev().enumerate() {
        let size = cookie.name().len() + cookie.value().len();
        if kept + 1 > limits.max_cookies_per_jar
            || bytes.saturating_add(size) > limits.max_cookie_bytes_per_jar
        {
            break;
        }
        bytes += size;
        drop -= 1;
    }
    drop
}

/// A session's claim on its jar: dropping it releases the jar.
///
/// The MCP handler owns one of these for the life of its session worker, so the
/// jar is freed when the session ends — a `DELETE`-closed session, a
/// keep-alive-reaped one and a disconnected client all take the same path.
#[derive(Clone)]
pub struct SessionJarLease {
    store: Arc<SessionJarStore>,
    session: String,
}

impl SessionJarLease {
    /// The session this lease belongs to.
    #[must_use]
    pub fn session(&self) -> &str {
        &self.session
    }

    /// The store holding the jar.
    #[must_use]
    pub fn store(&self) -> &Arc<SessionJarStore> {
        &self.store
    }
}

impl Drop for SessionJarLease {
    fn drop(&mut self) {
        if self.store.release(&self.session) {
            tracing::debug!(
                session = %self.session,
                "session_cookie_jar_released_on_session_end"
            );
        }
    }
}

#[cfg(test)]
#[path = "session_jars_tests.rs"]
mod tests;
