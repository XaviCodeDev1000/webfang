use super::*;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Default test downloader: short timeouts, Chrome145 fingerprint, no
/// operator overrides, 3 retries with 1s→10s backoff, default page cap.
/// Covers the majority of wiremock tests; variants below exist only for
/// call sites whose arguments genuinely differ (no silent unification).
pub(super) fn test_downloader() -> WreqDownloader {
    WreqDownloader::new(
        10,
        5,
        Profile::Chrome145,
        None,
        Vec::new(),
        None,
        None,
        3,
        1000,
        10000,
        crate::domain::downloader_factory::DEFAULT_MAX_PAGE_BYTES,
    )
    .expect("test downloader builds")
}

/// Longer-timeout variant (30s/10s) for tests that never hit the network
/// (construction proofs, SSRF-guard wiring). The profile stays a
/// parameter so the TLS triangulation test keeps passing each catalog
/// profile through the builder instead of a hardcoded default.
pub(super) fn test_downloader_with_profile(profile: Profile) -> WreqDownloader {
    WreqDownloader::new(
        30,
        10,
        profile,
        None,
        Vec::new(),
        None,
        None,
        3,
        1000,
        10000,
        crate::domain::downloader_factory::DEFAULT_MAX_PAGE_BYTES,
    )
    .unwrap_or_else(|e| panic!("client must build for profile {profile:?}: {e}"))
}

/// Pinned-User-Agent variant (#503): every request carries `user_agent`
/// and the 403 pool-rotation retry stays disabled.
pub(super) fn test_downloader_with_user_agent(user_agent: &str) -> WreqDownloader {
    WreqDownloader::new(
        10,
        5,
        Profile::Chrome145,
        Some(user_agent.to_string()),
        Vec::new(),
        None,
        None,
        3,
        1000,
        10000,
        crate::domain::downloader_factory::DEFAULT_MAX_PAGE_BYTES,
    )
    .expect("test downloader builds")
}

/// Fast-backoff variant for retry-exhaustion tests: same timeouts and
/// profile as the default, custom retry budget and backoff bounds so the
/// loop settles in milliseconds instead of seconds.
pub(super) fn test_downloader_with_backoff(
    max_retries: u32,
    backoff_base_ms: u64,
    backoff_max_ms: u64,
) -> WreqDownloader {
    WreqDownloader::new(
        10,
        5,
        Profile::Chrome145,
        None,
        Vec::new(),
        None,
        None,
        max_retries,
        backoff_base_ms,
        backoff_max_ms,
        crate::domain::downloader_factory::DEFAULT_MAX_PAGE_BYTES,
    )
    .expect("test downloader builds")
}

/// Mount a `GET /` mock returning `body` (200), then fetch it via a fresh
/// `WreqDownloader` and assert the basics (ok, status 200, body matches).
/// Returns the fetched `FetchedPage` so callers can add extra assertions.
#[allow(dead_code)]
pub(super) async fn fetch_mock_get(body: &str) -> FetchedPage {
    let mock_server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .mount(&mock_server)
        .await;

    let downloader = test_downloader();
    let url: Url = mock_server.uri().parse().unwrap();

    let result = downloader.fetch(&url).await;
    assert!(result.is_ok());
    let page = result.unwrap();
    assert_eq!(page.status, 200);
    assert_eq!(page.html, body);
    page
}
