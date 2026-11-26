use super::*;
use crate::domain::exporter::{LastError, RawRecord};
use crate::domain::page_state::PageStatus;
use std::fs;
use std::io::Write as _;
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

// --- E5: load_or_init fresh-start policy with named-path warning ------

#[test]
fn corrupt_file_is_a_typed_error_via_load_or_init() {
    let (_dir, store) = temp_store("corrupt.test");
    fs::write(store.state_path(), b"{{{{not json").unwrap();

    assert!(
        matches!(store.load(), Err(RecordStoreError::Corrupt { .. })),
        "raw load surfaces the typed error"
    );
    let err = store
        .load_or_init()
        .expect_err("corruption must be operator-visible, never a fresh empty store");
    assert!(
        matches!(err, RecordStoreError::Corrupt { .. }),
        "the typed cause must survive load_or_init: {err}"
    );
}

#[test]
fn unsupported_version_is_a_typed_error_via_load_or_init() {
    let (_dir, store) = temp_store("future.test");
    let future = r#"{"version":99,"records":{}}"#;
    fs::write(store.state_path(), future).unwrap();

    assert!(matches!(
        store.load(),
        Err(RecordStoreError::UnsupportedVersion { found: 99, .. })
    ));
    let err = store
        .load_or_init()
        .expect_err("a newer schema must never degrade to a fresh empty store");
    assert!(
        matches!(err, RecordStoreError::UnsupportedVersion { found: 99, .. }),
        "the typed cause must survive load_or_init: {err}"
    );
    assert!(
        store.state_path().exists(),
        "original file left untouched for forensics"
    );
}

/// M3 (#1617): the refusal to read a NEWER version is the downgrade
/// direction, and `load_or_init` turns it into a fresh store whose next
/// `save` renames over the live file. The v1->v2 upgrade has been
/// backup-first since SC5; this direction had no backup at all. Without
/// this, the older binary destroys the newer binary's records on the very
/// run that reported them unreadable.
#[test]
fn unsupported_version_preserves_a_bak_before_the_downgrade_rewrite() {
    let (_dir, store) = temp_store("downgrade.test");
    let future = r#"{"version":99,"records":{}}"#;
    fs::write(store.state_path(), future).unwrap();

    let _ = store.load_or_init();

    let backup = crate::application::resume::backup_sibling(&store.state_path());
    assert!(
        backup.exists(),
        "the downgrade rewrite must leave a .bak sibling"
    );
    assert_eq!(
        fs::read_to_string(&backup).unwrap(),
        future,
        "the .bak must carry the exact bytes the load refused"
    );
}

/// The same symmetry on the unreadable-envelope side: corrupt JSON is
/// abandoned too, so the next save would otherwise destroy it silently.
#[test]
fn corrupt_envelope_preserves_a_bak_before_the_rewrite() {
    let (_dir, store) = temp_store("corrupt-bak.test");
    let garbage = b"{{{{not json";
    fs::write(store.state_path(), garbage).unwrap();

    let _ = store.load_or_init();

    let path = store.state_path();
    let backup = crate::application::resume::backup_sibling(&path);
    assert!(backup.exists(), "corrupt envelope must leave a .bak");
    assert_eq!(fs::read(&backup).unwrap(), garbage);
}

#[test]
fn deterministic_serialization_btree_ordering_stable_diffs() {
    let (_dir, store) = temp_store("stable.test");
    let mut records = DomainRecords::new();
    for url in ["https://stable.test/z", "https://stable.test/a"] {
        let mut r = full_record(url);
        r.status = crate::domain::page_state::PageStatus::Discovered;
        records.insert(url.to_string(), r);
    }
    store.save(&records).unwrap();
    let first = fs::read_to_string(store.state_path()).unwrap();

    store.save(&records).unwrap();
    let second = fs::read_to_string(store.state_path()).unwrap();
    let first_value: serde_json::Value =
        serde_json::from_str(&first).expect("first serialization is valid JSON");
    let second_value: serde_json::Value =
        serde_json::from_str(&second).expect("second serialization is valid JSON");
    assert_eq!(
        first_value["records"], second_value["records"],
        "identical records must serialize identically (BTree ordering stable)"
    );
    assert!(
        first_value["updated_at"].is_i64() && second_value["updated_at"].is_i64(),
        "updated_at must be present as i64 in both serializations"
    );
    let a_pos = first.find("\"https://stable.test/a\"").expect("a present");
    let z_pos = first.find("\"https://stable.test/z\"").expect("z present");
    assert!(
        a_pos < z_pos,
        "BTreeMap ordering must be sorted for stable diffs"
    );
}

// --- D3 commit-point: crash between EXPORTED and COMMITTED saves ------
// ADR-0014 hybrid strategy: the filesystem-owned side of the commit
// point is verified deterministically via the StoreFs seam (the
// SIGKILL harness covers the same points end-to-end in
// tests/crash_matrix_test.rs). rename(2) IS the commit point; the
// derived counter is never stored (A3), so there is no bookkeeping
// write to lose.

/// Fault-injected fs whose rename(2) succeeds exactly `ok_renames`
/// times and fails every later call — a deterministic stand-in for a
/// process dying after N successful commits.
struct FailAfterRenames {
    ok_renames: std::sync::atomic::AtomicU32,
}

impl StoreFs for FailAfterRenames {
    fn write(&self, path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
        let mut file = std::fs::File::create(path)?;
        file.write_all(bytes)?;
        file.flush()
    }

    fn rename(&self, from: &std::path::Path, to: &std::path::Path) -> std::io::Result<()> {
        if self
            .ok_renames
            .try_update(
                std::sync::atomic::Ordering::SeqCst,
                std::sync::atomic::Ordering::SeqCst,
                |n| (n > 0).then(|| n - 1),
            )
            .is_ok()
        {
            return std::fs::rename(from, to);
        }
        Err(std::io::Error::other("simulated crash before rename"))
    }
}

fn exported_record(url: &str) -> RawRecord {
    let mut r = full_record(url);
    r.status = PageStatus::Exported;
    r.last_error = None;
    r
}

fn committed_record(url: &str) -> RawRecord {
    let mut r = full_record(url);
    r.status = PageStatus::Committed;
    r.last_error = None;
    r
}

/// Crash between the EXPORTED save and the COMMITTED save (export_factory
/// drives save_notifying(Exported) then save_notifying(Committed); a
/// SIGKILL at POST_FLUSH_PRE_COMMIT lands exactly here): the second
/// save dies before its rename, so the on-disk state is still the
/// full EXPORTED envelope — not lost, not committed, not torn. A
/// recovered process reloads EXPORTED and re-drives the record to
/// COMMITTED.
#[test]
fn crash_between_saves_leaves_exported_state_reloadable_and_completable() {
    let dir = tempdir().unwrap();
    let state_dir = dir.path().to_path_buf();
    let url = "https://d3.test/page";
    let mut records = DomainRecords::new();
    records.insert(url.to_string(), exported_record(url));

    // Save #1 (EXPORTED): rename #1 succeeds.
    let store = RecordStore::with_fs(
        "d3.test",
        state_dir.clone(),
        std::sync::Arc::new(FailAfterRenames {
            ok_renames: std::sync::atomic::AtomicU32::new(1),
        }),
    );
    store.save(&records).expect("EXPORTED save must commit");

    // Save #2 (COMMITTED): the process "crashes" — rename #2 fails.
    let mut committed = DomainRecords::new();
    committed.insert(url.to_string(), committed_record(url));
    let err = store
        .save(&committed)
        .expect_err("crash-before-rename must surface as typed Io error");
    assert!(matches!(err, RecordStoreError::Io { .. }));

    // Recovered process (fresh store, real fs): still EXPORTED —
    // neither lost nor committed; the atomic rename kept the previous
    // full envelope intact.
    let recovered = RecordStore::new("d3.test").with_state_dir(state_dir);
    let loaded = recovered.load().expect("reload must succeed");
    assert_eq!(loaded.len(), 1, "no record lost by the crashed save");
    assert_eq!(loaded[url].status, PageStatus::Exported);
    assert_eq!(
        crate::domain::record_transition::derived_total_exported(&loaded),
        0,
        "nothing may count as COMMITTED after the crash"
    );

    // Re-drive: the recovery run completes the transition.
    recovered
        .save(&committed)
        .expect("recovery save must commit");
    let final_state = recovered.load().expect("post-recovery load");
    assert_eq!(final_state[url].status, PageStatus::Committed);
    assert_eq!(
        crate::domain::record_transition::derived_total_exported(&final_state),
        1
    );
}

/// Crash AFTER the COMMITTED rename but before any post-save
/// bookkeeping: there is none to lose — the counter is derived (A3),
/// never stored, so a reload sees COMMITTED directly. This pins the
/// "rename is the commit point" contract.
#[test]
fn crash_after_committed_rename_sees_committed_on_reload() {
    let (_dir, store) = temp_store("d3post.test");
    let url = "https://d3post.test/page";
    let mut records = DomainRecords::new();
    records.insert(url.to_string(), committed_record(url));
    store.save(&records).expect("COMMITTED save must commit");

    // Fresh store = recovered process: COMMITTED survives with zero
    // bookkeeping replay, and the derived counter sees it.
    let recovered = RecordStore::new("d3post.test")
        .with_state_dir(store.state_path().parent().unwrap().to_path_buf());
    let loaded = recovered.load().expect("reload must succeed");
    assert_eq!(loaded[url].status, PageStatus::Committed);
    assert_eq!(
        crate::domain::record_transition::derived_total_exported(&loaded),
        1,
        "the derived counter sees the committed record without any stored counter"
    );
}

/// #1230 made `<state>.lock` a PERMANENT sentinel (never unlinked:
/// `flock(2)` guards an inode, not a path, so unlink-on-drop lets a
/// second writer create a fresh inode and enter the critical section
/// while the first writer is still inside it). #1432 pins that
/// decision: this test fails if `Drop` ever unlinks the sentinel
/// again, which would resurrect the concurrent `--resume` state loss.
///
/// The `pid=` marker is asserted by prefix only: `acquire`
/// truncates and rewrites the sentinel on every acquisition, so the
/// value legitimately varies per acquisition.
#[test]
fn dropping_the_store_lock_releases_but_never_unlinks_the_sentinel() {
    let dir = tempdir().expect("tempdir");
    let state_path = dir.path().join("sentinel.test.json");
    let mut lock_os = state_path.as_os_str().to_owned();
    lock_os.push(".lock");
    let lock_path = PathBuf::from(lock_os);

    {
        let guard = StoreLock::acquire(&state_path).expect("acquire must succeed");
        assert!(
            lock_path.exists(),
            "acquire must create the <state>.lock sentinel"
        );
        drop(guard);
    }

    assert!(
        lock_path.exists(),
        "dropping StoreLock must release the flock but NEVER unlink the sentinel (#1230)"
    );
    let content = fs::read_to_string(&lock_path).expect("sentinel readable");
    assert!(
        content.starts_with("pid="),
        "sentinel keeps its pid= marker prefix, got {content:?}"
    );
    let _second = StoreLock::acquire(&state_path)
        .expect("re-acquire after drop must succeed: drop released the lock");
}
