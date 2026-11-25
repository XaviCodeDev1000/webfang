use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::error::ScraperError;
use crate::infrastructure::persistence::pool::{
    rusqlite, Config, Hook, HookError, Manager, Pool, Runtime, Timeouts,
};

/// Explicit pool wait budgets (#S13): acquiring a connection, creating one and
/// recycling one are all bounded, so a stalled database file cannot hang a
/// caller forever. `deadpool` leaves every timeout unset by default.
const POOL_TIMEOUTS: Timeouts = Timeouts {
    wait: Some(Duration::from_secs(30)),
    create: Some(Duration::from_secs(10)),
    recycle: Some(Duration::from_secs(10)),
};

/// WAL + per-connection pragmas, applied on every new pooled connection via the
/// `post_create` hook. `journal_mode=WAL` is persistent (DB header); the other
/// two are per-connection and therefore MUST be set on each connection.
const INIT_PRAGMAS: &str = "\
PRAGMA journal_mode = WAL;\
PRAGMA synchronous = FULL;\
PRAGMA cache_size = -4000;";

/// Typed failure of the startup `PRAGMA integrity_check`.
#[derive(Debug, thiserror::Error)]
pub(crate) enum IntegrityCheckError {
    /// The check ran and reported corruption.
    #[error("sqlite integrity_check reported corruption: {detail}")]
    Corrupt {
        /// Diagnostic rows returned by the pragma.
        detail: String,
    },
    /// The check could not run (unreadable or malformed database file).
    #[error("sqlite integrity_check could not run: {0}")]
    Unavailable(#[source] rusqlite::Error),
}

/// Run `PRAGMA integrity_check` on a connection and classify its result.
pub(crate) fn verify_integrity(conn: &rusqlite::Connection) -> Result<(), IntegrityCheckError> {
    let result: String = conn
        .query_row("PRAGMA integrity_check", [], |row| row.get(0))
        .map_err(IntegrityCheckError::Unavailable)?;
    if result.trim().eq_ignore_ascii_case("ok") {
        Ok(())
    } else {
        Err(IntegrityCheckError::Corrupt { detail: result })
    }
}

// ============================================================================
// Pool construction
// ============================================================================
/// Build a WAL-mode SQLite connection pool backed by [`crate::infrastructure::persistence::pool`].
///
/// `pool_size` is clamped to at least 1 (the caller — `ElasticConfig` — already
/// applies the frozen `cpu_cores`/floor-4 sizing). The parent directory of
/// `db_path` is created if missing. Per-connection pragmas are applied via a
/// `post_create` hook so every connection honours the WAL-mode contract.
///
/// The pool is created lazily — the database file is only materialized on the
/// first [`Pool::get`] (i.e. inside [`crate::infrastructure::persistence::sqlite::SqliteVectorRepository::setup_schema`]),
/// which is the explicit fail-fast point if the path is not writable.
pub fn create_pool(db_path: &Path, pool_size: usize) -> Result<Pool, ScraperError> {
    // Fail-fast: ensure the parent directory exists before opening the DB.
    if let Some(parent) = db_path.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            std::fs::create_dir_all(parent).map_err(|e| {
                ScraperError::persistence(format!("crear directorio {parent:?}: {e}"))
            })?;
        }
    }

    let cfg = Config::new(db_path.to_path_buf());
    let manager = Manager::from_config(&cfg, Runtime::Tokio1);
    let pool = Pool::builder(manager)
        .max_size(pool_size.max(1))
        .timeouts(POOL_TIMEOUTS)
        .runtime(Runtime::Tokio1)
        .post_create(pragma_hook())
        .build()
        .map_err(|e| ScraperError::persistence(format!("construir pool SQLite: {e}")))?;
    Ok(pool)
}

/// Create a SQLite pool backed by `:memory:` for testing.
///
/// Uses a single connection with no WAL pragmas (in-memory databases don't
/// support WAL mode). The pool **must** be kept alive for the entire test
/// lifetime — dropping the pool closes all connections and destroys the
/// in-memory database.
pub fn create_memory_pool() -> Result<Pool, ScraperError> {
    let cfg = Config::new(PathBuf::from(":memory:"));
    let manager = Manager::from_config(&cfg, Runtime::Tokio1);
    let pool = Pool::builder(manager)
        .max_size(1)
        .timeouts(POOL_TIMEOUTS)
        .runtime(Runtime::Tokio1)
        // No post_create hook: in-memory databases don't support WAL mode.
        .build()
        .map_err(|e| ScraperError::persistence(format!("construir pool SQLite en memoria: {e}")))?;
    Ok(pool)
}

/// `post_create` hook applying the WAL-mode pragmas to each new connection,
/// plus the startup integrity gate.
///
/// `PRAGMA integrity_check` is O(database size), so it runs once per POOL —
/// claimed before the first connection verifies, on the first connection the
/// pool creates — never on every new connection. The pragmas still run on
/// every connection because `synchronous` and `cache_size` are per-connection.
///
/// The check runs BEFORE the pragmas: a corrupt file must not first be
/// rewritten by `journal_mode=WAL` and then reported as a generic pragma
/// failure.
fn pragma_hook() -> Hook {
    let integrity_checked = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    Hook::async_fn(move |obj, _metrics| {
        let integrity_checked = std::sync::Arc::clone(&integrity_checked);
        Box::pin(async move {
            if !integrity_checked.swap(true, std::sync::atomic::Ordering::AcqRel) {
                obj.interact(|conn| verify_integrity(conn))
                    .await
                    .map_err(|e| HookError::message(format!("integrity_check (interact): {e}")))?
                    .map_err(|e| HookError::message(format!("integrity_check: {e}")))?;
            }
            obj.interact(|conn| conn.execute_batch(INIT_PRAGMAS))
                .await
                .map_err(|e| HookError::message(format!("init pragmas (interact): {e}")))?
                .map_err(|e| HookError::message(format!("init pragmas: {e}")))?;
            Ok(())
        })
    })
}
