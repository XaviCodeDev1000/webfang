//! HTTP client implementation
//!
//! Wraps `wreq::Client` with retry logic, UA rotation, and WAF detection.

use super::factory::build_wreq_client;
use super::retry::compute_backoff_delay;
use crate::domain::body_cap;
use crate::domain::error::cause::FetchPhase;
use crate::domain::error::mapping::{http_failure, url_host_of};
use crate::domain::http_config::HttpClientConfig;
use crate::domain::http_error::{HttpError, HttpResult};
use crate::domain::session_port::{SessionId, SessionPort};
use crate::domain::user_agent::fallback_agents;
use crate::error::ScraperError;
use governor::state::{InMemoryState, NotKeyed};
use governor::{Quota, RateLimiter};

use crate::application::rate_limiter::{
    record_pacing_wait, GovernorClock, GovernorMiddleware, PacingContext,
};
use std::num::NonZeroU32;
use std::sync::Arc;
use tracing::{debug, warn};
use url::Url;
use wreq::Client;

pub use super::factory::{
    create_http_client, create_http_client_with_config, get_random_user_agent_from_pool,
};

/// Response handling: capped body reads, WAF inspection and status retries.
#[path = "client_response.rs"]
mod client_response;

/// HTTP client wrapper with configurable retry behavior
///
/// Wraps `wreq::Client` and adds:
/// - Custom headers from config
/// - Status-specific retry logic
/// - User-agent rotation on 403
/// - Exponential backoff on 429 and 5xx
/// - Request timeout configuration
/// - Rate limiting
/// - URL validation
/// - TLS fingerprint rotation
/// - Request timeout configuration
/// - Rate limiting
/// - URL validation
/// - TLS fingerprint rotation
pub struct HttpClient {
    /// Internal wreq client
    client: Client,
    /// Configuration for headers and retry
    config: HttpClientConfig,
    /// Pool of user agents for rotation
    user_agents: Vec<String>,
    /// Rate limiter for requests per minute
    ///
    /// `GovernorClock` = `QuantaClock` in production, `MonotonicClock` under
    /// Miri (quanta's raw-cpuid inline asm is not executable under Miri, #1514).
    rate_limiter: Option<RateLimiter<NotKeyed, InMemoryState, GovernorClock, GovernorMiddleware>>,
    /// Per-domain session health pool (optional)
    session_pool: Option<Arc<dyn SessionPort>>,
}

impl HttpClient {
    /// Create a new HTTP client with the given configuration
    ///
    /// # Errors
    ///
    /// Returns `ScraperError::Config` if client creation fails
    pub fn new(config: HttpClientConfig) -> Result<Self, ScraperError> {
        let client = build_wreq_client(&config, None)?;

        let mut user_agents = fallback_agents();
        if let Some(ref ua) = config.user_agent {
            user_agents.insert(0, ua.clone());
        }

        // Create rate limiter if configured
        let rate_limiter = if let Some(rpm) = config.rate_limit_rpm {
            if rpm == 0 {
                return Err(ScraperError::Config(
                    "rate_limit_rpm must be greater than 0".into(),
                ));
            }
            // rpm > 0 is guaranteed by the early-return check above, so
            // `NonZeroU32::new` cannot fail — this is a proven invariant.
            #[allow(clippy::expect_used)]
            let quota =
                // LCOV_EXCL_LINE defensive: nonzero-invariant — rpm > 0 is proven by the early-return check above
                Quota::per_minute(NonZeroU32::new(rpm).expect(
                    "invariant: rpm > 0 was already checked above — NonZeroU32 cannot fail",
                ));
            Some(RateLimiter::direct_with_clock(
                quota,
                &GovernorClock::default(),
            ))
        } else {
            None
        };

        debug!("HttpClient created with {} user agents", user_agents.len());
        if let Some(rpm) = config.rate_limit_rpm {
            debug!("Rate limiter configured: {} requests per minute", rpm);
        }

        Ok(Self {
            client,
            config,
            user_agents,
            rate_limiter,
            session_pool: None,
        })
    }

    /// Set the session health pool for per-domain gating.
    #[must_use]
    pub fn with_session_pool(mut self, pool: Arc<dyn SessionPort>) -> Self {
        self.session_pool = Some(pool);
        self
    }
    /// Get a reference to the inner `wreq::Client`.
    ///
    /// Useful when the client needs to be passed to application-layer
    /// functions that expect a raw `&wreq::Client`.
    #[must_use]
    pub fn client(&self) -> &Client {
        &self.client
    }

    /// Perform GET request with retry logic
    ///
    /// Handles status codes as follows:
    /// - 200-299: Returns body as String
    /// - 403: Logs + retries once with rotated user-agent
    /// - 429: Exponential backoff respecting Retry-After header
    /// - 500-599: Exponential backoff with automatic retry
    ///
    /// # Errors
    ///
    /// Returns `HttpError` for failed requests or invalid URLs
    pub async fn get(&self, url: &str) -> HttpResult<String> {
        // Validate URL first
        let parsed_url =
            Url::parse(url).map_err(|e| HttpError::Request(format!("Invalid URL: {e}")))?;

        // Ensure URL has http or https scheme
        if !matches!(parsed_url.scheme(), "http" | "https") {
            return Err(HttpError::Request(
                "URL must use http or https scheme".into(),
            ));
        }

        // Session pool gating: acquire before network request
        let (session_id, domain) = if let Some(ref pool) = self.session_pool {
            let domain = crate::application::url_filter::extract_domain(url)
                .unwrap_or_else(|| parsed_url.host_str().unwrap_or("unknown").to_string());
            match pool.acquire(&domain) {
                Some(id) => (Some(id), domain),
                None => return Err(HttpError::DomainBanned(domain)),
            }
        } else {
            (None, String::new())
        };
        // Apply rate limiting if configured. #1610: this client carries a bare
        // governor limiter (per-session quota), so it measures its own wait and
        // reports it through the shared pacing emitter — the wait is
        // attributable to this URL instead of silently inflating every
        // request's wall clock.
        if let Some(ref limiter) = self.rate_limiter {
            let wait_ctx = PacingContext::bare("http_client").with_url(url);
            let started = std::time::Instant::now();
            limiter.until_ready().await;
            record_pacing_wait(&wait_ctx, started.elapsed(), "granted");
        }

        // Track in-flight requests

        let result = self.get_inner(url).await;

        // Report outcome to session pool
        self.report_outcome(&domain, session_id, &result);

        result
    }

    /// Report request outcome to the session pool for health tracking.
    fn report_outcome(
        &self,
        domain: &str,
        session_id: Option<SessionId>,
        result: &HttpResult<String>,
    ) {
        let (Some(pool), Some(id)) = (self.session_pool.as_ref(), session_id) else {
            return;
        };
        match result {
            Ok(_) => pool.report_success(domain, id),
            Err(e) => {
                let status = match e {
                    HttpError::WafChallenge(_) | HttpError::Forbidden => 403,
                    HttpError::RateLimited(_) => 429,
                    HttpError::Timeout { .. } => 504,
                    HttpError::Connection { .. } => 503,
                    // #2841: an unclassified transport failure is the same
                    // signal to the pool as a connection failure — the remote
                    // was unavailable — NOT a 404-style "don't report".
                    HttpError::Transport { .. } => 503,
                    HttpError::ClientError(404) => return, // don't report 404
                    HttpError::ClientError(code) => *code,
                    HttpError::ServerError(code) => *code,
                    HttpError::Request(_) => return,
                    HttpError::DomainBanned(_) => return,
                    HttpError::BodyTooLarge { .. } => return,
                };
                pool.report_failure(domain, id, status);
            },
        }
    }

    async fn get_inner(&self, url: &str) -> HttpResult<String> {
        let mut ua_index = 0;

        loop {
            if ua_index >= self.user_agents.len() && ua_index > 0 {
                return Err(HttpError::Forbidden);
            }

            let ua = self.select_user_agent(ua_index);
            // F-08 parity (RC-1 slice 5): the initial send retries request
            // timeouts within the retry budget, matching Stack A's
            // `fetch_inner`. Builder-class and connect errors stay terminal
            // (F-09: single attempt).
            let response = self.send_with_timeout_retry(url, &ua, ua_index).await?;

            let status = response.status();

            match status.as_u16() {
                200..=299 => return self.handle_success(url, response, status.as_u16()).await,
                403 => {
                    warn!(url = %url, status = 403, "403 Forbidden — rotating User-Agent");
                    if ua_index == 0 {
                        ua_index += 1;
                        continue;
                    }
                    return Err(HttpError::Forbidden);
                },
                429 => return self.handle_rate_limited(url, response, &ua).await,
                500..=599 => {
                    return self
                        .handle_server_error(url, response, status.as_u16(), &ua)
                        .await
                },
                code if (300..=399).contains(&code) => {
                    // The redirect policy already handled the follow; reaching
                    // here means the redirect could not be resolved (#649).
                    return Err(HttpError::ClientError(code));
                },
                code if (400..=499).contains(&code) => {
                    return Err(HttpError::ClientError(code));
                },
                code => {
                    return Err(HttpError::ServerError(code));
                },
            }
        }
    }

    /// Send the GET request, retrying request timeouts (F-08).
    ///
    /// Stack A parity (`fetch_inner`, #1231): a request timeout is the most
    /// common transient failure in crawling, and the configured timeout
    /// still caps EACH attempt, so the retry budget of `max_retries` extra
    /// attempts is exactly what the operator asked for. Backoff reuses the
    /// shared exponential computation (`compute_backoff_delay`).
    ///
    /// Every non-timeout transport error stays terminal (F-09, #1236):
    /// builder-class errors (unsupported scheme, malformed URL — already
    /// rejected pre-dial in `get`) and connect failures surface after a
    /// single attempt, mapped by `map_send_error` as before.
    async fn send_with_timeout_retry(
        &self,
        url: &str,
        ua: &str,
        ua_index: usize,
    ) -> HttpResult<wreq::Response> {
        let mut attempt = 0;
        loop {
            let request = self.build_request(url, ua, ua_index);
            let request_started = std::time::Instant::now();
            let url_host = url_host_of(url);
            match request.send().await {
                Ok(response) => return Ok(response),
                Err(e) => {
                    // #2841: classified through the ONE seam with this attempt's timing;
                    // non-timeouts stay terminal (F-09); the timeout that exhausts
                    // the budget keeps its classified cause (S3).
                    let timeout_err = e.is_timeout();
                    let mapped =
                        http_failure(e, FetchPhase::Connect, url_host.as_deref(), request_started);
                    if !timeout_err || attempt >= self.config.max_retries {
                        return Err(mapped);
                    }
                    attempt += 1;
                    let delay = compute_backoff_delay(
                        attempt,
                        None,
                        self.config.backoff_base_ms,
                        self.config.backoff_max_ms,
                    );
                    debug!(
                        attempt,
                        delay_ms = delay,
                        max_retries = self.config.max_retries,
                        "request timeout — retrying"
                    );
                    tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
                },
            }
        }
    }

    /// Select the user-agent to use for a given rotation index.
    fn select_user_agent(&self, ua_index: usize) -> String {
        self.user_agents
            .get(ua_index % self.user_agents.len())
            .cloned()
            .unwrap_or_else(|| {
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36".into()
            })
    }

    /// Build the GET request for `url`, applying the configured headers.
    ///
    /// Uses minimal headers for WAF-bypass attempts (`ua_index >= 4`).
    fn build_request(&self, url: &str, ua: &str, ua_index: usize) -> wreq::RequestBuilder {
        let mut request = self
            .client
            .get(url)
            .header("Accept-Language", &self.config.accept_language)
            .header("Accept", &self.config.accept)
            .header("User-Agent", ua);

        if ua_index < 4 {
            request = request
                .header("Referer", &self.config.referer)
                .header("Cache-Control", &self.config.cache_control);
        }

        for (name, value) in &self.config.custom_headers {
            request = request.header(name, value);
        }

        request
    }

}

// ============================================================================
// HttpClientPort implementation for HttpClient
// ============================================================================

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
impl crate::domain::http_port::HttpClientPort for HttpClient {
    fn get(
        &self,
        url: &str,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = crate::domain::http_error::HttpResult<
                        crate::domain::http_port::HttpResponse,
                    >,
                > + Send
                + '_,
        >,
    > {
        let url = url.to_owned();
        Box::pin(async move {
            // Use the inner wreq client for the raw request, then build
            // a full HttpResponse (status + body + headers).
            let request_started = std::time::Instant::now();
            let resp = self.client.get(url.as_str()).send().await.map_err(|e| {
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

/// Shared arrange helpers for the `HttpClient` wiremock tests (T7 slice-2).
///
/// Every test below repeats the same scaffold — start a mock server, mount a
/// `GET` route, build a client with a retry preset, `get` a URL — so the
/// helpers live here once, next to the `parity_config` convention they extend.
/// Test-only code: only referenced from `#[cfg(test)]` modules.
#[cfg(test)]
#[path = "client/arrange.rs"]
mod arrange;

#[cfg(test)]
#[path = "client/tests.rs"]
#[cfg(not(miri))] // all tests create wreq::Client with btls-sys FFI (unsupported by Miri)
mod tests;

#[cfg(test)]
#[path = "client/wiremock_tests.rs"]
#[cfg(not(miri))] // all tests create wreq::Client with btls-sys FFI (unsupported by Miri)
mod wiremock_tests;

#[cfg(test)]
#[path = "client/waf_detection_tests.rs"]
#[cfg(not(miri))]
mod waf_detection_tests;

#[cfg(test)]
#[path = "client/session_pool_tests.rs"]
#[cfg(not(miri))]
mod session_pool_tests;

#[cfg(test)]
#[cfg(not(miri))]
mod timeout_retry_tests;
