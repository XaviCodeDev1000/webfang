//! The per-page fetch of a crawl task.
//!
//! The fetch, the WAF ban it may trigger and the cookie ingestion it
//! produces: the part of the worker that talks to the network. Extracted from
//! `crawl_task.rs` so the grandfathered file stops growing: a pure move, no
//! behaviour change.

use tracing::warn;
use url::Url;

use crate::application::crawler::checkpoint::BannedDomain;
use crate::application::crawler::crawl_task_ctx::CrawlTaskCtx;
use crate::application::crawler::ports::{waf_challenge_message, FetchOutcome};
use crate::domain::downloader_port::Cookie;
use crate::domain::CorrelationId;
use crate::domain::CrawlError;
use crate::infrastructure::observability::{log_scrape_error, log_scrape_error_with_cause};

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
fn ban_waf_domain(ctx: &CrawlTaskCtx, parsed_url: &Url, waf_msg: &str) {
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
