use super::wiremock_fetch_tests::RawHeaderEquals;
/// #2235: the `--skip wreq_downloader` of the Miri lane is GONE, so this
/// module compiles under Miri again. Every test here drives a real
/// `wreq::Client` (btls-sys FFI), so each one carries its own gate with
/// that reason.
use super::*;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Issue #503 policy: a pinned UA disables the 403 pool-agent rotation.
/// The first 403 surfaces as a terminal error (no retry under a rotated
/// identity), and the next fetch still carries the pinned UA.
#[cfg_attr(miri, ignore = "btls-sys FFI (wreq Client) not supported by Miri")]
#[tokio::test]
async fn pinned_user_agent_disables_403_rotation() {
    const PINNED_UA: &str = "QA-Bot/9.9";
    let mock_server = MockServer::start().await;
    let pool_agent = UserAgentCache::fallback_agents().swap_remove(1);

    // First hit: 403 for the pinned UA (consumed once, then falls through).
    Mock::given(method("GET"))
        .and(path("/"))
        .and(header("User-Agent", PINNED_UA))
        .respond_with(ResponseTemplate::new(403))
        .up_to_n_times(1)
        .mount(&mock_server)
        .await;

    // Catches a rotated retry — must remain unmatched (expect(0)).
    Mock::given(method("GET"))
        .and(path("/"))
        .and(RawHeaderEquals {
            name: "user-agent",
            value: pool_agent,
        })
        .respond_with(ResponseTemplate::new(200).set_body_string("<html>rotated</html>"))
        .expect(0)
        .mount(&mock_server)
        .await;

    // Second hit: 200, but only for the pinned UA.
    Mock::given(method("GET"))
        .and(path("/"))
        .and(header("User-Agent", PINNED_UA))
        .respond_with(ResponseTemplate::new(200).set_body_string("<html>still pinned</html>"))
        .expect(1)
        .mount(&mock_server)
        .await;

    let downloader = super::test_support::test_downloader_with_user_agent(PINNED_UA);
    let url: Url = mock_server.uri().parse().unwrap();

    // First fetch: 403 is terminal — rotation is disabled when pinned.
    let err = downloader
        .fetch(&url)
        .await
        .expect_err("403 must surface as an error when the UA is pinned");
    assert!(
        matches!(err, DownloadError::Http { status: 403, .. }),
        "expected terminal Http 403, got: {err:?}"
    );

    // Second fetch: still carries the pinned UA (only the pinned mock
    // answers now), proving no identity drift after the 403.
    let page = downloader
        .fetch(&url)
        .await
        .expect("second fetch succeeds with the pinned UA");
    assert_eq!(page.status, 200);
    assert_eq!(page.html, "<html>still pinned</html>");
}

// ------------------------------------------------------------------
// Network resilience (#649)
// ------------------------------------------------------------------

/// 5xx must trigger the unified retry loop (1 initial + 3 retries), and
/// exhaustion must report the LAST observed status — not a hardcoded 429
/// (#649 Bugs 2 & 5). One server exercises both: first reply 429, then 500.
#[cfg_attr(miri, ignore = "btls-sys FFI (wreq Client) not supported by Miri")]
#[tokio::test]
async fn test_unified_retry_fires_and_reports_last_status() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    let server = MockServer::start().await;
    let counter = Arc::new(AtomicUsize::new(0));
    let counter_clone_if = Arc::clone(&counter);

    Mock::given(method("GET"))
        .respond_with(move |_req: &wiremock::Request| {
            let count = counter_clone_if.fetch_add(1, Ordering::SeqCst);
            if count < 2 {
                ResponseTemplate::new(429).insert_header("retry-after", "0")
            } else {
                ResponseTemplate::new(500)
            }
        })
        .mount(&server)
        .await;

    let downloader = super::test_support::test_downloader_with_backoff(3, 1, 5);
    let url: Url = format!("{}/", server.uri()).parse().expect("valid url");

    // 2×429 + 2×500 = 4 requests; the 5xx half proves Bug 2, the final
    // 500 status proves Bug 5 (last observed status, not hardcoded 429).
    match downloader.fetch(&url).await {
        Err(DownloadError::Http { status: 500, .. }) => {},
        other => panic!("Expected status 500 after 429→500 run, got {other:?}"),
    }
    assert_eq!(
        counter.load(Ordering::SeqCst),
        4,
        "Expected 2×429 + 2×500 = 4 requests"
    );
}

/// Unpinned baseline: the pre-#503 rotation behavior is preserved —
/// a 403 triggers one retry with the pool agent and succeeds.
#[cfg_attr(miri, ignore = "btls-sys FFI (wreq Client) not supported by Miri")]
#[tokio::test]
async fn unpinned_user_agent_rotates_pool_agent_on_403() {
    let mock_server = MockServer::start().await;
    let pool_agent = UserAgentCache::fallback_agents().swap_remove(1);

    // First hit: 403 regardless of UA (consumed once).
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(403))
        .up_to_n_times(1)
        .mount(&mock_server)
        .await;

    // The rotated retry must arrive with the pool agent.
    Mock::given(method("GET"))
        .and(path("/"))
        .and(RawHeaderEquals {
            name: "user-agent",
            value: pool_agent,
        })
        .respond_with(ResponseTemplate::new(200).set_body_string("<html>rotated</html>"))
        .expect(1)
        .mount(&mock_server)
        .await;

    let downloader = super::test_support::test_downloader();
    let url: Url = mock_server.uri().parse().unwrap();

    let page = downloader
        .fetch(&url)
        .await
        .expect("rotated retry succeeds for unpinned downloads");
    assert_eq!(page.status, 200);
    assert_eq!(page.html, "<html>rotated</html>");
}

/// Issue #1430: an always-403 origin must cost EXACTLY two requests —
/// one with the default UA, one with the rotated pool agent — and the
/// final error must preserve the last observed status (403).
/// The rotated retry consumes the attempt: it falls through to the
/// terminal handling instead of `continue`-ing back to the loop top
/// (which fired a third, default-UA request against the WAF).
#[cfg_attr(miri, ignore = "btls-sys FFI (wreq Client) not supported by Miri")]
#[tokio::test]
async fn always_403_costs_exactly_two_requests_and_reports_403() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    let mock_server = MockServer::start().await;
    let pool_agent = UserAgentCache::fallback_agents().swap_remove(1);
    let hits = Arc::new(AtomicUsize::new(0));
    let uas: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let hits_clone = Arc::clone(&hits);
    let uas_clone = Arc::clone(&uas);

    Mock::given(method("GET"))
        .respond_with(move |req: &wiremock::Request| {
            hits_clone.fetch_add(1, Ordering::SeqCst);
            let ua = req
                .headers
                .get("user-agent")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("<missing>")
                .to_string();
            uas_clone.lock().unwrap().push(ua);
            ResponseTemplate::new(403)
        })
        .mount(&mock_server)
        .await;

    let downloader = super::test_support::test_downloader();
    let url: Url = mock_server.uri().parse().unwrap();

    match downloader.fetch(&url).await {
        Err(DownloadError::Http { status: 403, .. }) => {},
        other => panic!("Expected terminal Http 403, got {other:?}"),
    }
    assert_eq!(
        hits.load(Ordering::SeqCst),
        2,
        "always-403 must cost exactly 2 requests (default + rotated)"
    );
    let uas = uas.lock().unwrap();
    assert_eq!(
        uas.len(),
        2,
        "expected one default-UA and one rotated-UA request"
    );
    assert_ne!(
        uas[0], pool_agent,
        "first request must carry the default UA, got: {}",
        uas[0]
    );
    assert_eq!(
        uas[1], pool_agent,
        "second request must carry the rotated pool agent"
    );
}
// ------------------------------------------------------------------
// FIX-1 (#1231 F-08, #1236 F-09): retry classification contract tests.
// The retry loop must honor DownloadError::classify() for the whole
// transient family: request timeouts are retriable (the most common
// transient failure in crawling), while builder-class errors (invalid
// request: unsupported scheme, malformed URL) are permanent and must
// surface after a SINGLE attempt.
// ------------------------------------------------------------------

/// F-08: a request that times out is RETRIED and recovery is served.
/// wiremock: first /slow hit sleeps past the client timeout (served
/// once, `.up_to_n_times(1)`), subsequent /slow hits respond
/// instantly. Asserts exactly 2 outbound requests and a successful
/// page — the timeout retry fired and its result was used.
#[cfg_attr(miri, ignore = "btls-sys FFI (wreq Client) not supported by Miri")]
#[tokio::test]
async fn timeout_is_retried_and_recovery_is_served() {
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/slow"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("late")
                .set_delay(Duration::from_millis(1500)),
        )
        .up_to_n_times(1)
        .mount(&server)
        .await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/slow"))
        .respond_with(ResponseTemplate::new(200).set_body_string("fast"))
        .mount(&server)
        .await;

    // timeout 1s so the first /slow hit (1.5s) trips it; short backoff
    // keeps the test fast.
    let dl = WreqDownloader::new(
        1,
        1,
        Profile::Chrome145,
        None,
        Vec::new(),
        None,
        None,
        2,
        10,
        50,
        crate::domain::downloader_factory::DEFAULT_MAX_PAGE_BYTES,
    )
    .expect("test downloader builds");

    let slow_url: Url = format!("{}/slow", server.uri())
        .parse()
        .expect("server uri parses");
    let page = dl
        .fetch(&slow_url)
        .await
        .expect("retry after timeout must recover");
    assert_eq!(page.html, "fast");
    let hits = server
        .received_requests()
        .await
        .expect("received requests")
        .len();
    assert_eq!(
        hits, 2,
        "timeout retry must produce exactly one re-request, got {hits}"
    );
}

/// F-09: a builder-class error (unsupported scheme) must NOT be
/// retried — single attempt, immediate permanent error.
#[cfg_attr(miri, ignore = "btls-sys FFI (wreq Client) not supported by Miri")]
#[tokio::test]
async fn unsupported_scheme_is_not_retried() {
    let dl = WreqDownloader::new(
        5,
        5,
        Profile::Chrome145,
        None,
        Vec::new(),
        None,
        None,
        3,
        10,
        50,
        crate::domain::downloader_factory::DEFAULT_MAX_PAGE_BYTES,
    )
    .expect("test downloader builds");

    let url = url::Url::parse("ftp://example.com/x").expect("parseable scheme");
    let start = std::time::Instant::now();
    let err = dl.fetch(&url).await.expect_err("ftp must fail");
    let elapsed = start.elapsed();

    // No exponential backoff between attempts: a single attempt fails
    // fast. The attempt count itself is not observable without a
    // socket; the typed error + fast failure are the observables.
    assert!(
        matches!(err, DownloadError::InvalidUrl(_)),
        "builder error must map to InvalidUrl, got: {err:?}"
    );
    assert!(
        elapsed < Duration::from_secs(2),
        "no-retry path must fail fast, took {elapsed:?}"
    );
}

// FIX-1 #1231 F-12: the decompressed body cap. Both tests build real
// gzip wire bodies with the existing `async-compression` workspace dep
// (same helper pattern as the sitemap_parser tests); wreq's
// `.gzip(true)` client transparently inflates them, so the stream
// `read_body_capped` accumulates is the DECOMPRESSED payload.

/// Compress `data` with gzip over tokio (async-compression bufread).
async fn gzip_compress(data: &[u8]) -> Vec<u8> {
    use async_compression::tokio::bufread::GzipEncoder;
    use tokio::io::{AsyncReadExt, BufReader};

    let mut encoder = GzipEncoder::new(BufReader::new(std::io::Cursor::new(data)));
    let mut out = Vec::new();
    encoder
        .read_to_end(&mut out)
        .await
        .expect("in-memory gzip encode");
    out
}

/// F-12 (negative): a "gzip bomb" — 134 bytes on the wire inflating to
/// 100 KiB — must abort at the configured DECOMPRESSED cap with
/// `BodyTooLarge`, and because that error classifies PermanentFatal it
/// must surface after exactly ONE outbound request (no retry).
#[cfg_attr(miri, ignore = "btls-sys FFI (wreq Client) not supported by Miri")]
#[tokio::test]
async fn gzip_bomb_is_rejected_at_the_decompressed_cap() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/bomb"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(gzip_compress(&[b'a'; 102_400]).await, "application/gzip")
                .insert_header("content-encoding", "gzip"),
        )
        .mount(&server)
        .await;

    // 64 KiB cap: below the 100 KiB inflated body.
    let bomb_cap: u64 = 64 * 1024;
    let dl = WreqDownloader::new(
        10,
        5,
        Profile::Chrome145,
        None,
        Vec::new(),
        None,
        None,
        3,
        10,
        50,
        bomb_cap,
    )
    .expect("test downloader builds");

    let bomb_url: Url = format!("{}/bomb", server.uri())
        .parse()
        .expect("server uri parses");
    match dl.fetch(&bomb_url).await {
        Ok(page) => panic!(
            "100 KiB inflated body must exceed the 64 KiB cap, got {} bytes",
            page.html.len()
        ),
        Err(DownloadError::BodyTooLarge { limit }) => {
            assert_eq!(limit, bomb_cap, "error must carry the configured cap");
        },
        Err(other) => panic!("expected BodyTooLarge, got: {other:?}"),
    }

    let hits = server
        .received_requests()
        .await
        .expect("received requests")
        .len();
    assert_eq!(
        hits, 1,
        "BodyTooLarge is PermanentFatal: exactly one attempt, got {hits}"
    );
}

/// F-12 (positive): a gzipped body UNDER the cap is served in full
/// after transparent decompression — the cap counts decompressed bytes
/// and leaves the happy path untouched.
#[cfg_attr(miri, ignore = "btls-sys FFI (wreq Client) not supported by Miri")]
#[tokio::test]
async fn gzipped_body_under_the_cap_is_served_decompressed() {
    let server = MockServer::start().await;
    let body = b"hello compressed world";
    Mock::given(method("GET"))
        .and(path("/small"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(gzip_compress(body).await, "application/gzip")
                .insert_header("content-encoding", "gzip"),
        )
        .mount(&server)
        .await;

    let dl = super::test_support::test_downloader_with_backoff(3, 10, 50);

    let url: Url = format!("{}/small", server.uri())
        .parse()
        .expect("server uri parses");
    let page = dl
        .fetch(&url)
        .await
        .expect("under-cap gzip body must fetch normally");
    assert_eq!(page.html, "hello compressed world");
}
