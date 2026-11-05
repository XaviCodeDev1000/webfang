//! Tests for the `ResourceGovernor` admission and RAM-threshold logic
//! (RAII permits, #1610 and #509 groups, observability harness).
//!
//! Extracted from `resource_governor.rs` to bring it under the ~500-line
//! file-size rule. Movement is verbatim; the module path
//! `resource_governor::tests` is preserved via the sibling declaration and
//! both gates (`cfg(test)` + `cfg(not(miri))`) travel on it.
/// Task 2.7: the governor's live budget must equal the budget model's
/// pure derivation for the same injected RAM values (single source of
/// truth), including the legacy floor on tiny machines.
#[test]
fn compute_max_instances_matches_budget_model_derivation() {
    use crate::domain::budget::{detector::FixedDetector, BudgetModel, BudgetOverrides};
    for total_gib in [1_u64, 2, 4, 8, 16, 32, 64] {
        let total = total_gib * 1024 * 1024 * 1024;
        let detector = FixedDetector::with_detection(
            std::num::NonZeroUsize::new(8).expect("8 is non-zero"),
            Some(total),
        );
        let model = BudgetModel::build(BudgetOverrides::default(), &detector);
        let model_max = model
            .max_chrome_instances()
            .map(crate::domain::budget::tiers::MaxChromeInstances::get)
            .unwrap_or(1);
        // Legacy inline formula (pre-delegation reference):
        let legacy = (((total as f64 * 0.6) as u64) / 200_000_000).max(1) as usize;
        assert_eq!(
            model_max, legacy,
            "model must reproduce legacy at {total_gib}GiB"
        );
        assert_eq!(
            ResourceGovernor::with_max_instances(model_max, CancellationToken::new())
                .available_permits(),
            model_max,
            "governor accepts the model-derived ceiling at {total_gib}GiB"
        );
    }
}

use super::*;

use std::time::Duration;

// The sysinfo-backed tests below read /proc via `sysconf`, which Miri
// cannot execute — gate them there. The semaphore/cancellation tests use
// the explicit-budget seam (`with_max_instances`) and DO run under Miri.
#[test]
#[cfg_attr(miri, ignore = "sysinfo sysconf (RAM probe) unsupported by Miri")]
fn test_governor_creation() {
    let gov = ResourceGovernor::new();
    // On any real machine we should have at least 1 permit
    assert!(gov.available_permits() >= 1);
}

#[test]
#[cfg_attr(miri, ignore = "sysinfo sysconf (RAM probe) unsupported by Miri")]
fn test_check_resources_returns_ok() {
    let gov = ResourceGovernor::new();
    // On CI or dev machines RAM usage is typically well below 80%
    let result = gov.check_resources();
    assert!(result.is_ok());
}

#[test]
#[cfg_attr(miri, ignore = "sysinfo sysconf (RAM probe) unsupported by Miri")]
fn test_total_ram_nonzero() {
    let bytes = ResourceGovernor::total_ram_bytes();
    assert!(bytes > 0, "total RAM should be > 0 on any running system");
}

#[test]
#[cfg_attr(miri, ignore = "sysinfo sysconf (RAM probe) unsupported by Miri")]
fn test_usage_percent_range() {
    let pct = ResourceGovernor::ram_usage_percent();
    assert!(pct <= 100);
}

#[test]
fn test_resource_error_display() {
    let err = ResourceError::RamTooHigh(92);
    assert!(err.to_string().contains("92%"));
}

#[test]
fn test_resource_error_into_download_error() {
    let res_err = ResourceError::Insufficient("test".into());
    let dl_err: DownloadError = res_err.into();
    assert!(matches!(dl_err, DownloadError::ResourceExhausted(_)));
}

// ========================================================================
// RAII permit tests (#509) — prove no semaphore permit leaks
// ========================================================================

#[tokio::test]
async fn acquire_releases_permit_on_drop() {
    let gov = ResourceGovernor::with_max_instances(2, CancellationToken::new());
    assert_eq!(gov.available_permits(), 2);

    let permit = gov.acquire().await.expect("first acquire must succeed");
    assert_eq!(gov.available_permits(), 1);

    drop(permit);
    assert_eq!(
        gov.available_permits(),
        2,
        "dropping the permit must return it to the pool"
    );
}

#[tokio::test]
async fn acquire_grants_permit_when_token_idle() {
    let gov = ResourceGovernor::with_max_instances(1, CancellationToken::new());

    let permit = tokio::time::timeout(Duration::from_secs(1), gov.acquire())
        .await
        .expect("acquire must not hang when permits are available");
    assert!(permit.is_ok());
}

// ========================================================================
// Admission-wait observability (#1610, OBS-M2)
// ========================================================================

/// `tracing` caches the per-callsite `Interest` process-wide: whichever
/// test reaches the `semaphore acquire` callsite first WITHOUT a subscriber
/// freezes it at `never` and the assertions below would pass vacuously. A
/// global sink subscriber installed once keeps every callsite interested;
/// the per-test `FileTraceLayer` is what collects.
static GLOBAL_SUBSCRIBER: std::sync::Once = std::sync::Once::new();

/// Run `body` under a real `FileTraceLayer` and return the records whose
/// `message` equals `message`.
///
/// The production layer is the assertion surface — what this proves is
/// that the event survives the same path `--trace-file` uses. The body
/// drives its own runtime so every event it emits stays inside the
/// `with_default` scope.
fn capture<F, Fut>(message: &'static str, body: F) -> Vec<serde_json::Value>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    use crate::infrastructure::observability::FileTraceLayer;
    use tracing_subscriber::layer::SubscriberExt;

    GLOBAL_SUBSCRIBER.call_once(|| {
        let _ = tracing::subscriber::set_global_default(
            tracing_subscriber::fmt()
                .with_writer(std::io::sink)
                .finish(),
        );
    });

    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("trace.jsonl");
    let layer = FileTraceLayer::new(path.clone()).expect("trace layer");
    let dispatch = tracing::Dispatch::new(tracing_subscriber::registry().with(layer));
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    tracing::dispatcher::with_default(&dispatch, || runtime.block_on(body()));

    std::fs::read_to_string(&path)
        .expect("trace file")
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str::<serde_json::Value>(l).expect("jsonl line"))
        .filter(|v| v["message"] == message)
        .collect()
}

/// A CONTENDED acquire emits the wait it paid: `available_permits = 0`
/// (the occupancy it queued behind) and a measurable cost. This is what
/// turns "the render queue serialised us" from an inference drawn from
/// slowness into a queryable fact.
#[test]
fn contended_acquire_emits_measured_wait() {
    let events = capture("semaphore acquire", || async {
        let gov = std::sync::Arc::new(ResourceGovernor::with_max_instances(
            1,
            CancellationToken::new(),
        ));
        let held = gov.acquire().await.expect("first permit");

        // A second waiter cannot proceed until `held` is released, so the
        // wait is real rather than a scheduling accident.
        let waiter = {
            let gov = std::sync::Arc::clone(&gov);
            tokio::spawn(async move { gov.acquire().await.map(drop).is_ok() })
        };
        tokio::time::sleep(Duration::from_millis(60)).await;
        drop(held);
        assert!(
            waiter.await.expect("waiter task"),
            "the queued waiter must be granted once the permit frees"
        );
    });

    assert_eq!(events.len(), 2, "both acquires emit");
    let queued = events
        .iter()
        .find(|e| e["fields"]["available_permits"] == serde_json::json!(0))
        .expect("the queued acquire must report zero free permits");
    let fields = queued["fields"].as_object().expect("fields");
    assert_eq!(
        fields["operation"].as_str(),
        Some("browser.resource_governor.acquire")
    );
    assert_eq!(fields["outcome"].as_str(), Some("granted"));
    assert_eq!(fields["total_permits"].as_u64(), Some(1));
    assert!(
        fields["waited_ms"].as_u64().is_some_and(|ms| ms >= 40),
        "a queued acquire must report a measurable wait, got {:?}",
        fields["waited_ms"]
    );
}

/// #1610: a wait released by shutdown reports `outcome = "cancelled"`, not
/// a generic failure — an admission skip and an admission denial are
/// different operational facts.
#[test]
fn cancelled_acquire_emits_cancelled_outcome() {
    let events = capture("semaphore acquire", || async {
        let cancel = CancellationToken::new();
        let gov = ResourceGovernor::with_max_instances(1, cancel.clone());
        let held = gov.acquire().await.expect("single permit");
        cancel.cancel();
        let result = gov.acquire().await;
        assert!(matches!(result, Err(DownloadError::Cancelled)));
        drop(held);
    });

    assert_eq!(events.len(), 2);
    assert_eq!(events[1]["fields"]["outcome"].as_str(), Some("cancelled"));
    assert_eq!(
        events[1]["fields"]["available_permits"].as_u64(),
        Some(0),
        "the cancelled waiter queued behind a held permit"
    );
}

// ========================================================================
// Cancellation tests (#509)
// ========================================================================

#[tokio::test]
async fn acquire_returns_cancelled_when_blocked_and_token_fires() {
    let cancel = CancellationToken::new();
    let gov = std::sync::Arc::new(ResourceGovernor::with_max_instances(1, cancel.clone()));

    // Take the single permit so the next acquire must block.
    let held = gov.acquire().await.expect("single permit must be granted");
    assert_eq!(gov.available_permits(), 0);

    let waiter = {
        let gov = std::sync::Arc::clone(&gov);
        tokio::spawn(async move { gov.acquire().await })
    };
    tokio::time::sleep(Duration::from_millis(50)).await;
    cancel.cancel();

    let result = tokio::time::timeout(Duration::from_secs(1), waiter)
        .await
        .expect("cancelled acquire must return within the bound");
    assert!(matches!(result, Ok(Err(DownloadError::Cancelled))));

    // The cancelled wait must not have consumed a permit.
    assert_eq!(gov.available_permits(), 0);

    // RAII: releasing the held permit makes it visible again.
    drop(held);
    assert_eq!(gov.available_permits(), 1);
}
