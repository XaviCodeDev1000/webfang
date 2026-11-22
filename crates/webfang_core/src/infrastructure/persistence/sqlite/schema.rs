use rusqlite::Connection;

use crate::error::ScraperError;

// ============================================================================
// Schema (frozen design.md §schema)
// ============================================================================
/// Forward-only schema (`CREATE ... IF NOT EXISTS`). Run once by
/// [`crate::infrastructure::persistence::sqlite::SqliteVectorRepository::setup_schema`]. No destructive migration in v1.
pub(super) const SCHEMA_DDL: &str = "\
CREATE TABLE IF NOT EXISTS resources (\
    url TEXT PRIMARY KEY,\
    title TEXT,\
    content_hash TEXT,\
    size_bytes INTEGER,\
    status TEXT,\
    created_at TEXT,\
    updated_at TEXT,\
    metadata_json TEXT\
);\
CREATE TABLE IF NOT EXISTS chunks (\
    id TEXT PRIMARY KEY,\
    resource_url TEXT NOT NULL REFERENCES resources(url),\
    chunk_index INTEGER,\
    content TEXT,\
    embedding_vector BLOB,\
    created_at TEXT\
);\
CREATE INDEX IF NOT EXISTS idx_chunks_resource ON chunks(resource_url);\
CREATE INDEX IF NOT EXISTS idx_resources_content_hash ON resources(content_hash);\
CREATE TABLE IF NOT EXISTS notes (\
    id INTEGER PRIMARY KEY AUTOINCREMENT,\
    path TEXT UNIQUE NOT NULL,\
    content_hash TEXT NOT NULL,\
    mtime_secs INTEGER NOT NULL,\
    created_at TEXT,\
    updated_at TEXT\
);\
CREATE TABLE IF NOT EXISTS note_chunks (\
    id INTEGER PRIMARY KEY AUTOINCREMENT,\
    note_id INTEGER NOT NULL REFERENCES notes(id) ON DELETE CASCADE,\
    chunk_index INTEGER NOT NULL,\
    content TEXT NOT NULL,\
    embedding_vector BLOB,\
    spans_json TEXT,\
    created_at TEXT\
);\
CREATE INDEX IF NOT EXISTS idx_note_chunks_note ON note_chunks(note_id);\
CREATE INDEX IF NOT EXISTS idx_notes_path ON notes(path);";

// ============================================================================
// Schema version marker (M4/D4, #1617)
// ============================================================================
/// The SQLite `user_version` this build writes, and the only one it opens.
///
/// This is the version marker the upgrade audit found missing. Before it, the
/// schema version of a database could only be *inferred* — `bytes_to_f32_vec`
/// decided a stored vector was corrupt purely because its BLOB length was not
/// a multiple of four, which is indistinguishable from a file written by a
/// build with a different column layout. `PRAGMA user_version` is SQLite's own
/// on-disk 32-bit header slot: it travels with the file, needs no table, and no
/// DDL can drop it.
///
/// The gate is REFUSE by default, with exactly one in-place migration: a
/// database stamped `PREVIOUS_SCHEMA_VERSION` is upgraded to
/// [`SCHEMA_VERSION`] before any DDL runs (see `migrate_to_current`). There
/// was no migration mechanism before the locators column forced the first
/// one; every future version bump must pair with its own migration step, and
/// a database from any OTHER version fails closed BEFORE the DDL — the file
/// is never opened for writing by a build that does not understand it. The
/// one other tolerated case is `0`: a database written before the marker
/// existed, or a fresh one. Its schema is by construction the one this
/// module's DDL creates idempotently, so it is adopted rather than refused.
///
/// Bump this only together with a migration, and only after that migration
/// exists — a bump without one turns every existing database into a refusal.
pub const SCHEMA_VERSION: i64 = 2;

/// The schema version a database may carry when it still needs the
/// v1→v2 migration (the `note_chunks.spans_json` locator column).
const PREVIOUS_SCHEMA_VERSION: i64 = 1;

/// Verify the on-disk schema marker, migrate the previous version, and stamp.
///
/// `0` means unversioned — a brand-new file, or one written by a pre-marker
/// build. Both carry the schema [`SCHEMA_DDL`] creates idempotently, so the
/// marker is stamped and the caller proceeds (after the additive migration,
/// which is a no-op on a truly fresh file). [`PREVIOUS_SCHEMA_VERSION`] is
/// migrated in place first. Any OTHER value is a database this build must not
/// write to, and the call fails with a Spanish message naming both versions
/// rather than corrupting rows it cannot interpret.
///
/// # Errors
///
/// [`ScraperError::Persistence`] when the marker belongs to a foreign schema
/// version, when the migration fails, or when the pragma cannot be read or
/// written.
pub(crate) fn enforce_schema_version(conn: &Connection) -> Result<(), ScraperError> {
    let found: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|e| ScraperError::persistence(format!("leer user_version: {e}")))?;

    if found != 0 && found != PREVIOUS_SCHEMA_VERSION && found != SCHEMA_VERSION {
        return Err(ScraperError::persistence(format!(
            "base de datos con versión de esquema {found}, incompatible con la versión {SCHEMA_VERSION} \
             de webfang; actualizá webfang o usá una base de datos distinta \
             (nunca se modifica una base desconocida)"
        )));
    }

    if found == SCHEMA_VERSION {
        return Ok(());
    }

    if found == PREVIOUS_SCHEMA_VERSION {
        migrate_to_current(conn)?;
    }

    // `PRAGMA user_version` takes a literal, not a bound parameter. The value
    // is an `i64` const, so interpolation here cannot inject SQL — formatting
    // an integer is total.
    conn.execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION};"))
        .map_err(|e| ScraperError::persistence(format!("escribir user_version: {e}")))
}

/// Migrate a [`PREVIOUS_SCHEMA_VERSION`] database to [`SCHEMA_VERSION`] in
/// place: add the `note_chunks.spans_json` locator column (additive `ALTER`,
/// NULL for pre-existing rows — they keep ranking, they just carry no
/// locators). Idempotent: the column is added only when the table exists and
/// lacks it, so a partially migrated or already-migrated database is a no-op.
///
/// # Errors
///
/// [`ScraperError::Persistence`] when the DDL fails.
fn migrate_to_current(conn: &Connection) -> Result<(), ScraperError> {
    let table_exists: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'note_chunks'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| ScraperError::persistence(format!("migración: leer sqlite_master: {e}")))?;
    if table_exists == 0 {
        return Ok(()); // fresh database: the DDL creates the column
    }

    let has_column: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('note_chunks') WHERE name = 'spans_json'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| ScraperError::persistence(format!("migración: leer columnas: {e}")))?;
    if has_column == 0 {
        conn.execute_batch("ALTER TABLE note_chunks ADD COLUMN spans_json TEXT;")
            .map_err(|e| ScraperError::persistence(format!("migración v1→v2: {e}")))?;
    }
    Ok(())
}
