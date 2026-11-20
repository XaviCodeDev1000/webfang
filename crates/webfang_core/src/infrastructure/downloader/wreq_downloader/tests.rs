/// #2235: the per-submodule `--skip wreq_downloader` of the Miri lane is
/// GONE — a submodule skip deleted this whole test tree from the lane and
/// hid the UB living inside it. What the skip protected is now a PER-TEST
/// gate: every test that builds a `wreq::Client` carries the btls-sys FFI
/// reason, and the pure ones (the `Set-Cookie` parser) run under Miri.
use super::cookies::parse_set_cookie;
use super::*;

#[cfg_attr(miri, ignore = "btls-sys FFI (wreq Client) not supported by Miri")]
#[test]
fn test_wreq_downloader_creation() {
    let downloader = super::test_support::test_downloader_with_profile(Profile::Chrome145);
    assert!(!downloader.supports_interactions());
    assert_eq!(downloader.memory_cost(), WREQ_MEMORY_COST);
}

#[cfg_attr(miri, ignore = "btls-sys FFI (wreq Client) not supported by Miri")]
#[test]
fn test_wreq_downloader_honors_tls_profile() {
    // The constructor must accept every catalog profile and build a client
    // with it (triangulation: the parameter reaches the builder instead of
    // a hardcoded default).
    for profile in [Profile::Chrome145, Profile::Chrome131, Profile::Firefox135] {
        let downloader = super::test_support::test_downloader_with_profile(profile);
        assert!(!downloader.supports_interactions());
    }
}

#[cfg_attr(miri, ignore = "btls-sys FFI (wreq Client) not supported by Miri")]
#[test]
fn test_wreq_downloader_from_client() {
    let client = Client::builder()
        .emulation(Profile::Chrome145)
        .build()
        .unwrap();
    let downloader = WreqDownloader::from_client(client, 60, 15);
    assert!(!downloader.supports_interactions());
}

/// SSRF choke-point wiring proof (#1060): the client `WreqDownloader::new`
/// actually builds must carry the guard obtained from the `SsrfGuard` port.
/// `localhost` resolves to loopback through getaddrinfo with no network
/// dependency, so the connect attempt must be rejected by the validating
/// resolver — not by the (absent) listener on port 9.
#[cfg_attr(miri, ignore = "btls-sys FFI (wreq Client) not supported by Miri")]
#[tokio::test]
async fn downloader_client_enforces_ssrf_guard_from_the_port() {
    // Env hermeticity (#926): the escape hatch is captured at client-build
    // time, so clearing it must be serialized against siblings that set it.
    // `EnvGuard` holds the shared process-env lock and restores on drop.
    let _env = webfang_test_utils::EnvGuard::clean(&[
        crate::domain::ssrf_guard::DISABLE_VALIDATING_RESOLVER_ENV,
    ]);
    let downloader = super::test_support::test_downloader_with_profile(Profile::Chrome145);

    let err = downloader
        .client
        .get("http://localhost:9/")
        .send()
        .await
        .expect_err("hostname resolving to loopback must fail at connect");
    assert!(
        format!("{err:?}").contains("ForbiddenResolutionError"),
        "failure must come from the SSRF resolver, not the network: {err:?}"
    );
}

#[test]
fn test_parse_set_cookie_basic() {
    let header = "session=abc123; Path=/; HttpOnly; Secure";
    let url: Url = "https://example.com".parse().unwrap();
    let cookie = parse_set_cookie(header, &url).unwrap();

    assert_eq!(cookie.name, "session");
    assert_eq!(cookie.value, "abc123");
    assert_eq!(cookie.domain, "example.com");
    assert_eq!(cookie.path, "/");
    assert!(cookie.http_only);
    assert!(cookie.secure);
}

#[test]
fn test_parse_set_cookie_custom_domain() {
    let header = "token=xyz; Domain=.api.example.com; Path=/api";
    let url: Url = "https://example.com".parse().unwrap();
    let cookie = parse_set_cookie(header, &url).unwrap();

    assert_eq!(cookie.name, "token");
    assert_eq!(cookie.value, "xyz");
    assert_eq!(cookie.domain, ".api.example.com");
    assert_eq!(cookie.path, "/api");
    assert!(!cookie.http_only);
    assert!(!cookie.secure);
}

#[test]
fn test_parse_set_cookie_empty_name() {
    let header = "=value; Path=/";
    let url: Url = "https://example.com".parse().unwrap();
    assert!(parse_set_cookie(header, &url).is_none());
}

#[test]
fn test_parse_set_cookie_no_equals() {
    let header = "invalid";
    let url: Url = "https://example.com".parse().unwrap();
    assert!(parse_set_cookie(header, &url).is_none());
}

#[test]
fn test_parse_set_cookie_empty_header() {
    let url: Url = "https://example.com".parse().unwrap();
    assert!(parse_set_cookie("", &url).is_none());
}

#[cfg_attr(miri, ignore = "btls-sys FFI (wreq Client) not supported by Miri")]
#[tokio::test]
async fn test_fetch_uses_mock_server() {
    let expected_body = "<html><body><h1>mock</h1></body></html>";
    super::test_support::fetch_mock_get(expected_body).await;
}
