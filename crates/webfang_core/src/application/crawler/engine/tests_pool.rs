use super::tests::{entry_off, pool_session, FakeSessionPort};
use super::*;
use url::Url;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// 3.F wiring + #1291 fix: a pool injected through `CrawlPorts` reaches
/// the spawned workers on the SESSION path — the counters live on this
/// exact instance, so nonzero traffic proves the same `Arc` is shared by
/// every task context derived from the session.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ports_injected_pool_reaches_crawl_workers() {
    let _entry_off = entry_off();
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<html><body>seed</body></html>"))
        .mount(&server)
        .await;
    let seed = Url::parse(&format!("{}/", server.uri())).expect("seed URL");
    let config = CrawlerConfig::builder(seed)
        .max_depth(0)
        .max_pages(10)
        .ignore_robots(true)
        .build();
    let pool = Arc::new(FakeSessionPort::default());

    let mut engine =
        Engine::from_session(pool_session(config, Some(Arc::clone(&pool)), false, None))
            .expect("engine must build from a pooled session");
    let result = engine.run().await.expect("crawl must complete");
    engine.shutdown().await;

    assert_eq!(result.total_pages, 1, "seed must be crawled");
    assert!(
        pool.acquire_count() >= 1,
        "workers must acquire from the ports-injected pool"
    );
    assert!(
        pool.success_count() >= 1,
        "a successful fetch must report back into the ports-injected pool"
    );
}

/// #1291 wiring fix: the `session_pool_enabled` transport flag derives a
/// pool into the SESSION ports — the place `task_ctx` reads. Before this
/// fix the flag-built pool was attached to the engine mirror only, which
/// the session path never read, silently dropping per-domain gating.
#[tokio::test]
async fn session_pool_enabled_flag_installs_pool_into_session_ports() {
    let seed = Url::parse("https://example.com").expect("valid seed URL");
    let config = CrawlerConfig::builder(seed).max_depth(0).build();

    let engine =
        Engine::from_session(pool_session(config, None, true, None)).expect("engine must build");

    let owned = engine
        .session
        .as_ref()
        .expect("pre-run engine owns its session");
    assert!(
        owned.ports.session_pool.is_some(),
        "the session_pool_enabled flag must derive a pool into the session ports"
    );
}

/// 3.F end-to-end: the options flow (`session_pool_enabled: true`) builds
/// the real pool through the composition-root helper
/// `application::container::build_crawl_session_pool` and the crawl still
/// completes — the port-gated fetch path must not block a healthy domain.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn crawl_site_with_options_session_pool_enabled_crawls_through_port() {
    let _entry_off = entry_off();
    let server = MockServer::start().await;
    let port = server.address().port();
    Mock::given(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<html><body>seed</body></html>"))
        .mount(&server)
        .await;

    let seed = Url::parse(&format!("http://127.0.0.1:{port}/")).expect("valid seed URL");
    let config = CrawlerConfig::builder(seed)
        .max_depth(0)
        .max_pages(10)
        .ignore_robots(true)
        .build();
    let options = EngineOptions {
        session_pool_enabled: true,
        ignore_robots: true,
        ..Default::default()
    };

    let result = crawl_site_with_options(config, options, &CorrelationId::new())
        .await
        .expect("crawl with session pool enabled must succeed");
    assert_eq!(
        result.total_pages, 1,
        "seed must be crawled through the port-wired pool"
    );
}

// ——— #1369 deprecated-entry drift guards ———

/// Drift guard for the deprecated `crawl_site` shim: it must keep
/// producing the same result as the explicit seam with the same option
/// set (robots from config, capture off, checkpoint off) until removal.
#[allow(deprecated)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn deprecated_crawl_site_shim_matches_explicit_options_entry() {
    let _entry_off = entry_off();
    let server = MockServer::start().await;
    // Two runs, one per entry; each fetches exactly the seed (max_depth 0,
    // robots ignored via the option the shim derives from config).
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<html><body>seed</body></html>"))
        .expect(2)
        .mount(&server)
        .await;

    let seed = Url::parse(&format!("http://127.0.0.1:{}/", server.address().port()))
        .expect("valid seed URL");
    let config = CrawlerConfig::builder(seed)
        .max_depth(0)
        .max_pages(1)
        .ignore_robots(true)
        .build();

    let shim = crawl_site(config.clone())
        .await
        .expect("deprecated shim must still work");
    let explicit = crawl_site_with_options(
        config,
        EngineOptions {
            ignore_robots: true,
            ..Default::default()
        },
        &CorrelationId::new(),
    )
    .await
    .expect("explicit entry must work");

    assert_eq!(shim.total_pages, 1, "shim must crawl the seed");
    assert_eq!(
        shim.total_pages, explicit.total_pages,
        "shim and explicit entry must agree on page count"
    );
    // #1375 strong parity: same-inputs must mean same OUTPUT, not just
    // same counts. `DiscoveredUrl` carries url/depth/parent/content-type,
    // so this compares the full discovery record of every page.
    assert_eq!(
        shim.urls, explicit.urls,
        "shim and explicit entry must discover identical URLs (content, not just count)"
    );
    assert_eq!(
        shim.error_breakdown, explicit.error_breakdown,
        "shim and explicit entry must agree on the error breakdown"
    );
    assert_eq!(shim.errors, 0);
    assert_eq!(explicit.errors, 0);
}

/// Drift guard for the deprecated `crawl_site_capturing` shim: its sink
/// must behave exactly like `EngineOptions::content_sink` on the explicit
/// seam — same pages captured, same content.
#[allow(deprecated)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn deprecated_crawl_site_capturing_shim_matches_content_sink_option() {
    let _entry_off = entry_off();
    use crate::application::crawler::content_sink::{CrawlContentSink, InMemoryContentSink};

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string("<html><body>captured</body></html>"),
        )
        .expect(2)
        .mount(&server)
        .await;

    let seed = Url::parse(&format!("http://127.0.0.1:{}/", server.address().port()))
        .expect("valid seed URL");
    let config = CrawlerConfig::builder(seed)
        .max_depth(0)
        .max_pages(1)
        .ignore_robots(true)
        .build();

    let shim_sink = std::sync::Arc::new(InMemoryContentSink::new());
    let shim = crawl_site_capturing(
        config.clone(),
        shim_sink.clone() as std::sync::Arc<dyn CrawlContentSink>,
    )
    .await
    .expect("deprecated capturing shim must still work");

    let explicit_sink = std::sync::Arc::new(InMemoryContentSink::new());
    let explicit = crawl_site_with_options(
        config,
        EngineOptions {
            ignore_robots: true,
            content_sink: Some(explicit_sink.clone() as std::sync::Arc<dyn CrawlContentSink>),
            ..Default::default()
        },
        &CorrelationId::new(),
    )
    .await
    .expect("explicit entry with content_sink must work");

    assert_eq!(shim.total_pages, 1);
    assert_eq!(explicit.total_pages, 1);
    let shim_pages = shim_sink.take_pages();
    let explicit_pages = explicit_sink.take_pages();
    assert_eq!(shim_pages.len(), 1, "shim sink must capture the body");
    assert_eq!(
        shim_pages.len(),
        explicit_pages.len(),
        "both entries must capture the same page count"
    );
    // #1375 strong parity: the full captured record (url + body), not
    // just the first body — same-inputs must produce identical capture
    // output before the shim is removed.
    assert_eq!(
        shim_pages, explicit_pages,
        "shim sink and content_sink option must capture identical pages"
    );
}
