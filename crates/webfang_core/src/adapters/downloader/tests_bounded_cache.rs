//! Bounded-cache tests.
//! Split desde `adapters/downloader/mod.rs` bajo la regla ~500 (#2584, Ola 1).

use super::*;
use crate::infrastructure::observability::memory_probe;
use std::sync::Arc;

fn sample_asset(i: usize) -> DownloadedAsset {
    DownloadedAsset {
        url: format!("https://probe.example.com/assets/img-{i}.png"),
        local_path: format!("/tmp/probe-out/img-{i}.png").into(),
        mime_type: Some("image/png".to_string()),
        size: 12_345,
        content_hash: format!("{i:012x}"),
    }
}

/// Task 5.3 — RED→GREEN plateau proof: under a workload of N >> cap,
/// the bounded cache's entry count plateaus at the cap while the
/// unbounded baseline keeps growing linearly.
#[test]
#[cfg_attr(miri, ignore = "btls-sys FFI (wreq Client) unsupported by Miri")]
fn bounded_asset_cache_plateaus_at_capacity() {
    const CAP: usize = 10_000;
    const WORKLOAD: usize = 50_000;

    let bounded = Downloader::with_asset_cache_capacity(DownloadConfig::default(), CAP)
        .expect("downloader builds");
    // S17 made `new` bounded too, so the linear-growth baseline is now the
    // explicit unbounded opt-in rather than the default constructor.
    let unbounded = Downloader::with_asset_cache_capacity(DownloadConfig::default(), usize::MAX)
        .expect("downloader builds");

    for i in 0..WORKLOAD {
        let key = format!("https://probe.example.com/assets/img-{i}.png");
        let cell = tokio::sync::OnceCell::new();
        cell.set(sample_asset(i)).expect("fresh cell");
        bounded.insert_cached_cell(key.clone(), Arc::new(cell));
        let cell = tokio::sync::OnceCell::new();
        cell.set(sample_asset(i)).expect("fresh cell");
        unbounded.downloaded_urls.insert(key, Arc::new(cell));
    }

    assert!(
        bounded.downloaded_urls.len() <= CAP,
        "bounded cache must plateau at {CAP}, got {}",
        bounded.downloaded_urls.len()
    );
    assert_eq!(unbounded.downloaded_urls.len(), WORKLOAD);
}

/// S17: the default constructor must be finite. `usize::MAX` is now an
/// explicit opt-in, never what a caller gets by default.
#[test]
#[cfg_attr(miri, ignore = "btls TLS_method FFI via Downloader::new")]
fn default_downloader_cache_capacity_is_finite() {
    let downloader = Downloader::new(DownloadConfig::default()).expect("downloader builds");
    assert_ne!(
        downloader.asset_cache_capacity(),
        usize::MAX,
        "Downloader::new must not take the unbounded legacy path"
    );
    assert_eq!(
        downloader.asset_cache_capacity(),
        crate::adapters::downloader::cache::DEFAULT_ASSET_CACHE_CAPACITY
    );
}

/// Task 5.5 — byte-identity: a workload completing entirely WITHIN the
/// cap must produce dedup decisions and returned assets identical to the
/// unbounded baseline (the cap only changes behavior for entries that
/// would otherwise have been evicted anyway).
#[test]
#[cfg_attr(miri, ignore = "btls-sys FFI (wreq Client) unsupported by Miri")]
fn bounded_cache_within_cap_is_byte_identical_to_unbounded() {
    const CAP: usize = 100;
    const WORKLOAD: usize = 60; // < CAP: no eviction ever fires

    let bounded = Downloader::with_asset_cache_capacity(DownloadConfig::default(), CAP)
        .expect("downloader builds");
    let unbounded = Downloader::new(DownloadConfig::default()).expect("downloader builds");

    for i in 0..WORKLOAD {
        let key = format!("https://probe.example.com/assets/img-{i}.png");
        let asset = sample_asset(i);
        // Bounded path through the disciplined insert.
        let cell = tokio::sync::OnceCell::new();
        cell.set(asset.clone()).expect("fresh cell");
        bounded.insert_cached_cell(key.clone(), std::sync::Arc::new(cell));
        // Unbounded baseline.
        let cell = tokio::sync::OnceCell::new();
        cell.set(asset).expect("fresh cell");
        unbounded
            .downloaded_urls
            .insert(key, std::sync::Arc::new(cell));
    }

    assert_eq!(
        bounded.downloaded_urls.len(),
        unbounded.downloaded_urls.len()
    );
    for i in 0..WORKLOAD {
        let key = format!("https://probe.example.com/assets/img-{i}.png");
        let b = bounded.downloaded_urls.get(&key).expect("bounded entry");
        let u = unbounded
            .downloaded_urls
            .get(&key)
            .expect("unbounded entry");
        let b_asset = b.value().get().expect("initialized").clone();
        let u_asset = u.value().get().expect("initialized").clone();
        assert_eq!(b_asset.url, u_asset.url);
        assert_eq!(b_asset.local_path, u_asset.local_path);
        assert_eq!(b_asset.content_hash, u_asset.content_hash);
        assert_eq!(b_asset.mime_type, u_asset.mime_type);
        assert_eq!(b_asset.size, u_asset.size);
    }
}

/// Direct state builder for eviction-classifier tests: raw map insert +
/// ledger registration, NO automatic eviction (that is exactly what we
/// want to drive manually per scenario).
fn seed_cell(dl: &Downloader, key: &str, initialized: Option<&DownloadedAsset>, in_flight: bool) {
    let cell = match initialized {
        Some(asset) => {
            let c = tokio::sync::OnceCell::new();
            c.set(asset.clone()).expect("fresh cell");
            std::sync::Arc::new(c)
        },
        None => std::sync::Arc::new(tokio::sync::OnceCell::new()),
    };
    dl.downloaded_urls.insert(key.to_string(), cell);
    if let Ok(mut order) = dl.asset_cache_order.lock() {
        order.push_new(key.to_string());
    }
    if in_flight {
        dl.in_flight_downloads.insert(key.to_string(), ());
    }
}

/// Review HIGH regression: permanent-failure zombies (uninitialized cells
/// with NO active download) must be evictable so error-heavy long runs
/// stay bounded, while genuinely in-flight cells are preserved.
#[test]
#[cfg_attr(miri, ignore = "btls-sys FFI (wreq Client) unsupported by Miri")]
fn abandoned_failure_zombies_evicted_inflight_preserved() {
    const CAP: usize = 8;
    let dl = Downloader::with_asset_cache_capacity(DownloadConfig::default(), CAP)
        .expect("downloader builds");

    // Oldest first: 6 zombies (failed downloads never retried), then 3
    // initialized successes, then 2 actively-downloading cells.
    for i in 0..6 {
        seed_cell(
            &dl,
            &format!("https://probe.example.com/zombie-{i}.png"),
            None,
            false,
        );
    }
    for i in 0..3 {
        seed_cell(
            &dl,
            &format!("https://probe.example.com/success-{i}.png"),
            Some(&sample_asset(i)),
            false,
        );
    }
    for i in 0..2 {
        seed_cell(
            &dl,
            &format!("https://probe.example.com/inflight-{i}.png"),
            None,
            true,
        );
    }

    assert_eq!(dl.downloaded_urls.len(), 11);
    dl.evict_over_capacity(); // 11 - 8 = 3 excess

    // FIFO scans the 3 oldest zombies and evicts exactly those.
    assert_eq!(dl.downloaded_urls.len(), CAP, "cache bounded at cap");
    for i in 0..3 {
        let gone = format!("https://probe.example.com/zombie-{i}.png");
        assert!(!dl.downloaded_urls.contains_key(&gone), "{gone} evicted");
    }
    for i in 0..3 {
        let kept = format!("https://probe.example.com/success-{i}.png");
        assert!(dl.downloaded_urls.contains_key(&kept), "{kept} retained");
    }
    for i in 0..2 {
        let kept = format!("https://probe.example.com/inflight-{i}.png");
        assert!(
            dl.downloaded_urls.contains_key(&kept),
            "in-flight {kept} NEVER evicted"
        );
    }
}

/// A second eviction pass with an all-in-flight excess must terminate and
/// preserve every in-flight cell (rotation bound, no infinite loop).
#[test]
#[cfg_attr(miri, ignore = "btls-sys FFI (wreq Client) unsupported by Miri")]
fn eviction_terminates_when_excess_is_all_inflight() {
    const CAP: usize = 2;
    let dl = Downloader::with_asset_cache_capacity(DownloadConfig::default(), CAP)
        .expect("downloader builds");
    for i in 0..5 {
        seed_cell(
            &dl,
            &format!("https://probe.example.com/busy-{i}.png"),
            None,
            true,
        );
    }
    dl.evict_over_capacity();
    assert_eq!(
        dl.downloaded_urls.len(),
        5,
        "nothing evictable while in flight"
    );
    for i in 0..5 {
        let kept = format!("https://probe.example.com/busy-{i}.png");
        assert!(dl.downloaded_urls.contains_key(&kept));
    }
    // Once the downloads complete successfully, the cells are normal
    // FIFO victims again.
    for i in 0..5 {
        let key = format!("https://probe.example.com/busy-{i}.png");
        dl.in_flight_downloads.remove(&key);
        if let Some(entry) = dl.downloaded_urls.get(&key) {
            entry.value().set(sample_asset(i)).expect("init");
        }
    }
    dl.evict_over_capacity();
    assert_eq!(dl.downloaded_urls.len(), CAP, "bounded after completion");
}

/// Legacy unbounded mode skips the insertion ledger entirely: no per-URL
/// strings, no O(n) membership work (review MEDIUM fix).
#[test]
#[cfg_attr(miri, ignore = "btls-sys FFI (wreq Client) unsupported by Miri")]
fn legacy_unbounded_skips_insertion_ledger() {
    let dl = Downloader::new(DownloadConfig::default()).expect("downloader builds");
    for i in 0..200 {
        let cell = tokio::sync::OnceCell::new();
        cell.set(sample_asset(i)).expect("fresh cell");
        dl.downloaded_urls.insert(
            format!("https://probe.example.com/img-{i}.png"),
            std::sync::Arc::new(cell),
        );
    }
    assert_eq!(dl.downloaded_urls.len(), 200, "all entries cached");
    assert_eq!(
        dl.asset_cache_order.lock().expect("ledger").len(),
        0,
        "ledger untouched in legacy mode"
    );
}

/// Task 5.4 — AFTER numbers: same 50k-entry fill as the BEFORE probe,
/// now under the production-derived cap; RSS delta must be bounded by
/// roughly cap * per-entry cost instead of linear in the workload.
#[test]
#[cfg_attr(miri, ignore = "btls-sys FFI (wreq Client) unsupported by Miri")]
fn memory_probe_downloaded_urls_after_cap() {
    const CAP: usize = 24_576; // asset tier 3 × 8_192 (production derivation)
    let downloader = Downloader::with_asset_cache_capacity(
        DownloadConfig::default(),
        crate::adapters::downloader::ASSET_CACHE_ENTRIES_PER_PERMIT
            * crate::domain::budget::tiers::DownloadConcurrency::new(3)
                .expect("3 > 0")
                .get(),
    )
    .expect("downloader builds");
    let before = memory_probe::rss_bytes();

    for i in 0..50_000 {
        let cell = tokio::sync::OnceCell::new();
        cell.set(sample_asset(i)).expect("fresh cell");
        downloader.insert_cached_cell(
            format!("https://probe.example.com/assets/img-{i}.png"),
            Arc::new(cell),
        );
    }

    let after = memory_probe::rss_bytes();
    assert!(downloader.downloaded_urls.len() <= CAP);
    memory_probe::append_report(
        "AFTER - downloaded_urls dedup cache (bounded)",
        &format!(
            "entries={} (cap={CAP}) rss_before={} rss_after={} delta={} | BEFORE was 28.8 MiB at 50k entries",
            downloader.downloaded_urls.len(),
            memory_probe::fmt_rss(before),
            memory_probe::fmt_rss(after),
            memory_probe::fmt_rss(after.and_then(|a| before.map(|b| a.saturating_sub(b)))),
        ),
    );
}
