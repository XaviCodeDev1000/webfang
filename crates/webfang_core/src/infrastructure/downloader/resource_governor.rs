//! RAM-aware resource governor for heavyweight downloaders.
//!
//! Uses `sysinfo` to read available system memory and a [`tokio::sync::Semaphore`]
//! to gate concurrent Chrome/headless-browser instances. The formula:
//!
//! ```text
//! max_instances = (available_ram_bytes * 0.6) / CHROME_INSTANCE_COST
//! ```
//!
//! Thresholds:
//! - **80 %** RAM used → warning log, max_instances halved
//! - **90 %** RAM used → all new Chrome permits denied
//!
//! # Admission waits are observable (#1610, OBS-P1-2 / OBS-M2)
//!
//! [`ResourceGovernor::acquire`] is the only admission gate a heavyweight
//! downloader passes through, and it used to be silent: a render queue that
//! serialised behind the permit budget looked exactly like a slow browser.
//! Every acquire now emits one `semaphore_acquire` event carrying the measured
//! wait, the occupancy it waited behind, and how the wait ended.
//!
//! The held permit is an [`tokio::sync::OwnedSemaphorePermit`], not a
//! `MutexGuard`/`RwLockReadGuard`: it is the documented semaphore pattern and
//! it is NOT the "never hold a lock across `.await`" rule that governs mutexes.
//!
//! # Sanitizer coverage
//!
//! - **Miri: inapplicable** — `sysinfo` calls `sysconf(SC_PHYS_PAGES)`, which
//!   Miri cannot execute (unsupported operation). Tests are gated with
//!   `#[cfg(not(miri))]` and document the reason.
//! - **TSan: covered** — the semaphore-based concurrent gating is exercised
//!   under ThreadSanitizer in CI (`sanitizers.yml`), where sysconf works.

use std::sync::Arc;

use sysinfo::System;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

use super::DownloadError;

/// RAM usage percentage that triggers a warning and reduces capacity by half.
///
/// Derived from the budget model's [`RamThresholds`](crate::domain::budget::derivation::RamThresholds) so the governor and the
/// pure derivation share ONE threshold source of truth (#897 item 4). Infra
/// → domain is the allowed dependency direction.
const WARNING_THRESHOLD: u8 =
    crate::domain::budget::derivation::RamThresholds::DEFAULT_WARNING_PERCENT;

/// RAM usage percentage that denies all new Chrome instances; see
/// [`WARNING_THRESHOLD`] for the single-source-of-truth rationale.
const CRITICAL_THRESHOLD: u8 =
    crate::domain::budget::derivation::RamThresholds::DEFAULT_CRITICAL_PERCENT;

/// `operation` field of every event this governor emits (#1610).
const RESOURCE_GOVERNOR_OPERATION: &str = "browser.resource_governor.acquire";

/// Gates concurrent heavyweight downloader instances based on system RAM.
///
/// Holds a [`CancellationToken`] (#509) so a caller blocked in
/// [`acquire`](Self::acquire) can be woken on engine shutdown instead of
/// hanging on the semaphore forever. [`new`](Self::new) uses a fresh token
/// that never fires (pre-#509 behavior); the crawl engine injects its own
/// token via [`with_cancel_token`](Self::with_cancel_token).
pub struct ResourceGovernor {
    semaphore: Arc<Semaphore>,
    cancel: CancellationToken,
    /// Configured permit budget, kept so the emitted event can report the
    /// occupancy a waiter observed (`available` / `total`) instead of only the
    /// free count, which alone cannot distinguish "full" from "empty" (#1610).
    max_permits: usize,
}

impl ResourceGovernor {
    /// Create a governor calibrated to current system RAM.
    ///
    /// The embedded cancellation token never fires; use
    /// [`with_cancel_token`](Self::with_cancel_token) to wire shutdown.
    pub fn new() -> Self {
        Self::with_cancel_token(CancellationToken::new())
    }

    /// Create a governor calibrated to current system RAM whose
    /// [`acquire`](Self::acquire) aborts when `cancel` fires (#509).
    pub fn with_cancel_token(cancel: CancellationToken) -> Self {
        Self::with_max_instances(Self::compute_max_instances(), cancel)
    }

    /// Create a governor with an explicit permit budget.
    ///
    /// Calibration seam behind [`new`](Self::new) /
    /// [`with_cancel_token`](Self::with_cancel_token): lets constrained
    /// environments (containers) and tests set the budget directly instead
    /// of deriving it from system RAM.
    pub fn with_max_instances(max_permits: usize, cancel: CancellationToken) -> Self {
        let max_permits = max_permits.max(1);
        debug!("ResourceGovernor: max_permits={max_permits}");

        Self {
            semaphore: Arc::new(Semaphore::new(max_permits)),
            cancel,
            max_permits,
        }
    }

    /// Check whether resources are available and return a permit count, or an
    /// error if the system is under too much memory pressure.
    ///
    /// The returned `usize` represents how many permits *could* be acquired
    /// right now (0 when denied).
    pub fn check_resources(&self) -> Result<usize, ResourceError> {
        let usage = Self::ram_usage_percent();

        if usage >= CRITICAL_THRESHOLD {
            warn!("RAM usage {usage}% >= {CRITICAL_THRESHOLD}%: new Chrome instances denied");
            return Err(ResourceError::RamTooHigh(usage));
        }

        let available = self.semaphore.available_permits();

        if usage >= WARNING_THRESHOLD {
            let reduced = available / 2;
            warn!(
                "RAM usage {usage}% >= {WARNING_THRESHOLD}%: available permits reduced {available} -> {reduced}"
            );
            return Ok(reduced);
        }

        Ok(available)
    }

    /// Acquire an owned semaphore permit, returning an error when resources
    /// are exhausted.
    ///
    /// Returns a [`tokio::sync::OwnedSemaphorePermit`] (`'static`) so callers can hold
    /// it across async boundaries without tying it to the governor's lifetime.
    ///
    /// The wait aborts with [`DownloadError::Cancelled`] if the governor's
    /// cancellation token fires while blocked (#509). Dropping the pending
    /// acquire consumes no permit, so a cancelled wait cannot leak one.
    ///
    /// # Errors
    ///
    /// Returns [`DownloadError::Cancelled`] on cancellation and
    /// [`DownloadError::Internal`] if the semaphore was closed.
    pub async fn acquire(&self) -> Result<tokio::sync::OwnedSemaphorePermit, DownloadError> {
        let arc = Arc::clone(&self.semaphore);
        // Occupancy BEFORE the wait: what the waiter had to queue behind.
        let available_before = arc.available_permits();
        let started = std::time::Instant::now();

        let outcome = tokio::select! {
            result = arc.acquire_owned() => {
                // LCOV_EXCL_LINE defensive: semaphore-closed — acquire_owned fails only when the governor is shut down
                result.map_err(|_| {
                    DownloadError::Internal("resource governor semaphore closed".to_string())
                })
            },
            () = self.cancel.cancelled() => Err(DownloadError::Cancelled),
        };

        // #1610 (OBS-M2): the wait is the admission decision, so it emits
        // whether or not it blocked — a zero-cost acquire is what makes a
        // NON-saturated governor provable. DEBUG keeps the console quiet;
        // --trace-file always captures it.
        debug!(
            operation = RESOURCE_GOVERNOR_OPERATION,
            waited_ms = started.elapsed().as_millis() as u64,
            available_permits = available_before,
            total_permits = self.max_permits,
            outcome = match &outcome {
                Ok(_) => "granted",
                Err(DownloadError::Cancelled) => "cancelled",
                Err(_) => "closed",
            },
            "semaphore acquire"
        );

        outcome
    }

    /// Current number of available permits.
    pub fn available_permits(&self) -> usize {
        self.semaphore.available_permits()
    }

    /// Compute `max_instances` from current system RAM.
    ///
    /// Delegates to the budget model's pure derivation (task 2.7): one source
    /// of truth for the RAM formula. At zero usage pressure the decision can
    /// never be `Deny`, so the legacy floor (`max(1)`) is preserved via the
    /// derivation's own minimum.
    fn compute_max_instances() -> usize {
        let total = Self::total_ram_bytes();
        match crate::domain::budget::derivation::derive_max_instances(
            total,
            0,
            crate::domain::budget::derivation::RamThresholds::default(),
        ) {
            crate::domain::budget::derivation::MaxChromeDecision::Allow(instances) => {
                instances.get()
            },
            // Unreachable at zero pressure; keep the legacy floor defensively.
            crate::domain::budget::derivation::MaxChromeDecision::Deny => 1,
        }
    }

    /// Total system RAM in bytes.
    fn total_ram_bytes() -> u64 {
        let mut sys = System::new();
        sys.refresh_memory();
        sys.total_memory()
    }

    /// RAM usage as a percentage (0–100).
    pub fn ram_usage_percent() -> u8 {
        let mut sys = System::new();
        sys.refresh_memory();
        let total = sys.total_memory();
        if total == 0 {
            return 0;
        }
        let used = sys.used_memory();
        ((used * 100) / total) as u8
    }
}

impl Default for ResourceGovernor {
    fn default() -> Self {
        Self::new()
    }
}

/// Errors raised when system resources cannot support a new instance.
#[derive(Debug, thiserror::Error)]
pub enum ResourceError {
    /// RAM usage exceeded the critical threshold.
    #[error("RAM usage too high ({0}%): new Chrome instances denied")]
    RamTooHigh(u8),

    /// Generic resource exhaustion.
    #[error("insufficient resources: {0}")]
    Insufficient(String),
}

impl From<ResourceError> for DownloadError {
    fn from(err: ResourceError) -> Self {
        DownloadError::ResourceExhausted(err.to_string())
    }
}

#[cfg(test)]
#[cfg(not(miri))] // sysinfo uses sysconf (unsupported by Miri — but TSan covers the semaphore)
mod tests;
