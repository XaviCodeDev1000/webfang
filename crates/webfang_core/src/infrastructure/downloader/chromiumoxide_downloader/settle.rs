//! E2E tests that launch a real headless Chrome (#1277, #1129). Split from
//! tests.rs by the 500-line hard limit of the file-size ratchet.

use super::*;

/// F-52-b E2E (#1277): settle contract against loopback fixtures.
///
/// Two hydration mechanisms, two waits: timer-driven mutation (d200)
/// is captured by a sufficient fixed floor; network-driven hydration
/// (dnet, delayed /api/data round-trip) is captured by network-idle.
/// `None` captures immediately (historical behavior) — asserted for
/// success only, never for content, because CDP latency past the load
/// event is inherently racy. Skips gracefully where no Chrome binary
/// exists (bare CI).
#[cfg(feature = "chromium")]
#[cfg_attr(
    miri,
    ignore = "chromiumoxide spawns a headless Chrome process → posix_spawnattr_init unsupported by Miri (#775)"
)]
#[tokio::test]
async fn settle_modes_capture_delayed_mutation() {
    if !chrome_present_for_e2e() {
        eprintln!("skipping settle E2E: no Chrome binary on PATH");
        return;
    }
    let server = wiremock::MockServer::start().await;
    macro_rules! mount_page {
        ($name:literal, $fixture:literal) => {
            wiremock::Mock::given(wiremock::matchers::method("GET"))
                .and(wiremock::matchers::path($name))
                .respond_with(
                    wiremock::ResponseTemplate::new(200)
                        .set_body_raw(include_str!($fixture), "text/html"),
                )
                .mount(&server)
                .await;
        };
    }
    mount_page!(
        "d200",
        "../../../../../../evidence/mode-d/fixtures/f52/d200.html"
    );
    mount_page!(
        "d1200",
        "../../../../../../evidence/mode-d/fixtures/f52/d1200.html"
    );
    mount_page!(
        "dnet",
        "../../../../../../evidence/mode-d/fixtures/f52/dnet.html"
    );
    // The API round-trip responds after a server-side delay, keeping
    // the request in flight long past any CDP round-trip latency.
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("api/data"))
        .respond_with(
            wiremock::ResponseTemplate::new(200)
                .set_body_raw(
                    "DNETM RENDERED MARKER content fetched over the network after load.",
                    "text/plain",
                )
                .set_delay(std::time::Duration::from_millis(800)),
        )
        .mount(&server)
        .await;
    let server_uri = server.uri();
    async fn fetch_page(server_uri: &str, mode: PostLoadWait, path: &str) -> FetchedPage {
        let dl = ChromiumoxideDownloader::new(
            Arc::new(RwLock::new(CookieBridge::new())),
            mode,
            30,
            None,
        );
        let url: Url = format!("{server_uri}{path}").parse().expect("wiremock uri");
        dl.fetch(&url).await.expect("loopback fetch must succeed")
    }
    // Mutation signal: the placeholder is gone from the live DOM once
    // hydration replaces the slot. (The marker *string* also lives
    // inside the page's own <script>, so asserting on it would pass
    // vacuously — the placeholder is the honest signal.)
    let placeholder = "PLACEHOLDER_INNER_HTML_STATIC_TEXT";
    // Timer mechanism: a sufficient fixed floor captures; the floor
    // does not stretch to later mutations.
    let page = fetch_page(&server_uri, PostLoadWait::Fixed(400), "/d200").await;
    assert!(
        !page.html.contains(placeholder),
        "fixed(400) must capture the 200ms timer mutation"
    );
    let page = fetch_page(&server_uri, PostLoadWait::Fixed(400), "/d1200").await;
    assert!(
        page.html.contains(placeholder),
        "fixed(400) must not capture the 1200ms timer mutation"
    );
    // Network mechanism: idle observes the round-trip; immediate
    // capture races it (800ms server delay makes the outcome stable).
    let page = fetch_page(&server_uri, PostLoadWait::Idle, "/dnet").await;
    assert!(
        !page.html.contains(placeholder),
        "idle must capture the network-driven hydration"
    );
    let page = fetch_page(&server_uri, PostLoadWait::None, "/dnet").await;
    assert!(
        page.html.contains(placeholder),
        "none must capture before the 800ms network round-trip lands"
    );
    // Historical immediacy: `None` succeeds; content intentionally
    // unasserted (CDP latency past load is racy by nature).
    fetch_page(&server_uri, PostLoadWait::None, "/d200").await;
}

/// Best-effort Chrome presence probe so the E2E skips (rather than
/// fails) on machines without a browser. Test-only; the production
/// gate owns real resolution (preflight + #1278).
#[cfg(feature = "chromium")]
fn chrome_present_for_e2e() -> bool {
    [
        "google-chrome",
        "google-chrome-stable",
        "chromium",
        "chromium-browser",
    ]
    .iter()
    .any(|binary| {
        std::process::Command::new(binary)
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    })
}

/// The guard must abort the pump task when the fetch scope exits without
/// `join` (error `?` or async cancellation, #1129). Proved by observing
/// the spawned future's `Drop`: an aborted task drops its future.
#[tokio::test]
#[cfg(feature = "chromium")]
async fn handler_guard_aborts_pump_on_early_exit() {
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };

    struct DropFlag(Arc<AtomicBool>);
    impl Drop for DropFlag {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    let future_dropped = Arc::new(AtomicBool::new(false));
    let flag = DropFlag(future_dropped.clone());
    let job = tokio::spawn(async move {
        let _flag = flag;
        std::future::pending::<()>().await;
    });
    drop(HandlerGuard(Some(job)));

    timeout(Duration::from_secs(5), async {
        while !future_dropped.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("aborted pump task must drop its future promptly");
}
