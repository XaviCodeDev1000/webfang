#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

//! S12 durability contract for the SQLite pool: FULL synchronous inside WAL,
//! and a typed startup failure when `integrity_check` finds a corrupt file.
//!
//! The persistence module is feature-gated, so the whole target compiles empty
//! in builds that do not opt into SQLite.

#![cfg(feature = "persistence")]

use tempfile::TempDir;

use webfang_core::infrastructure::persistence::{create_pool, SqliteVectorRepository};

/// WAL stays on, but `synchronous` must be FULL — NORMAL lets a committed
/// transaction disappear on power loss.
#[tokio::test]
async fn pooled_connections_keep_wal_with_full_synchronous() {
    let dir = TempDir::new().unwrap();
    let pool = create_pool(&dir.path().join("wal.db"), 2).unwrap();
    let conn = pool.get().await.unwrap();
    let synchronous: i64 = conn
        .interact(|c| c.query_row("PRAGMA synchronous", [], |r| r.get(0)))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(synchronous, 2, "FULL = 2; NORMAL = 1 is not durable");
    let journal: String = conn
        .interact(|c| c.query_row("PRAGMA journal_mode", [], |r| r.get(0)))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(journal.to_ascii_lowercase(), "wal", "WAL must be preserved");
}

/// A corrupt database fails startup with a typed integrity error instead of
/// being silently treated as a fresh schema.
#[tokio::test]
async fn corrupt_database_fails_startup_integrity_check() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("corrupt.db");
    std::fs::write(&path, b"not a sqlite database").unwrap();
    let pool = create_pool(&path, 1).unwrap();
    let err = match SqliteVectorRepository::setup_schema(&pool).await {
        Ok(()) => panic!("a corrupt DB must fail startup"),
        Err(err) => err,
    };
    let msg = err.to_string();
    assert!(
        msg.contains("integrity_check"),
        "the typed error must name the failed check: {msg}"
    );
}
