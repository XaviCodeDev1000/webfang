//! Scope-aware cookie jar wrapper around the hardened [`HttpClient`].
//!
//! [`HttpClient`] owns one `wreq` client, and `wreq` applies that client's jar
//! to every request it sends. A [`JarScope::Operation`] caller needs the jar to
//! be replaced for a single operation instead, so this wrapper decides the jar
//! per request and hands it to
//! [`HttpClient::get_with_jar`](crate::application::http_client::HttpClient),
//! which installs it on the request builder.
//!
//! The wrapper is the only place that knows about scopes. The domain sees
//! [`HttpClientPort`] and nothing else, so the `wreq` jar type stays inside the
//! application layer.

use std::pin::Pin;
use std::sync::Arc;

use wreq::cookie::Jar;

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
    /// Operator seed (`--cookie`), merged into every per-operation jar.
    seed: Option<Arc<Jar>>,
}

impl ScopedHttpClient {
    /// Wrap `inner`, applying `scope` to every request.
    ///
    /// `seed` holds the operator `--cookie` cookies. They are merged into each
    /// per-operation jar; they never replace the cookies the site set.
    #[must_use]
    pub fn new(inner: Arc<HttpClient>, scope: JarScope, seed: Option<Arc<Jar>>) -> Self {
        Self { inner, scope, seed }
    }

    /// The scope this client applies.
    #[must_use]
    pub fn scope(&self) -> JarScope {
        self.scope
    }
}

/// A fresh jar carrying the seed cookies that apply to `url`.
///
/// The seed jar is built for the first target URL, so its cookies are filtered
/// through RFC domain and path matching instead of copied wholesale: a seed for
/// one host must not leak to another.
fn operation_jar(seed: Option<&Jar>, url: &str) -> Arc<Jar> {
    let jar = Jar::default();
    if let Some(seed) = seed {
        for cookie in seed.matches(url) {
            jar.add(cookie, url);
        }
    }
    Arc::new(jar)
}

impl HttpClientPort for ScopedHttpClient {
    fn get(
        &self,
        url: &str,
    ) -> Pin<Box<dyn std::future::Future<Output = HttpResult<HttpResponse>> + Send + '_>> {
        let url = url.to_owned();
        let scope = self.scope;
        let seed = self.seed.clone();
        Box::pin(async move {
            let jar = match scope {
                JarScope::Shared => None,
                JarScope::Operation => Some(operation_jar(seed.as_deref(), &url)),
            };
            self.inner.get_with_jar(&url, jar).await
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
}
