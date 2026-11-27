// Test-only module: `.unwrap()`/`.expect()` below operate on unreachable
use super::*;
use proptest::prelude::*;
use std::fs;
use tempfile::TempDir;

fn arb_datetime_utc() -> impl proptest::strategy::Strategy<Value = DateTime<Utc>> {
    (0i64..=100_000_000_000)
        .prop_map(|secs| DateTime::<Utc>::from_timestamp(secs, 0).expect("bounded"))
}

fn arb_banned_domain() -> impl proptest::strategy::Strategy<Value = BannedDomain> {
    (
        any::<String>(),
        proptest::option::of(arb_datetime_utc()),
        any::<String>(),
    )
        .prop_map(|(domain, banned_until, reason)| BannedDomain {
            domain,
            banned_until,
            reason,
        })
}

fn arb_checkpoint() -> impl proptest::strategy::Strategy<Value = CrawlCheckpoint> {
    (
        proptest::collection::hash_set(any::<String>(), 0..=20),
        proptest::collection::hash_set(any::<String>(), 0..=20),
        proptest::collection::vec(any::<String>(), 0..=20),
        any::<u64>(),
        proptest::collection::vec(arb_banned_domain(), 0..=20),
        proptest::strategy::Just(CURRENT_CHECKPOINT_VERSION),
    )
        .prop_map(
            |(visited, settled, queued, pages_crawled, banned_domains, version)| CrawlCheckpoint {
                visited,
                settled,
                queued,
                pages_crawled,
                banned_domains,
                version,
            },
        )
}

fn sample_checkpoint() -> CrawlCheckpoint {
    let mut visited = HashSet::new();
    visited.insert("https://example.com".to_string());
    visited.insert("https://example.com/about".to_string());

    // One terminal verdict: `visited` also holds dispatched pages whose
    // outcome is unknown, so the two sets must not be conflated (#3079).
    let mut settled = HashSet::new();
    settled.insert("https://example.com/about".to_string());

    CrawlCheckpoint {
        visited,
        settled,
        queued: vec![
            "https://example.com/contact".to_string(),
            "https://example.com/blog".to_string(),
        ],
        pages_crawled: 42,
        banned_domains: Vec::new(),
        version: CURRENT_CHECKPOINT_VERSION,
    }
}

/// Saves `checkpoint` to a fresh temp dir under the canonical
/// "checkpoint.json" name and hands back the dir (bound to keep the file
/// alive), the path and the store, so each test keeps its own
/// load/tamper flow.
fn saved_checkpoint(checkpoint: &CrawlCheckpoint) -> (TempDir, PathBuf, BincodeCheckpoint) {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("checkpoint.json");
    let store = BincodeCheckpoint::new();
    store.save(checkpoint, &path).unwrap();
    (tmp, path, store)
}

#[test]
fn round_trip_save_load() {
    let original = sample_checkpoint();
    let (_tmp, path, store) = saved_checkpoint(&original);
    let loaded = store.load(&path).unwrap();

    assert_eq!(original, loaded);
}

#[test]
fn empty_state_round_trip() {
    let original = CrawlCheckpoint::new();
    let (_tmp, path, store) = saved_checkpoint(&original);
    let loaded = store.load(&path).unwrap();

    assert_eq!(original, loaded);
    assert!(loaded.visited.is_empty());
    assert_eq!(loaded.pages_crawled, 0);
}

#[test]
fn corruption_detection_tamper_checksum() {
    let (_tmp, path, store) = saved_checkpoint(&sample_checkpoint());

    // Tamper with the checksum (first 4 bytes)
    let mut data = fs::read(&path).unwrap();
    data[0] ^= 0xFF; // flip bits in checksum
    fs::write(&path, &data).unwrap();

    // Load should return None (corrupted)
    let loaded = store.load(&path);
    assert!(loaded.is_none(), "corrupted checkpoint should return None");
}

#[test]
fn corruption_detection_tamper_payload() {
    let (_tmp, path, store) = saved_checkpoint(&sample_checkpoint());

    // Tamper with the payload (after the 4-byte checksum)
    let mut data = fs::read(&path).unwrap();
    if data.len() > 4 {
        data[4] ^= 0xFF;
    }
    fs::write(&path, &data).unwrap();

    // Load should return None (corrupted)
    let loaded = store.load(&path);
    assert!(loaded.is_none(), "corrupted payload should return None");
}

#[test]
fn load_nonexistent_returns_none() {
    let store = BincodeCheckpoint::new();
    let loaded = store.load(Path::new("/tmp/nonexistent_checkpoint_12345.bin"));
    assert!(loaded.is_none());
}

#[test]
fn load_truncated_file_returns_none() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("checkpoint.json");

    // Write only 2 bytes (less than CRC32 header)
    fs::write(&path, [0u8; 2]).unwrap();

    let store = BincodeCheckpoint::new();
    let loaded = store.load(&path);
    assert!(loaded.is_none());
}

#[test]
fn tmp_path_convention() {
    let path = Path::new("/tmp/checkpoint.bin");
    let tmp = tmp_path_for(path);
    assert_eq!(tmp, PathBuf::from("/tmp/checkpoint.bin.tmp"));
}

#[test]
fn checksum_is_first_four_bytes() {
    let (_tmp, path, _store) = saved_checkpoint(&sample_checkpoint());

    let data = fs::read(&path).unwrap();
    let stored = u32::from_ne_bytes([data[0], data[1], data[2], data[3]]);
    let payload = &data[4..];
    let computed = crc32fast::hash(payload);

    assert_eq!(stored, computed);
}

/// M3 (#1617): a CRC32 mismatch abandons the file the same way a stale
/// version does — the caller starts fresh and the next `save` renames over
/// it — so it must leave the same `.bak` behind. Before this, only the
/// version gate backed up, and an integrity failure was a silent
/// one-way door for every byte the checksum covered.
#[test]
fn crc32_mismatch_preserves_bak_before_the_rewrite() {
    let (_tmp, path, store) = saved_checkpoint(&sample_checkpoint());

    // Tamper with a single payload byte: the checksum no longer matches,
    // and the file is not a legacy pure-JSON checkpoint either.
    let mut data = fs::read(&path).unwrap();
    let last = data.len() - 1;
    data[last] ^= 0xFF;
    fs::write(&path, &data).unwrap();

    assert!(store.load(&path).is_none(), "corrupt checkpoint is refused");

    let backup = crate::application::resume::backup_sibling(&path);
    assert!(backup.exists(), "CRC32 failure must leave a .bak sibling");
    assert_eq!(
        fs::read(&backup).unwrap(),
        data,
        "the .bak must carry the exact rejected bytes"
    );
}

#[test]
fn atomic_rename_removes_tmp_on_failure() {
    // Create a read-only directory to force rename failure
    let tmp = TempDir::new().unwrap();
    let readonly_dir = tmp.path().join("readonly");
    fs::create_dir(&readonly_dir).unwrap();

    // Make the parent dir read-only so rename into it fails
    // This test verifies cleanup of .tmp file on rename failure
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // Inside the unix block: the only path that uses them, and on
        // Windows the bindings would be unused.
        let store = BincodeCheckpoint::new();
        let state = CrawlCheckpoint::new();
        fs::set_permissions(&readonly_dir, fs::Permissions::from_mode(0o444)).unwrap();

        let bad_path = readonly_dir.join("checkpoint.bin");
        let result = store.save(&state, &bad_path);

        // Should return error
        assert!(result.is_err());

        // .tmp file should be cleaned up
        let tmp_path = tmp_path_for(&bad_path);
        assert!(
            !tmp_path.exists(),
            ".tmp file should be cleaned up after failed rename"
        );

        // Restore permissions for cleanup
        fs::set_permissions(&readonly_dir, fs::Permissions::from_mode(0o755)).unwrap();
    }
}

#[test]
fn checkpoint_display() {
    let cp = sample_checkpoint();
    let display = format!("{cp}");
    assert!(display.contains("pages=42"));
    assert!(display.contains("visited=2"));
    assert!(
        display.contains("queued=2"),
        "the bounded frontier is part of the shape"
    );
}

// ── Phase 1 RED: Tests for new behavior (types don't exist yet) ──────

#[test]
fn test_banned_domains_roundtrip() {
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

    let mut original = sample_checkpoint();
    original.banned_domains = banned;

    let (_tmp, path, store) = saved_checkpoint(&original);
    let loaded = store.load(&path).unwrap();

    assert_eq!(loaded.banned_domains.len(), 2);
    assert_eq!(loaded.banned_domains[0].domain, "waf.example.com");
    assert!(loaded.banned_domains[0].banned_until.is_none());
    assert_eq!(loaded.banned_domains[0].reason, "WAF challenge");
    assert_eq!(loaded.banned_domains[1].domain, "rate.example.com");
    assert!(loaded.banned_domains[1].banned_until.is_some());
    assert_eq!(original, loaded);
}

#[cfg_attr(miri, ignore = "copy_file_range(326) unsupported by Miri")]
#[test]
fn test_old_format_json_loads() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("checkpoint.json");

    // Write a pure-JSON old-schema file (visited as Vec, no CRC32 header)
    let old_json = r#"{
        "visited": ["https://a.com", "https://b.com"],
        "queued": ["https://c.com"],
        "pages_crawled": 10,
        "version": 1,
        "banned_domains": [
            {
                "domain": "old.example.com",
                "banned_until": null,
                "reason": "WAF challenge"
            }
        ]
    }"#;
    fs::write(&path, old_json).unwrap();

    let store = BincodeCheckpoint::new();
    let loaded = store.load(&path);

    assert!(
        loaded.is_none(),
        "a pre-v2 checkpoint must be discarded, not resumed (#1234)",
    );
}

// Miri trap (#1794 family): discarding the stale version calls
// `preserve_pre_migration_backup`, whose `std::fs::copy` lowers to the
// copy_file_range(326) syscall that Miri does not support. It is the last
// in-filter test reaching that copy without the gate — `record_store.rs`
// copies outside the Miri (core) module filter, and
// `preserve_abandoned_bytes` writes instead of copying.
#[cfg_attr(miri, ignore = "copy_file_range(326) unsupported by Miri")]
#[test]
fn test_stale_version_preserves_bak() {
    // #1587: discarding a stale checkpoint version must preserve the
    // pre-migration file as a `.bak` sibling before the next save
    // overwrites it.
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("checkpoint.json");
    let old_json = r#"{
        "visited": ["https://a.com"],
        "queued": [],
        "pages_crawled": 3,
        "version": 1,
        "banned_domains": []
    }"#;
    fs::write(&path, old_json).unwrap();

    let store = BincodeCheckpoint::new();
    assert!(store.load(&path).is_none());

    let backup = crate::application::resume::backup_sibling(&path);
    assert!(
        backup.exists(),
        "pre-migration checkpoint must be preserved as .bak"
    );
    assert_eq!(
        fs::read_to_string(&backup).unwrap(),
        old_json,
        "backup must carry the exact pre-migration bytes"
    );
}

#[test]
fn test_checkpoint_path_helper() {
    let tmp = TempDir::new().unwrap();
    let cp_path = CheckpointPath::new(tmp.path());
    let file = cp_path.file();
    assert!(file.to_string_lossy().contains("crawl_checkpoint.json"));
    assert!(file.starts_with(tmp.path()));
}

#[test]
fn test_seed_hash_is_stable_and_hex() {
    let a = super::seed_hash("https://example.com/seed");
    let b = super::seed_hash("https://example.com/seed");
    assert_eq!(a, b, "same seed must map to the same scope");
    assert_eq!(a.len(), 16, "8 bytes hex-encoded");
    assert!(
        a.chars().all(|c| c.is_ascii_hexdigit()),
        "scope hash must be hex, got {a}"
    );
}

#[test]
fn test_file_for_seed_scopes_concurrent_sites() {
    // F-01 (a): two seeds sharing one base dir must get disjoint files so
    // concurrent --resume runs cannot collide.
    let tmp = TempDir::new().unwrap();
    let cp_path = CheckpointPath::new(tmp.path());
    let fa = cp_path.file_for_seed("https://a.example/seed");
    let fb = cp_path.file_for_seed("https://b.example/seed");
    assert_ne!(fa, fb, "different seeds must scope to different files");
    assert_eq!(fa, cp_path.file_for_seed("https://a.example/seed"));
    for f in [&fa, &fb] {
        assert!(f.starts_with(tmp.path()));
        let name = f
            .file_name()
            .expect("scoped file has a name")
            .to_string_lossy();
        assert!(
            name.starts_with("crawl_checkpoint_") && name.ends_with(".json"),
            "unexpected scoped name {name}"
        );
    }
}

#[test]
fn test_checkpoint_path_ensure_dir() {
    let tmp = TempDir::new().unwrap();
    let nested = tmp.path().join("deep").join("nested");
    let cp_path = CheckpointPath::new(&nested);
    cp_path.ensure_dir().unwrap();
    assert!(nested.exists());
}

/// `ensure_dir` must surface a filesystem failure as `Err` instead of
/// panicking, so `Engine::with_checkpoint` can disable checkpointing and
/// keep crawling (#393). Here the base directory sits *under a regular
/// file*, a location that can never be created as a directory.
#[test]
fn test_ensure_dir_returns_err_when_base_dir_is_under_a_file() {
    // Arrange: a regular file where a directory component would need to be.
    let tmp = TempDir::new().expect("tempdir should be created");
    let blocker = tmp.path().join("not_a_dir");
    fs::write(&blocker, "i am a file, not a directory").expect("blocker file should be written");

    // Act: try to ensure a directory nested under the file.
    let cp_path = CheckpointPath::new(blocker.join("nested"));
    let result = cp_path.ensure_dir();

    // Assert: the failure is reported as Err (not a panic), with context.
    let err = result.expect_err("ensure_dir must fail when the path is under a regular file");
    assert!(
        err.contains("failed to create checkpoint dir"),
        "error should carry creation context, got: {err}"
    );
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1024))]

    #[cfg_attr(miri, ignore)]
    #[test]
    fn prop_roundtrip_save_load(state in arb_checkpoint()) {
        let (_tmp, path, store) = saved_checkpoint(&state);
        let loaded = store.load(&path).unwrap();
        prop_assert_eq!(state, loaded);
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn prop_corruption_tamper_crc32(
        state in arb_checkpoint(),
        xor_byte in 1u8..=255,
        crc_offset in 0u8..4,
    ) {
        let (_tmp, path, store) = saved_checkpoint(&state);

        let mut data = fs::read(&path).unwrap();
        data[crc_offset as usize] ^= xor_byte;
        fs::write(&path, &data).unwrap();

        prop_assert!(store.load(&path).is_none());
    }
}
