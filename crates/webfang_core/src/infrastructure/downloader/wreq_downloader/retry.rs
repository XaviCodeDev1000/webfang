//! Retry-policy arithmetic for the attempt ladder.
//!
//! Extracted from `wreq_downloader.rs` to keep that grandfathered file from
//! growing: a pure move, no behaviour change.

use std::time::Duration;

use super::WreqDownloader;

impl WreqDownloader {
    /// Calculate the exponential backoff delay for a given attempt, capped at
    /// `backoff_max_ms`.
    pub(super) fn backoff_delay_ms(&self, attempt: u32) -> u64 {
        self.backoff_base_ms
            .saturating_mul(2_u64.saturating_pow(attempt))
            .min(self.backoff_max_ms)
    }

    /// Sleep before the next attempt — skipped on the final one, where the
    /// loop is about to exit and the delay would only add dead latency.
    pub(crate) async fn sleep_before_retry(&self, attempt: u32, delay_ms: u64) {
        if attempt < self.max_retries {
            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
        }
    }
}
