#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

//! Integration tests for checkpoint persistence — real I/O with temp dirs.
//!
//! Exercises save/load roundtrip, resume from checkpoint, corrupt file
//! handling, backward-compatible old-format loading, and TempDir cleanup.
//! Uses the consolidated application-layer `CheckpointStore` trait API.

use std::collections::HashSet;
use std::fs;
use tempfile::TempDir;
use webfang_core::{
    BannedDomain, BincodeCheckpoint, CheckpointStore, CrawlCheckpoint, CURRENT_CHECKPOINT_VERSION,
};

/// Helper: build a CrawlCheckpoint from components.
fn checkpoint_from(
    visited: HashSet<String>,
    pages_crawled: u64,
    banned_domains: Vec<BannedDomain>,
) -> CrawlCheckpoint {
    CrawlCheckpoint {
        visited,
        queued: Vec::new(),
        pages_crawled,
        banned_domains,
        version: CURRENT_CHECKPOINT_VERSION,
    }
}

/// Save and load a checkpoint — data survives the roundtrip.
#[tokio::test]
async fn test_save_and_load_roundtrip() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("checkpoint.json");

    let mut visited = HashSet::new();
    visited.insert("https://a.com".to_string());
    visited.insert("https://b.com".to_string());
    visited.insert("https://c.com".to_string());

    let store = BincodeCheckpoint::new();
    let state = checkpoint_from(visited, 42, vec![]);
    store.save(&state, &path).unwrap();

    let loaded = store.load(&path).unwrap();
    assert_eq!(loaded.visited.len(), 3);
    assert!(loaded.visited.contains("https://a.com"));
    assert!(loaded.visited.contains("https://c.com"));
    assert_eq!(loaded.pages_crawled, 42);
    assert_eq!(loaded.version, CURRENT_CHECKPOINT_VERSION);
}

/// Loading a non-existent checkpoint returns None.
#[tokio::test]
async fn test_load_nonexistent_returns_none() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("nope.json");

    let store = BincodeCheckpoint::new();
    let loaded = store.load(&path);
    assert!(loaded.is_none(), "non-existent file should return None");
}

/// Resume from checkpoint: load → add more data → save → reload verifies append.
#[tokio::test]
async fn test_resume_from_checkpoint() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("resume.json");
    let store = BincodeCheckpoint::new();

    // Phase 1: initial crawl
    let mut visited = HashSet::new();
    visited.insert("https://page1.com".to_string());
    let cp1 = checkpoint_from(visited, 1, vec![]);
    store.save(&cp1, &path).unwrap();

    // Phase 2: resume and continue
    let mut loaded = store.load(&path).unwrap();
    loaded.visited.insert("https://page2.com".to_string());
    loaded.pages_crawled = 2;
    store.save(&loaded, &path).unwrap();

    // Phase 3: verify final state
    let final_cp = store.load(&path).unwrap();
    assert_eq!(final_cp.visited.len(), 2);
    assert!(final_cp.visited.contains("https://page1.com"));
    assert!(final_cp.visited.contains("https://page2.com"));
    assert_eq!(final_cp.pages_crawled, 2);
}

/// Corrupt file — load returns None (not a panic).
#[tokio::test]
async fn test_corrupt_file_returns_none() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("corrupt.json");
    fs::write(&path, b"{not valid json!!!").unwrap();

    let store = BincodeCheckpoint::new();
    let result = store.load(&path);
    assert!(result.is_none(), "corrupt file should return None");
}

/// Save overwrites previous checkpoint (not append).
#[tokio::test]
async fn test_save_overwrites_previous() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("overwrite.json");
    let store = BincodeCheckpoint::new();

    // Save first checkpoint with 5 visited URLs
    let mut visited1 = HashSet::new();
    for i in 0..5 {
        visited1.insert(format!("https://page{i}.com"));
    }
    let cp1 = checkpoint_from(visited1, 5, vec![]);
    store.save(&cp1, &path).unwrap();

    // Save second checkpoint with only 2 visited URLs
    let mut visited2 = HashSet::new();
    visited2.insert("https://x.com".to_string());
    visited2.insert("https://y.com".to_string());
    let cp2 = checkpoint_from(visited2, 2, vec![]);
    store.save(&cp2, &path).unwrap();

    let loaded = store.load(&path).unwrap();
    assert_eq!(loaded.visited.len(), 2, "should have 2 URLs, not 5+2");
    assert_eq!(loaded.pages_crawled, 2);
}

/// Save to an existing file replaces it cleanly.
#[tokio::test]
async fn test_save_replaces_existing_file() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("replace.json");

    // Write initial content
    fs::write(&path, b"old content").unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), "old content");

    // Save checkpoint over it
    let store = BincodeCheckpoint::new();
    let cp = checkpoint_from(HashSet::new(), 1, vec![]);
    store.save(&cp, &path).unwrap();

    // Verify it's valid checkpoint, not "old content"
    let loaded = store.load(&path).unwrap();
    assert_eq!(loaded.pages_crawled, 1);
}

/// Banned domains roundtrip through save/load.
#[tokio::test]
async fn test_banned_domains_roundtrip() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("banned.json");

    let banned = vec![
        BannedDomain {
            domain: "waf.example.com".into(),
            banned_until: None,
            reason: "WAF challenge".into(),
        },
        BannedDomain {
            domain: "rate.example.com".into(),
            banned_until: Some("2026-12-31T23:59:59Z".parse().unwrap()),
            reason: "rate limit exceeded".into(),
        },
    ];

    let store = BincodeCheckpoint::new();
    let cp = checkpoint_from(HashSet::new(), 0, banned);
    store.save(&cp, &path).unwrap();

    let loaded = store.load(&path).unwrap();
    assert_eq!(loaded.banned_domains.len(), 2);
    assert_eq!(loaded.banned_domains[0].domain, "waf.example.com");
    assert!(loaded.banned_domains[0].banned_until.is_none());
    assert_eq!(loaded.banned_domains[1].reason, "rate limit exceeded");
    assert!(loaded.banned_domains[1].banned_until.is_some());
}

/// Large checkpoint with many URLs saves and loads correctly.
#[tokio::test]
async fn test_large_checkpoint_roundtrip() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("large.json");

    let mut visited = HashSet::new();
    for i in 0..1000 {
        visited.insert(format!("https://visited{i}.example.com/page{i}"));
    }

    let store = BincodeCheckpoint::new();
    let cp = checkpoint_from(visited, 1000, vec![]);
    store.save(&cp, &path).unwrap();

    let loaded = store.load(&path).unwrap();
    assert_eq!(loaded.visited.len(), 1000);
    assert_eq!(loaded.pages_crawled, 1000);

    // F-39's real cost was size: one interrupt on a link-dense page wrote a
    // ~190 KB checkpoint holding 4955 frontier URLs against 49 visited. With no
    // frontier field the payload grows only with `visited`, which is bounded by
    // the crawl itself.
    let raw = fs::read(&path).unwrap();
    assert!(
        raw.len() < 60_000,
        "checkpoint payload ballooned to {} bytes for 1000 visited URLs — a \
         frontier field may have crept back in (#1234)",
        raw.len(),
    );
}

/// An old-format pure-JSON checkpoint (no CRC32, no current version) is
/// DISCARDED, not resumed.
///
/// The pre-#1234 behaviour was to migrate it and carry on. That is unsound once
/// the schema changes: a v1 payload's fields cannot be interpreted by a v2
/// reader, and `visited` alone silently under-reports what the previous run did.
/// Gate 0's discard+log contract applies — fresh start, file left for
/// inspection, no crash.
#[tokio::test]
async fn test_old_format_is_discarded_not_resumed() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("old_format.json");

    // Write old-format pure JSON (visited as array, no CRC32 header)
    let old_json = r#"{"visited":["https://old1.com","https://old2.com"],"queued":["https://q.com"],"pages_crawled":7,"version":1,"banned_domains":[]}"#;
    fs::write(&path, old_json).unwrap();

    let store = BincodeCheckpoint::new();
    assert!(
        store.load(&path).is_none(),
        "a superseded checkpoint version must be discarded (#1234)",
    );
    // Discarded, not destroyed: the bytes stay for inspection.
    assert_eq!(fs::read_to_string(&path).unwrap(), old_json);
}

// ===========================================================================
// #1234 / F-39 — the checkpoint must not persist the discovery frontier
// ===========================================================================

/// Frame a JSON payload the way `BincodeCheckpoint::save` does: a 4-byte CRC32
/// header followed by the payload. Needed to forge checkpoints written by a
/// *different* schema version.
fn frame(payload: &[u8]) -> Vec<u8> {
    let mut bytes = crc32fast::hash(payload).to_ne_bytes().to_vec();
    bytes.extend_from_slice(payload);
    bytes
}

/// The frontier is persisted, but the schema version that made it bounded is 2.
///
/// #1234 / F-39: the pathological file was 4955 queued URLs against 49 visited,
/// written because nothing bounded `queued` and `max_pages` was never part of
/// the checkpoint. Deleting the field outright was tried and measured — it
/// strands `--resume` at zero pages, because rediscovery needs the seed
/// re-fetched and the seed is in `visited`. The fix is therefore the BOUND:
/// the frontier can never exceed the budget the run set for itself. The bound
/// itself is enforced in `CrawlScheduler::snapshot_pending_bounded` (unit-tested
/// there, where the priority queue lives); this test pins the schema contract
/// that carries it.
#[tokio::test]
async fn checkpoint_frontier_is_versioned_and_round_trips() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("checkpoint.json");
    let store = BincodeCheckpoint::new();

    let mut cp = CrawlCheckpoint::new();
    cp.visited.insert("https://a.com".to_string());
    cp.queued = (0..5).map(|i| format!("https://q{i}.com/p")).collect();
    cp.pages_crawled = 3;
    store.save(&cp, &path).expect("save should succeed");

    let raw = fs::read(&path).expect("read checkpoint");
    let doc: serde_json::Value =
        serde_json::from_slice(&raw[4..]).expect("payload is JSON after the CRC32 header");
    assert_eq!(
        doc.get("version").and_then(serde_json::Value::as_u64),
        Some(2),
        "the bounded-frontier change is a schema change and must bump the version",
    );

    let loaded = store.load(&path).expect("current version must load");
    assert_eq!(loaded.queued.len(), 5, "the frontier still round-trips");
}

/// A checkpoint from a superseded schema version is discarded with a log, and
/// the run starts fresh — never resumed with a half-understood state, never a
/// crash. This is the Gate 0 discard+log contract that `ExportState` already
/// follows; the checkpoint path had a `version` field that nothing ever read.
#[tokio::test]
async fn stale_checkpoint_version_is_discarded_not_resumed() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("checkpoint.json");
    let store = BincodeCheckpoint::new();

    let v1 = serde_json::json!({
        "visited": ["https://a.com"],
        "queued": (0..50).map(|i| format!("https://q{i}.com/p")).collect::<Vec<String>>(),
        "pages_crawled": 7u64,
        "banned_domains": [],
        "version": 1u32,
    });
    fs::write(&path, frame(v1.to_string().as_bytes())).expect("write v1 checkpoint");

    assert!(
        store.load(&path).is_none(),
        "a superseded checkpoint version must be discarded, not resumed (#1234)",
    );

    // A current-version checkpoint still round-trips, so the discard is a
    // version decision and not an accidental "nothing ever loads".
    let cp = CrawlCheckpoint::new();
    store.save(&cp, &path).expect("save current version");
    assert!(
        store.load(&path).is_some(),
        "the current checkpoint version must load",
    );
}
