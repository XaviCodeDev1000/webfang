use super::*;

#[test]
fn test_parse_crawl_delay_basic() {
    let robots_body = "\
User-agent: *
Crawl-delay: 5
Disallow: /tmp/";

    assert_eq!(parse_crawl_delay(robots_body), Some(5.0));
}

#[test]
fn test_parse_crawl_delay_none() {
    let no_delay = "User-agent: *\nDisallow: /";
    assert_eq!(parse_crawl_delay(no_delay), None);
}

#[test]
fn test_parse_crawl_delay_fractional() {
    let fractional = "User-agent: *\nCrawl-delay: 0.5\n";
    assert_eq!(parse_crawl_delay(fractional), Some(0.5));
}

#[test]
fn test_parse_crawl_delay_case_insensitive() {
    let robots_body = "user-agent: *\nCrawl-Delay: 10\n";
    assert_eq!(parse_crawl_delay(robots_body), Some(10.0));

    let robots_body_upper = "User-Agent: *\nCRAWL-DELAY: 3\n";
    assert_eq!(parse_crawl_delay(robots_body_upper), Some(3.0));
}

#[cfg_attr(miri, ignore)] // btls/wreq FFI (BoringSSL TLS_method) not supported by Miri
#[test]
fn test_fetcher_new_builds_client_for_profiles() {
    let fetcher = RobotsFetcher::new(Profile::Chrome145, 30).expect("client should build");
    // A freshly built fetcher has no cached rules for any domain.
    assert_eq!(fetcher.get_crawl_delay("example.com"), None);

    // The constructor honors the caller's profile selection.
    assert!(RobotsFetcher::new(Profile::Firefox135, 5).is_ok());
}

/// SSRF choke-point wiring proof (#1060): the client `RobotsFetcher::new`
/// actually builds must carry the guard. `localhost` resolves to loopback
/// through getaddrinfo with no network dependency, so a connect attempt must
/// be rejected by the validating resolver the `SsrfGuard` port installs —
/// proving the site consumes the port instead of hand-wiring (or dropping)
/// the layers.
#[cfg_attr(miri, ignore = "btls-sys FFI (wreq Client) not supported by Miri")]
#[tokio::test]
async fn fetcher_client_enforces_ssrf_guard_from_the_port() {
    // Env hermeticity (#926): `ValidatingResolver::new()` captures the escape
    // hatch at client-build time, so this mutation must be serialized against
    // siblings that set it. `cargo test` (and the Coverage job's `cargo
    // llvm-cov` run) shares one process env across threads; only nextest
    // isolates. `EnvGuard` holds the shared lock and restores on drop.
    let _env = webfang_test_utils::EnvGuard::clean(&[
        crate::domain::ssrf_guard::DISABLE_VALIDATING_RESOLVER_ENV,
    ]);
    let fetcher = RobotsFetcher::new(Profile::Chrome145, 30).expect("client should build");

    let err = fetcher
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

/// Seed a cached `Rules` decision for a domain, bypassing the fetch.
fn seed_rules(fetcher: &RobotsFetcher, domain: &str, content: &str, crawl_delay: Option<f64>) {
    fetcher.cache.insert(
        domain.to_string(),
        init_cache_cell(RobotsCacheEntry::Rules(Arc::new(RobotsRules {
            content: content.to_string(),
            crawl_delay_secs: crawl_delay,
        }))),
    );
}

#[cfg_attr(miri, ignore)] // btls/wreq FFI (BoringSSL TLS_method) not supported by Miri
#[tokio::test]
async fn test_robots_cache_hit() {
    let fetcher = RobotsFetcher::new(Profile::Chrome145, 30).expect("client should build");
    seed_rules(
        &fetcher,
        "example.com",
        "User-agent: *\nDisallow: /private/\n",
        Some(2.0),
    );

    // Should allow public URL
    assert!(fetcher
        .is_allowed("https://example.com/public", "example.com")
        .await
        .allows());
    // Should disallow private URL
    assert!(!fetcher
        .is_allowed("https://example.com/private/secret", "example.com")
        .await
        .allows());
}

#[cfg_attr(miri, ignore)] // btls/wreq FFI (BoringSSL TLS_method) not supported by Miri
#[test]
fn test_get_crawl_delay_returns_cached_value() {
    let fetcher = RobotsFetcher::new(Profile::Chrome145, 30).expect("client should build");
    seed_rules(&fetcher, "slow-site.com", "", Some(7.5));

    assert_eq!(fetcher.get_crawl_delay("slow-site.com"), Some(7.5));
    assert_eq!(fetcher.get_crawl_delay("unknown.com"), None);
}

#[cfg_attr(miri, ignore)] // btls/wreq FFI (BoringSSL TLS_method) not supported by Miri
#[test]
fn test_get_crawl_delay_is_none_for_negative_entry() {
    let fetcher = RobotsFetcher::new(Profile::Chrome145, 30).expect("client should build");
    fetcher.cache.insert(
        "no-robots.com".to_string(),
        init_cache_cell(RobotsCacheEntry::AllowAll),
    );

    assert_eq!(fetcher.get_crawl_delay("no-robots.com"), None);
}

#[cfg_attr(miri, ignore)] // btls/wreq FFI (BoringSSL TLS_method) not supported by Miri
#[tokio::test]
async fn test_cached_allow_all_entry_allows_without_fetch() {
    // A cached negative entry must be served from the cache — the URL used
    // points at a host with no routes, so any fetch attempt would fail
    // (and be observable via timing); correctness here is enforced by the
    // entry being served synchronously from the map.
    let fetcher = RobotsFetcher::new(Profile::Chrome145, 1).expect("client should build");
    fetcher.cache.insert(
        "dead.example".to_string(),
        init_cache_cell(RobotsCacheEntry::AllowAll),
    );

    assert!(fetcher
        .is_allowed("http://dead.example/anything", "dead.example")
        .await
        .allows());
}

#[test]
fn test_robots_txt_empty_disallow_all() {
    let robots_body = "User-agent: *\nDisallow: /\n";
    let mut matcher = DefaultMatcher::default();
    assert!(
        !matcher.one_agent_allowed_by_robots(robots_body, "*", "https://example.com/anything"),
        "Disallow: / should block everything"
    );
}

#[test]
fn test_robots_txt_empty_permissive() {
    let robots_body = "User-agent: *\n";
    let mut matcher = DefaultMatcher::default();
    assert!(
        matcher.one_agent_allowed_by_robots(robots_body, "*", "https://example.com/anything"),
        "Empty robots.txt (no Disallow) should allow everything"
    );
}
