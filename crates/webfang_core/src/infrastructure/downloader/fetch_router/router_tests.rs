use super::*;
use crate::domain::JsStrategy;
use std::path::PathBuf;

fn test_cookie_bridge() -> Arc<RwLock<CookieBridge>> {
    Arc::new(RwLock::new(CookieBridge::new()))
}

#[test]
fn build_fetch_router_static_returns_static_variant() {
    let router = build_fetch_router(
        &JsStrategy::Static,
        30,
        Profile::Chrome145,
        test_cookie_bridge(),
        false,
        None,
        Vec::new(),
        None,
        None,
        CancellationToken::new(),
        3,
        1000,
        10000,
        "obscura",
        50_000_000,
        // F-52-b: variant-selection tests settle immediately.
        PostLoadWait::None,
        // F-52-c: variant-selection tests need no pinned binary.
        None,
    )
    .expect("static router must build");
    assert!(
        matches!(router, FetchRouter::Static(_)),
        "Static strategy must produce FetchRouter::Static"
    );
}

#[test]
fn build_fetch_router_hybrid_returns_hybrid_variant() {
    let router = build_fetch_router(
        &JsStrategy::Hybrid,
        30,
        Profile::Chrome145,
        test_cookie_bridge(),
        false,
        None,
        Vec::new(),
        None,
        None,
        CancellationToken::new(),
        3,
        1000,
        10000,
        "obscura",
        50_000_000,
        // F-52-b: variant-selection tests settle immediately.
        PostLoadWait::None,
        // F-52-c: variant-selection tests need no pinned binary.
        None,
    )
    .expect("hybrid router must build");
    assert!(
        matches!(router, FetchRouter::Hybrid(_)),
        "Hybrid strategy must produce FetchRouter::Hybrid"
    );
}

#[test]
fn build_fetch_router_full_returns_full_variant() {
    let router = build_fetch_router(
        &JsStrategy::Full,
        30,
        Profile::Chrome145,
        test_cookie_bridge(),
        false,
        None,
        Vec::new(),
        None,
        None,
        CancellationToken::new(),
        3,
        1000,
        10000,
        "obscura",
        50_000_000,
        // F-52-b: variant-selection tests settle immediately.
        PostLoadWait::None,
        // F-52-c: variant-selection tests need no pinned binary.
        None,
    )
    .expect("full router must build");
    assert!(
        matches!(router, FetchRouter::Full(..)),
        "Full strategy must produce FetchRouter::Full (chrome-direct), not Hybrid"
    );
}

/// F-52-c (#1278): the configured Chrome binary must reach the Full
/// launcher verbatim — the seam test proves gate-certified == launched
/// without spawning a browser (mirrors the obscura `.binary()` seam).
#[cfg(feature = "chromium")]
#[test]
fn build_fetch_router_full_passes_configured_chrome_binary() {
    let pinned = PathBuf::from("/opt/google/chrome/chrome");
    let router = build_fetch_router(
        &JsStrategy::Full,
        30,
        Profile::Chrome145,
        test_cookie_bridge(),
        false,
        None,
        Vec::new(),
        None,
        None,
        CancellationToken::new(),
        3,
        1000,
        10000,
        "obscura",
        50_000_000,
        PostLoadWait::None,
        Some(pinned.clone()),
    )
    .expect("full router must build");
    match router {
        FetchRouter::Full(dl, _) => assert_eq!(
            dl.chrome_binary(),
            Some(pinned.as_path()),
            "the Full launcher must carry the configured chrome binary"
        ),
        _other => panic!("expected Full router for the chrome-binary seam"),
    }
}

/// #787: the `--obscura-binary` value must reach the Hybrid Layer 2
/// downloader verbatim — the seam test proves the configured path is
/// passed through instead of being replaced by the bare `obscura` name.
#[test]
fn build_obscura_layer_passes_configured_binary() {
    let layer = build_obscura_layer(30, "/opt/tools/obscura", None, 50_000_000);
    assert_eq!(
        layer.binary(),
        PathBuf::from("/opt/tools/obscura").as_path(),
        "the Hybrid Layer 2 must use the configured --obscura-binary"
    );
}

/// #787: the default bare `obscura` name still resolves to the same
/// default binary (backwards compatibility).
#[test]
fn build_obscura_layer_default_preserved() {
    let layer = build_obscura_layer(30, "obscura", None, 50_000_000);
    assert_eq!(
        layer.binary(),
        PathBuf::from("obscura").as_path(),
        "the default bare name must be preserved on Layer 2"
    );
}

/// S1+S4 (#2722): the operator-pinned User-Agent and the stdout cap must
/// reach the Hybrid Layer 2 downloader through the seam — `None` passes
/// no UA (obscura keeps its own default).
#[test]
fn build_obscura_layer_propagates_user_agent_and_cap() {
    let layer = build_obscura_layer(30, "obscura", Some("OperatorUA/3.1"), 12_345_678);
    assert_eq!(
        layer.user_agent(),
        Some("OperatorUA/3.1"),
        "the pinned User-Agent must reach the Hybrid Layer 2"
    );
    assert_eq!(
        layer.max_page_bytes(),
        12_345_678,
        "the stdout cap must reach the Hybrid Layer 2"
    );
    assert_eq!(
        build_obscura_layer(30, "obscura", None, 50_000_000).user_agent(),
        None,
        "an unpinned UA must not manufacture one on Layer 2"
    );
}

/// F-52-b (#1277): the configured settle mode must reach the Full
/// launcher verbatim — proves propagation without spawning a browser.
#[cfg(feature = "chromium")]
#[test]
fn build_fetch_router_full_passes_configured_wait_mode() {
    let router = build_fetch_router(
        &JsStrategy::Full,
        30,
        Profile::Chrome145,
        test_cookie_bridge(),
        false,
        None,
        Vec::new(),
        None,
        None,
        CancellationToken::new(),
        3,
        1000,
        10000,
        "obscura",
        50_000_000,
        PostLoadWait::Fixed(750),
        None,
    )
    .expect("full router must build");
    match router {
        FetchRouter::Full(dl, _) => assert_eq!(
            dl.post_load_wait(),
            PostLoadWait::Fixed(750),
            "the Full launcher must carry the configured wait mode"
        ),
        _other => panic!("expected Full router for the wait-mode seam"),
    }
}

/// S17: the Full arm must consume the permit COUNT, not only the `Err` case.
/// With the only permit already held, `check_resources` answers `Ok(0)` and the
/// router must refuse the fetch before launching Chrome.
#[tokio::test]
async fn full_router_denies_fetch_when_no_permit_is_available() {
    use crate::domain::downloader_port::Downloader as _;
    use crate::infrastructure::downloader::chromiumoxide_downloader::ChromiumoxideDownloader;
    use crate::infrastructure::downloader::resource_governor::ResourceGovernor;
    use std::sync::Arc;

    let governor = Arc::new(ResourceGovernor::with_max_instances(
        1,
        CancellationToken::new(),
    ));
    let held = governor.acquire().await.expect("the single permit is free");
    let downloader =
        ChromiumoxideDownloader::new(test_cookie_bridge(), PostLoadWait::None, 30, None);
    let router = FetchRouter::Full(Arc::new(downloader), governor);

    let url: url::Url = "https://example.com/page".parse().expect("valid url");
    let err = router
        .fetch(&url)
        .await
        .expect_err("an exhausted governor must deny the fetch");
    assert!(
        err.to_string().contains("no fetch permits available"),
        "expected the resource denial, got: {err}"
    );
    drop(held);
}
