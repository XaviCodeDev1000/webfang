use crate::error::ScraperError;
use crate::infrastructure::persistence::pool::Pool;

/// Age-based retention prune (#1827): delete `chunks`/`resources` and
/// `note_chunks`/`notes` rows whose `created_at` is older than
/// `cutoff_iso` (RFC-3339). Children are deleted before their parents;
/// rows with NULL or unparseable `created_at` are never pruned
/// (conservative). `VACUUM` returns the freed pages to the OS.
///
/// Errors propagate: the retention caller degrades to a warning — a
/// cleanup step must never fail the run.
pub async fn prune_older_than(pool: &Pool, cutoff_iso: &str) -> Result<u64, ScraperError> {
    let cutoff = cutoff_iso.to_owned();
    let conn = pool.get().await.map_err(|e| {
        ScraperError::persistence(format!("retention: obtener conexión del pool: {e}"))
    })?;
    conn.interact(move |c| -> Result<u64, rusqlite::Error> {
        c.execute_batch("PRAGMA foreign_keys = ON;")?;
        let mut removed: u64 = 0;
        removed += c.execute(
            "DELETE FROM chunks WHERE resource_url IN \
             (SELECT url FROM resources WHERE created_at IS NOT NULL \
             AND julianday(created_at) < julianday(?1))",
            rusqlite::params![cutoff],
        )? as u64;
        removed += c.execute(
            "DELETE FROM resources WHERE created_at IS NOT NULL \
             AND julianday(created_at) < julianday(?1)",
            rusqlite::params![cutoff],
        )? as u64;
        removed += c.execute(
            "DELETE FROM note_chunks WHERE note_id IN \
             (SELECT id FROM notes WHERE created_at IS NOT NULL \
             AND julianday(created_at) < julianday(?1))",
            rusqlite::params![cutoff],
        )? as u64;
        removed += c.execute(
            "DELETE FROM notes WHERE created_at IS NOT NULL \
             AND julianday(created_at) < julianday(?1)",
            rusqlite::params![cutoff],
        )? as u64;
        c.execute_batch("VACUUM;")?;
        Ok(removed)
    })
    .await
    .map_err(|e| ScraperError::persistence(format!("retention (interact): {e}")))?
    .map_err(|e| ScraperError::persistence(format!("retention: {e}")))
}
