//! `RecordStore` coverage for the degradation marker (T6, issue #3117).
//!
//! Its own file so neither it nor `record_store_tests.rs` grows past the
//! 500-line ratchet; the fixtures are the parent module's, not copies.

use super::tests::{full_record, temp_store};
use super::*;
use crate::domain::page_state::PageStatus;

/// T6 (#3117): a store written BEFORE the degradation marker existed must
/// keep loading, and re-saving it must not invent the field — that is what
/// makes the marker ADDITIVE rather than a migration.
///
/// The record object below is the pre-T6 shape verbatim (nine keys, no
/// `degraded_reason`), so this test is also the compatibility pin the issue
/// asks for: `#[serde(default)]` on read, `skip_serializing_if` on write.
#[test]
fn store_written_before_the_degradation_marker_still_loads() {
    let (_dir, store) = temp_store("pre-t6.test");
    let envelope = serde_json::json!({
        "version": 2,
        "domain_key": "pre-t6.test",
        "created_at": 1_760_000_000_000_i64,
        "updated_at": 1_760_000_000_000_i64,
        "records": {
            "https://old.test/": {
                "url": "https://old.test/",
                "canonical_url": "https://old.test/",
                "run_id": "018f3c1e-7a2b-4c0d-9e1f-2a3b4c5d6e7f",
                "content_hash": "sha256:deadbeef",
                "attempts": 2,
                "status": "COMMITTED",
                "last_error": null,
                "output_location": "out/export.jsonl",
                "updated_at": 1_760_000_000_000_i64
            }
        }
    });
    let path = store.state_path();
    fs::create_dir_all(path.parent().unwrap()).expect("create state dir");
    fs::write(&path, serde_json::to_vec_pretty(&envelope).unwrap()).expect("write old store");

    let loaded = store.load().expect("a pre-marker store must still load");
    let record = loaded
        .get("https://old.test/")
        .expect("the pre-marker record survives");
    assert_eq!(record.url, "https://old.test/");
    assert_eq!(record.status, PageStatus::Committed);

    store.save(&loaded).expect("save must succeed");
    let bytes = fs::read_to_string(&path).expect("state file readable");
    let value: serde_json::Value = serde_json::from_str(&bytes).expect("valid json");
    let rewritten = value["records"]["https://old.test/"]
        .as_object()
        .expect("the record survives the rewrite")
        .clone();
    assert_eq!(
        rewritten.len(),
        9,
        "re-saving a pre-marker record must not invent a field: {rewritten:?}"
    );
    assert!(
        rewritten.get("degraded_reason").is_none(),
        "the marker must stay absent (never null) when there is no degradation"
    );
}

/// T6 (#3117): the degradation marker is durable AND additive. A record that
/// carries it round-trips it; a record that does not writes no field at all,
/// so the pre-T6 shape survives byte-for-byte.
#[test]
fn the_degradation_marker_round_trips_and_is_absent_when_clean() {
    let (_dir, store) = temp_store("marker.test");
    let mut degraded = full_record("https://marker.test/degraded");
    degraded.degraded_reason = Some("ChunkTooLarge".to_string());
    let clean = full_record("https://marker.test/clean");
    let mut records = DomainRecords::new();
    records.insert(degraded.canonical_url.clone(), degraded);
    records.insert(clean.canonical_url.clone(), clean);

    store.save(&records).expect("save must succeed");
    let loaded = store.load().expect("load must succeed");
    assert_eq!(
        loaded["https://marker.test/degraded"]
            .degraded_reason
            .as_deref(),
        Some("ChunkTooLarge"),
        "the cause must survive persistence, not only the fact of degradation"
    );
    assert_eq!(
        loaded["https://marker.test/clean"].degraded_reason, None,
        "a page that did not degrade must not claim one"
    );

    let bytes = fs::read_to_string(store.state_path()).expect("state file readable");
    let value: serde_json::Value = serde_json::from_str(&bytes).expect("valid json");
    assert_eq!(
        value["records"]["https://marker.test/degraded"]["degraded_reason"],
        "ChunkTooLarge"
    );
    assert!(
        value["records"]["https://marker.test/clean"]
            .get("degraded_reason")
            .is_none(),
        "the marker must be ABSENT (not null) on a clean record"
    );
}
