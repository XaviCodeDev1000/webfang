//! Memory-probe tests.
//! Split desde `adapters/downloader/mod.rs` bajo la regla ~500 (#2584, Ola 1).

use super::*;
use crate::infrastructure::observability::memory_probe;
use std::sync::Arc;

#[test]
#[cfg_attr(miri, ignore = "btls-sys FFI (wreq Client) unsupported by Miri")]
fn probe_downloaded_urls_growth_50k_assets() {
    const N: usize = 50_000;
    let downloader = Downloader::new(DownloadConfig::default()).expect("downloader builds");
    let before = memory_probe::rss_bytes();

    for i in 0..N {
        let cell = tokio::sync::OnceCell::new();
        cell.set(DownloadedAsset {
            url: format!("https://probe.example.com/assets/img-{i}.png"),
            local_path: format!("/tmp/probe-out/img-{i}.png").into(),
            mime_type: Some("image/png".to_string()),
            size: 12_345,
            content_hash: format!("{i:012x}"),
        })
        .expect("fresh cell accepts first set");
        downloader.downloaded_urls.insert(
            format!("https://probe.example.com/assets/img-{i}.png"),
            Arc::new(cell),
        );
    }

    let after = memory_probe::rss_bytes();
    assert_eq!(downloader.downloaded_urls.len(), N);
    memory_probe::append_report(
        "BEFORE — downloaded_urls dedup cache",
        &format!(
            "entries={} rss_before={} rss_after={} delta={}",
            downloader.downloaded_urls.len(),
            memory_probe::fmt_rss(before),
            memory_probe::fmt_rss(after),
            memory_probe::fmt_rss(after.and_then(|a| before.map(|b| a.saturating_sub(b)))),
        ),
    );
}
