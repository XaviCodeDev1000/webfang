//! Cookie wiring for the [`Container`]: the operator seed and the jar scope.
//!
//! Split out of `container.rs` by seam, not by line arithmetic: both items here
//! are about who owns a cookie jar, and nothing else in the container is.
//!
//! - [`Container::fresh_scrape_cookie_set`] — the per-run `--cookie` seed of the
//!   CLI scrape path.
//! - [`Container::with_cookie_jar_scope`] — the server-level jar scope, which
//!   rebuilds the HTTP port facade over the same hardened client.

use std::sync::Arc;

use wreq::cookie::Jar;

use super::Container;
use crate::application::crawl_options::CrawlOptions;
use crate::application::http_client::ScopedHttpClient;
use crate::domain::ports::HttpClientPort;
use crate::domain::JarScope;

/// The HTTP port facade for `scope`, over the given hardened client.
///
/// The client is shared by every facade this container builds, so rebuilding
/// the facade after a scope change keeps one connection pool, one session
/// health map and one rate limiter.
pub(super) fn scoped_facade(
    concrete: &Arc<crate::application::http_client::HttpClient>,
    scope: JarScope,
    seed: Option<Arc<Jar>>,
) -> Arc<dyn HttpClientPort> {
    Arc::new(ScopedHttpClient::new(Arc::clone(concrete), scope, seed))
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
    /// The facade is rebuilt over the SAME hardened client, so connection
    /// pooling, session health and pacing are untouched. The real consumer is
    /// the long-lived MCP server, which needs a per-session jar and connects
    /// here in T2a-2; CLI runs stay on the default.
    #[must_use]
    pub fn with_cookie_jar_scope(mut self, scope: JarScope, seed: Option<Arc<Jar>>) -> Self {
        tracing::debug!(
            cookie_jar_scope = %scope,
            seeded = seed.is_some(),
            "http_client_facade_rebuilt_for_cookie_jar_scope"
        );
        self.http_client = scoped_facade(&self.http_client_concrete, scope, seed);
        self
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
