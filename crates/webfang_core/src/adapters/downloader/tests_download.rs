//! Download tests (wiremock: dedup, retries, redirect SSRF, streaming).
//! Split from `adapters/downloader/mod.rs` under the ~500 rule (#2584, Ola 1).

use super::tests_common::vu;
use super::*;
use tempfile::TempDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[cfg_attr(miri, ignore = "btls-sys FFI (wreq Client) not supported by Miri")]
#[tokio::test]
async fn test_download_streaming_limit() {
    let temp_dir = TempDir::new().unwrap();
    let config = DownloadConfig {
        output_dir: temp_dir.path().to_path_buf(),
        max_file_size: 1024,
        ..Default::default()
    };
    let downloader = Downloader::new(config).unwrap();
    assert_eq!(downloader.config.max_file_size, 1024);
}

#[cfg_attr(miri, ignore = "btls-sys FFI (wreq Client) not supported by Miri")]
#[tokio::test]

async fn test_download_batch_empty() {
    let temp_dir = TempDir::new().unwrap();
    let config = DownloadConfig {
        output_dir: temp_dir.path().to_path_buf(),
        ..Default::default()
    };
    let downloader = Downloader::new(config).unwrap();
    let results = downloader.download_batch(&[]).await;
    assert!(results.is_empty());
}

/// Mount an asset route returning `body` and `expect(1)` — the wire-level
/// proof of the #782 dedup contract: at most ONE network request for the
/// URL no matter how many times it is handed to a `Downloader`.
async fn mount_asset_expect_once(server: &MockServer, asset_path: &str, body: Vec<u8>) {
    Mock::given(method("GET"))
        .and(path(asset_path))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(body)
                .insert_header("content-type", "image/png"),
        )
        .expect(1)
        .mount(server)
        .await;
}

/// #782: sequential occurrences of the same asset URL (later crawl pages)
/// must NOT re-download — the second call reuses the cached file and both
/// resolve to the same local path. wiremock's `expect(1)` verifies the
/// single request at server drop.
#[cfg_attr(miri, ignore = "btls-sys FFI (wreq Client) not supported by Miri")]
#[tokio::test]

async fn test_download_dedup_sequential_reuses_cached_file() {
    let server = MockServer::start().await;
    mount_asset_expect_once(&server, "/img.png", b"png-bytes".to_vec()).await;

    let temp_dir = TempDir::new().unwrap();
    let downloader = Downloader::new(DownloadConfig {
        output_dir: temp_dir.path().to_path_buf(),
        max_retries: 0,
        ..Default::default()
    })
    .unwrap();

    let url = vu(&format!("{}/img.png", server.uri()));
    let first = downloader.download(&url).await.unwrap();
    let second = downloader.download(&url).await.unwrap();

    assert_eq!(
        first.local_path, second.local_path,
        "the duplicate occurrence must resolve to the same already-downloaded file"
    );
    assert!(
        first.local_path.exists(),
        "cached asset file must exist on disk"
    );
}

/// #782: concurrent arrivals for the same asset URL (parallel pages) must
/// produce exactly ONE origin request — the losers wait on the winner's
/// result instead of opening second connections. The response delay
/// forces the two downloads to overlap.
#[cfg_attr(miri, ignore = "btls-sys FFI (wreq Client) not supported by Miri")]
#[tokio::test]

async fn test_download_dedup_concurrent_single_request() {
    use std::sync::Arc;

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/busy.png"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(b"busy-png".to_vec())
                .insert_header("content-type", "image/png")
                .set_delay(Duration::from_millis(200)),
        )
        .expect(1)
        .mount(&server)
        .await;

    let temp_dir = TempDir::new().unwrap();
    let downloader = Arc::new(
        Downloader::new(DownloadConfig {
            output_dir: temp_dir.path().to_path_buf(),
            max_retries: 0,
            ..Default::default()
        })
        .unwrap(),
    );
    let url = vu(&format!("{}/busy.png", server.uri()));

    let winner = {
        let d = Arc::clone(&downloader);
        let u = url.clone();
        tokio::spawn(async move { d.download(&u).await })
    };
    let loser = {
        let d = Arc::clone(&downloader);
        tokio::spawn(async move { d.download(&url).await })
    };

    let first = winner.await.unwrap().unwrap();
    let second = loser.await.unwrap().unwrap();

    assert_eq!(
        first.local_path, second.local_path,
        "both concurrent callers must resolve to the same file"
    );
}

/// #782 guardrail: failures are NOT cached — a failed download leaves the
/// registry cell uninitialized, so a later page may retry the URL and
/// every caller keeps its typed error verbatim (no error poisoning).
#[cfg_attr(miri, ignore = "btls-sys FFI (wreq Client) not supported by Miri")]
#[tokio::test]

async fn test_download_failure_not_cached_in_dedup_registry() {
    let server = MockServer::start().await;
    // First occurrence fails (404 is non-retriable), then the route heals.
    Mock::given(method("GET"))
        .and(path("/flaky.png"))
        .respond_with(ResponseTemplate::new(404).set_body_string("gone"))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/flaky.png"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(b"healed".to_vec())
                .insert_header("content-type", "image/png"),
        )
        .up_to_n_times(2)
        .mount(&server)
        .await;

    let temp_dir = TempDir::new().unwrap();
    let downloader = Downloader::new(DownloadConfig {
        output_dir: temp_dir.path().to_path_buf(),
        max_retries: 0,
        ..Default::default()
    })
    .unwrap();

    let url = vu(&format!("{}/flaky.png", server.uri()));
    let failure = downloader.download(&url).await;
    assert!(
        matches!(failure, Err(ScraperError::Http { status: 404, .. })),
        "first failure must surface the typed HTTP error: {failure:?}"
    );

    // Later page retries the SAME downloader — must not be poisoned by the
    // earlier failure.
    let retry = downloader.download(&url).await;
    assert!(
        retry.is_ok(),
        "failed outcome must not be cached; retry should succeed: {retry:?}"
    );
}

/// Mount the entry route of a redirect-flow fixture plus its destination.
/// The `location` header targets a forbidden literal IP, so the guard must
/// stop the flow before `/target` is ever requested — `expect(0)` is the
/// wire-level proof of that.
async fn mount_redirect_with_untouched_target(
    server: &MockServer,
    from_path: &str,
    to_location: &str,
) {
    let redirect = ResponseTemplate::new(301).insert_header("location", to_location);
    let entry = ResponseTemplate::new(200).set_body_string("body-never-fetched");

    Mock::given(method("GET"))
        .and(path(from_path))
        .respond_with(redirect)
        .mount(server)
        .await;

    Mock::given(method("GET"))
        .and(path("/target"))
        .respond_with(entry)
        .expect(0)
        .mount(server)
        .await;
}

/// SSRF redirect guard (#703): even an entry URL that passed validation
/// cannot hop onto a literal forbidden IP through a redirect — the 301
/// itself becomes a terminal HTTP error and `/target` is never fetched
/// (wire-level proof via `expect(0)`).
#[cfg_attr(miri, ignore = "btls-sys FFI (wreq Client) not supported by Miri")]
#[tokio::test]

async fn test_redirect_to_forbidden_literal_ip_is_stopped() {
    // Keep the guard active: the escape hatch must be unset in this
    // process (a no-op under nextest, which gives every test its own
    // process; defensive against shared-process harnesses).
    let _guard = webfang_test_utils::EnvGuard::clean(&[
        crate::domain::ssrf_guard::DISABLE_REDIRECT_GUARD_ENV,
    ]);
    let mock_server = MockServer::start().await;

    // Location points at a different loopback literal — forbidden by the
    // guard, unreachable in practice, and never requested.
    mount_redirect_with_untouched_target(&mock_server, "/download", "http://127.0.0.2:9/target")
        .await;

    let temp_dir = TempDir::new().unwrap();
    let downloader = Downloader::new(DownloadConfig {
        output_dir: temp_dir.path().to_path_buf(),
        max_retries: 0,
        ..Default::default()
    })
    .unwrap();

    match downloader
        .download(&vu(&format!("{}/download", mock_server.uri())))
        .await
    {
        Err(ScraperError::Http { status, .. }) => assert_eq!(status, 301),
        other => panic!("expected redirect to be stopped, got: {other:?}"),
    }
}

/// cargo-mutants PR-diff gate: a transient (5xx) failure must be RETRIED, not
/// returned immediately. The mutant `classify() != TransientRetriable` → `==`
/// in `download_with_retry` makes the first 503 surface as the final error;
/// this test fails under that mutant because the second mock (200) would
/// never be reached and the result would be the 503.
#[cfg_attr(miri, ignore = "btls-sys FFI (wreq Client) not supported by Miri")]
#[tokio::test]
async fn transient_5xx_is_retried_until_success() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/flaky-500.png"))
        .respond_with(ResponseTemplate::new(503).set_body_string("busy"))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/flaky-500.png"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(b"ok".to_vec())
                .insert_header("content-type", "image/png"),
        )
        .mount(&server)
        .await;

    let temp_dir = TempDir::new().unwrap();
    let downloader = Downloader::new(DownloadConfig {
        output_dir: temp_dir.path().to_path_buf(),
        max_retries: 2,
        backoff_base_ms: 1,
        backoff_max_ms: 2,
        ..Default::default()
    })
    .unwrap();

    let url = vu(&format!("{}/flaky-500.png", server.uri()));
    let asset = downloader
        .download(&url)
        .await
        .expect("retry must heal the 503");
    assert_eq!(asset.size, 2);
    server.verify().await;
}

/// Same gate, other mutant direction: a NON-transient (4xx) failure must NOT
/// be retried — the mutant `attempt == max_retries` → `!=` would retry it.
/// `expect(1)` on the 404 mock proves exactly one wire request happened.
#[cfg_attr(miri, ignore = "btls-sys FFI (wreq Client) not supported by Miri")]
#[tokio::test]
async fn permanent_4xx_is_not_retried() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/gone.png"))
        .respond_with(ResponseTemplate::new(404).set_body_string("gone"))
        .expect(1)
        .mount(&server)
        .await;

    let temp_dir = TempDir::new().unwrap();
    let downloader = Downloader::new(DownloadConfig {
        output_dir: temp_dir.path().to_path_buf(),
        max_retries: 3,
        backoff_base_ms: 1,
        backoff_max_ms: 2,
        ..Default::default()
    })
    .unwrap();

    let url = vu(&format!("{}/gone.png", server.uri()));
    let failure = downloader.download(&url).await;
    assert!(
        matches!(failure, Err(ScraperError::Http { status: 404, .. })),
        "4xx must surface immediately: {failure:?}"
    );
    server.verify().await;
}
