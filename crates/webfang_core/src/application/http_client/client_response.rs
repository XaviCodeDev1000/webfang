//! HTTP response handling: body reads, WAF inspection and status retries.
//!
//! The three status handlers below are the response half of `HttpClient::get`:
//! they read a capped body, inspect it for a WAF challenge and delegate retry
//! to the shared backoff machinery. They live beside their parent so the
//! hardened request path in `client.rs` stays focused on pacing, session gating
//! and status dispatch.

use std::collections::HashMap;
use std::sync::Arc;

use tracing::{debug, warn};
use wreq::cookie::Jar;
use wreq::header::HeaderMap;

use super::HttpClient;
use crate::application::http_client::retry::{retry_with_backoff, RetryPolicy};
use crate::domain::body_cap;
use crate::domain::http_error::{HttpError, HttpResult};
use crate::domain::waf::{waf_inspector, InspectionContext};

impl HttpClient {
    /// Handle a 2xx response: read the body and run context-aware WAF inspection.
    pub(super) async fn handle_success(
        &self,
        url: &str,
        response: wreq::Response,
        status: u16,
    ) -> HttpResult<String> {
        // Build the inspection context BEFORE consuming the body
        // (headers borrow must end before `.text()`).
        let ctx = inspection_context(status, response.headers(), self.config.ignore_waf);

        let body = body_cap::read_body_capped(response, self.config.max_page_bytes).await?;

        // Context-aware WAF inspection (REQ-WAF-05). A classified block returns
        // immediately — the old fallback ladder (~4 requests / ~5s of UA rotation)
        // is gone: with tiered, evidence-based detection a block is a genuine
        // challenge that UA rotation cannot bypass, and false positives no longer
        // reach this branch.
        if let Some(err) = waf_block_error(url, status, &body, &ctx) {
            return Err(err);
        }

        Ok(body)
    }

    /// Handle a 429 response: honor `Retry-After` and retry with backoff.
    pub(super) async fn handle_rate_limited(
        &self,
        url: &str,
        response: wreq::Response,
        ua: &str,
        jar: Option<&Arc<Jar>>,
    ) -> HttpResult<String> {
        // BUG 6 fix: when Retry-After is absent, fall back to exponential backoff
        // instead of a fixed 1-second delay.
        let retry_after = response
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse().ok());

        debug!("429 Rate Limited, retry after {:?}s", retry_after);

        retry_with_backoff(
            &self.client,
            url,
            &self.config,
            ua,
            jar,
            RetryPolicy {
                // None → compute_backoff_delay falls through to exponential path
                retry_after_secs: retry_after,
                retryable: |code: u16| code == 429 || (500..=599).contains(&code),
                exhausted: HttpError::RateLimited(retry_after.unwrap_or(0)),
                label: "429",
            },
        )
        .await
    }

    /// Handle a 5xx response: inspect for WAF challenges, then retry with backoff.
    pub(super) async fn handle_server_error(
        &self,
        url: &str,
        response: wreq::Response,
        status: u16,
        ua: &str,
        jar: Option<&Arc<Jar>>,
    ) -> HttpResult<String> {
        debug!("{} from {}", status, url);

        // Inspect the initial response BEFORE retrying (REQ-WAF-05).
        // Approved behavior change: a 503 Cloudflare challenge is classified as
        // a WAF block instead of dying as a generic ServerError after exhausting
        // retries.
        //
        // Scope, stated because this comment used to read like global coverage: this
        // is the AI extraction stack — `application::llm_extraction` is its only
        // production consumer, and the crawler never fetched through here. The page
        // fetch path had no non-2xx inspection at all until F-11 added it in
        // `infrastructure::downloader::wreq_downloader`. The two stacks now share the
        // status-aware inspector and the canonical WAF-status predicate in `domain`;
        // they never shared this call site.
        let ctx = inspection_context(status, response.headers(), self.config.ignore_waf);

        // Non-critical enrichment (FIX C): a body-transfer failure (connection
        // reset mid-body) must not bypass the retry loop with a fatal Request
        // error — the base never read the 5xx body. Fall back to an empty body so
        // inspection proceeds on headers only (cf-mitigated detection still
        // works) and the retry loop runs. The 2xx branch keeps its fatal body
        // read: there the body IS the scrape content.
        let body = match body_cap::read_body_capped(response, self.config.max_page_bytes).await {
            Ok(b) => b,
            Err(e) => {
                debug!(url = %url, error = %e, "5xx body read failed; inspecting headers only");
                String::new()
            },
        };

        if let Some(err) = waf_block_error(url, status, &body, &ctx) {
            return Err(err);
        }

        retry_with_backoff(
            &self.client,
            url,
            &self.config,
            ua,
            jar,
            RetryPolicy {
                retry_after_secs: None,
                retryable: |code: u16| (500..=599).contains(&code),
                exhausted: HttpError::ServerError(status),
                label: "5xx",
            },
        )
        .await
    }
}

/// Return a WAF-challenge error when the body is classified as blocked.
fn waf_block_error(
    url: &str,
    status: u16,
    body: &str,
    ctx: &InspectionContext,
) -> Option<HttpError> {
    let verdict = waf_inspector().inspect(body, ctx);
    if verdict.is_blocked {
        warn!(
            url = %url,
            status = %status,
            evidences = verdict.evidences.len(),
            "WAF/CAPTCHA challenge detected; blocking"
        );
        Some(HttpError::WafChallenge(verdict.evidence_chain()))
    } else {
        None
    }
}

/// Build a WAF [`InspectionContext`] from an HTTP response's status and headers.
///
/// Only the `wreq` → lowercased-`HashMap` adapter is local here: the context itself
/// comes from [`InspectionContext::from_lowercase_headers`], the same constructor the
/// crawler's fetch path uses, so the two stacks cannot drift on how status,
/// content-type (REQ-WAF-02) and control-header evidence (REQ-WAF-03) are packaged.
/// `ignore_waf` short-circuits inspection to a clean verdict (REQ-WAF-07). The header
/// borrow ends when this returns (the map is cloned), so the caller can still consume
/// the response body.
fn inspection_context(status: u16, headers: &HeaderMap, ignore_waf: bool) -> InspectionContext {
    let mut map = HashMap::new();
    for (name, value) in headers {
        if let Ok(v) = value.to_str() {
            map.insert(name.as_str().to_lowercase(), v.to_string());
        }
    }
    InspectionContext::from_lowercase_headers(status, &map, ignore_waf)
}
