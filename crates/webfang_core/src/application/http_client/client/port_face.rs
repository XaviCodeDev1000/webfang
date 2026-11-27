//! The port face of the hardened client: one raw GET, reporting status as data.
//!
//! Split out of `client.rs` for the file-size ratchet, without changing a line
//! of the contract documented below: the hardened request path
//! (`HttpClient::get`) and this port path (`HttpClientPort::get`) carry two
//! deliberately different contracts.

use std::sync::Arc;

use crate::domain::body_cap;
use crate::domain::error::cause::FetchPhase;
use crate::domain::error::mapping::url_host_of;

use super::HttpClient;

/// Raw, single-shot [`HttpClientPort`] implementation for [`HttpClient`].
///
/// # Why this path is deliberately raw
///
/// [`HttpClient`] exposes two request paths with different contracts, and they
/// are intentionally **not** unified:
///
/// - The hardened path — `HttpClient::get` / `get_inner` — returns only the
///   response body as a `String`, after applying rate limiting, per-domain
///   session gating, status-specific retries (429 / 5xx), user-agent rotation
///   on 403, and WAF/CAPTCHA inspection. Non-2xx outcomes are surfaced as
///   [`HttpError`] variants.
/// - This port implementation returns a full [`HttpResponse`]
///   (`status` + `headers` + `body`) for a **single** GET with no retry, no
///   user-agent rotation and no WAF inspection, reporting non-2xx as data in
///   `status` rather than as an error. It exists for callers that depend on the
///   domain port and manage their own request lifecycle.
///
/// Routing this impl through the hardened path would change its observable
/// behavior — it would start retrying, rotating user agents, and turning
/// 5xx/429 responses into [`HttpError`]s instead of returning the status — so
/// the divergence is preserved and documented here rather than collapsed
/// (see #444). The bare-client factories (e.g. [`create_http_client`]) are raw
/// for the same reason.
///
/// [`HttpClientPort`]: crate::domain::http_port::HttpClientPort
/// [`HttpClient`]: crate::application::http_client::HttpClient
/// [`HttpResponse`]: crate::domain::http_port::HttpResponse
/// [`HttpError`]: crate::domain::http_error::HttpError
/// [`create_http_client`]: crate::application::http_client::create_http_client
///
/// The port face of the hardened client: every ordinary fetch keeps using the
/// client's own cookie jar. Scoped fetching goes through
/// `HttpClient::get_with_jar`, which is crate-internal on purpose — the jar
/// type never reaches the domain port.
/// The boxed future the domain port hands back.
///
/// Named once: the port, its wrapper and every implementor would otherwise
/// spell the same twelve-line type, and the duplication ratchet counts each
/// copy of it.
type HttpFuture<'a> = std::pin::Pin<
    Box<
        dyn std::future::Future<
                Output = crate::domain::http_error::HttpResult<
                    crate::domain::http_port::HttpResponse,
                >,
            > + Send
            + 'a,
    >,
>;

impl crate::domain::http_port::HttpClientPort for HttpClient {
    fn get(&self, url: &str) -> HttpFuture<'_> {
        self.get_with_jar(url, None)
    }
}

impl HttpClient {
    /// Fetch a URL, optionally replacing the cookie jar for this request only.
    ///
    /// `None` is the historical behavior: the client's own jar applies.
    /// `Some(jar)` scopes cookies to that jar for this request and the hops it
    /// follows, overriding the client's jar exactly as `wreq` documents for
    /// [`RequestBuilder::cookie_provider`](wreq::Client::get). The caller owns
    /// the jar, so a per-operation jar can be dropped when the operation ends.
    pub(crate) fn get_with_jar(
        &self,
        url: &str,
        jar: Option<Arc<wreq::cookie::Jar>>,
    ) -> HttpFuture<'_> {
        let url = url.to_owned();
        Box::pin(async move {
            // Use the inner wreq client for the raw request, then build
            // a full HttpResponse (status + body + headers).
            let request_started = std::time::Instant::now();
            let builder = self.client.get(url.as_str());
            let builder = match jar.as_ref() {
                Some(jar) => builder.cookie_provider(Arc::clone(jar)),
                None => builder,
            };
            let resp = builder.send().await.map_err(|e| {
                // #2841: the ONE classification seam, shared with `http_failure`
                // — no second tri-branch to drift.
                crate::domain::error::mapping::http_failure(
                    e,
                    FetchPhase::Connect,
                    url_host_of(&url).as_deref(),
                    request_started,
                )
            })?;

            let status = resp.status().as_u16();
            let mut headers = std::collections::HashMap::new();
            for (key, value) in resp.headers() {
                if let Ok(v) = value.to_str() {
                    headers.insert(key.as_str().to_lowercase(), v.to_string());
                }
            }
            let body = body_cap::read_body_capped(resp, self.config.max_page_bytes).await?;

            Ok(crate::domain::http_port::HttpResponse {
                status,
                body,
                headers,
            })
        })
    }
}
