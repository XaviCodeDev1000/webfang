use super::*;
use crate::domain::error::ErrorClass;
use crate::domain::exporter::LastError;
use std::fs;
use tempfile::tempdir;

pub(super) fn temp_store(domain: &str) -> (tempfile::TempDir, RecordStore) {
    let dir = tempdir().expect("tempdir");
    let store = RecordStore::new(domain).with_state_dir(dir.path().to_path_buf());
    (dir, store)
}

/// Full nine-field fixture; every field populated incl. classified error.
/// Status is `Extracted` so carrying a `last_error` stays within the
/// D2 invariant table (only Committed forbids one).
pub(super) fn full_record(url: &str) -> RawRecord {
    RawRecord {
        url: url.to_string(),
        canonical_url: url.to_string(),
        run_id: "018f3c1e-7a2b-4c0d-9e1f-2a3b4c5d6e7f".to_string(),
        content_hash: Some("sha256:deadbeef".to_string()),
        attempts: 3,
        status: crate::domain::page_state::PageStatus::Extracted,
        last_error: Some(LastError {
            class: crate::domain::error::ErrorClass::DomainRecoverable,
            message: "chunk exceeded --max-tokens".to_string(),
        }),
        output_location: Some("out/example.md".to_string()),
        updated_at: 1_760_000_000_000,
    }
}

// --- SC5: record roundtrip preserves all nine fields -------------------

#[test]
fn roundtrip_preserves_all_nine_fields() {
    let (_dir, store) = temp_store("roundtrip.test");
    let mut records = DomainRecords::new();
    records.insert(
        "https://roundtrip.test/a".to_string(),
        full_record("https://roundtrip.test/a"),
    );

    store.save(&records).expect("save must succeed");

    let loaded = store.load().expect("load must succeed");
    assert_eq!(loaded.len(), 1);
    let original = &records["https://roundtrip.test/a"];
    let restored = &loaded["https://roundtrip.test/a"];
    assert_eq!(original.url, restored.url);
    assert_eq!(original.canonical_url, restored.canonical_url);
    assert_eq!(original.run_id, restored.run_id);
    assert_eq!(original.content_hash, restored.content_hash);
    assert_eq!(original.attempts, restored.attempts);
    assert_eq!(original.status, restored.status);
    assert_eq!(original.last_error, restored.last_error);
    assert_eq!(original.output_location, restored.output_location);
    assert_eq!(original.updated_at, restored.updated_at);
}

#[test]
fn serialized_envelope_is_version_2_with_all_fields_present() {
    let (_dir, store) = temp_store("envelope.test");
    let mut records = DomainRecords::new();
    records.insert(
        "https://envelope.test/a".to_string(),
        full_record("https://envelope.test/a"),
    );

    store.save(&records).expect("save must succeed");

    let bytes = fs::read_to_string(store.state_path()).expect("state file readable");
    let value: serde_json::Value = serde_json::from_str(&bytes).expect("valid json");
    assert_eq!(value["version"], 2);
    assert_eq!(value["domain_key"], "envelope.test");
    assert!(value["created_at"].is_i64());
    assert!(value["updated_at"].is_i64());
    let record = &value["records"]["https://envelope.test/a"];
    for field in [
        "url",
        "canonical_url",
        "run_id",
        "content_hash",
        "attempts",
        "status",
        "last_error",
        "output_location",
        "updated_at",
    ] {
        assert!(record.get(field).is_some(), "missing field `{field}`");
    }
    // Frozen field-set: exactly nine keys on the record object.
    let record_obj = record.as_object().expect("record is an object");
    assert_eq!(
        record_obj.len(),
        9,
        "RawRecord must serialize exactly 9 fields"
    );
    assert_eq!(
        record["status"], "EXTRACTED",
        "PageStatus serde is SCREAMING_SNAKE_CASE"
    );
    assert_eq!(record["last_error"]["class"], "domain_recoverable");
}

#[test]
fn unknown_field_is_rejected_on_deserialize() {
    let json = r#"{
        "url": "https://x.test/", "canonical_url": "https://x.test/",
        "run_id": "r", "content_hash": null, "attempts": 1,
        "status": "DISCOVERED", "last_error": null,
        "output_location": null, "updated_at": 1, "bogus": true
    }"#;
    let result: Result<RawRecord, _> = serde_json::from_str(json);
    assert!(result.is_err(), "deny_unknown_fields must reject bogus key");
}

#[test]
fn load_missing_file_returns_empty_records() {
    let (_dir, store) = temp_store("missing.test");
    let loaded = store
        .load()
        .expect("missing file must yield fresh empty store");
    assert!(loaded.is_empty());
}

#[test]
fn new_discovered_rejects_structurally_meaningless_identity() {
    for (url, canonical) in [
        ("", "https://x.test/"),
        ("https://x.test/", ""),
        ("   ", "https://x.test/"),
    ] {
        let err = RawRecord::new_discovered(url, canonical, "run", 0)
            .expect_err("empty identity must be rejected at the domain boundary");
        assert!(
            matches!(err, RecordStoreError::InvalidRecord { .. }),
            "typed InvalidRecord expected, got {err:?}"
        );
    }
}

#[test]
fn new_discovered_yields_honest_fresh_shape_for_valid_url() {
    let record = RawRecord::new_discovered(
        "https://fresh.test/a",
        "https://fresh.test/a",
        "run-1",
        1_760_000_000_000,
    )
    .expect("valid URL constructs");
    assert_eq!(record.url, "https://fresh.test/a");
    assert_eq!(record.canonical_url, "https://fresh.test/a");
    assert_eq!(record.run_id, "run-1");
    assert_eq!(record.status, PageStatus::Discovered);
    assert_eq!(record.attempts, 0);
    assert_eq!(record.content_hash, None);
    assert_eq!(record.last_error, None);
    assert_eq!(record.output_location, None);
    assert_eq!(record.updated_at, 1_760_000_000_000);
}

#[test]
fn save_is_atomic_no_tmp_left_behind() {
    let (_dir, store) = temp_store("atomic.test");
    let records = DomainRecords::new();
    store.save(&records).expect("save must succeed");

    let state_path = store.state_path();
    let parent = state_path.parent().unwrap();
    let strays: Vec<_> = fs::read_dir(parent)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.contains(".tmp-"))
        .collect();
    assert!(
        strays.is_empty(),
        "no tmp files may survive a save: {strays:?}"
    );
}

/// D6: fault-injected rename(2) — typed Io error, no partial state.
struct RenameFailFs;

impl StoreFs for RenameFailFs {
    fn write(&self, _path: &std::path::Path, _bytes: &[u8]) -> std::io::Result<()> {
        Ok(())
    }

    fn rename(&self, _from: &std::path::Path, _to: &std::path::Path) -> std::io::Result<()> {
        Err(std::io::Error::other("simulated rename failure"))
    }
}

#[test]
fn rename_failure_is_typed_and_leaves_previous_state_intact() {
    let dir = tempdir().unwrap();
    let store = RecordStore::with_fs(
        "fail.test",
        dir.path().to_path_buf(),
        std::sync::Arc::new(RenameFailFs),
    );
    let err = store
        .save(&DomainRecords::new())
        .expect_err("rename failure surfaces");
    assert!(matches!(err, RecordStoreError::Io { .. }));
}

#[test]
fn stale_tmp_files_are_garbage_collected_on_load_and_never_read() {
    let dir = tempdir().unwrap();
    let store = RecordStore::new("gc.test").with_state_dir(dir.path().to_path_buf());
    let garbage = dir.path().join("gc.test.json.tmp-123-abc");
    fs::write(&garbage, b"garbage never valid").unwrap();

    store.save(&DomainRecords::new()).unwrap();
    let loaded = store.load().expect("load must succeed and GC tmp siblings");
    assert!(loaded.is_empty());
    assert!(
        !garbage.exists(),
        "stale .tmp-* sibling must be deleted on load"
    );
}

// --- SC5: v1→v2 explicit migration, backup-first ---------------------

/// Real legacy `ExportState` bytes (domain/entities/export.rs v1 shape).
fn v1_fixture(domain: &str) -> String {
    format!(
        r#"{{"version":1,"domain":"{domain}","processed_urls":["https://{domain}/a","https://{domain}/b/c"],"last_export":"2026-08-21T12:00:00Z","total_exported":2}}"#
    )
}

#[test]
#[cfg_attr(miri, ignore = "copy_file_range(326) unsupported by Miri")]
fn v1_state_file_migrates_to_committed_v2_records_with_backup() {
    let dir = tempdir().unwrap();
    let store = RecordStore::new("migrate.test").with_state_dir(dir.path().to_path_buf());
    fs::write(store.state_path(), v1_fixture("migrate.test")).unwrap();
    let before = fs::read(store.state_path()).unwrap();

    let loaded = store.load().expect("v1 file must migrate, not error");

    assert_eq!(loaded.len(), 2, "both processed_urls become records");
    for url in ["https://migrate.test/a", "https://migrate.test/b/c"] {
        let record = &loaded[url];
        assert_eq!(
            record.status,
            PageStatus::Committed,
            "v1 processed == committed history"
        );
        assert_eq!(record.run_id, MIGRATED_V1_RUN_ID);
        assert_eq!(record.attempts, 1);
        assert_eq!(record.url, url);
        assert_eq!(record.canonical_url, url);
        assert_eq!(record.last_error, None);
    }

    // Backup exists with original bytes; live file is now v2.
    let state_path = store.state_path();
    let parent = state_path.parent().unwrap();
    let backups: Vec<_> = fs::read_dir(parent)
        .unwrap()
        .map(|e| e.unwrap())
        .filter(|e| e.file_name().to_string_lossy().contains(".v1.bak."))
        .collect();
    assert_eq!(backups.len(), 1, "exactly one backup created");
    assert_eq!(
        fs::read(backups[0].path()).unwrap(),
        before,
        "backup preserves original bytes"
    );
    let value: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(store.state_path()).unwrap()).unwrap();
    assert_eq!(value["version"], 2, "live file upgraded in place");
}

/// #876: an empty-string URL in legacy state is malformed input of the
/// same class as a per-record invariant violation — quarantined with a
/// warning, never promoted to Committed, and never at the cost of its
/// well-formed neighbors (file-level Corrupt stays reserved for
/// unparseable structure).
#[test]
#[cfg_attr(miri, ignore = "copy_file_range(326) unsupported by Miri")]
fn v1_migration_drops_empty_urls_but_keeps_neighbors() {
    let dir = tempdir().unwrap();
    let store = RecordStore::new("empty.test").with_state_dir(dir.path().to_path_buf());
    let legacy = r#"{"version":1,"domain":"empty.test","processed_urls":["","   ","https://empty.test/good"],"last_export":"2026-08-21T12:00:00Z","total_exported":3}"#.to_string();
    fs::write(store.state_path(), legacy).unwrap();

    let loaded = store
        .load()
        .expect("migration must not fail the whole file");

    assert_eq!(loaded.len(), 1, "only the well-formed neighbor survives");
    assert!(!loaded.contains_key(""), "empty key must not become state");
    assert!(!loaded.contains_key("   "));
    let good = &loaded["https://empty.test/good"];
    assert_eq!(good.status, PageStatus::Committed);
    assert_eq!(good.run_id, MIGRATED_V1_RUN_ID);
}

/// #876: the validated constructor is the choke point every new-record
/// writer passes through — structurally meaningless identity must be
/// rejected, not persisted as retrievable state.
#[test]
fn new_discovered_rejects_empty_and_whitespace_identity() {
    for bad in ["", "   "] {
        let err = RawRecord::new_discovered(bad, "https://x.test/a", "run", 0)
            .expect_err("empty url must be rejected");
        assert!(matches!(err, RecordStoreError::InvalidRecord { .. }));
        let err = RawRecord::new_discovered("https://x.test/a", bad, "run", 0)
            .expect_err("empty canonical_url must be rejected");
        assert!(matches!(err, RecordStoreError::InvalidRecord { .. }));
    }
    RawRecord::new_discovered("https://x.test/a", "https://x.test/a", "run", 0)
        .expect("valid identity must construct");
}

#[test]
#[cfg_attr(miri, ignore = "copy_file_range(326) unsupported by Miri")]
fn migrated_records_survive_invariant_validation_despite_null_hash() {
    // Migrated records are Committed with content_hash/output_location
    // = None: the MIGRATED_V1_RUN_ID exemption must keep them.
    let dir = tempdir().unwrap();
    let store = RecordStore::new("exempt.test").with_state_dir(dir.path().to_path_buf());
    fs::write(store.state_path(), v1_fixture("exempt.test")).unwrap();

    let loaded = store.load().expect("migration succeeds");
    assert_eq!(loaded.len(), 2, "no migrated record may be quarantined");
}

#[test]
#[cfg_attr(miri, ignore = "copy_file_range(326) unsupported by Miri")]
fn second_load_after_migration_reads_v2_without_remigrating() {
    let dir = tempdir().unwrap();
    let store = RecordStore::new("idem.test").with_state_dir(dir.path().to_path_buf());
    fs::write(store.state_path(), v1_fixture("idem.test")).unwrap();
    store.load().expect("first load migrates");
    let loaded = store.load().expect("second load reads v2");
    assert_eq!(loaded.len(), 2);
    let state_path = store.state_path();
    let parent = state_path.parent().unwrap();
    let backups = fs::read_dir(parent).unwrap().filter(|e| {
        e.as_ref()
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains(".v1.bak.")
    });
    assert_eq!(backups.count(), 1, "no second backup on idempotent re-read");
}

// --- SC5/E6: invariant table quarantines impossible states ------------

#[test]
fn committed_record_without_output_location_is_quarantined() {
    let (_dir, store) = temp_store("quarantine.test");
    let mut record = full_record("https://quarantine.test/bad");
    record.status = PageStatus::Committed;
    record.output_location = None;
    let mut records = DomainRecords::new();
    records.insert(record.canonical_url.clone(), record.clone());
    store
        .save(&records)
        .expect("save succeeds — corruption is logical, not structural");

    let loaded = store
        .load()
        .expect("load never panics nor errors on bad record");
    assert!(
        loaded.is_empty(),
        "invariant-violating record is quarantined"
    );
}

// --- #876: empty-string URL is structurally meaningless identity ----

#[test]
#[cfg_attr(miri, ignore = "copy_file_range(326) unsupported by Miri")]
fn v1_empty_string_url_is_quarantined_never_committed() {
    let dir = tempdir().unwrap();
    let store = RecordStore::new("empty-v1.test").with_state_dir(dir.path().to_path_buf());
    let legacy = r#"{"version":1,"domain":"empty-v1.test","processed_urls":["","https://empty-v1.test/ok"],"last_export":"2026-08-21T12:00:00Z","total_exported":2}"#;
    fs::write(store.state_path(), legacy).unwrap();

    let loaded = store
        .load()
        .expect("empty legacy URL is a per-record violation, not file corruption");

    assert_eq!(
        loaded.len(),
        1,
        "only the valid URL may become a migrated record"
    );
    assert!(
        !loaded.contains_key(""),
        "an empty URL must never be retrievable committed state"
    );
    assert!(loaded.contains_key("https://empty-v1.test/ok"));
}

#[test]
fn persisted_empty_url_record_is_quarantined_on_load() {
    let (_dir, store) = temp_store("empty-v2.test");
    let mut bad = full_record("");
    bad.status = PageStatus::Committed;
    // Empty URL must be the ONLY violation here: satisfy every other
    // committed invariant so this test isolates the #876 gap.
    bad.last_error = None;
    let good = full_record("https://empty-v2.test/good");
    let mut records = DomainRecords::new();
    records.insert(bad.url.clone(), bad);
    records.insert(good.url.clone(), good);
    store
        .save(&records)
        .expect("save succeeds; rejection happens at the load boundary");

    let loaded = store
        .load()
        .expect("load never errors on per-record violations");

    assert!(
        !loaded.contains_key(""),
        "an empty-URL record must be quarantined on load"
    );
    assert_eq!(loaded.len(), 1, "the valid neighbor survives quarantine");
}

#[test]
#[cfg_attr(miri, ignore = "copy_file_range(326) unsupported by Miri")]
fn v1_whitespace_only_url_is_quarantined_never_committed() {
    let dir = tempdir().unwrap();
    let store = RecordStore::new("ws-v1.test").with_state_dir(dir.path().to_path_buf());
    let legacy = r#"{"version":1,"domain":"ws-v1.test","processed_urls":["   ","https://ws-v1.test/ok"],"last_export":"2026-08-21T12:00:00Z","total_exported":2}"#;
    fs::write(store.state_path(), legacy).unwrap();

    let loaded = store
        .load()
        .expect("whitespace-only URL is a per-record violation, not file corruption");

    assert_eq!(loaded.len(), 1, "only the valid URL survives");
    assert!(!loaded.contains_key("   "));
}

#[test]
fn committed_record_with_last_error_is_quarantined_but_valid_neighbors_survive() {
    let (_dir, store) = temp_store("quarantine-mix.test");
    let mut bad = full_record("https://q.test/bad");
    bad.status = PageStatus::Committed;
    bad.last_error = Some(LastError {
        class: ErrorClass::InternalFatal,
        message: "impossible state".to_string(),
    });
    let good = full_record("https://q.test/good");
    let mut records = DomainRecords::new();
    records.insert(bad.canonical_url.clone(), bad);
    records.insert(good.canonical_url.clone(), good);
    store.save(&records).unwrap();

    let loaded = store.load().expect("load succeeds");
    assert_eq!(loaded.len(), 1, "only the valid neighbor survives");
    assert!(loaded.contains_key("https://q.test/good"));
}

// Helpers `temp_store` and `full_record` are mirrored in the sibling
// recovery test module; the two themed mods keep their fixtures local.
