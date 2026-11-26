use super::*;

// ---- Test doubles --------------------------------------------------

struct StubDownloader {
    html: String,
    cost: usize,
    interactions: bool,
    /// When set, `fetch` returns this error instead of the stubbed page.
    /// Lets one `StubDownloader` stand in for both content and failure
    /// layers (e.g. a Layer-2 WAF challenge) without a second test double.
    /// `DownloadError` is `Clone` (see `mod.rs`), so the error is copied out
    /// of `&self` without moving.
    fail_with: Option<DownloadError>,
}

impl StubDownloader {
    fn static_page() -> Self {
        Self {
            html: "<html><body><article><h1>Hello</h1><p>Enough content here to pass the threshold check and avoid SPA detection.</p></article></body></html>".into(),
            cost: 1_000_000,
            interactions: false,
            fail_with: None,
        }
    }

    fn spa_page() -> Self {
        Self {
            html: r#"<!DOCTYPE html><html><body><div id="root"></div></body></html>"#.into(),
            cost: 1_000_000,
            interactions: false,
            fail_with: None,
        }
    }

    /// #758: a FAT JS shell — thousands of raw HTML bytes (heavy script
    /// payload) with near-zero visible text and no known mount-point
    /// marker. The old raw-byte heuristic classified this as static
    /// content; the visible-text gate must escalate it (the
    /// quotes.toscrape.com/js/ case).
    fn fat_shell_page() -> Self {
        let fat_script = "var x = 1;".repeat(600);
        Self {
            html: format!(
                "<!DOCTYPE html><html><head><title>JS App</title>\
                 <script>{fat_script}</script></head><body></body></html>"
            ),
            cost: 1_000_000,
            interactions: false,
            fail_with: None,
        }
    }

    fn empty_page() -> Self {
        Self {
            html: String::new(),
            cost: 1_000_000,
            interactions: false,
            fail_with: None,
        }
    }

    fn waf_page() -> Self {
        Self {
            html: r#"<!DOCTYPE html><html><body><div id="challenge-running">Checking your browser</div></body></html>"#.into(),
            cost: 1_000_000,
            interactions: false,
            fail_with: None,
        }
    }

    fn with_cost(mut self, cost: usize) -> Self {
        self.cost = cost;
        self
    }

    fn with_interactions(mut self, v: bool) -> Self {
        self.interactions = v;
        self
    }

    /// Make this stub fail with the given error (used to simulate a
    /// downloader layer surfacing a Network / Http / WafChallenge error).
    fn fails_with(mut self, error: DownloadError) -> Self {
        self.fail_with = Some(error);
        self
    }
}

impl Downloader for StubDownloader {
    fn fetch<'a>(&'a self, url: &'a Url) -> BoxFuture<'a, Result<FetchedPage, DownloadError>> {
        if let Some(err) = &self.fail_with {
            let err = err.clone();
            return Box::pin(async move { Err(err) });
        }
        Box::pin(async move {
            Ok(FetchedPage {
                url: url.clone(),
                html: self.html.clone(),
                status: 200,
                headers: std::collections::HashMap::new(),
                cookies: vec![],
            })
        })
    }

    fn supports_interactions(&self) -> bool {
        self.interactions
    }

    fn memory_cost(&self) -> usize {
        self.cost
    }
}

// ---- Tests ---------------------------------------------------------

#[tokio::test]
async fn test_layer1_sufficient_no_escalation() {
    let router = HybridRouter::new(
        StubDownloader::static_page(),
        StubDownloader::spa_page(),
        StubDownloader::static_page().with_interactions(true),
        false,
        CancellationToken::new(),
    );
    let url: Url = "https://example.com".parse().unwrap();
    let page = router.fetch(&url).await.unwrap();
    assert!(page.html.contains("Hello"));
}

/// Exercise the router against `url` and assert the surfaced error is a
/// WAF challenge. Collapses the repeated `fetch().await.unwrap_err()` +
/// `matches!(..WafChallenge)` tail shared by every "abort" scenario so the
/// test bodies are not exact clones (jscpd `--min-tokens 50`).
async fn assert_waf_aborts(
    router: &HybridRouter<StubDownloader, StubDownloader, StubDownloader>,
    url: &str,
) {
    let url: Url = url.parse().unwrap();
    let err = router.fetch(&url).await.unwrap_err();
    assert!(
        matches!(err, DownloadError::WafChallenge(_)),
        "router must abort on a WAF challenge, got: {err}"
    );
}

/// Exercise the router against `url` and assert the final page carries the
/// L3 static content marker. Shared by every "escalates to Layer 3" scenario.
async fn assert_escalates_to_layer3(
    router: &HybridRouter<StubDownloader, StubDownloader, StubDownloader>,
    url: &str,
) {
    let url: Url = url.parse().unwrap();
    let page = router.fetch(&url).await.unwrap();
    assert!(page.html.contains("Enough content"));
}

#[tokio::test]
async fn test_spa_detected_escalates_to_layer2() {
    let router = HybridRouter::new(
        StubDownloader::spa_page(),
        StubDownloader::static_page().with_cost(30_000_000),
        StubDownloader::static_page().with_interactions(true),
        false,
        CancellationToken::new(),
    );
    assert_escalates_to_layer3(&router, "https://spa.example.com").await;
}

/// #758 regression: a fat JS shell (large raw HTML, near-zero visible
/// text, no mount-point marker) must escalate to Layer 2 instead of
/// being accepted as static content.
#[tokio::test]
async fn test_fat_shell_escalates_to_layer2() {
    let router = HybridRouter::new(
        StubDownloader::fat_shell_page(),
        StubDownloader::static_page().with_cost(30_000_000),
        StubDownloader::static_page().with_interactions(true),
        false,
        CancellationToken::new(),
    );
    assert_escalates_to_layer3(&router, "https://js.example.com").await;
}

#[tokio::test]
async fn test_waf_at_layer1_aborts() {
    let router = HybridRouter::new(
        StubDownloader::waf_page(),
        StubDownloader::static_page(),
        StubDownloader::static_page(),
        false,
        CancellationToken::new(),
    );
    assert_waf_aborts(&router, "https://waf.example.com").await;
}

#[tokio::test]
async fn test_waf_at_layer2_obscura_aborts() {
    // Fase 4 scenario 7: a WAF challenge surfaced by Layer 2 (Obscura) must
    // short-circuit escalation. Layer 1 returns usable static content (so
    // the L1 spa path does NOT abort), then Obscura returns
    // DownloadError::WafChallenge — the router must abort with that error
    // and NEVER reach Layer 3 (hybrid_router.rs:120-122).
    let router = HybridRouter::new(
        StubDownloader::spa_page(),
        StubDownloader::spa_page().fails_with(DownloadError::WafChallenge(
            "obscura hit a WAF challenge".into(),
        )),
        StubDownloader::static_page().with_interactions(true),
        false,
        CancellationToken::new(),
    );
    assert_waf_aborts(&router, "https://obscura-waf.example.com").await;
    let url: Url = "https://obscura-waf.example.com".parse().unwrap();
    let err = router.fetch(&url).await.unwrap_err();
    assert!(
        err.to_string().contains("obscura hit a WAF challenge"),
        "abort must carry Obscura's WAF cause, got: {err}"
    );
}

#[tokio::test]
async fn test_ignore_waf_true_does_not_suppress_layer2_waf() {
    // Pin the boundary of REQ-WAF-07: `ignore_waf` only mutates the
    // spa-detection verdict (detect_spa). An explicit WafChallenge returned
    // by the Obscura layer is NOT suppressed by ignore_waf — the router
    // still aborts (hybrid_router.rs:120-122). This proves scenario 8
    // (ignore_waf no-abort) is scoped to the L1 spa path, not the L2
    // explicit-challenge path.
    let router = HybridRouter::new(
        StubDownloader::spa_page(),
        StubDownloader::spa_page().fails_with(DownloadError::WafChallenge(
            "obscura WAF under ignore_waf".into(),
        )),
        StubDownloader::static_page().with_interactions(true),
        true,
        CancellationToken::new(),
    );
    assert_waf_aborts(&router, "https://obscura-waf.example.com").await;
    let url: Url = "https://obscura-waf.example.com".parse().unwrap();
    let err = router.fetch(&url).await.unwrap_err();
    assert!(err.to_string().contains("obscura WAF under ignore_waf"));
}

#[tokio::test]
async fn test_ignore_waf_false_t1_challenge_aborts() {
    // Mirror pinning current behavior: with ignore_waf=false a genuine
    // T1 challenge still aborts via the spa path (REQ-WAF-07).
    let router = HybridRouter::new(
        StubDownloader::waf_page(),
        StubDownloader::static_page(),
        StubDownloader::static_page(),
        false,
        CancellationToken::new(),
    );
    assert_waf_aborts(&router, "https://waf.example.com").await;
}

#[tokio::test]
async fn test_ignore_waf_true_t1_challenge_does_not_abort() {
    // REQ-WAF-07 (W1): with ignore_waf=true the spa path must NOT abort on
    // a genuine T1 challenge — the WAF classification yields a clean verdict
    // and the page is treated per normal spa/static logic. The challenge
    // stub carries near-zero visible text, so the #758 text gate escalates
    // it to Layer 2 instead of accepting it as static; the invariant under
    // test is that the fetch never aborts with a WafChallenge error.
    let router = HybridRouter::new(
        StubDownloader::waf_page(),
        StubDownloader::static_page(),
        StubDownloader::static_page(),
        true,
        CancellationToken::new(),
    );
    let url: Url = "https://waf.example.com".parse().unwrap();
    let page = router
        .fetch(&url)
        .await
        .expect("ignore_waf=true must not abort on a T1 challenge");
    assert!(
        page.html.contains("Enough content"),
        "the text-poor challenge page must escalate to Layer 2"
    );
}

#[tokio::test]
async fn test_layer2_empty_escalates_to_layer3() {
    let router = HybridRouter::new(
        StubDownloader::spa_page(),
        StubDownloader::empty_page(),
        StubDownloader::static_page().with_interactions(true),
        false,
        CancellationToken::new(),
    );
    assert_escalates_to_layer3(&router, "https://spa.example.com").await;
}

/// S17: `check_resources` returning `Ok(0)` is a DENIAL. With the only permit
/// already held, the router must refuse the escalation before the L2 layer
/// runs instead of acquiring (and blocking on) a permit the governor did not
/// grant.
#[tokio::test]
async fn exhausted_governor_denies_escalation_to_layer2() {
    use super::super::resource_governor::ResourceGovernor;

    let governor = ResourceGovernor::with_max_instances(1, CancellationToken::new());
    let held = governor.acquire().await.expect("the single permit is free");

    let router = HybridRouter::with_governor(
        StubDownloader::spa_page(),
        StubDownloader::static_page(),
        StubDownloader::static_page().with_interactions(true),
        false,
        governor,
    );

    let url: Url = "https://spa.example.com".parse().unwrap();
    let err = router
        .fetch(&url)
        .await
        .expect_err("escalation must be denied with zero permits");
    assert!(
        err.to_string().contains("no escalation permits available"),
        "expected the resource denial, got: {err}"
    );
    drop(held);
}

/// S5 (#2722 H3): a WAF challenge that obscura RENDERS (its response body
/// carries the challenge markup) must abort the escalation exactly like an
/// explicit L2 error — the router must classify the L2 body, not accept
/// any non-empty page. Layer 3 fails loudly if reached, so the abort can
/// only have come from the L2 verdict.
#[tokio::test]
async fn test_waf_body_rendered_by_layer2_aborts() {
    let router = HybridRouter::new(
        StubDownloader::spa_page(),
        StubDownloader::waf_page(),
        StubDownloader::static_page()
            .with_interactions(true)
            .fails_with(DownloadError::Network(Box::new(std::io::Error::new(
                std::io::ErrorKind::ConnectionRefused,
                "L3 must never be reached",
            )))),
        false,
        CancellationToken::new(),
    );
    assert_waf_aborts(&router, "https://obscura-rendered-waf.example.com").await;
}

/// S5 (#2722 H3): an L2 body that STILL looks like a SPA (the mount point
/// survived obscura's render) must escalate to Layer 3 instead of being
/// accepted for its non-emptiness.
#[tokio::test]
async fn test_layer2_spa_body_escalates_to_layer3() {
    let router = HybridRouter::new(
        StubDownloader::spa_page(),
        StubDownloader::spa_page(),
        StubDownloader::static_page().with_interactions(true),
        false,
        CancellationToken::new(),
    );
    assert_escalates_to_layer3(&router, "https://spa-twice.example.com").await;
}

#[tokio::test]
async fn test_layer2_non_waf_error_escalates_to_layer3() {
    // Gap #511-1: Layer 2 failing with a non-WAF DownloadError (here: HTTP
    // 500, a distinct variant from L1/L3 Network errors) must NOT abort —
    // the router escalates to Layer 3 (hybrid_router.rs:125-127).
    let router = HybridRouter::new(
        StubDownloader::spa_page(),
        StubDownloader::spa_page().fails_with(DownloadError::Http {
            status: 500,
            message: "obscura subprocess failed".into(),
        }),
        StubDownloader::static_page().with_interactions(true),
        false,
        CancellationToken::new(),
    );
    assert_escalates_to_layer3(&router, "https://spa.example.com").await;
}

#[tokio::test]
async fn test_all_layers_exhausted_returns_final_error() {
    // Gap #511-2: when all three layers fail, the router returns the LAST
    // layer's error (hybrid_router.rs:148-151). L2 fails with Http{500}
    // and L3 with Network("chromium crashed"); asserting the Network
    // variant + message proves the surfaced error is L3's, not L2's.
    let router = HybridRouter::new(
        StubDownloader::spa_page(),
        StubDownloader::spa_page().fails_with(DownloadError::Http {
            status: 500,
            message: "obscura subprocess failed".into(),
        }),
        StubDownloader::spa_page().fails_with(DownloadError::Network(Box::new(
            std::io::Error::new(std::io::ErrorKind::ConnectionRefused, "chromium crashed"),
        ))),
        false,
        CancellationToken::new(),
    );
    let url: Url = "https://spa.example.com".parse().unwrap();
    let err = router.fetch(&url).await.unwrap_err();
    assert!(matches!(err, DownloadError::Network(_)));
    assert!(
        err.to_string().contains("chromium crashed"),
        "final error must carry Layer 3's cause, got: {err}"
    );
}

#[tokio::test]
async fn test_layer1_failure_propagates() {
    let router = HybridRouter::new(
        StubDownloader::spa_page().fails_with(DownloadError::Network(Box::new(
            std::io::Error::new(std::io::ErrorKind::ConnectionRefused, "dns failed"),
        ))),
        StubDownloader::static_page(),
        StubDownloader::static_page(),
        false,
        CancellationToken::new(),
    );
    let url: Url = "https://down.example.com".parse().unwrap();
    let err = router.fetch(&url).await.unwrap_err();
    assert!(matches!(err, DownloadError::Network(_)));
}

#[test]
fn test_hybrid_router_memory_cost_sums() {
    let router = HybridRouter::new(
        StubDownloader::static_page().with_cost(1_000_000),
        StubDownloader::static_page().with_cost(30_000_000),
        StubDownloader::static_page()
            .with_cost(200_000_000)
            .with_interactions(true),
        false,
        CancellationToken::new(),
    );
    assert_eq!(router.memory_cost(), 231_000_000);
}

#[test]
fn test_hybrid_router_supports_interactions_from_layer3() {
    let router = HybridRouter::new(
        StubDownloader::static_page(),
        StubDownloader::static_page(),
        StubDownloader::static_page().with_interactions(true),
        false,
        CancellationToken::new(),
    );
    assert!(router.supports_interactions());

    let router = HybridRouter::new(
        StubDownloader::static_page(),
        StubDownloader::static_page(),
        StubDownloader::static_page(),
        false,
        CancellationToken::new(),
    );
    assert!(!router.supports_interactions());
}

// ---- #1009: cancel-token wiring ------------------------------------
//
// The plan budgets ~30L for the cancel-token fix. The strongest
// observable property we can assert without exposing the private
// `governor` field is the wiring contract: a HybridRouter built with a
// pre-cancelled token must NOT hang — its embedded governor listens to
// the same token via `with_cancel_token` (parity with the Full
// strategy, see #509), and the semantic guarantee of that builder is
// that future permit awaits unblock immediately. The non-hang check is
// the cheapest verifiable slice; the deeper "acquire().await returns
// Cancelled" assertion already lives next to the governor's own
// `with_cancel_token` definition (`resource_governor.rs::acquire_returns_cancelled_when_blocked_and_token_fires`).
#[tokio::test]
async fn test_hybrid_router_with_cancelled_token_does_not_hang() {
    use std::time::Duration;

    let cancel = CancellationToken::new();
    cancel.cancel();

    let router = HybridRouter::new(
        StubDownloader::spa_page(),
        StubDownloader::static_page(),
        StubDownloader::static_page().with_interactions(true),
        false,
        cancel,
    );

    let url: Url = "https://example.com".parse().unwrap();
    // The L1 stub returns SPA content, so escalation is attempted. We
    // don't assert a specific variant — the invariant is that the
    // router terminates within a tight bound instead of hanging on an
    // uncanceled semaphore wait.
    let result = tokio::time::timeout(Duration::from_secs(2), router.fetch(&url)).await;
    assert!(
        result.is_ok(),
        "HybridRouter with pre-cancelled token must not hang"
    );
}
