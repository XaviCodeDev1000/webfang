//! Connection-pragma invariants on freshly pooled SQLite connections.

use super::sqlite::{fresh_pool, pragma_int, pragma_string};

#[tokio::test]
async fn test_wal_pragmas_applied_to_connection() {
    // A freshly-created pooled connection must carry all three pragmas
    // (proves the post_create hook runs on each connection).
    let (_dir, pool) = fresh_pool(4).await;
    assert_eq!(pragma_string(&pool, "journal_mode").await, "wal");
    // synchronous: 0=OFF,1=NORMAL,2=FULL,3=EXTRA — S12 requires FULL.
    assert_eq!(pragma_int(&pool, "synchronous").await, 2);
    assert_eq!(pragma_int(&pool, "cache_size").await, -4000);
}
