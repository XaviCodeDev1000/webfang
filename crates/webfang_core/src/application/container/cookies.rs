//! Cookie wiring for the [`Container`]: the operator seed and the jar scope.
//!
//! Split out of `container.rs` by seam, not by line arithmetic: both items here
//! are about who owns a cookie jar, and nothing else in the container is.
//!
//! - [`Container::fresh_scrape_cookie_set`] — the per-run `--cookie` seed of the
//!   CLI scrape path.
//! - [`Container::with_cookie_jar_scope`] — the server-level jar scope, which
//!   rebuilds the HTTP port facade over the same hardened client and the same
//!   live session jars.
//! - [`Container::bind_session_cookies`] — the per-call binding that makes
//!   [`JarScope::Session`] real: the port a given MCP session must fetch through,
//!   plus the lease that frees its jar when the session ends.

use std::sync::Arc;

use wreq::cookie::Jar;

use super::Container;
use crate::application::crawl_options::CrawlOptions;
use crate::application::http_client::session_jars::SessionJarStore;
use crate::application::http_client::{HttpClient, ScopedHttpClient, SessionCookieBinding};
use crate::domain::JarScope;

/// The HTTP port facade for `scope`, over the given hardened client.
///
/// Every facade this container builds wraps the SAME client, so rebuilding one
/// keeps a single connection pool, a single session health map and a single rate
/// limiter. `sessions` carries the live session jars forward when the previous
/// facade owned them.
pub(super) fn facade(
    concrete: Arc<HttpClient>,
    scope: JarScope,
    seed: Option<Arc<Jar>>,
    sessions: Option<Arc<SessionJarStore>>,
) -> ScopedHttpClient {
    ScopedHttpClient::new(concrete, scope, seed).with_sessions(sessions)
}

impl Container {
    /// Rebuild the HTTP port facade with a different cookie-jar scope.
    ///
    /// [`Container::new`] wires [`JarScope::Shared`], the historical behavior:
    /// one jar for the whole process, so a cookie any response sets is offered
    /// to every later request. That default is unchanged here — the scope is
    /// opt-in configuration, so passing it changes nothing until a caller asks
    /// for another scope.
    ///
    /// [`JarScope::Operation`] gives every request a fresh jar that starts with
    /// the `seed` operator cookies (`--cookie`) matching the target URL and
    /// accumulates what the site sets while that request runs (redirect hops),
    /// then is dropped: nothing survives into the next operation.
    ///
    /// [`JarScope::Session`] gives every caller SESSION one jar, keyed by the
    /// MCP `Mcp-Session-Id`. The jars are bounded (count, size, idle time) by
    /// [`SessionJarStore`]. Session jars only exist once a session is bound
    /// ([`Self::bind_session_cookies`]); the real consumer is the long-lived MCP
    /// server, which owns both the session id and the session's end.
    ///
    /// The facade is rebuilt over the SAME hardened client and carries the live
    /// session jars forward, so connection pooling, session health, pacing and
    /// already-accepted cookies are untouched.
    #[must_use]
    pub fn with_cookie_jar_scope(mut self, scope: JarScope, seed: Option<Arc<Jar>>) -> Self {
        let sessions = if scope.is_session() {
            Some(
                self.http_client
                    .sessions()
                    .map_or_else(|| Arc::new(SessionJarStore::with_defaults()), Arc::clone),
            )
        } else {
            None
        };
        let limits = sessions.as_ref().map(|store| store.limits());
        tracing::debug!(
            cookie_jar_scope = %scope,
            seeded = seed.is_some(),
            live_session_jars = sessions.as_ref().map_or(0, |store| store.live_jars()),
            "http_client_facade_rebuilt_for_cookie_jar_scope"
        );
        if let Some(limits) = limits {
            tracing::info!(
                cookie_jar_scope = %scope,
                max_session_jars = limits.max_jars,
                max_cookies_per_jar = limits.max_cookies_per_jar,
                max_cookie_bytes_per_jar = limits.max_cookie_bytes_per_jar,
                idle_ttl_secs = limits.idle_ttl.as_secs(),
                "cookie_jar_session_scope_wired"
            );
        }
        self.http_client = Arc::new(facade(
            Arc::clone(self.http_client.hardened()),
            scope,
            seed,
            sessions,
        ));
        self
    }

    /// The cookie jar scope this container was built with.
    #[must_use]
    pub fn cookie_jar_scope(&self) -> JarScope {
        self.http_client.scope()
    }

    /// Bind one caller session to its own cookie jar.
    ///
    /// `None` when the container is not on [`JarScope::Session`], which is when
    /// there is no per-session jar to bind and the caller keeps using
    /// [`Container::http_client`] unchanged.
    ///
    /// The returned binding owns a lease: every fetch routed through
    /// [`SessionCookieBinding::port`] reads and writes the SAME jar across calls,
    /// and dropping the binding frees that jar. The MCP handler holds one for the
    /// life of its session worker, which is exactly how long the session exists.
    #[must_use]
    pub fn bind_session_cookies(&self, session: &str) -> Option<SessionCookieBinding> {
        let binding = self.http_client.bind_session(session);
        if binding.is_some() {
            tracing::debug!(
                cookie_jar_scope = %self.http_client.scope(),
                "session_cookie_jar_bound"
            );
        }
        binding
    }

    /// Fresh cookie bridge + pre-seeded wreq jar for one CLI scrape run.
    ///
    /// Seeds operator `--cookie` values (#890) into BOTH layers: the shared
    /// [`CookieBridge`](crate::domain::cookie_bridge::CookieBridge) (consumed by
    /// the Chromiumoxide L3 layer) and the wreq [`Jar`]
    /// (shared by the Static and Hybrid-L1 layers). Returns `(bridge, None)`
    /// when no cookies were requested or no target URL exists. A new bridge
    /// per run — never the server's long-lived bridge.
    ///
    /// Cookie names/values are credentials: the debug event reports the count
    /// and host scope only, never the secrets.
    #[must_use]
    pub fn fresh_scrape_cookie_set(
        opts: &CrawlOptions,
        urls: &[url::Url],
    ) -> (
        Arc<tokio::sync::RwLock<crate::domain::cookie_bridge::CookieBridge>>,
        Option<Arc<wreq::cookie::Jar>>,
    ) {
        let mut bridge = crate::domain::cookie_bridge::CookieBridge::new();
        if let Some(first_url) = urls.first() {
            let domain = first_url.host_str().unwrap_or("").to_string();
            if !domain.is_empty() {
                for (name, value) in &opts.network.initial_cookies {
                    bridge.seed(name, value, &domain);
                }
            }
        }
        let bridge = Arc::new(tokio::sync::RwLock::new(bridge));
        let jar = if opts.network.initial_cookies.is_empty() {
            None
        } else {
            urls.first().map(|first_url| {
                let jar = wreq::cookie::Jar::default();
                for (name, value) in &opts.network.initial_cookies {
                    jar.add(format!("{name}={value}").as_str(), first_url.as_str());
                }
                tracing::debug!(
                seeded_cookie_count = opts.network.initial_cookies.len(),
                host = %first_url.host_str().unwrap_or(""),
                "seeding operator cookies into the scrape client jar"
                );
                Arc::new(jar)
            })
        };
        (bridge, jar)
    }
}
