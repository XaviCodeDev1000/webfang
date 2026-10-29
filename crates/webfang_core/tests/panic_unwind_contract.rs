#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

//! Unwind-contract regression tests (#1219).
//!
//! The `catch_unwind` guards in `CpuBridge::dispatch`,
//! `ElasticIngestion::ingest_batch`, and `mcp_server::panic_hook` only contain
//! panics under unwinding. With `panic = "abort"` in a build profile, a single
//! controlled panic aborts the whole test process instead of surfacing as
//! `Err` / `BatchResult::panics` — so these tests FAIL (by aborting) under an
//! abort profile and PASS under unwind.
//!
//! Scope note: these run under the dev profile (unwind) as a proxy. A
//! release-profile run would be the direct proof, but release builds are out
//! of budget here; the profile fix itself (no `panic = "abort"` in
//! `[profile.release]` / `[profile.core]`) is verified by inspection.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use futures::future::BoxFuture;
use tokio::sync::oneshot;

use webfang_core::application::elastic_ingestion::ElasticIngestion;
use webfang_core::domain::config::AutotuningConfig;
use webfang_core::domain::cpu_executor::{CpuExecutorPort, ProcessedChunk};
use webfang_core::domain::crawler_port::ResourceDownloadPort;
use webfang_core::domain::repository::VectorRepository;
use webfang_core::domain::text_chunker::ByteSpan;
use webfang_core::error::ScraperError;
use webfang_core::infrastructure::bridge::CpuBridge;
use webfang_core::infrastructure::content_processing::AggressiveProcessor;
use webfang_core::infrastructure::cpu_pool::RayonCpuPool;

// ---------------------------------------------------------------------------
// Bridge path: controlled panic through CpuBridge::dispatch is contained.
// ---------------------------------------------------------------------------

/// Panic contract (#1219): panics are caught, process survives.
#[tokio::test]
async fn bridge_dispatch_contains_controlled_panic_and_pool_survives() {
    let pool = RayonCpuPool::new(2).expect("pool builds");
    let bridge = CpuBridge::new(pool, Arc::new(AggressiveProcessor));

    let outcome = bridge
        .dispatch(|| panic!("controlled unwind probe"))
        .await
        .expect("oneshot must deliver the captured panic, not close");
    assert!(
        outcome.is_err(),
        "panic must surface as Err, not Ok or abort"
    );
    assert!(
        outcome.unwrap_err().to_string().contains("panic"),
        "error must mention panic"
    );

    // The Rayon pool MUST survive — a second dispatch works.
    let second = bridge
        .dispatch(|| 7)
        .await
        .expect("second oneshot must not be closed")
        .expect("second work returns Ok");
    assert_eq!(second, 7);
}

// ---------------------------------------------------------------------------
// Elastic path: panic inside ingest_batch is counted, process survives.
// ---------------------------------------------------------------------------

/// Downloader that panics on every fetch — the controlled fault injected
/// ahead of the CPU/persist layers.
struct PanickingDownloader;

impl ResourceDownloadPort for PanickingDownloader {
    fn download<'a>(&'a self, _url: &'a str) -> BoxFuture<'a, Result<Vec<u8>, ScraperError>> {
        Box::pin(async move { panic!("controlled elastic panic probe") })
    }
}

/// Downloader that serves a tiny HTML page (lets a follow-up batch succeed).
struct GoodDownloader;

impl ResourceDownloadPort for GoodDownloader {
    fn download<'a>(&'a self, _url: &'a str) -> BoxFuture<'a, Result<Vec<u8>, ScraperError>> {
        Box::pin(async move { Ok(b"<main><p>probe content</p></main>".to_vec()) })
    }
}

/// Minimal bridge stub: one text chunk, no embedding (ONNX lives elsewhere).
struct StubBridge;

impl CpuExecutorPort for StubBridge {
    fn dispatch(
        &self,
        work: Box<dyn FnOnce() -> String + Send + 'static>,
    ) -> oneshot::Receiver<Result<String, ScraperError>> {
        let (tx, rx) = oneshot::channel();
        let _ = tx.send(Ok(work()));
        rx
    }

    fn dispatch_resource(
        &self,
        _url: String,
        content: String,
        _size: u64,
    ) -> oneshot::Receiver<Result<Vec<ProcessedChunk>, ScraperError>> {
        let (tx, rx) = oneshot::channel();
        let _ = tx.send(Ok(vec![ProcessedChunk {
            content,
            embedding: None,
            spans: None,
        }]));
        rx
    }
}

/// Trivial repo stub: empty store, every write succeeds.
#[derive(Default)]
struct StubRepo;

impl VectorRepository for StubRepo {
    fn save_resource<'a>(
        &'a self,
        url: &'a str,
        _title: &'a str,
        _content_hash: &'a str,
        _size_bytes: u64,
    ) -> Pin<Box<dyn Future<Output = Result<String, ScraperError>> + Send + 'a>> {
        Box::pin(async move { Ok(url.to_string()) })
    }

    fn save_chunk<'a>(
        &'a self,
        _id: &'a str,
        _resource_url: &'a str,
        _chunk_index: i64,
        _content: &'a str,
        _embedding: Option<&'a [f32]>,
        _spans: Option<&'a [ByteSpan]>,
    ) -> Pin<Box<dyn Future<Output = Result<(), ScraperError>> + Send + 'a>> {
        Box::pin(async move { Ok(()) })
    }

    fn resource_exists_by_hash<'a>(
        &'a self,
        _content_hash: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Option<String>, ScraperError>> + Send + 'a>> {
        Box::pin(async move { Ok(None) })
    }

    fn get_vector<'a>(
        &'a self,
        _chunk_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Option<Vec<f32>>, ScraperError>> + Send + 'a>> {
        Box::pin(async move { Ok(None) })
    }
}

fn test_config() -> AutotuningConfig {
    AutotuningConfig {
        cpu_cores: 2,
        ram_budget_bytes: 1 << 20,
    }
}

/// Panic contract (#1219): panics are caught, process survives.
#[tokio::test]
async fn elastic_ingest_batch_counts_panic_and_process_survives() {
    let bad = ElasticIngestion::new(
        Arc::new(PanickingDownloader),
        Arc::new(StubBridge),
        StubRepo,
        test_config(),
    );
    let urls = vec!["https://example.com/probe".to_string()];
    let batch = bad
        .ingest_batch(&urls)
        .await
        .expect("batch must return Ok even when a future panics");
    assert_eq!(batch.success, 0);
    assert!(batch.errors.is_empty());
    assert_eq!(
        batch.panics, 1,
        "the controlled panic must be counted, not fatal"
    );

    // Process survives: a healthy pipeline still completes afterwards.
    let good = ElasticIngestion::new(
        Arc::new(GoodDownloader),
        Arc::new(StubBridge),
        StubRepo,
        test_config(),
    );
    let batch = good
        .ingest_batch(&urls)
        .await
        .expect("healthy batch must succeed");
    assert_eq!(batch.success, 1);
    assert_eq!(batch.panics, 0);
}
