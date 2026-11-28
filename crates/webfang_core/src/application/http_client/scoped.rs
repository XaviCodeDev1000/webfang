//! Scope-aware cookie jar wrapper around the hardened [`HttpClient`].
//!
//! [`HttpClient`] owns one `wreq` client, and `wreq` applies that client's jar
//! to every request it sends. A [`JarScope::Operation`] or
//! [`JarScope::Session`] caller needs the jar to be replaced instead, so this
//! wrapper decides the jar per request and hands it to
//! [`HttpClient::get_with_jar`](crate::application::http_client::HttpClient),
//! which installs it on the request builder.
//!
//! The wrapper is the only place that knows about scopes. The domain sees
//! [`HttpClientPort`] and nothing else, so the `wreq` jar type stays inside the
//! application layer.

use std::pin::Pin;
use std::sync::Arc;

use wreq::cookie::Jar;

use crate::application::http_client::session_jars::{SessionJarLease, SessionJarStore};
use crate::application::http_client::HttpClient;
use crate::domain::http_error::HttpResult;
use crate::domain::http_port::{HttpClientPort, HttpResponse};
use crate::domain::JarScope;

/// Wraps the hardened client and picks the cookie jar for each request.
pub struct ScopedHttpClient {
    /// The hardened client that actually performs the fetch.
    inner: Arc<HttpClient>,
    /// How long a jar lives.
    scope: JarScope,
    /// Operator seed (`--cookie`), merged into every scoped jar.
    seed: Option<Arc<Jar>>,
    /// The live jars of [`JarScope::Session`]; `None` for every other scope.
    sessions: Option<Arc<SessionJarStore>>,
    /// The session this facade serves. `None` means "not bound to a session",
    /// which is the documented fallback (see [`ScopedHttpClient::for_session`]).
    session: Option<String>,
}

impl ScopedHttpClient {
    /// Wrap `inner`, applying `scope` to every request.
    ///
    /// `seed` holds the operator `--cookie` cookies. They are merged into each
    /// scoped jar; they never replace the cookies the site set.
    #[must_use]
    pub fn new(inner: Arc<HttpClient>, scope: JarScope, seed: Option<Arc<Jar>>) -> Self {
        let sessions = scope
            .is_session()
            .then(|| Arc::new(SessionJarStore::with_defaults()));
        Self {
            inner,
            scope,
            seed,
            sessions,
            session: None,
        }
    }

    /// Keep the store this facade already owns, or attach one.
    ///
    /// Rebuilding the facade must not throw the live session jars away, so a
    /// scope change carries the existing store forward instead of allocating a
    /// second one.
    #[must_use]
    pub fn with_sessions(mut self, sessions: Option<Arc<SessionJarStore>>) -> Self {
        self.sessions = sessions;
        self
    }

    /// The scope this client applies.
    #[must_use]
    pub fn scope(&self) -> JarScope {
        self.scope
    }

    /// The operator seed every scoped jar starts with.
    #[must_use]
    pub fn seed(&self) -> Option<&Arc<Jar>> {
        self.seed.as_ref()
    }

    /// The hardened client underneath; shared with every facade built from this
    /// one.
    #[must_use]
    pub fn hardened(&self) -> &Arc<HttpClient> {
        &self.inner
    }

    /// The live session jars, when the scope keeps any.
    #[must_use]
    pub fn sessions(&self) -> Option<&Arc<SessionJarStore>> {
        self.sessions.as_ref()
    }

    /// A facade bound to one caller session, over the same hardened client.
    /// `None` when the scope is not [`JarScope::Session`] — there is no
    /// per-session jar to bind.
    ///
    /// Binding is what makes the scope real: without a session id a
    /// `session`-scoped facade behaves exactly like [`JarScope::Shared`] (the
    /// client's own process jar), which is the documented fallback for a
    /// request that carries no `Mcp-Session-Id` at all — the `initialize` call,
    /// a stateless transport and every stdio run.
    #[must_use]
    pub fn for_session(&self, lease: &SessionJarLease) -> Option<Self> {
        let sessions = self.sessions()?;
        Some(Self {
            inner: Arc::clone(&self.inner),
            scope: self.scope,
            seed: self.seed.clone(),
            sessions: Some(Arc::clone(sessions)),
            session: Some(lease.session().to_owned()),
        })
    }

    /// The jar this request must use.
    fn jar_for(&self, url: &str) -> Option<Arc<Jar>> {
        match self.scope {
            JarScope::Shared => None,
            JarScope::Operation => Some(operation_jar(self.seed.as_deref(), url)),
            // No session bound: the client's own process jar applies, which is
            // the pre-`session` behavior.
            JarScope::Session => {
                let session = self.session.as_deref()?;
                let store = self.sessions.as_ref()?;
                Some(store.jar_for(session, self.seed.as_deref(), url))
            },
        }
    }
}

/// A fresh jar carrying the seed cookies that apply to `url`.
///
/// The seed is filtered PER URL through RFC domain and path matching instead of
/// copied wholesale, and the filtered copy is what the operation accumulates the
/// site's cookies in; a seed for one host therefore cannot leak to another.
fn operation_jar(seed: Option<&Jar>, url: &str) -> Arc<Jar> {
    let jar = Jar::default();
    if let Some(seed) = seed {
        // `Jar::matches` answers the question a REQUEST asks: which cookies
        // apply to this URI (RFC 6265 domain + path + secure + expiry). The
        // same jar's `get` is a different question — an EXACT host + path scope
        // lookup — so seeding through `get` would drop a cookie stored for `/`
        // the moment the target is `/page`. This is why the filter is `matches`.
        for cookie in seed.matches(url) {
            jar.add(cookie, url);
        }
    }
    Arc::new(jar)
}

/// A session's cookie scope: the port to fetch through, plus the lease that
/// frees the session's jar when it is dropped.
///
/// This is the whole per-call surface the MCP layer needs: the handler holds one
/// binding for the life of its session, routes every fetch in that session
/// through [`port`](Self::port), and dropping the binding releases the jar.
#[derive(Clone)]
pub struct SessionCookieBinding {
    port: Arc<dyn HttpClientPort>,
    lease: SessionJarLease,
}

impl SessionCookieBinding {
    /// The port every fetch of this session must go through.
    #[must_use]
    pub fn port(&self) -> Arc<dyn HttpClientPort> {
        Arc::clone(&self.port)
    }

    /// The session this binding belongs to.
    #[must_use]
    pub fn session(&self) -> &str {
        self.lease.session()
    }
}

impl ScopedHttpClient {
    /// Bind `session` to its own jar: the port to fetch through, plus the lease
    /// that releases the jar when the owner drops it.
    ///
    /// `None` when the scope keeps no per-session jars, so the caller can fall
    /// back to the container's own facade without inspecting the scope.
    #[must_use]
    pub fn bind_session(&self, session: &str) -> Option<SessionCookieBinding> {
        let store = self.sessions()?;
        let lease = store.lease(session);
        let bound = self.for_session(&lease)?;
        Some(SessionCookieBinding {
            port: Arc::new(bound),
            lease,
        })
    }
}

impl HttpClientPort for ScopedHttpClient {
    fn get(
        &self,
        url: &str,
    ) -> Pin<Box<dyn std::future::Future<Output = HttpResult<HttpResponse>> + Send + '_>> {
        let url = url.to_owned();
        let jar = self.jar_for(&url);
        let session = self.session.clone();
        let sessions = self.sessions.clone();
        Box::pin(async move {
            let result = self.inner.get_with_jar(&url, jar).await;
            // The jar has grown only now: a response's `Set-Cookie` is stored
            // while the request runs, so the bound is applied afterwards.
            if let (Some(store), Some(session)) = (sessions.as_ref(), session.as_deref()) {
                store.trim(session);
            }
            result
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operation_jar_is_empty_without_a_seed() {
        let jar = operation_jar(None, "https://example.com/page");
        assert!(jar.is_empty());
    }

    #[test]
    fn operation_jar_carries_seed_cookies_for_the_target_host() {
        let seed = Jar::default();
        seed.add("session=abc", "https://example.com/");

        let jar = operation_jar(Some(&seed), "https://example.com/page");

        assert_eq!(jar.len(), 1);
        // `Jar::get` is an EXACT host+`path` scope lookup, so a cookie stored for
        // `/` is not what it answers for `/page`; the question a request asks is
        // what `matches` offers for its URI.
        let offered: Vec<String> = jar
            .matches("https://example.com/page")
            .map(|cookie| format!("{}={}", cookie.name(), cookie.value()))
            .collect();
        assert_eq!(offered, vec!["session=abc".to_owned()]);
    }

    #[test]
    fn operation_jar_does_not_leak_a_seed_to_another_host() {
        let seed = Jar::default();
        seed.add("session=abc", "https://example.com/");

        let jar = operation_jar(Some(&seed), "https://other.example.org/page");

        assert!(jar.is_empty());
    }

    #[test]
    fn each_operation_gets_an_independent_jar() {
        let seed = Jar::default();
        let first = operation_jar(Some(&seed), "https://example.com/");
        first.add("crawl=1", "https://example.com/");

        let second = operation_jar(Some(&seed), "https://example.com/");

        assert_eq!(first.len(), 1);
        assert!(second.is_empty());
    }

    #[test]
    fn only_the_session_scope_keeps_a_store() {
        let shared = ScopedHttpClient::new(
            Arc::new(HttpClient::new(Default::default()).expect("client builds")),
            JarScope::Shared,
            None,
        );
        assert!(shared.sessions().is_none());

        let session = ScopedHttpClient::new(shared.hardened().clone(), JarScope::Session, None);
        assert!(session.sessions().is_some());
    }

    #[test]
    fn a_session_less_facade_offers_no_jar_of_its_own() {
        // The documented fallback: `session` without a bound session id is the
        // client's own process jar, i.e. `shared`.
        let client = ScopedHttpClient::new(
            Arc::new(HttpClient::new(Default::default()).expect("client builds")),
            JarScope::Session,
            None,
        );
        assert!(client.jar_for("https://example.com/").is_none());
    }

    #[test]
    fn a_bound_facade_offers_the_stores_jar_for_its_session() {
        let client = ScopedHttpClient::new(
            Arc::new(HttpClient::new(Default::default()).expect("client builds")),
            JarScope::Session,
            None,
        );
        let store = client
            .sessions()
            .cloned()
            .expect("session scope keeps jars");
        let lease = store.lease("session-a");
        let bound = client.for_session(&lease).expect("session scope binds");

        let jar = bound.jar_for("https://example.com/").expect("a jar");
        jar.add("probe=1", "https://example.com/");

        assert_eq!(
            store.cookie_count("session-a"),
            Some(1),
            "the bound facade writes into the session's stored jar"
        );
        assert_eq!(
            client.sessions().and_then(|s| s.cookie_count("session-b")),
            None,
            "another session has no jar at all"
        );
    }

    #[test]
    fn a_facade_without_a_session_scope_cannot_be_bound() {
        let client = ScopedHttpClient::new(
            Arc::new(HttpClient::new(Default::default()).expect("client builds")),
            JarScope::Operation,
            None,
        );
        let store = Arc::new(SessionJarStore::with_defaults());
        let lease = store.lease("session-a");
        assert!(client.for_session(&lease).is_none());
    }
}
