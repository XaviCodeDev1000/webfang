use super::*;
use crate::domain::persistence::CheckpointCfg;
use crate::error::ScraperError;

fn test_config() -> CrawlerConfig {
    CrawlerConfig::builder(url::Url::parse("https://example.com").expect("seed")).build()
}

fn test_transport() -> TransportPolicy {
    TransportPolicy {
        js_strategy: JsStrategy::Static,
        tls_emulation: wreq_util::Profile::Chrome145,
        ignore_waf: false,
        max_retries: 3,
        backoff_base_ms: 1000,
        backoff_max_ms: 10000,
        obscura_binary: "obscura".to_string(),
        chrome_binary: None,
        post_load_wait: PostLoadWait::Idle,
        session_pool_enabled: false,
        autoscale_enabled: false,
        ignore_robots: false,
    }
}

fn test_ports() -> CrawlPorts {
    CrawlPorts {
        session_pool: None,
        downloader_factory: None,
        content_sink: None,
        pipeline: None,
    }
}

fn test_identity() -> CrawlIdentity {
    CrawlIdentity {
        root: CorrelationId::new(),
        run_label: "example.com".to_string(),
    }
}

fn build_ok(mode: PersistenceMode) -> CrawlSession {
    CrawlSession::builder()
        .config(test_config())
        .persistence(mode)
        .transport(test_transport())
        .ports(test_ports())
        .identity(test_identity())
        .build()
        .expect("valid session must build")
}

#[test]
fn build_requires_every_part() {
    assert!(
        matches!(
            CrawlSession::builder().build(),
            Err(CrawlSessionError::InvalidConfiguration(_))
        ),
        "empty builder must fail"
    );
}

/// sitemap-crawl-run-parity 1.1 (RED): a fresh builder carries no extra
/// seeds, so every existing call site keeps today's single-seed behaviour.
/// Fails until the `extra_seeds` builder field exists (D1).
#[test]
fn default_builder_carries_empty_extra_seeds() {
    let builder = CrawlSessionBuilder::default();
    assert!(
        builder.extra_seeds.is_empty(),
        "fresh builder must carry no extra seeds (existing call sites unaffected)"
    );
}

#[test]
fn build_rejects_non_fetchable_seed() {
    let config =
        CrawlerConfig::builder(url::Url::parse("ftp://example.com/x").expect("seed")).build();
    let result = CrawlSession::builder()
        .config(config)
        .persistence(PersistenceMode::Disabled)
        .transport(test_transport())
        .ports(test_ports())
        .build();
    assert!(
        matches!(result, Err(CrawlSessionError::InvalidConfiguration(_))),
        "ftp seed must fail"
    );
}

#[test]
fn build_rejects_inverted_backoff() {
    let mut transport = test_transport();
    transport.backoff_base_ms = 5000;
    transport.backoff_max_ms = 1000;
    let result = CrawlSession::builder()
        .config(test_config())
        .persistence(PersistenceMode::Disabled)
        .transport(transport)
        .ports(test_ports())
        .build();
    assert!(
        matches!(result, Err(CrawlSessionError::InvalidConfiguration(_))),
        "inverted backoff must fail"
    );
}

#[test]
fn build_rejects_zero_checkpoint_interval() {
    let mode = PersistenceMode::Checkpoint {
        cfg: CheckpointCfg {
            dir: std::path::PathBuf::from("/tmp/x"),
            interval: 0,
        },
    };
    let result = CrawlSession::builder()
        .config(test_config())
        .persistence(mode)
        .transport(test_transport())
        .ports(test_ports())
        .build();
    assert!(
        matches!(result, Err(CrawlSessionError::InvalidConfiguration(_))),
        "zero interval with target must fail"
    );
}

/// #1439: identity is a REQUIRED part — a fully-populated builder without
/// `.identity(...)` must fail like any other missing part, never silently
/// mint a second run-root for the operation that owns one already.
#[test]
fn build_rejects_missing_identity() {
    let result = CrawlSession::builder()
        .config(test_config())
        .persistence(PersistenceMode::Disabled)
        .transport(test_transport())
        .ports(test_ports())
        .build();
    match result {
        Err(CrawlSessionError::InvalidConfiguration(msg)) => {
            assert_eq!(
                msg, "missing run identity",
                "#1439: the mint-free refusal must name the missing part"
            );
        },
        Err(other) => {
            panic!("identity-less build must fail as InvalidConfiguration, got {other}")
        },
        Ok(_) => panic!(
            "identity-less build must NEVER succeed — the silent mint is what \
             #1439 removed"
        ),
    }
}

#[tokio::test]
async fn identity_travels_from_the_caller_verbatim() {
    let a = build_ok(PersistenceMode::Disabled);
    let b = build_ok(PersistenceMode::Disabled);
    assert_ne!(
        a.identity().root,
        b.identity().root,
        "two separately-minted callers must not share a root"
    );
    assert_eq!(a.identity().run_label, "example.com");
    // Consume: an un-run session is a must_use leak by design (D7).
    let _ = a.finish(true, async || CrawlCheckpoint::new()).await;
    let _ = b.finish(true, async || CrawlCheckpoint::new()).await;
}

#[tokio::test]
async fn begin_disabled_is_fresh_without_io() {
    let mut session = build_ok(PersistenceMode::Disabled);
    let outcome = session.begin();
    assert_eq!(
        outcome,
        BeginOutcome {
            resumed: false,
            degraded: false,
        }
    );
    let _ = session.finish(true, async || CrawlCheckpoint::new()).await;
}

#[tokio::test]
async fn begin_checkpoint_without_file_starts_fresh() {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let mode = PersistenceMode::Checkpoint {
        cfg: CheckpointCfg {
            dir: tmp.path().to_path_buf(),
            interval: 100,
        },
    };
    let mut session = build_ok(mode);
    let outcome = session.begin();
    assert_eq!(
        outcome,
        BeginOutcome {
            resumed: false,
            degraded: false,
        }
    );
    let _ = session.finish(true, async || CrawlCheckpoint::new()).await;
}

#[tokio::test]
async fn begin_unwritable_dir_degrades_without_failing() {
    // A regular file where the directory should be: ensure_dir fails
    // deterministically on every platform (no chmod games).
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let blocker = tmp.path().join("blocker");
    std::fs::write(&blocker, b"x").expect("blocker file");
    let mode = PersistenceMode::Checkpoint {
        cfg: CheckpointCfg {
            dir: blocker.join("subdir"),
            interval: 100,
        },
    };
    let mut session = build_ok(mode);
    let outcome = session.begin();
    assert_eq!(
        outcome,
        BeginOutcome {
            resumed: false,
            degraded: true,
        }
    );
    let _ = session.finish(true, async || CrawlCheckpoint::new()).await;
}

#[tokio::test]
async fn begin_resumes_matching_checkpoint() {
    use crate::application::crawler::checkpoint::{
        BincodeCheckpoint, CheckpointPath, CheckpointStore,
    };

    let tmp = tempfile::TempDir::new().expect("tempdir");
    // NOTE: `Url::parse` normalizes the seed with a trailing slash — the
    // scoped filename hashes the normalized form, so the fixture must use
    // it too (same string `begin()` derives from the config).
    let seed = "https://example.com/";
    let scoped = CheckpointPath::new(tmp.path()).file_for_seed(seed);
    // CheckpointPath scopes per-seed; ensure the dir the production way.
    CheckpointPath::new(tmp.path())
        .ensure_dir()
        .expect("ensure dir");
    BincodeCheckpoint::new()
        .save(&CrawlCheckpoint::new(), &scoped)
        .expect("seed checkpoint");
    let mode = PersistenceMode::Checkpoint {
        cfg: CheckpointCfg {
            dir: tmp.path().to_path_buf(),
            interval: 100,
        },
    };
    let mut session = build_ok(mode);
    let outcome = session.begin();
    assert_eq!(
        outcome,
        BeginOutcome {
            resumed: true,
            degraded: false,
        }
    );
    let _ = session.finish(true, async || CrawlCheckpoint::new()).await;
}

/// P8-3 (issue #1289): a corrupt checkpoint file must never crash the
/// run nor silently resume into an empty result — the StateStore
/// contract is corrupt → discard → start fresh → re-scrape. Pins both
/// corruption modes (CRC mismatch over garbage, valid CRC over
/// non-checkpoint JSON) and proves the session-close rewrite replaces
/// the corrupt file with loadable state.
#[tokio::test]
async fn corrupt_checkpoint_file_starts_fresh_and_resscrapes() {
    use crate::application::crawler::checkpoint::{CheckpointPath, CheckpointStore};

    let tmp = tempfile::TempDir::new().expect("tempdir");
    // `Url::parse` normalizes the seed with a trailing slash — the
    // fixture must hash the same normalized string `begin()` derives.
    let seed = "https://example.com/";
    let scoped = CheckpointPath::new(tmp.path()).file_for_seed(seed);
    CheckpointPath::new(tmp.path())
        .ensure_dir()
        .expect("ensure dir");

    // Corruption mode 1: garbage bytes — CRC32 header mismatch.
    std::fs::write(&scoped, b"not a checkpoint at all").expect("corrupt file");
    let mode = PersistenceMode::Checkpoint {
        cfg: CheckpointCfg {
            dir: tmp.path().to_path_buf(),
            interval: 100,
        },
    };
    let mut session = build_ok(mode.clone());
    let outcome = session.begin();
    assert_eq!(
        outcome,
        BeginOutcome {
            resumed: false,
            degraded: false,
        },
        "garbage checkpoint must start fresh, not fail"
    );
    let close = session.finish(false, async || CrawlCheckpoint::new()).await;
    assert_eq!(close.action, CheckpointAction::Write);
    assert!(
        BincodeCheckpoint::new().load(&scoped).is_some(),
        "close must replace the corrupt file with loadable state"
    );

    // Corruption mode 2: valid CRC32 header over non-checkpoint JSON —
    // integrity passes, deserialization fails, still a fresh start.
    let payload = br#"{"visited": "i am not a checkpoint"}"#;
    let mut corrupt = Vec::with_capacity(4 + payload.len());
    corrupt.extend_from_slice(&crc32fast::hash(payload).to_ne_bytes());
    corrupt.extend_from_slice(payload);
    std::fs::write(&scoped, &corrupt).expect("corrupt file v2");
    let mut session = build_ok(mode);
    let outcome = session.begin();
    assert_eq!(
        outcome,
        BeginOutcome {
            resumed: false,
            degraded: false,
        },
        "CRC-valid non-checkpoint JSON must start fresh, not fail"
    );
    let _ = session.finish(true, async || CrawlCheckpoint::new()).await;
}

#[tokio::test]
async fn finish_decision_table() {
    // (completed, enabled) -> action; mirrors today's F-01 branches.
    // The Write case performs real IO — the fixture uses an ephemeral
    // TempDir, never a shared path.
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let table = [
        (true, true, CheckpointAction::Delete),
        (false, true, CheckpointAction::Write),
        (true, false, CheckpointAction::Skip),
        (false, false, CheckpointAction::Skip),
    ];
    for (completed, enabled, expected) in table {
        let mode = if enabled {
            PersistenceMode::Checkpoint {
                cfg: CheckpointCfg {
                    dir: tmp.path().to_path_buf(),
                    interval: 100,
                },
            }
        } else {
            PersistenceMode::Disabled
        };
        let session = build_ok(mode);
        let close = session
            .finish(completed, async || CrawlCheckpoint::new())
            .await;
        assert_eq!(close.action, expected);
    }
}

#[test]
fn error_maps_into_existing_chain() {
    let err: ScraperError = CrawlSessionError::InvalidConfiguration("bad".to_string()).into();
    assert!(matches!(err, ScraperError::Config(_)));
    let err: ScraperError = CrawlSessionError::CheckpointUnwritable {
        path: std::path::PathBuf::from("/nope"),
        reason: "denied".to_string(),
    }
    .into();
    assert!(matches!(err, ScraperError::Config(_)));
    let err: ScraperError = CrawlSessionError::Internal("bug".to_string()).into();
    assert!(matches!(err, ScraperError::Internal(_)));
}
