//! Crawl-task session-pool bookkeeping.
//!
//! Acquiring a session before the fetch and reporting success back after it.
//!
//! Extracted from `crawl_task.rs` to keep that grandfathered file from growing:
//! a pure move, no behaviour change.

use tracing::debug;

use crate::application::crawler::crawl_task_ctx::CrawlTaskCtx;
use crate::domain::session_port::SessionId;

/// Acquire a session from the pool for `url_str`'s domain.
///
/// Returns `Ok(None)` when no pool is configured, and `Err(())` when the pool
/// exists but has no available session (the caller should skip the URL).
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
