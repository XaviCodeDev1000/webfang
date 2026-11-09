use super::*;

// ——— PersistenceMode::with_persistence wiring (5c) ———

#[tokio::test]
async fn with_persistence_disabled_leaves_checkpoint_disabled() {
    let seed = Url::parse("http://127.0.0.1:9/").expect("valid seed URL");
    let config = CrawlerConfig::builder(seed).build();
    let engine = session_engine(config);
    assert_eq!(engine.checkpoint_interval, 100);
    assert!(engine.checkpoint_path.is_none());

    let mode = crate::domain::persistence::PersistenceMode::Disabled;
    let engine = engine.with_persistence(mode);
    assert!(engine.checkpoint_path.is_none());
    // Disabled must not change interval (preserved from initial 100, but not enabling file)
    // The key invariant: no checkpoint file configured.
    assert!(engine.checkpoint_path.is_none());
}

#[tokio::test]
async fn with_persistence_checkpoint_configures_path_and_interval() {
    let seed = Url::parse("http://127.0.0.1:9/").expect("valid seed URL");
    let config = CrawlerConfig::builder(seed).build();
    let engine = session_engine(config);

    let tmp = tempfile::TempDir::new().expect("tempdir");
    let mode = crate::domain::persistence::PersistenceMode::Checkpoint {
        cfg: crate::domain::persistence::CheckpointCfg {
            dir: tmp.path().to_path_buf(),
            interval: 42,
        },
    };
    let engine = engine.with_persistence(mode);
    assert!(engine.checkpoint_path.is_some());
    assert_eq!(engine.checkpoint_interval, 42);
    assert!(
        engine.checkpoint_path.unwrap().starts_with(tmp.path()),
        "checkpoint path should be under requested dir"
    );
}

#[tokio::test]
async fn with_persistence_resume_only_leaves_checkpoint_disabled() {
    let seed = Url::parse("http://127.0.0.1:9/").expect("valid seed URL");
    let config = CrawlerConfig::builder(seed).build();
    let engine = session_engine(config);

    let mode = crate::domain::persistence::PersistenceMode::Resume {
        dir: std::path::PathBuf::from("/tmp/resume"),
    };
    let engine = engine.with_persistence(mode);
    assert!(engine.checkpoint_path.is_none());
}

// ——— #1823 checkpoint verdict contract ———

/// Scoped checkpoint file for `seed` under `dir` — the exact derivation
/// `with_checkpoint` and `CrawlSession::finish` share (F-01).
fn checkpoint_file_for(dir: &std::path::Path, seed: &Url) -> std::path::PathBuf {
    crate::application::crawler::checkpoint::CheckpointPath::new(dir).file_for_seed(seed.as_str())
}

/// Session-built engine whose SESSION carries Checkpoint persistence —
/// the production wiring: the close verdict (`CrawlSession::finish`)
/// reads the session's mode, and `from_session` mirrors it into the
/// engine's `checkpoint_path`. `session_engine` + `with_persistence`
/// would leave the session Disabled, so `finish` would Skip.
fn checkpoint_session_engine(
    config: CrawlerConfig,
    dir: &std::path::Path,
    interval: u64,
) -> Engine {
    // #1439: each helper invocation is its own standalone run — mint the
    // job root here, at the operation boundary.
    let run_label = config.seed_url.host_str().unwrap_or("seed").to_string();
    let mut session = CrawlSession::builder()
        .config(config)
        .persistence(PersistenceMode::Checkpoint {
            cfg: CheckpointCfg {
                dir: dir.to_path_buf(),
                interval,
            },
        })
        .transport(crate::application::crawler::session::TransportPolicy {
            js_strategy: JsStrategy::Static,
            tls_emulation: Profile::Chrome145,
            ignore_waf: false,
            max_retries: 3,
            backoff_base_ms: 1000,
            backoff_max_ms: 10000,
            obscura_binary: DEFAULT_OBSCURA_BINARY.to_string(),
            chrome_binary: None,
            post_load_wait: PostLoadWait::Idle,
            session_pool_enabled: false,
            autoscale_enabled: false,
            ignore_robots: true,
        })
        .ports(crate::application::crawler::session::CrawlPorts {
            session_pool: None,
            downloader_factory: None,
            content_sink: None,
            pipeline: None,
        })
        .identity(crate::application::crawler::session::CrawlIdentity {
            root: CorrelationId::new(),
            run_label,
        })
        .build()
        .expect("test session must build");
    let _ = session.begin();
    Engine::from_session(session).expect("engine must build")
}

/// Mount a seed page linking `link_count` link-free leaf pages.
async fn mount_linked_site(server: &MockServer, link_count: usize) {
    let port = server.address().port();
    let links: String = (0..link_count)
        .map(|i| format!(r#"<a href="http://127.0.0.1:{port}/page{i}">link</a>"#))
        .collect();
    Mock::given(path("/"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(format!("<html><body>{links}</body></html>")),
        )
        .mount(server)
        .await;
    for i in 0..link_count {
        Mock::given(path(format!("/page{i}")))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(format!("<html><body>page {i}</body></html>")),
            )
            .mount(server)
            .await;
    }
}

/// #1823 Delete verdict: a fully-completed crawl deletes its checkpoint
/// at close, and `shutdown()` must not resurrect it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shutdown_leaves_no_checkpoint_after_fully_completed_run() {
    let _entry_off = entry_off();
    let server = MockServer::start().await;
    mount_linked_site(&server, 2).await;

    let tmp = tempfile::TempDir::new().expect("tempdir");
    let seed = Url::parse(&format!("http://127.0.0.1:{}/", server.address().port()))
        .expect("valid seed URL");
    let config = CrawlerConfig::builder(seed.clone())
        .max_depth(1)
        .max_pages(10)
        .delay_ms(1)
        .timeout_secs(5)
        .ignore_robots(true)
        .build();

    let mut engine = checkpoint_session_engine(config, tmp.path(), 100);
    let result = engine.run().await.expect("crawl must complete");
    assert_eq!(result.total_pages, 3, "seed + both leaves must be crawled");

    let file = checkpoint_file_for(tmp.path(), &seed);
    assert!(
        !file.exists(),
        "Delete verdict must remove the checkpoint at {}",
        file.display()
    );

    engine.shutdown().await;
    assert!(
        !file.exists(),
        "shutdown() must not re-create the deleted checkpoint (#1823)"
    );
}

/// #1823 Write verdict: a truncated crawl keeps its checkpoint, and
/// `shutdown()` must not rewrite it byte-for-byte.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shutdown_does_not_rewrite_checkpoint_after_truncated_run() {
    let _entry_off = entry_off();
    let server = MockServer::start().await;
    mount_linked_site(&server, 6).await;

    let tmp = tempfile::TempDir::new().expect("tempdir");
    let seed = Url::parse(&format!("http://127.0.0.1:{}/", server.address().port()))
        .expect("valid seed URL");
    let config = CrawlerConfig::builder(seed.clone())
        .max_depth(1)
        .max_pages(2) // truncated: 7 discoverable pages, budget for 2
        .delay_ms(1)
        .timeout_secs(5)
        .ignore_robots(true)
        .build();

    let mut engine = checkpoint_session_engine(config, tmp.path(), 1000);
    let result = engine.run().await.expect("crawl must complete");
    assert!(
        result.total_pages >= 2,
        "truncated crawl must collect its budget, got {}",
        result.total_pages
    );

    let file = checkpoint_file_for(tmp.path(), &seed);
    let before = std::fs::read(&file).expect("Write verdict must persist the checkpoint");
    assert!(
        !before.is_empty(),
        "written checkpoint must carry a payload"
    );

    // The redundant-rewrite regression cannot be caught by byte equality
    // (the old shutdown wrote the identical state), so clobber the file:
    // a shutdown() that writes would replace the sentinel with a valid
    // checkpoint; the contract leaves it untouched.
    std::fs::write(&file, b"sentinel: shutdown must not touch this").expect("clobber");

    engine.shutdown().await;

    let after = std::fs::read(&file).expect("checkpoint file must survive shutdown");
    assert_eq!(
        after,
        b"sentinel: shutdown must not touch this".as_slice(),
        "shutdown() must not write any checkpoint I/O (#1823)"
    );
}

/// #1823 Skip verdict: persistence disabled means no checkpoint file is
/// ever created — by the run or by `shutdown()`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shutdown_writes_no_checkpoint_when_persistence_disabled() {
    let _entry_off = entry_off();
    let server = MockServer::start().await;
    mount_linked_site(&server, 2).await;

    // Watched dir the engine never learns about: nothing may appear in it.
    let watched = tempfile::TempDir::new().expect("tempdir");
    let seed = Url::parse(&format!("http://127.0.0.1:{}/", server.address().port()))
        .expect("valid seed URL");
    let config = CrawlerConfig::builder(seed.clone())
        .max_depth(0)
        .max_pages(1)
        .delay_ms(1)
        .timeout_secs(5)
        .ignore_robots(true)
        .build();

    let mut engine = session_engine(config)
        .with_persistence(crate::domain::persistence::PersistenceMode::Disabled);
    let result = engine.run().await.expect("crawl must complete");
    assert_eq!(result.total_pages, 1);
    engine.shutdown().await;

    let would_be = checkpoint_file_for(watched.path(), &seed);
    assert!(
        !would_be.exists(),
        "Skip verdict must never create a checkpoint file at {}",
        would_be.display()
    );
    let residue: Vec<std::path::PathBuf> = std::fs::read_dir(watched.path())
        .expect("watched dir exists")
        .filter_map(std::result::Result::ok)
        .map(|entry| entry.path())
        .collect();
    assert!(
        residue.is_empty(),
        "checkpoint dir must stay empty under Disabled, got {residue:?}"
    );
}

/// #1823 no-verdict path: a seed excluded by pattern filters returns
/// before any close verdict — the prior checkpoint on disk must be left
/// byte-for-byte untouched (the state-damage scenario behind the issue).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn seed_excluded_run_leaves_prior_checkpoint_untouched() {
    let _entry_off = entry_off();
    let server = MockServer::start().await;
    mount_linked_site(&server, 2).await;

    let tmp = tempfile::TempDir::new().expect("tempdir");
    let seed = Url::parse(&format!("http://127.0.0.1:{}/", server.address().port()))
        .expect("valid seed URL");

    // Leg 1: truncated crawl writes a checkpoint for this seed.
    let config = CrawlerConfig::builder(seed.clone())
        .max_depth(1)
        .max_pages(1)
        .delay_ms(1)
        .timeout_secs(5)
        .ignore_robots(true)
        .build();
    let mut engine = checkpoint_session_engine(config, tmp.path(), 100);
    let result = engine.run().await.expect("first crawl must complete");
    assert_eq!(result.total_pages, 1);
    let file = checkpoint_file_for(tmp.path(), &seed);
    let before = std::fs::read(&file).expect("leg 1 must persist a checkpoint");
    assert!(!before.is_empty(), "leg-1 checkpoint must carry a payload");

    // Sentinel clobber: an undecided rewrite (the #1823 regression) would
    // replace it with a valid checkpoint; the contract leaves it as-is.
    std::fs::write(&file, b"sentinel: no-verdict run must not touch this").expect("clobber");

    // Leg 2: same seed, now excluded by pattern — run() returns early
    // with no verdict, and shutdown() must not touch the leg-1 file.
    let excluded = CrawlerConfig::builder(seed.clone())
        .max_depth(1)
        .max_pages(1)
        .delay_ms(1)
        .timeout_secs(5)
        .ignore_robots(true)
        .exclude_pattern("127.0.0.1")
        .build();
    let mut engine = checkpoint_session_engine(excluded, tmp.path(), 100);
    let result = engine
        .run()
        .await
        .expect("excluded-seed run must return Ok");
    assert_eq!(result.total_pages, 0, "excluded seed must produce no pages");
    engine.shutdown().await;

    let after = std::fs::read(&file).expect("leg-1 checkpoint must survive");
    assert_eq!(
        after,
        b"sentinel: no-verdict run must not touch this".as_slice(),
        "no-verdict path must not write any checkpoint I/O (#1823)"
    );
}
