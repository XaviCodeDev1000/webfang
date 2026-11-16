//! Name-resolution retry budget (#2823).
//!
//! A resolver that never answers is not the site refusing us: the request never
//! reached a peer, so waiting longer cannot change the outcome. This module owns
//! the SEPARATE budget such a failure gets, so the site-rejection ladder
//! (`max_retries` with exponential backoff) is never charged for it.
//!
//! Extracted from `wreq_downloader.rs` to keep that grandfathered file from
//! growing: a pure move, no behaviour change.

use std::time::Duration;

use tracing::warn;
use url::Url;

use super::WreqDownloader;
use crate::domain::downloader_port::DownloadError;

/// Extra attempts a NAME RESOLUTION failure gets, on top of the first one.
///
/// It does NOT consume `max_retries`: a broken resolver is OUR problem, not the
/// site's, and the site-rejection ladder must stay intact for the URLs that
/// actually reach a peer (#2823).
pub(crate) const DNS_RETRY_BUDGET: u32 = 2;

/// The fixed delay between resolution attempts, in milliseconds.
///
/// Deliberately short and CONSTANT: waiting longer does not make a resolver
/// answer, and an exponential ladder here is what turned a broken DNS into a
/// WAF-looking slowdown of a whole 50-URL run (#2823).
pub(crate) const DNS_RETRY_DELAY_MS: u64 = 250;

impl WreqDownloader {
    /// Send one request, retrying a NAME RESOLUTION failure on its OWN budget.
    ///
    /// Every OTHER failure is returned at once: the caller's ladder decides
    /// whether it earns a retry.
    pub(super) async fn send_request_on_dns_budget(
        &self,
        url: &Url,
    ) -> Result<wreq::Response, DownloadError> {
        let mut dns_attempts = 0u32;
        loop {
            match self.send_request(url, None).await {
                Ok(response) => return Ok(response),
                Err(err) => {
                    if !crate::domain::error::mapping::is_resolution_failure(&err) {
                        return Err(err);
                    }
                    if dns_attempts >= DNS_RETRY_BUDGET {
                        return Err(err);
                    }
                    dns_attempts += 1;
                    warn!(
                        url = %url,
                        attempt = dns_attempts,
                        max_dns_retries = DNS_RETRY_BUDGET,
                        delay_ms = DNS_RETRY_DELAY_MS,
                        error = %err,
                        "name resolution failed — retrying on the resolver budget"
                    );
                    tokio::time::sleep(Duration::from_millis(DNS_RETRY_DELAY_MS)).await;
                },
            }
        }
    }
}

#[cfg(test)]
impl WreqDownloader {
    /// Test-only constructor: an injected client with an explicit retry policy.
    ///
    /// The production constructors derive the policy from the operator's knobs
    /// (seconds and milliseconds that are right for a real run). A test that
    /// drives a resolver failure needs the policy pinned to milliseconds so the
    /// retry ACCOUNTING is observable without a real network or a real clock
    /// (#2823).
    pub(crate) fn from_client_with_retry(
        client: wreq::Client,
        timeout_secs: u64,
        max_retries: u32,
        backoff_base_ms: u64,
        backoff_max_ms: u64,
    ) -> Self {
        Self {
            client: std::sync::Arc::new(client),
            timeout_secs,
            pinned_ua: None,
            max_page_bytes: crate::domain::downloader_factory::DEFAULT_MAX_PAGE_BYTES,
            max_retries,
            backoff_base_ms,
            backoff_max_ms,
            ignore_waf: false,
        }
    }
}
