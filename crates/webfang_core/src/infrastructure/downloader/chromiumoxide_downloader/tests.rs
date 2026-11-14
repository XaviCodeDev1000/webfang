//! #2235: the `--skip chromiumoxide_downloader` of the Miri lane is GONE —
//! a submodule skip deleted this whole test tree from the lane and hid the UB
//! living inside it. The stub build (no `chromium` feature) is pure Rust and
//! runs under Miri; the test that spawns a headless Chrome carries its own
//! gate (Command::spawn is unsupported by Miri).
//!
//! Split out of chromiumoxide_downloader.rs by the file-size ratchet (#2793 PR
//! fix): the file was grandfathered at 1061 lines and could not grow.

#[allow(unused_imports)]
use super::*;

#[cfg(not(feature = "chromium"))]
#[tokio::test]
async fn test_chromiumoxide_returns_stub_error() {
    let dl = ChromiumoxideDownloader::new(
        Arc::new(RwLock::new(CookieBridge::new())),
        PostLoadWait::None,
        30,
        None,
    );
    let url: Url = "https://example.com".parse().unwrap();
    let err = dl.fetch(&url).await.unwrap_err();
    assert!(
        matches!(err, DownloadError::FeatureGated(ref msg) if msg.contains("not enabled")),
        "expected FeatureGated stub error (PermanentFatal, no retry), got: {err}"
    );
}

#[test]
#[cfg(feature = "chromium")]
fn test_chromiumoxide_metadata() {
    let dl = ChromiumoxideDownloader::new(
        Arc::new(RwLock::new(CookieBridge::new())),
        PostLoadWait::None,
        30,
        None,
    );
    assert!(dl.supports_interactions());
    assert_eq!(dl.memory_cost(), 200_000_000);
}

#[test]
#[cfg(feature = "chromium")]
fn launch_bound_is_explicit_not_upstream_default() {
    assert!(CHROME_LAUNCH_TIMEOUT > std::time::Duration::from_secs(20));
    assert!(CHROME_LAUNCH_TIMEOUT <= std::time::Duration::from_secs(120));
}

/// #1367 regression kit.
///
/// Two layers:
///
/// 1. [`InFlightTracker`] unit tests — pure request-id pairing, fully
///    deterministic, pinning the cross-stream reorder that made the old
///    scalar counter leak (finish polled before send) and the
///    closed-settle semantics (redirect dedup, orphan completion,
///    genuine pending request).
/// 2. [`drain_until_quiet`] loop tests over synthetic `IdleStreams` —
///    unbounded channels with the sender halves held alive for the
///    whole test (dropping them closes the stream — that case is
///    pinned by the closed-stream regression below).
#[cfg(feature = "chromium")]
mod drain_tests {
    use super::super::{ChromiumoxideDownloader, IdleStreams, InFlightTracker, PostLoadWait};
    use chromiumoxide::cdp::browser_protocol::network::{
        EventLoadingFailed, EventLoadingFinished, EventRequestWillBeSent, RequestId, ResourceType,
    };
    use chromiumoxide::listeners::EventStream;
    use futures::channel::mpsc;
    use futures::SinkExt;
    use std::sync::Arc;

    type EventTx = mpsc::UnboundedSender<Arc<dyn chromiumoxide::cdp::Event>>;

    /// All three sender halves; every drain-loop test holds this alive
    /// until the drain is done, so open channels never masquerade as
    /// closed streams.
    struct Senders {
        sent: EventTx,
        finished: EventTx,
        failed: EventTx,
    }

    /// Minimal `EventRequestWillBeSent` with the given request id — only
    /// `request_id` is read by the tracker; the tail is inert filler.
    fn sent_event(rid: &str) -> EventRequestWillBeSent {
        EventRequestWillBeSent {
            request_id: RequestId::new(rid),
            loader_id: Default::default(),
            document_url: "http://127.0.0.1/doc".to_string(),
            request: chromiumoxide::cdp::browser_protocol::network::Request::builder()
                .url("http://127.0.0.1/doc")
                .method("GET")
                .headers(chromiumoxide::cdp::browser_protocol::network::Headers::new(
                    serde_json::json!({}),
                ))
                .initial_priority(
                    chromiumoxide::cdp::browser_protocol::network::ResourcePriority::VeryHigh,
                )
                .referrer_policy(
                    chromiumoxide::cdp::browser_protocol::network::RequestReferrerPolicy::StrictOriginWhenCrossOrigin,
                )
                .build()
                .expect("builder tail is complete"),
            timestamp: Default::default(),
            wall_time: Default::default(),
            initiator: chromiumoxide::cdp::browser_protocol::network::Initiator::new(
                chromiumoxide::cdp::browser_protocol::network::InitiatorType::Other,
            ),
            redirect_has_extra_info: false,
            redirect_response: None,
            r#type: None,
            frame_id: None,
            has_user_gesture: None,
        }
    }

    fn finished_event(rid: &str) -> EventLoadingFinished {
        EventLoadingFinished {
            request_id: RequestId::new(rid),
            timestamp: Default::default(),
            encoded_data_length: 0.0,
        }
    }

    fn failed_event(rid: &str) -> EventLoadingFailed {
        EventLoadingFailed {
            request_id: RequestId::new(rid),
            timestamp: Default::default(),
            r#type: ResourceType::Other,
            error_text: "net::ERR_TEST".to_string(),
            canceled: None,
            blocked_reason: None,
            cors_error_status: None,
        }
    }

    /// Wire three fresh unbounded channels into `IdleStreams` (same
    /// wiring `subscribe_idle_streams` produces via `event_listener`).
    fn synthetic_streams() -> (Senders, IdleStreams) {
        let (sent_tx, sent_rx) = mpsc::unbounded();
        let (finished_tx, finished_rx) = mpsc::unbounded();
        let (failed_tx, failed_rx) = mpsc::unbounded();
        let senders = Senders {
            sent: sent_tx,
            finished: finished_tx,
            failed: failed_tx,
        };
        let streams = IdleStreams {
            will_be_sent: EventStream::new(sent_rx),
            loading_finished: EventStream::new(finished_rx),
            loading_failed: EventStream::new(failed_rx),
        };
        (senders, streams)
    }

    // ---- InFlightTracker: pure request-id pairing (deterministic) --

    /// #1367 core reorder: `loadingFinished` paired before its
    /// `requestWillBeSent`. The old scalar counter discarded the orphan
    /// completion (`in_flight == 0 → continue`) and then leaked the
    /// later `sent` — the drain never went quiet and the settle burned
    /// its 30s ceiling. The tracker nets the pair to zero in either
    /// arrival order.
    #[test]
    fn tracker_finish_before_send_nets_to_quiet() {
        let mut tracker = InFlightTracker::default();
        tracker.on_completion(RequestId::new("nav-doc")); // polled FIRST
        assert!(tracker.is_quiet()); // orphan completion is not pending
        tracker.on_sent(RequestId::new("nav-doc")); // the late send
        assert!(
            tracker.is_quiet(),
            "#1367: finish-before-send must not leave a phantom pending entry"
        );
    }

    /// The causal order must pair too (the tracker is not simply
    /// "always quiet").
    #[test]
    fn tracker_send_then_finish_pairs() {
        let mut tracker = InFlightTracker::default();
        tracker.on_sent(RequestId::new("nav-doc"));
        assert!(
            !tracker.is_quiet(),
            "a request between sent and finished is pending"
        );
        tracker.on_completion(RequestId::new("nav-doc"));
        assert!(tracker.is_quiet());
    }

    /// Redirect re-send: CDP reuses the `requestId` across a redirect
    /// chain — `sent(rid)` may arrive twice for one eventual
    /// completion. Set dedup keeps the pair balanced.
    #[test]
    fn tracker_deduplicates_redirect_re_sends() {
        let mut tracker = InFlightTracker::default();
        tracker.on_sent(RequestId::new("redirect-chain"));
        tracker.on_sent(RequestId::new("redirect-chain"));
        tracker.on_completion(RequestId::new("redirect-chain"));
        assert!(
            tracker.is_quiet(),
            "redirect re-send must deduplicate to one pending slot"
        );
    }

    /// Failed loads complete requests too: `loadingFailed` balances
    /// `sent` exactly like `loadingFinished` does.
    #[test]
    fn tracker_failed_balances_sent() {
        let mut tracker = InFlightTracker::default();
        tracker.on_sent(RequestId::new("blocked-request"));
        tracker.on_completion(RequestId::new("blocked-request"));
        assert!(tracker.is_quiet());
    }

    /// Pre-subscription completions (favicon style, observed in the
    /// #1367 traces): a completion whose send was never observed does
    /// NOT block quiet — the request already finished.
    #[test]
    fn tracker_orphan_completion_does_not_block_quiet() {
        let mut tracker = InFlightTracker::default();
        tracker.on_completion(RequestId::new("favicon-before-enable"));
        assert!(tracker.is_quiet());
    }

    // ---- drain_until_quiet: loop behavior over synthetic streams ---

    /// The production shape: all lifecycle events already buffered when
    /// the settle starts polling. Whatever order `select!` polls the
    /// three streams in, every send/finish pair must net and the drain
    /// must return after one quiet window.
    #[tokio::test]
    async fn drain_reaches_quiet_with_buffered_pairs() {
        let (mut tx, streams) = synthetic_streams();
        tx.finished
            .send(Arc::new(finished_event("nav-doc")) as Arc<dyn chromiumoxide::cdp::Event>)
            .await
            .expect("finished channel alive");
        tx.sent
            .send(Arc::new(sent_event("nav-doc")) as Arc<dyn chromiumoxide::cdp::Event>)
            .await
            .expect("sent channel alive");
        tx.sent
            .send(Arc::new(sent_event("favicon")) as Arc<dyn chromiumoxide::cdp::Event>)
            .await
            .expect("sent channel alive");
        tx.failed
            .send(Arc::new(failed_event("favicon")) as Arc<dyn chromiumoxide::cdp::Event>)
            .await
            .expect("failed channel alive");
        let outcome = tokio::time::timeout(
            std::time::Duration::from_secs(3),
            ChromiumoxideDownloader::drain_until_quiet(streams, PostLoadWait::IDLE_WINDOW),
        )
        .await;
        assert!(
            matches!(outcome, Ok(())),
            "buffered send/finish pairs must net to quiet, got {outcome:?}"
        );
    }

    /// A genuinely pending request (sent, no completion — a slow XHR)
    /// keeps the drain awake past the quiet window. Sanity: the fix
    /// did not turn the drain into an immediate return.
    #[tokio::test]
    async fn pending_request_keeps_drain_awake() {
        let (mut tx, streams) = synthetic_streams();
        tx.sent
            .send(Arc::new(sent_event("still-loading")) as Arc<dyn chromiumoxide::cdp::Event>)
            .await
            .expect("sent channel alive");
        let outcome = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            ChromiumoxideDownloader::drain_until_quiet(
                streams,
                std::time::Duration::from_millis(50),
            ),
        )
        .await;
        assert!(
            outcome.is_err(),
            "an unfinished request must keep the drain awake past the quiet window"
        );
    }

    /// Livelock regression: when the browser dies mid-settle the CDP
    /// pump drops the event senders and the streams close. Without the
    /// branch guards every poll of a closed branch returns
    /// `Ready(None)`, `select!` completes it instantly forever, the
    /// quiet-window sleep never elapses, and the settle ceiling cannot
    /// observe its deadline — the fetch hangs instead of bounding.
    /// With the guards the closed branches disable, the window fires,
    /// and the drain returns quiet.
    #[tokio::test]
    async fn closed_streams_do_not_livelock_the_drain() {
        let (tx, streams) = synthetic_streams();
        drop(tx); // all three senders gone: every stream is closed.
        let outcome = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            ChromiumoxideDownloader::drain_until_quiet(
                streams,
                std::time::Duration::from_millis(50),
            ),
        )
        .await;
        assert!(
            matches!(outcome, Ok(())),
            "closed streams must reach quiet, not livelock the select loop, got {outcome:?}"
        );
    }
}
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

#[test]
#[cfg(not(feature = "chromium"))]
fn test_chromiumoxide_metadata_stub() {
    let dl = ChromiumoxideDownloader::new(
        Arc::new(RwLock::new(CookieBridge::new())),
        PostLoadWait::None,
        30,
        None,
    );
    assert!(!dl.supports_interactions());
    assert_eq!(dl.memory_cost(), 0);
}

/// Kills the cargo-mutants body-replacement survivor of `fetch` (#2793 PR
/// gate): the mutants shard runs without Chrome, so the E2E gate skips and a
/// mutant replacing the whole fetch body with `Ok(Default::default())`
/// survived. This exercises the scheme guard, the first fetch behavior that
/// runs before any Chrome launch, with no Chrome dependency.
#[tokio::test]
#[cfg(feature = "chromium")]
async fn fetch_rejects_non_http_scheme_before_launching_chrome() {
    let dl = ChromiumoxideDownloader::new(
        Arc::new(RwLock::new(CookieBridge::new())),
        PostLoadWait::None,
        30,
        None,
    );
    let url: Url = "ftp://example.com/file".parse().unwrap();
    let err = dl.fetch(&url).await.unwrap_err();
    assert!(
        matches!(err, DownloadError::InvalidUrl(ref msg) if msg.contains("unsupported scheme: ftp")),
        "expected InvalidUrl for a non-http scheme, got: {err}"
    );
}
