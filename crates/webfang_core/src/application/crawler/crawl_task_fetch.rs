//! Fetch/session helpers of the per-page crawl task.
//!
//! Moved verbatim from `crawl_task.rs` (mission #2827, task T6): per-domain
//! session acquisition, the fetch path with its WAF-challenge domain ban,
//! the cookie bridge, session-success reporting, and content capture.
//!
//! Security-sensitive: `fetch_page` + `ban_waf_domain` implement the
//! WAF-challenge to domain-ban half of the fetch guard chain, and
//! `ingest_cookies` is the cookie bridge (the write guard is dropped at the
//! end of its block, never held across an `.await`).

use tracing::{debug, warn};
use url::Url;

use super::checkpoint::BannedDomain;
use super::crawl_task_ctx::CrawlTaskCtx;
use super::ports::{waf_challenge_message, FetchOutcome};
use crate::domain::downloader_port::Cookie;
use crate::domain::session_port::SessionId;
use crate::domain::{CorrelationId, CrawlError};
use crate::infrastructure::observability::{log_scrape_error, log_scrape_error_with_cause};

/// Acquire a per-domain session from the pool, if one is configured.
///
/// Returns `Ok(Some(id))` on acquisition, `Ok(None)` when no pool is enabled,
/// and `Err(())` when the pool exists but has no available session (the caller
/// should skip the URL).
pub(super) fn acquire_session(ctx: &CrawlTaskCtx, url_str: &str) -> Result<Option<SessionId>, ()> {
    let Some(ref pool) = ctx.session_pool else {
        return Ok(None);
    };
    let domain = url::Url::parse(url_str)
        .ok()
        .and_then(|u| u.host_str().map(String::from))
        .unwrap_or_default();
    match pool.acquire(&domain) {
        Some(id) => Ok(Some(id)),
        None => {
            debug!("Domain {} has no available sessions, skipping", domain);
            Err(())
        },
    }
}

/// Fetch the page, handling WAF challenges (banning the domain) and logging
/// fetch failures before propagating the error.
pub(super) async fn fetch_page(
    ctx: &CrawlTaskCtx,
    parsed_url: &Url,
    url_str: &str,
    page_correlation: &CorrelationId,
) -> Result<FetchOutcome, CrawlError> {
    match ctx.fetcher.fetch_page(parsed_url, &ctx.config).await {
        Ok(outcome) => Ok(outcome),
        Err(e) => {
            if let Some(waf_msg) = waf_challenge_message(&e) {
                ban_waf_domain(ctx, parsed_url, &waf_msg);
                log_scrape_error(
                    &waf_msg,
                    url_str,
                    "fetch",
                    Some(page_correlation),
                    "WAF challenge detected",
                );
            } else {
                log_scrape_error_with_cause(
                    &e,
                    url_str,
                    "fetch",
                    Some(page_correlation),
                    "page fetch failed",
                );
            }
            Err(e)
        },
    }
}

/// Ban a domain that triggered a WAF challenge, unless it is already banned.
pub(super) fn ban_waf_domain(ctx: &CrawlTaskCtx, parsed_url: &Url, waf_msg: &str) {
    let Some(domain) = parsed_url.host_str() else {
        return;
    };
    let banned = BannedDomain {
        domain: domain.to_string(),
        banned_until: None,
        reason: waf_msg.to_string(),
    };
    if let Ok(mut domains) = ctx.banned_domains.write() {
        if !domains.iter().any(|d| d.domain == domain) {
            domains.push(banned);
            warn!(domain = %domain, reason = %waf_msg, "domain banned due to WAF challenge");
        }
    }
}

/// Ingest any cookies returned by the fetch into the shared cookie bridge.
///
/// #1119: the bridge is a `tokio::sync::RwLock`, so the write lock is
/// awaited — a contended lock yields the worker instead of parking an
/// executor thread — and the guard is dropped at the end of the block,
/// never held across an `.await`.
pub(super) async fn ingest_cookies(ctx: &CrawlTaskCtx, fetched_cookies: &[Cookie]) {
    if fetched_cookies.is_empty() {
        return;
    }
    let mut bridge = ctx.cookie_bridge.write().await;
    for cookie in fetched_cookies {
        bridge.add(cookie.clone());
    }
}

/// Report a successful fetch back to the session pool for the acquired session.
pub(super) fn report_session_success(
    ctx: &CrawlTaskCtx,
    url_str: &str,
    session_id: Option<SessionId>,
) {
    let Some(ref pool) = ctx.session_pool else {
        return;
    };
    let Some(id) = session_id else {
        return;
    };
    if let Ok(parsed) = url::Url::parse(url_str) {
        if let Some(domain) = parsed.host_str() {
            pool.report_success(domain, id);
        }
    }
}

/// Hand the fetched body to the content sink when one is configured (#631).
///
/// Capture happens right after a successful fetch and before pipeline gating,
/// so a `Skip`/`Reject` outcome cannot silently drop the page from the export
/// set. The sink is synchronous and must not block.
pub(super) fn capture_content(ctx: &CrawlTaskCtx, url_str: &str, response: &str) {
    let Some(ref sink) = ctx.content_sink else {
        return;
    };
    sink.capture(url_str, response);
}
