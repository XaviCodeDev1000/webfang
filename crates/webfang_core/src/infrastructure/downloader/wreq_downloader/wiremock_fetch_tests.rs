/// #2235: the `--skip wreq_downloader` of the Miri lane is GONE, so this
/// module compiles under Miri again. Every test here drives a real
/// `wreq::Client` (btls-sys FFI), so each one carries its own gate with
/// that reason.
use super::*;
use wiremock::matchers::{header, method, path};
use wiremock::{Match, Mock, MockServer, ResponseTemplate};

#[cfg_attr(miri, ignore = "btls-sys FFI (wreq Client) not supported by Miri")]
#[tokio::test]
async fn test_fetch_200_returns_body() {
    let expected_body = "<html><body>Hello World</body></html>";
    let page = super::test_support::fetch_mock_get(expected_body).await;
    assert_eq!(page.html, expected_body);
}

#[cfg_attr(miri, ignore = "btls-sys FFI (wreq Client) not supported by Miri")]
#[tokio::test]
async fn test_fetch_404_returns_error() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/notfound"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&mock_server)
        .await;

    let downloader = super::test_support::test_downloader();
    let url: Url = format!("{}/notfound", mock_server.uri()).parse().unwrap();

    let result = downloader.fetch(&url).await;
    assert!(result.is_err());
    assert!(matches!(
        result.unwrap_err(),
        DownloadError::Http { status: 404, .. }
    ));
}

#[cfg_attr(miri, ignore = "btls-sys FFI (wreq Client) not supported by Miri")]
#[tokio::test]
async fn test_fetch_extracts_cookies() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("<html></html>")
                .insert_header("set-cookie", "session=abc123; Path=/; HttpOnly"),
        )
        .mount(&mock_server)
        .await;

    let downloader = super::test_support::test_downloader();
    let url: Url = mock_server.uri().parse().unwrap();

    let result = downloader.fetch(&url).await;
    assert!(result.is_ok());

    let page = result.unwrap();
    assert_eq!(page.status, 200);
    assert!(!page.cookies.is_empty());

    let cookie = &page.cookies[0];
    assert_eq!(cookie.name, "session");
    assert_eq!(cookie.value, "abc123");
    assert!(cookie.http_only);
}

#[cfg_attr(miri, ignore = "btls-sys FFI (wreq Client) not supported by Miri")]
#[tokio::test]
async fn test_fetch_returns_final_url() {
    // The SSRF redirect guard (#703) stops redirects targeting a literal
    // forbidden IP, and wiremock binds 127.0.0.1 — lift the guard for this
    // redirect-flow test before the client is built.
    let _guard = webfang_test_utils::EnvGuard::with(&[(
        crate::infrastructure::ssrf::DISABLE_REDIRECT_GUARD_ENV,
        "1",
    )]);
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/redirect"))
        .respond_with(ResponseTemplate::new(301).insert_header("location", "/target"))
        .mount(&mock_server)
        .await;

    Mock::given(method("GET"))
        .and(path("/target"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<html></html>"))
        .mount(&mock_server)
        .await;

    let downloader = super::test_support::test_downloader();
    let url: Url = format!("{}/redirect", mock_server.uri()).parse().unwrap();

    let result = downloader.fetch(&url).await;
    assert!(result.is_ok());

    let page = result.unwrap();
    assert!(page.url.as_str().contains("/target"));
}

/// SSRF redirect guard (#703): a redirect whose `Location` is a literal
/// forbidden IP must be stopped even when the entry URL itself passed
/// validation. The redirect response surfaces as a terminal HTTP error and
/// the target is never requested (`expect(0)` proves wiremock saw no hit).
#[cfg_attr(miri, ignore = "btls-sys FFI (wreq Client) not supported by Miri")]
#[tokio::test]
async fn test_redirect_to_forbidden_literal_ip_is_stopped() {
    // Defensive under shared-process harnesses: the escape hatch must be
    // unset for this process so the guard is active. (nextest isolates
    // each test in its own process, so this is a no-op there.)
    let _guard = webfang_test_utils::EnvGuard::clean(&[
        crate::infrastructure::ssrf::DISABLE_REDIRECT_GUARD_ENV,
    ]);
    let mock_server = MockServer::start().await;

    // Location points at a different loopback literal — forbidden by the
    // guard, unreachable in practice, and never requested.
    Mock::given(method("GET"))
        .and(path("/redirect"))
        .respond_with(
            ResponseTemplate::new(301).insert_header("location", "http://127.0.0.2:9/target"),
        )
        .mount(&mock_server)
        .await;

    Mock::given(method("GET"))
        .and(path("/target"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<html></html>"))
        .expect(0)
        .mount(&mock_server)
        .await;

    // Fresh client built in this process: the guard env hatch stays unset.
    let downloader = super::test_support::test_downloader();
    let url: Url = format!("{}/redirect", mock_server.uri()).parse().unwrap();

    let result = downloader.fetch(&url).await;
    match result {
        Err(DownloadError::Http { status, .. }) => assert_eq!(status, 301),
        other => panic!("expected redirect to be stopped, got: {other:?}"),
    }
}

// ------------------------------------------------------------------
// Pinned User-Agent (#503)
// ------------------------------------------------------------------

/// Wire-level proof that a pinned UA wins over the emulation-profile
/// default UA: the TLS/HTTP2 fingerprint (Chrome145) is fully enabled,
/// yet the server receives exactly the pinned value. The mock only
/// matches `QA-Bot/9.9` — if the profile UA leaked onto the wire the
/// request would 404 and the fetch would fail.
#[cfg_attr(miri, ignore = "btls-sys FFI (wreq Client) not supported by Miri")]
#[tokio::test]
async fn pinned_user_agent_beats_emulation_profile_on_the_wire() {
    const PINNED_UA: &str = "QA-Bot/9.9";
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/"))
        .and(header("User-Agent", PINNED_UA))
        .respond_with(ResponseTemplate::new(200).set_body_string("<html></html>"))
        .expect(1)
        .mount(&mock_server)
        .await;

    let downloader = super::test_support::test_downloader_with_user_agent(PINNED_UA);
    let url: Url = mock_server.uri().parse().unwrap();

    let page = downloader
        .fetch(&url)
        .await
        .expect("fetch succeeds only if the pinned UA matched on the wire");
    assert_eq!(page.status, 200);

    // Second proof: the server-side record shows exactly the pinned UA.
    let requests = mock_server
        .received_requests()
        .await
        .expect("server records requests");
    let ua = requests
        .first()
        .expect("one request recorded")
        .headers
        .get("user-agent")
        .expect("User-Agent header present");
    assert_eq!(
        ua.to_str().expect("User-Agent must be valid ASCII"),
        PINNED_UA
    );
}

// ------------------------------------------------------------------
// #890 — operator headers, Accept-Language, and seeded cookies
// ------------------------------------------------------------------

/// Wire-level proof that an operator custom header (`--header`, #890)
/// reaches the outgoing scrape request even though the Chrome145
/// emulation profile does not send it by default.
#[cfg_attr(miri, ignore = "btls-sys FFI (wreq Client) not supported by Miri")]
#[tokio::test]
async fn custom_header_reaches_the_wire() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/"))
        .and(RawHeaderEquals {
            name: "x-wftest",
            value: "hola".to_string(),
        })
        .respond_with(ResponseTemplate::new(200).set_body_string("<html></html>"))
        .expect(1)
        .mount(&mock_server)
        .await;

    let downloader = WreqDownloader::new(
        10,
        5,
        Profile::Chrome145,
        None,
        vec![("X-WFTest".to_string(), "hola".to_string())],
        None,
        None,
        3,
        1000,
        10000,
        crate::domain::downloader_factory::DEFAULT_MAX_PAGE_BYTES,
    )
    .unwrap();
    let url: Url = mock_server.uri().parse().unwrap();

    let page = downloader
        .fetch(&url)
        .await
        .expect("fetch succeeds only if the custom header matched on the wire");
    assert_eq!(page.status, 200);
}

/// Wire-level proof that the configured Accept-Language (`
/// --accept-language`, #890) replaces the emulation-profile default on
/// the wire instead of being silently dropped (#890 reproduction showed
/// `es-AR` arriving as the profile's `en-US,en;q=0.9`).
#[cfg_attr(miri, ignore = "btls-sys FFI (wreq Client) not supported by Miri")]
#[tokio::test]
async fn accept_language_replaces_profile_default_on_the_wire() {
    const LANG: &str = "es-AR,es;q=0.9";
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/"))
        .and(RawHeaderEquals {
            name: "accept-language",
            value: LANG.to_string(),
        })
        .respond_with(ResponseTemplate::new(200).set_body_string("<html></html>"))
        .expect(1)
        .mount(&mock_server)
        .await;

    let downloader = WreqDownloader::new(
        10,
        5,
        Profile::Chrome145,
        None,
        Vec::new(),
        Some(LANG.to_string()),
        None,
        3,
        1000,
        10000,
        crate::domain::downloader_factory::DEFAULT_MAX_PAGE_BYTES,
    )
    .unwrap();
    let url: Url = mock_server.uri().parse().unwrap();

    let page = downloader
        .fetch(&url)
        .await
        .expect("fetch succeeds only if the configured language matched on the wire");
    assert_eq!(page.status, 200);
}

/// Malformed operator input (#890, review M1): an invalid header NAME must
/// fail LOUDLY at client build time with a typed internal error naming the
/// offending header — never a silent drop (the same silent-drop class #890
/// exists to eliminate).
///
/// Miri: NO gate, on purpose. The validation happens BEFORE the client is
/// built (`WreqDownloader::new` returns on the invalid header), so this
/// test never reaches the btls-sys FFI and runs under Miri — proof that
/// the operator input is rejected without opening a TLS backend.
#[tokio::test]
async fn invalid_custom_header_name_fails_loudly_at_build() {
    let error = WreqDownloader::new(
        10,
        5,
        Profile::Chrome145,
        None,
        vec![("bad header name\n".to_string(), "v".to_string())],
        None,
        None,
        3,
        1000,
        10000,
        crate::domain::downloader_factory::DEFAULT_MAX_PAGE_BYTES,
    )
    .err()
    .expect("newline is not a valid HTTP header name");
    match error {
        DownloadError::Internal(msg) => {
            assert!(msg.contains("invalid --header name"), "got: {msg}");
        },
        other => panic!("expected DownloadError::Internal, got: {other:?}"),
    }
}

/// Malformed operator input (#890, review M1): an invalid header VALUE and
/// an invalid Accept-Language must both fail loudly at client build time.
///
/// Miri: NO gate, on purpose — same reason as the sibling above: both
/// values are rejected before the client is built, so the btls-sys FFI is
/// never reached and the test runs under Miri.
#[tokio::test]
async fn invalid_header_value_and_language_fail_loudly_at_build() {
    let value_error = WreqDownloader::new(
        10,
        5,
        Profile::Chrome145,
        None,
        vec![("X-Ok".to_string(), "bad\nvalue".to_string())],
        None,
        None,
        3,
        1000,
        10000,
        crate::domain::downloader_factory::DEFAULT_MAX_PAGE_BYTES,
    )
    .err()
    .expect("newline is not a valid HTTP header value");
    assert!(
        matches!(value_error, DownloadError::Internal(ref m) if m.contains("invalid --header value")),
        "got: {value_error:?}"
    );

    let lang_error = WreqDownloader::new(
        10,
        5,
        Profile::Chrome145,
        None,
        Vec::new(),
        Some("es\u{0}-AR".to_string()),
        None,
        3,
        1000,
        10000,
        crate::domain::downloader_factory::DEFAULT_MAX_PAGE_BYTES,
    )
    .err()
    .expect("NUL is not a valid Accept-Language value");
    assert!(
        matches!(lang_error, DownloadError::Internal(ref m) if m.contains("invalid --accept-language")),
        "got: {lang_error:?}"
    );
}

/// Wire-level proof that a pre-seeded cookie jar (`--cookie`, #890) is
/// carried by Static/Hybrid-L1 wreq requests from the very first fetch.
#[cfg_attr(miri, ignore = "btls-sys FFI (wreq Client) not supported by Miri")]
#[tokio::test]
async fn seeded_cookie_jar_carries_operator_cookies_on_the_wire() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/"))
        .and(header("cookie", "k=v"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<html></html>"))
        .expect(1)
        .mount(&mock_server)
        .await;

    let jar = Jar::default();
    jar.add("k=v", mock_server.uri().as_str());
    let downloader = WreqDownloader::new(
        10,
        5,
        Profile::Chrome145,
        None,
        Vec::new(),
        None,
        Some(std::sync::Arc::new(jar)),
        3,
        1000,
        10000,
        crate::domain::downloader_factory::DEFAULT_MAX_PAGE_BYTES,
    )
    .unwrap();
    let url: Url = mock_server.uri().parse().unwrap();

    let page = downloader
        .fetch(&url)
        .await
        .expect("fetch succeeds only if the seeded cookie reached the wire");
    assert_eq!(page.status, 200);
}

/// Exact-match matcher for a single raw header value.
///
/// Unlike `wiremock::matchers::header`, it does NOT comma-split the
/// received value: the fallback pool agents embed commas
/// ("(KHTML, like Gecko)"), which the built-in exact matcher splits into
/// two values and therefore never matches.
pub(super) struct RawHeaderEquals {
    pub(super) name: &'static str,
    pub(super) value: String,
}

impl Match for RawHeaderEquals {
    fn matches(&self, request: &wiremock::Request) -> bool {
        request
            .headers
            .get_all(self.name)
            .iter()
            .any(|v| v.to_str().is_ok_and(|s| s == self.value))
    }
}
