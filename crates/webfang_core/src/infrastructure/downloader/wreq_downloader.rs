//! Wreq-based downloader implementation.
//!
//! Wraps a shared `wreq::Client` behind `Arc` for connection pooling.
//! Extracts cookies from responses and returns [`FetchedPage`] with HTML + cookies.
//!
//! Following **own-arc-shared**: Uses `Arc<Client>` for thread-safe shared ownership
//! of the connection pool. The client is created once and shared across all requests.

use std::sync::Arc;
use std::time::Duration;

use futures::future::BoxFuture;
use tracing::{debug, instrument, warn};
use url::Url;
use wreq::cookie::Jar;
use wreq::header::{HeaderMap, HeaderName, HeaderValue};
use wreq::Client;
use wreq_util::Profile;

use self::cookies::{extract_cookies, lowercase_headers, parse_retry_after_ms};
use self::wreq_body::BoundedRead;
use super::cause::FetchPhase;
use super::{Cookie, DownloadError, Downloader, FetchedPage};
use crate::domain::waf::{is_t2_blocking_status, waf_inspector, InspectionContext};
use crate::error::ErrorClass;
use crate::infrastructure::user_agent::UserAgentCache;

mod cookies;
pub(crate) mod dns_budget;
mod retry;
mod wreq_body;

/// Estimated memory cost of a wreq client instance in bytes.
///
/// This accounts for the connection pool, TLS session cache, and internal buffers.
/// Value is approximate — real usage varies by pool size and active connections.
const WREQ_MEMORY_COST: usize = 1_024 * 1_024; // ~1 MB

/// Downloader implementation backed by `wreq` with connection pooling.
///
/// The internal `wreq::Client` is shared via `Arc` — all requests reuse the same
/// connection pool, avoiding the per-request client creation anti-pattern.
///
/// # Examples
///
/// ```ignore
/// use webfang_core::infrastructure::downloader::wreq_downloader::WreqDownloader;
/// use webfang_core::infrastructure::downloader::Downloader;
///
/// let downloader = WreqDownloader::new(30, 10, wreq_util::Profile::Chrome145, None, Vec::new(), None, None, 3, 1000, 10000, 50_000_000).unwrap();
/// let page = downloader.fetch(&"https://example.com".parse().unwrap()).await.unwrap();
/// assert_eq!(page.status, 200);
/// ```
pub struct WreqDownloader {
    client: Arc<Client>,
    timeout_secs: u64,
    /// User-Agent pinned by the operator (`--user-agent`, #503).
    ///
    /// Applied once at client-build time so every request — first fetch and
    /// all retries — carries it. When set, the 403 pool-rotation retry is
    /// disabled: the operator asked to be identified exactly as configured.
    pinned_ua: Option<String>,
    max_retries: u32,
    /// Base delay for the exponential backoff applied to retriable failures.
    backoff_base_ms: u64,
    backoff_max_ms: u64,
    /// Decompressed-body cap for page fetches (FIX-1, #1231 F-12). The read
    /// aborts mid-body once the streamed byte count exceeds this value, so
    /// memory stays bounded even against a decompression bomb with a tiny
    /// declared Content-Length.
    max_page_bytes: u64,
    /// Bypass WAF/CAPTCHA classification entirely (`--ignore-waf`, REQ-WAF-07).
    /// Set through [`WreqDownloader::with_ignore_waf`]; `false` enforces detection.
    ignore_waf: bool,
}

impl WreqDownloader {
    /// Create a new WreqDownloader with the given TLS/HTTP2 emulation profile.
    ///
    /// The client is built once and shared via `Arc` for connection pooling.
    /// Pass [`Profile::Chrome145`] for the historical default fingerprint.
    ///
    /// # Arguments
    ///
    /// * `timeout_secs` - Request timeout in seconds
    /// * `connect_timeout_secs` - Connection timeout in seconds
    /// * `tls_emulation` - TLS/HTTP2 fingerprint profile applied to the client
    /// * `user_agent` - Optional pinned User-Agent (#503). Applied at client
    ///   build time via the builder's `user_agent` API, AFTER the emulation
    ///   profile, so it wins over the profile-default UA on the wire. When
    ///   set, the 403 pool-rotation retry is disabled.
    /// * `custom_headers` - Operator headers (`--header`, #890). Applied at
    ///   client build time via `default_headers`, AFTER the emulation
    ///   profile and any pinned UA, so they replace same-named profile
    ///   defaults (case-insensitive per HTTP semantics).
    /// * `accept_language` - Optional Accept-Language value (`--accept-language`,
    ///   #890). Applied AFTER the emulation profile so it wins over the
    ///   profile-default language.
    /// * `initial_cookie_jar` - Pre-seeded wreq cookie store (`--cookie`,
    ///   #890). When provided it replaces the default empty jar, so Static
    ///   and Hybrid-L1 requests carry the operator's cookies from the first
    ///   fetch. The Chromiumoxide L3 path keeps using [`crate::domain::cookie_bridge::CookieBridge`].
    ///
    /// # Errors
    ///
    /// Returns [`DownloadError::Internal`] if the wreq client cannot be built
    /// or if a custom header name/value is not valid for HTTP.
    // 10 params: the wreq layer's full dependency set (profile, UA, operator
    // headers/cookies, retry backoff) — same pattern as build_fetch_router.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        timeout_secs: u64,
        connect_timeout_secs: u64,
        tls_emulation: Profile,
        user_agent: Option<String>,
        custom_headers: Vec<(String, String)>,
        accept_language: Option<String>,
        initial_cookie_jar: Option<Arc<Jar>>,
        max_retries: u32,
        backoff_base_ms: u64,
        backoff_max_ms: u64,
        max_page_bytes: u64,
    ) -> Result<Self, DownloadError> {
        // Canonical detector seam (Q2): same "auto" as every other subsystem.
        let pool_size = std::cmp::max(
            6,
            crate::domain::budget::detector::system_parallelism().get() - 1,
        );

        // `.emulation(profile)` installs the profile-default headers (including
        // a browser UA) via `default_headers`; a pinned UA must be set AFTER
        // it so `HeaderMap::insert` replaces the profile value (#503).
        let builder = Client::builder().emulation(tls_emulation);
        let builder = match user_agent.as_deref() {
            Some(ua) => builder.user_agent(ua),
            None => builder,
        };
        // #890: operator values are applied as client default headers AFTER
        // the emulation profile. `default_headers` replaces per header name
        // (case-insensitive — HTTP/2 names are lowercase-canonical), so user
        // supplied values win over profile defaults on the wire.
        let mut extra_headers = HeaderMap::new();
        if let Some(lang) = accept_language.as_deref() {
            let value = HeaderValue::from_str(lang).map_err(|e| {
                DownloadError::Internal(format!("invalid --accept-language value {lang:?}: {e}"))
            })?;
            extra_headers.insert(wreq::header::ACCEPT_LANGUAGE, value);
        }
        for (name, value) in &custom_headers {
            let name = HeaderName::from_bytes(name.as_bytes()).map_err(|e| {
                DownloadError::Internal(format!("invalid --header name {name:?}: {e}"))
            })?;
            let value = HeaderValue::from_str(value).map_err(|e| {
                DownloadError::Internal(format!("invalid --header value for {name}: {e}"))
            })?;
            extra_headers.insert(name, value);
        }
        let builder = if extra_headers.is_empty() {
            builder
        } else {
            // #890 observability: WHICH overrides are active (names +
            // count only — never values; cookies and fingerprint headers
            // are credentials/sensitive surface).
            debug!(
                header_names = %extra_headers.keys().map(|k| k.as_str()).collect::<Vec<_>>().join(","),
                custom_header_count = custom_headers.len(),
                accept_language_override = accept_language.is_some(),
                "operator header overrides applied to scrape client"
            );
            builder.default_headers(extra_headers)
        };
        // #890: a pre-seeded jar (`--cookie`) replaces the default empty one
        // so Static/Hybrid-L1 requests carry the operator cookies from the
        // first fetch. `cookie_store(true)` must NOT run afterwards — it
        // would swap in a fresh empty jar.
        let builder = match initial_cookie_jar {
            Some(jar) => {
                // #890 observability: attachment event only — cookie
                // values AND counts live where the jar is seeded
                // (cli/scrape_flow.rs); cookies are credentials.
                debug!("pre-seeded operator cookie jar attached to scrape client");
                builder.cookie_provider(jar)
            },
            None => builder.cookie_store(true),
        };
        let builder = builder
            .timeout(Duration::from_secs(timeout_secs))
            .connect_timeout(Duration::from_secs(connect_timeout_secs))
            .pool_max_idle_per_host(pool_size)
            .pool_idle_timeout(Duration::from_secs(60))
            .gzip(true)
            .brotli(true);
        // SSRF guard (#703) applied through the domain `SsrfGuard` port:
        // default 10-hop limit + stops redirects that target a literal
        // forbidden IP (belt-and-suspenders). Hostname targets — including
        // every redirect hop — are enforced at connect time by the
        // validating DNS resolver the guard installs.
        let client = crate::domain::ssrf_guard::ssrf_guard()
            .secure_client(builder)
            .build()
            // LCOV_EXCL_LINE defensive: wreq-client-build — client construction fails only on invalid TLS profile, an invariant
            .map_err(|e| DownloadError::Internal(format!("failed to build wreq client: {e}")))?;

        debug!(
            pool_size = pool_size,
            timeout_secs = timeout_secs,
            connect_timeout_secs = connect_timeout_secs,
            ua = user_agent.as_deref().unwrap_or("emulation-default"),
            max_retries = max_retries,
            backoff_base_ms = backoff_base_ms,
            backoff_max_ms = backoff_max_ms,
            "WreqDownloader created"
        );

        Ok(Self {
            client: Arc::new(client),
            timeout_secs,
            pinned_ua: user_agent,
            max_retries,
            backoff_base_ms,
            backoff_max_ms,
            max_page_bytes,
            // WAF enforcement is ON by default; `--ignore-waf` opts out through
            // [`WreqDownloader::with_ignore_waf`] at construction time.
            ignore_waf: false,
        })
    }

    /// Opt out of WAF/CAPTCHA classification (`--ignore-waf`, REQ-WAF-07).
    ///
    /// A builder rather than a twelfth `new` parameter on purpose: a positional `bool`
    /// would have rewritten ~20 construction sites (most of them tests) to change one
    /// default, and the flag is genuinely optional behaviour, which a method name states
    /// better than an argument does.
    ///
    /// On the non-2xx path this does NOT rescue the fetch — a challenge body is never
    /// scraped as content. It changes only the classification of the failure: a plain
    /// HTTP error instead of a WAF block, same exit code (F-11).
    #[must_use]
    pub fn with_ignore_waf(mut self, ignore_waf: bool) -> Self {
        self.ignore_waf = ignore_waf;
        self
    }

    /// Create a WreqDownloader from an existing `wreq::Client`.
    ///
    /// Useful when you need custom client configuration beyond the defaults.
    pub fn from_client(client: Client, timeout_secs: u64, _connect_timeout_secs: u64) -> Self {
        Self {
            client: Arc::new(client),
            timeout_secs,
            pinned_ua: None,
            max_page_bytes: crate::domain::downloader_factory::DEFAULT_MAX_PAGE_BYTES,
            max_retries: 3,
            backoff_base_ms: 1000,
            backoff_max_ms: 10000,
            ignore_waf: false,
        }
    }
}

impl WreqDownloader {
    /// Describe the effective User-Agent of a request for tracing (#503):
    /// the request-level override (403 rotation), else the pinned value,
    /// else the emulation-profile default.
    fn effective_ua<'a>(&'a self, request_ua: Option<&'a str>) -> &'a str {
        request_ua
            .or(self.pinned_ua.as_deref())
            .unwrap_or("emulation-default")
    }

    #[instrument(skip(self), fields(url = %url, ua = %self.effective_ua(user_agent)))]
    async fn send_request(
        &self,
        url: &Url,
        user_agent: Option<&str>,
    ) -> Result<wreq::Response, DownloadError> {
        let builder = self.client.get(url.as_str());
        let builder = match user_agent {
            Some(ua) => builder.header("User-Agent", ua),
            None => builder,
        };
        let request_started = std::time::Instant::now();
        builder.send().await.map_err(|e| {
            crate::domain::error::mapping::download_failure(
                e,
                self.timeout_secs,
                FetchPhase::Connect,
                url.host_str(),
                request_started,
            )
        })
    }

    /// Consume a successful response into a [`FetchedPage`].
    async fn build_page(
        &self,
        response: wreq::Response,
        url: &Url,
    ) -> Result<FetchedPage, DownloadError> {
        let status = response.status().as_u16();

        // Extract cookies before consuming the response body
        let cookies = extract_cookies(url, &response);

        // Extract the final URL after redirects
        let final_url = Url::parse(&response.uri().to_string())
            .map_err(|e| DownloadError::InvalidUrl(e.to_string()))?;

        // Capture response headers (lowercased keys) before consuming the body.
        let headers = response
            .headers()
            .iter()
            .map(|(name, value)| {
                (
                    name.as_str().to_ascii_lowercase(),
                    value.to_str().unwrap_or_default().to_string(),
                )
            })
            .collect();

        let html = self.read_body_capped(response).await?;

        debug!(
            "Fetched {} ({} bytes, {} cookies)",
            final_url,
            html.len(),
            cookies.len()
        );

        Ok(FetchedPage {
            url: final_url,
            html,
            status,
            headers,
            cookies,
        })
    }

    /// Shared bounded body reader — the one place that streams, caps, decodes and
    /// consumes a `wreq::Response`. Both the page path and the WAF sniff path route
    /// through it so a fix to one cannot silently miss the other (F-11).
    ///
    /// `on_overflow` selects the only behavioural difference between them.
    async fn read_body_bounded(
        &self,
        response: wreq::Response,
        limit: u64,
        on_overflow: BoundedRead,
    ) -> Result<String, DownloadError> {
        use futures::StreamExt;

        // Charset from Content-Type BEFORE the body is consumed (headers stay
        // readable until the body stream is taken).
        let content_type = response
            .headers()
            .get(wreq::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| {
                value.split(';').find_map(|part| {
                    let part = part.trim();
                    part.strip_prefix("charset=")
                        .map(|c| c.trim_matches('"').trim().to_ascii_lowercase())
                })
            })
            .unwrap_or_else(|| "utf-8".to_string());

        let limit_len = usize::try_from(limit).unwrap_or(usize::MAX);
        let mut stream = response.bytes_stream();
        let mut buf = bytes::BytesMut::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(DownloadError::from)?;
            if buf.len().saturating_add(chunk.len()) > limit_len {
                match on_overflow {
                    BoundedRead::Abort => {
                        // The outer fetch span already carries the URL; the event only
                        // needs the machine-readable cap facts.
                        warn!(
                            limit = limit,
                            "response body exceeded the page size cap; aborting read"
                        );
                        return Err(DownloadError::BodyTooLarge { limit });
                    },
                    BoundedRead::Truncate => {
                        // Keep the prefix that fits and stop pulling the stream: the rest
                        // of the body is never read, so an oversized error page costs at
                        // most the sniff budget.
                        buf.extend_from_slice(&chunk[..limit_len.saturating_sub(buf.len())]);
                        break;
                    },
                }
            }
            buf.extend_from_slice(&chunk);
        }

        let (text, _, _) = encoding_rs::Encoding::for_label(content_type.as_bytes())
            .unwrap_or(encoding_rs::UTF_8)
            .decode(&buf);
        Ok(text.into_owned())
    }

    #[instrument(
        skip(self),
        fields(
            url = %url,
            // D5: stable identity of the shared pooled `Client` (Arc inner ptr).
            // Constant across fetches => observable proof of connection-pool reuse
            // (no silent re-handshake per request). See MAPA item 7.
            client_id = %format!("{:p}", Arc::as_ptr(&self.client))
        )
    )]
    // The attempt loop carries the ladder, the 403 rotation and the body read
    // in the order they must run. #2823 added the resolver branch to an
    // already ~90-line flow; splitting it would hide that order.
    #[allow(clippy::too_many_lines)]
    async fn fetch_inner(&self, url: &Url) -> Result<FetchedPage, DownloadError> {
        debug!("Fetching URL: {}", url);

        let mut last_status: u16 = 0;
        let mut last_error: Option<DownloadError> = None;

        for attempt in 0..=self.max_retries {
            let response = match self.send_request_on_dns_budget(url).await {
                Ok(res) => res,
                Err(dl_err) => {
                    // Single classification rule (FIX-1, #1236): retry
                    // everything the domain classifier deems transient, surface
                    // PermanentFatal immediately. Request timeouts are
                    // TransientBackoff (F-08, #1231): the most common transient
                    // failure in crawling (slow-not-dead peer), and the
                    // configured timeout still caps EACH attempt, so the retry
                    // budget of `max_retries` x `timeout_secs` is exactly what
                    // the operator asked for. Builder-class invalid requests
                    // (unsupported scheme, malformed URL) map to InvalidUrl ->
                    // PermanentFatal and fail after a single attempt (F-09,
                    // #1236); mid-body transients (Io::ConnectionReset /
                    // UnexpectedEof) stay retriable (#649 mid-body transient
                    // fix).
                    if matches!(dl_err.classify(), ErrorClass::PermanentFatal) {
                        return Err(dl_err);
                    }
                    // #2823: a resolution failure that reaches here has already
                    // spent its OWN budget. It is our resolver, never the site
                    // refusing us, so it is returned instead of consuming the
                    // site-rejection ladder — that is the whole point of the
                    // separate budget.
                    if crate::domain::error::mapping::is_resolution_failure(&dl_err) {
                        return Err(dl_err);
                    }
                    // The "retrying" event only fires when a retry will
                    // actually happen: on the last attempt the loop ends and
                    // the stored error surfaces without a wasted backoff.
                    let will_retry = attempt < self.max_retries;
                    if will_retry {
                        warn!(
                            url = %url,
                            attempt = attempt,
                            max_retries = self.max_retries,
                            error = %dl_err,
                            "transport failure fetching URL — retrying"
                        );
                    }
                    last_error = Some(dl_err);
                    if will_retry {
                        self.sleep_before_retry(attempt, self.backoff_delay_ms(attempt))
                            .await;
                    }
                    continue;
                },
            };

            last_status = response.status().as_u16();
            last_error = None;

            if response.status().is_success() {
                return self.build_page(response, url).await;
            }

            // ── WAF inspection on non-2xx responses (F-11) ─────────────────────────
            //
            // The tiered inspector used to see only successful responses, so the ordinary
            // Cloudflare shape — 403/503 carrying `cf-mitigated: challenge` — surfaced as a
            // generic HTTP error and `--ignore-waf` looked inert on it. The engine is
            // already status-aware (T1 Challenge blocks at any status, T2 Fingerprint needs
            // a WAF status — see `domain::waf`), so this was purely a missing call site,
            // not a detection gap.
            //
            // Placement matters: it runs BEFORE the rotated-UA request and before the
            // 429/5xx backoff, so a confirmed challenge costs zero retries. A WAF challenge
            // does not clear by rotating a User-Agent, and #1236's amplification lesson says
            // a request that cannot succeed should not be repeated.
            //
            // Retry-After is captured BEFORE anything reads the body: `read_body_snippet`
            // consumes the response, and the 429 branch below still needs the server's
            // requested delay or #1231's backoff silently degrades.
            let mut retry_after_ms =
                (last_status == 429).then(|| parse_retry_after_ms(&response, self.backoff_max_ms));

            if !self.ignore_waf {
                let headers = lowercase_headers(response.headers());
                let ctx = InspectionContext::from_lowercase_headers(last_status, &headers, false);

                // Phase 1 — headers only, no read at all. Control headers such as
                // `cf-mitigated` are T2 evidence and block on their own even with an empty
                // body (RES-01), which is exactly how a mitigating edge answers.
                let verdict = waf_inspector().inspect("", &ctx);
                let verdict = if verdict.is_blocked || !is_t2_blocking_status(ctx.status) {
                    verdict
                } else {
                    // Phase 2 — T1 marker prose, but only on a status a WAF actually
                    // answers with. T2 never blocks outside that set, so a plain 404 would
                    // pay a body read for a verdict that cannot change; a 404 serving
                    // challenge prose stays unclassified by design, and is reported as the
                    // HTTP error it is.
                    let snippet = self.read_body_snippet(response).await?;
                    waf_inspector().inspect(&snippet, &ctx)
                };

                if verdict.is_blocked {
                    let chain = verdict.evidence_chain();
                    warn!(
                        url = %url,
                        status = last_status,
                        evidences = verdict.evidences.len(),
                        "WAF/CAPTCHA challenge detected on a non-2xx response"
                    );
                    return Err(DownloadError::WafChallenge(chain));
                }
            }

            // 403: one implicit retry with rotated User-Agent (mirrors HttpClient).
            // Pinned UA (#503): the operator asked to be identified exactly as
            // configured, so pool rotation is skipped and the 403 falls through
            // to normal terminal-error handling.
            //
            // IMPORTANT: The rotated-UA retry MUST capture its response and status
            // so the unified retry loop can correctly handle 429/5xx that the
            // rotated attempt may return. The old helper `retry_with_rotated_ua`
            // discarded the non-2xx response, causing 403→429→200 sequences to
            // fail because the 429 was silently consumed.
            if last_status == 403 && attempt == 0 && self.pinned_ua.is_none() {
                let agents = UserAgentCache::fallback_agents();
                let rotated_ua = agents.get(1).map(String::as_str);
                warn!(url = %url, status = last_status, "403 Forbidden — retrying with rotated User-Agent");
                match self.send_request(url, rotated_ua).await {
                    Ok(res) if res.status().is_success() => {
                        return self.build_page(res, url).await;
                    },
                    Ok(res) => {
                        // Rotated retry returned non-2xx (e.g., a second
                        // 403, 429, 500). The rotation CONSUMES the attempt:
                        // fall through to the unified 429/5xx-or-terminal
                        // handling below instead of `continue`-ing back to
                        // the loop top. A bare `continue` re-enters the loop
                        // and fires another default-UA request for the same
                        // failure (#1430: always-403 cost 3 requests —
                        // default, rotated, default — instead of 2,
                        // amplification against WAFs), and skips the backoff
                        // sleep a rotated 429/5xx owes. The `attempt == 0`
                        // guard above stays the single "no rotation spent
                        // yet" gate — no parallel flag.
                        last_status = res.status().as_u16();
                        retry_after_ms = (last_status == 429)
                            .then(|| parse_retry_after_ms(&res, self.backoff_max_ms));
                    },
                    Err(e) => return Err(e),
                }
            }

            // Unified retry for 429 (rate limit) and 5xx (server error, #649).
            if last_status == 429 || (500..=599).contains(&last_status) {
                let delay_ms = if last_status == 429 {
                    // BUG 6 fix: use max(Retry-After, exponential_backoff) instead of fixed constant.
                    // parse_retry_after_ms returns the server's requested delay (capped at backoff_max_ms).
                    // backoff_delay_ms returns exponential delay (also capped).
                    // max() ensures we never retry faster than the server asked,
                    // but also never slower than our own exponential strategy.
                    std::cmp::max(retry_after_ms.unwrap_or(0), self.backoff_delay_ms(attempt))
                } else {
                    self.backoff_delay_ms(attempt) // 5xx already correct
                };
                warn!(
                    url = %url,
                    attempt = attempt,
                    max_retries = self.max_retries,
                    status = last_status,
                    delay_ms = delay_ms,
                    "retrying after HTTP status"
                );
                self.sleep_before_retry(attempt, delay_ms).await;
                continue;
            }

            // Terminal error (4xx and anything else non-retriable).
            return Err(DownloadError::Http {
                status: last_status,
                message: format!("HTTP {last_status}"),
            });
        }

        // Retries exhausted — surface the LAST observed status, not a hardcoded
        // one (#649 Bug 5): a run that started at 429 and ended at 500 must
        // report 500.
        Err(last_error.unwrap_or(DownloadError::Http {
            status: last_status,
            message: format!("retries exhausted at status {last_status}"),
        }))
    }
}

impl Downloader for WreqDownloader {
    fn fetch<'a>(&'a self, url: &'a Url) -> BoxFuture<'a, Result<FetchedPage, DownloadError>> {
        Box::pin(self.fetch_inner(url))
    }

    fn supports_interactions(&self) -> bool {
        false
    }

    fn memory_cost(&self) -> usize {
        WREQ_MEMORY_COST
    }
}

#[cfg(test)]
mod test_support;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod wiremock_fetch_tests;
#[cfg(test)]
mod wiremock_retry_tests;
